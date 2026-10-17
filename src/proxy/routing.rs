//! Window routing for one panel (replace-terminal-with-app-host D4): the
//! client-side xdg-shell handlers that hold an `xdg_surface` back until its
//! role is known, turn the panel's first toplevel into the layer surface and
//! forward every later toplevel unchanged.
//!
//! The empty-slot branch is pinwin's (D4, [`super::slot`]): the first
//! `get_toplevel` while the slot is empty creates a `zwlr_layer_surface_v1`
//! for the app's `wl_surface` and keeps the `xdg_toplevel` local; a toplevel
//! that arrives while the slot is full is forwarded to the compositor as a
//! normal window. The mapping between the held `xdg_surface` and the layer
//! surface — the hold-back in `ClientXdgSurface`, the configure and ack
//! mapping, and the layer-surface creation — is copied from wl-paper's
//! `apps/wl-paper/src/paper.rs` in <https://github.com/mahkoh/wl-proxy>
//! (GPL-3.0, the licence of this crate): `ClientXdgSurface::handle_get_toplevel`,
//! `handle_ack_configure` and `ProxyLayerSurface::handle_configure` are
//! the source. The slot test and the pass-through path are pinwin's. The
//! layer surface itself and its `closed` handling live in [`super::dock`]
//! (D11): `closed` re-docks the panel window instead of closing the app's
//! window.
//!
//! The handler parameters the proxy's own object would carry are named
//! `_slf`, the trait's name with the unused-prefix the codebase's lints
//! require, and a handler that needs its own object reaches it through a
//! `Weak` field instead of the parameter: a handler that held its own object
//! in an `Rc` would close a reference cycle, so the display, registry,
//! wm-base, toplevel and layer-surface handlers keep their own object weak
//! and keep the other objects they act on.
//!
//! The layer surface here is the minimal one rows 6.2 to 6.4 run with:
//! anchors, size, margins, layer and keyboard mode from [`PanelSettings`],
//! an exclusive zone of -1 (D9: the reserve surface owns the strip), and a
//! percent size resolved against the panel's output size, tracked from
//! `zxdg_output_v1` — the mode-and-scale fallback and a
//! 1920x1080 placeholder cover the window before that size is known
//! ([`super::output`]). The reserve itself is created and destroyed with the
//! panel window ([`super::reserve`]), and the panel window's buffer is
//! cropped to the app's window geometry through a proxy-owned `wp_viewport`
//! ([`super::viewport`]). Popups and decorations are row 6.3 (D5): a popup
//! of the panel window becomes a popup of its layer surface with the anchor
//! rectangle moved by the window geometry ([`super::popup`]), and each
//! decoration object is routed by its toplevel — answered server-side for
//! the panel window, forwarded to the compositor's real manager or answered
//! client-side for a passed-through window ([`super::decoration`]).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use wl_proxy::client::Client;
use wl_proxy::object::{Object, ObjectCoreApi, ObjectUtils};
use wl_proxy::protocols::viewporter::wp_viewport::WpViewport;
use wl_proxy::protocols::wayland::wl_output::WlOutput;
use wl_proxy::protocols::wayland::wl_surface::WlSurface;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
use wl_proxy::protocols::xdg_decoration_unstable_v1::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wl_proxy::protocols::xdg_shell::xdg_popup::XdgPopup;
use wl_proxy::protocols::xdg_shell::xdg_positioner::XdgPositioner;
use wl_proxy::protocols::xdg_shell::xdg_surface::{XdgSurface, XdgSurfaceHandler};
use wl_proxy::protocols::xdg_shell::xdg_toplevel::XdgToplevel;
use wl_proxy::protocols::xdg_shell::xdg_wm_base::XdgWmBase;

use super::apply;
use super::commit::PanelSurfaceCommits;
use super::dock;
use super::mapping::layer_of;
use super::output::{Globals, placeholder_output};
use super::popup::{ClientXdgPopup, ClientXdgPositioner, offset_anchor_rect};
use super::reserve::Reserve;
use super::slot::{Route, Slot};
use super::viewport::{StaticCrop, apply_steady_crop, write_crop};
use super::window::ClientXdgToplevel;
use crate::config::PanelSettings;
use crate::geometry::{Geometry, HeldGap, OutputSize, Size};

/// Whether `toplevel` is the panel's window (D5): only the slot's toplevel
/// carries a handler, a passed-through window uses the default forward path,
/// and a toplevel whose panel window ended has its handler unset. This is
/// what routes a decoration object at the time of each request.
pub(super) fn is_panel_window(toplevel: &XdgToplevel) -> bool {
    toplevel.try_get_handler_ref::<ClientXdgToplevel>().is_ok()
}

/// The routing state one panel shares between all its app connections (D4):
/// the slot, the settings the panel's layer surface is built from, the
/// compositor's layer-shell object once the proxy's registry found it, the
/// proxy-side globals and output tracking (D9), the panel window's surface
/// references and the reserve it created. One state per panel, not per
/// connection, so a second connection sees the window the first one created.
pub(super) struct Routing {
    /// A weak handle to this routing itself, so handlers the routing
    /// creates do not close a reference cycle through it (D11).
    self_weak: Weak<Routing>,
    /// The panel's live settings (D11, row 6.6): the settings its layer
    /// surface is built from, changed in place by a live apply, so a status
    /// reply reports what the panel runs at and the next dock builds from it.
    settings: RefCell<PanelSettings>,
    /// The output the panel is pinned to, by name (D11): the settings'
    /// value until a `set-output` changes it. `None` lets the compositor
    /// choose.
    output: RefCell<Option<String>>,
    slot: Cell<Slot>,
    /// The connection whose window holds the slot (row 7.3, D3): a client
    /// that disconnects without destroying its toplevel frees only the slot
    /// it took.
    slot_owner: RefCell<Option<Weak<Client>>>,
    layer_shell: RefCell<Option<Rc<ZwlrLayerShellV1>>>,
    /// The compositor's `zxdg_decoration_manager_v1`, bound by the proxy's
    /// own registry (D5): a passed-through window's decoration object is
    /// forwarded to it. `None` while the compositor announces none, which is
    /// when the app's registries hear the proxy's synthetic one instead.
    decoration_manager: RefCell<Option<Rc<ZxdgDecorationManagerV1>>>,
    /// The proxy-side globals and output tracking (D9): the compositor,
    /// `wl_shm` and `wp_viewporter` the reserve and crop are built from, and
    /// the output size a percent size resolves against.
    globals: Rc<Globals>,
    /// The panel window's surface id, which routes viewports and the output
    /// watch (D4, D9).
    panel_surface_id: Cell<Option<u64>>,
    /// The panel window's surface, kept weak so the crop can reach it.
    panel_surface: RefCell<Option<Weak<WlSurface>>>,
    /// The panel window's `xdg_surface`, kept weak: the re-dock re-docks the
    /// same window on a new layer surface (D11).
    panel_xdg_surface: RefCell<Option<Weak<XdgSurface>>>,
    /// The panel window's local toplevel, kept weak: the re-dock maps the
    /// new layer surface's events to the same app window (D11).
    panel_toplevel: RefCell<Option<Weak<XdgToplevel>>>,
    /// The panel's current layer surface, kept strong: the re-dock replaces
    /// it and the app's destroy tears it down, and nothing else keeps the
    /// object alive across a re-dock (D11).
    panel_layer_surface: RefCell<Option<Rc<ZwlrLayerSurfaceV1>>>,
    /// The proxy's own viewport on the panel surface, when the app gave the
    /// surface none (D8).
    proxy_viewport: RefCell<Option<Weak<WpViewport>>>,
    /// Whether the proxy created the panel surface's viewport (D8).
    has_proxy_viewport: Cell<bool>,
    /// Whether the app put its own viewport on the panel surface (D8): the
    /// proxy then mirrors it instead of cropping.
    panel_has_app_viewport: Cell<bool>,
    /// The reserve surface, created and destroyed with the panel window
    /// (D9).
    reserve: RefCell<Option<Rc<Reserve>>>,
    /// The global name of the output the reserve is on: the creation pin,
    /// or the output its surface entered (D11).
    reserve_output: Cell<Option<u32>>,
    /// The held gap the reserve draws (D9): its strip from the panel's
    /// start, re-published after every validated apply.
    held_gap: RefCell<Option<HeldGap>>,
    /// The across size the panel last applied, so a size re-apply only
    /// sends when the resolution actually changed.
    last_resolved_across: Cell<Option<u32>>,
}

impl Routing {
    /// The routing state for a panel with `settings`.
    pub(super) fn new(settings: PanelSettings) -> Rc<Self> {
        let output = settings.output().map(str::to_owned);
        Rc::new_cyclic(|self_ref| Self {
            self_weak: Weak::clone(self_ref),
            settings: RefCell::new(settings),
            output: RefCell::new(output),
            slot: Cell::new(Slot::new()),
            slot_owner: RefCell::new(None),
            layer_shell: RefCell::new(None),
            decoration_manager: RefCell::new(None),
            globals: Rc::new(Globals::new()),
            panel_surface_id: Cell::new(None),
            panel_surface: RefCell::new(None),
            panel_xdg_surface: RefCell::new(None),
            panel_toplevel: RefCell::new(None),
            panel_layer_surface: RefCell::new(None),
            proxy_viewport: RefCell::new(None),
            has_proxy_viewport: Cell::new(false),
            panel_has_app_viewport: Cell::new(false),
            reserve: RefCell::new(None),
            reserve_output: Cell::new(None),
            held_gap: RefCell::new(None),
            last_resolved_across: Cell::new(None),
        })
    }

    /// The proxy-side globals, for the handlers that fill them (D9).
    pub(super) fn globals(&self) -> Rc<Globals> {
        Rc::clone(&self.globals)
    }

    /// Whether the panel holds a window now: the status reply's `has_window`
    /// (spec "Status").
    pub(super) fn has_window(&self) -> bool {
        self.slot.get().has_window()
    }

    /// The panel's live settings, which its layer surface is built from
    /// (D11, row 6.6).
    pub(super) fn settings(&self) -> PanelSettings {
        self.settings.borrow().clone()
    }

    /// Changes the panel's live settings (row 6.6): a validated apply stores
    /// the settings the panel now runs at.
    pub(super) fn set_settings(&self, settings: PanelSettings) {
        *self.settings.borrow_mut() = settings;
    }

    /// A weak handle to this routing, so handlers it creates do not close
    /// a reference cycle through it (D11).
    pub(super) fn weak(&self) -> Weak<Routing> {
        Weak::clone(&self.self_weak)
    }

    /// The output the panel is pinned to, by name (D11): `None` lets the
    /// compositor choose.
    pub(super) fn output(&self) -> Option<String> {
        self.output.borrow().clone()
    }

    /// Changes the output the panel is pinned to, by name (D11).
    pub(super) fn set_output_name(&self, output: Option<String>) {
        *self.output.borrow_mut() = output;
    }

    /// The proxy-bound output the panel is pinned to (D11): the object to
    /// pass to `get_layer_surface` and the global name it was bound under.
    pub(super) fn pinned_output(&self) -> Option<(Rc<WlOutput>, u32)> {
        let name = self.output.borrow().clone()?;
        let global = self.globals.named_output(&name)?;
        let output = self.globals.output_by_global(global)?;
        Some((output, global))
    }

    /// The settings a status reply reports (spec "Status"): the panel's
    /// settings with the live output a `set-output` may have changed (D11).
    pub(super) fn status_settings(&self) -> PanelSettings {
        self.settings().with_output(self.output())
    }

    /// The app-side objects of the panel window, when it holds one (D4):
    /// the surface, its `xdg_surface` and the local toplevel (D11).
    pub(super) fn panel_window(&self) -> Option<(Rc<WlSurface>, Rc<XdgSurface>, Rc<XdgToplevel>)> {
        let surface = self
            .panel_surface
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)?;
        let xdg_surface = self
            .panel_xdg_surface
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)?;
        let toplevel = self
            .panel_toplevel
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)?;
        Some((surface, xdg_surface, toplevel))
    }

    /// The panel window's current layer surface (D4, D11): the size
    /// re-apply, the ack mapping and the popups target the live surface
    /// even after a re-dock.
    pub(super) fn panel_layer_surface(&self) -> Option<Rc<ZwlrLayerSurfaceV1>> {
        self.panel_layer_surface.borrow().clone()
    }

    /// Takes the panel window's current layer surface out, for a re-dock's
    /// teardown (D11).
    pub(super) fn take_panel_layer_surface(&self) -> Option<Rc<ZwlrLayerSurfaceV1>> {
        self.panel_layer_surface.borrow_mut().take()
    }

    /// Routes a new toplevel through the slot (D4) and records the
    /// connection that took it, so a disconnect can free the slot.
    fn route(&self, window: u64, owner: Option<&Rc<Client>>) -> Route {
        let mut slot = self.slot.get();
        let route = slot.route(window);
        if route == Route::Panel {
            *self.slot_owner.borrow_mut() = owner.map(Rc::downgrade);
        }
        self.slot.set(slot);
        route
    }

    /// Empties the slot when `window` is its own (D4).
    fn release(&self, window: u64) {
        let mut slot = self.slot.get();
        if slot.release(window) {
            self.slot_owner.borrow_mut().take();
        }
        self.slot.set(slot);
    }

    /// The window the slot holds for `client`, when that connection took it
    /// (row 7.3, D3, D4).
    pub(super) fn client_window(&self, client: &Weak<Client>) -> Option<u64> {
        let owned = self
            .slot_owner
            .borrow()
            .as_ref()
            .is_some_and(|owner| Weak::ptr_eq(owner, client));
        if !owned {
            return None;
        }
        self.slot.get().window()
    }

    /// The layer-shell object the proxy bound, once the registry announced
    /// it. The daemon's start check requires it (spec "Wayland layer-shell
    /// is required"); a connection that outruns the global gets a normal
    /// window instead of a panel.
    pub(super) fn layer_shell(&self) -> Option<Rc<ZwlrLayerShellV1>> {
        self.layer_shell.borrow().clone()
    }

    /// Keeps the layer-shell object the proxy registry bound.
    pub(super) fn set_layer_shell(&self, layer_shell: Rc<ZwlrLayerShellV1>) {
        *self.layer_shell.borrow_mut() = Some(layer_shell);
    }

    /// Whether the compositor has a real `zxdg_decoration_manager_v1` (D5):
    /// the proxy binds it when the compositor announces it, and only then are
    /// a passed-through window's decoration objects forwarded instead of
    /// answered client-side.
    pub(super) fn has_decoration_manager(&self) -> bool {
        self.decoration_manager.borrow().is_some()
    }

    /// The compositor's decoration manager, which a passed-through window's
    /// decoration object is forwarded to (D5).
    pub(super) fn decoration_manager(&self) -> Option<Rc<ZxdgDecorationManagerV1>> {
        self.decoration_manager.borrow().clone()
    }

    /// Keeps the decoration manager the proxy's registry bound (D5).
    pub(super) fn set_decoration_manager(&self, manager: Rc<ZxdgDecorationManagerV1>) {
        *self.decoration_manager.borrow_mut() = Some(manager);
    }

    /// The output size the panel is docked to, when it is known; the
    /// placeholder otherwise (D9).
    pub(super) fn known_output(&self) -> OutputSize {
        self.globals
            .panel_output_size()
            .unwrap_or_else(placeholder_output)
    }

    /// The panel surface's geometry: its anchors, margins and size across
    /// its edge from the settings, with a percent resolved against the
    /// panel's output size — the placeholder until the first size arrives
    /// (D4, D9).
    pub(super) fn geometry(&self) -> Geometry {
        self.geometry_against(self.known_output())
    }

    /// The panel surface's geometry with a percent resolved against
    /// `output` (D9, D11): a `set-output` resolves against the target
    /// output's size, so a size that resolves differently arrives with the
    /// new surface's configure.
    pub(super) fn geometry_against(&self, output: OutputSize) -> Geometry {
        let settings = self.settings();
        let across = settings.size().resolve(settings.edge(), output);
        Geometry::new(
            settings.edge(),
            across,
            settings.gutters(),
            settings.reserve(),
        )
    }

    /// The resolved width the status reply reports (spec "Status"): a pixel
    /// size as it is; a percent size once the panel's output size is known,
    /// 0 until then.
    pub(super) fn resolved_width(&self) -> u32 {
        let settings = self.settings();
        match self.globals.panel_output_size() {
            Some(output) => settings.size().resolve(settings.edge(), output),
            None => match settings.size() {
                Size::Pixels(px) => px.get(),
                Size::Percent(_) => 0,
            },
        }
    }

    /// Whether the surface named `surface_id` is the panel window (D8).
    pub(super) fn is_panel_surface(&self, surface_id: u64) -> bool {
        self.panel_surface_id.get() == Some(surface_id)
    }

    /// Whether the proxy created the panel surface's viewport (D8).
    pub(super) fn has_proxy_viewport(&self) -> bool {
        self.has_proxy_viewport.get()
    }

    /// Whether the app put its own viewport on the panel surface (D8).
    pub(super) fn panel_has_app_viewport(&self) -> bool {
        self.panel_has_app_viewport.get()
    }

    pub(super) fn set_panel_has_app_viewport(&self, present: bool) {
        self.panel_has_app_viewport.set(present);
    }

    /// Registers the panel window: the app-side objects and the layer
    /// surface the re-dock, the crop and the size re-apply write through
    /// (D4, D9, D11). The layer surface is kept strong, so the routing owns
    /// the live dock across a `set-output` or a `closed` re-dock.
    pub(super) fn set_panel_window(
        &self,
        surface: &Rc<WlSurface>,
        xdg_surface: &Rc<XdgSurface>,
        toplevel: &Rc<XdgToplevel>,
        layer_surface: &Rc<ZwlrLayerSurfaceV1>,
    ) {
        self.panel_surface_id.set(Some(surface.unique_id()));
        *self.panel_surface.borrow_mut() = Some(Rc::downgrade(surface));
        *self.panel_xdg_surface.borrow_mut() = Some(Rc::downgrade(xdg_surface));
        *self.panel_toplevel.borrow_mut() = Some(Rc::downgrade(toplevel));
        *self.panel_layer_surface.borrow_mut() = Some(Rc::clone(layer_surface));
    }

    /// The gap the reserve currently holds, when one exists (D9, D11).
    pub(super) fn held_gap(&self) -> Option<HeldGap> {
        self.held_gap.borrow().as_ref().copied()
    }

    /// The reserve the panel holds, for the apply path to re-zone (D9, row
    /// 6.6).
    pub(super) fn reserve(&self) -> Option<Rc<Reserve>> {
        self.reserve.borrow().clone()
    }

    /// Stores the gap a validated apply or re-apply left the strip at (D9,
    /// row 6.6).
    pub(super) fn set_held_gap(&self, gap: HeldGap) {
        *self.held_gap.borrow_mut() = Some(gap);
    }

    /// The across size the panel last applied (D9, row 6.6): the re-apply an
    /// output event triggers sends nothing while the resolution is the same.
    pub(super) fn last_resolved_across(&self) -> Option<u32> {
        self.last_resolved_across.get()
    }

    /// Records the across size the panel last applied (D9, row 6.6).
    pub(super) fn set_last_resolved_across(&self, across: Option<u32>) {
        self.last_resolved_across.set(across);
    }

    /// The global name of the output the reserve is on (D11): the creation
    /// pin, or the output its surface entered.
    pub(super) fn reserve_output(&self) -> Option<u32> {
        self.reserve_output.get()
    }

    /// Records the output the reserve's surface entered (D11).
    pub(super) fn set_reserve_output(&self, output_global: Option<u32>) {
        self.reserve_output.set(output_global);
    }

    /// Takes the reserve out, so its owner can destroy it (D11).
    pub(super) fn take_reserve(&self) -> Option<Rc<Reserve>> {
        self.reserve.borrow_mut().take()
    }

    /// Registers a freshly created reserve with its gap and output (D9, D11).
    pub(super) fn register_reserve(
        &self,
        reserve: Rc<Reserve>,
        gap: HeldGap,
        output_global: Option<u32>,
    ) {
        self.reserve_output.set(output_global);
        *self.reserve.borrow_mut() = Some(reserve);
        *self.held_gap.borrow_mut() = Some(gap);
    }

    /// Writes the steady-state crop into the panel surface's proxy-owned
    /// viewport (D5, D8), creating the viewport on first use. The values
    /// apply with the app's next commit — the proxy sends no commit of its
    /// own (D7 is row 8.1).
    pub(super) fn send_viewport_crop(&self, crop: &StaticCrop) {
        let Some(surface) = self.panel_surface.borrow().as_ref().and_then(Weak::upgrade) else {
            return;
        };
        // D8: a viewport the app created on this surface — before or after
        // the slot was taken — owns the mapping; the proxy never puts a
        // second one next to it.
        if self.globals.has_app_viewport(surface.unique_id()) {
            return;
        }
        let Some(viewporter) = self.globals.viewporter() else {
            return;
        };
        let existing = self
            .proxy_viewport
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade);
        let viewport = if let Some(viewport) = existing {
            viewport
        } else {
            let viewport = viewporter.new_send_get_viewport(&surface);
            self.has_proxy_viewport.set(true);
            *self.proxy_viewport.borrow_mut() = Some(Rc::downgrade(&viewport));
            viewport
        };
        write_crop(&viewport, Some(crop));
    }

    /// The output the panel entered is now known or its size changed: one
    /// re-apply pass (D9).
    pub(super) fn on_output_size_ready(&self, global_name: u32) {
        if self.globals.panel_output() != Some(global_name) {
            return;
        }
        apply::reapply(self);
    }

    /// The panel surface entered an output: record it, re-apply if a size
    /// is already known (D9), and move the strip if it is not on the
    /// panel's output (D11). Takes the routing through its `Rc`, so the
    /// reserve's re-dock can hand the routing's own weak handle to the
    /// handlers it creates.
    pub(super) fn on_panel_entered(self: &Rc<Self>, client_output_id: u64) {
        if !self.globals.set_panel_output(client_output_id) {
            return;
        }
        apply::reapply(self);
        dock::follow_reserve(self);
    }

    /// Ends the panel window's footprint (D4, D9, D11): the slot empties,
    /// the reserve is destroyed with the window it was created for and the
    /// surface references clear.
    pub(super) fn on_panel_window_destroyed(&self, window: u64) {
        self.release(window);
        self.slot_owner.borrow_mut().take();
        if let Some(reserve) = self.reserve.borrow_mut().take() {
            reserve.destroy();
        }
        self.reserve_output.set(None);
        self.panel_layer_surface.borrow_mut().take();
        *self.panel_surface.borrow_mut() = None;
        *self.panel_xdg_surface.borrow_mut() = None;
        *self.panel_toplevel.borrow_mut() = None;
        self.panel_surface_id.set(None);
        self.has_proxy_viewport.set(false);
        self.panel_has_app_viewport.set(false);
        self.proxy_viewport.borrow_mut().take();
        *self.held_gap.borrow_mut() = None;
        self.last_resolved_across.set(None);
    }
}

/// One app `xdg_surface` (D4): held back until its toplevel role appears,
/// then either the panel's layer surface or a forwarded normal window.
pub(super) struct ClientXdgSurface {
    routing: Rc<Routing>,
    wl_surface: Rc<WlSurface>,
    xdg_surface: Rc<XdgSurface>,
    xdg_wm_base: Rc<XdgWmBase>,
    /// Whether the compositor has this `xdg_surface` as an upstream object:
    /// false while the surface is held back, true after a pass-through
    /// forward.
    has_server: bool,
    /// The window geometry the app set on this surface, `[x, y, width,
    /// height]` (D5): a popup of the panel window offsets its anchor
    /// rectangle by the panel window's one, and the panel window's buffer is
    /// cropped to it through a proxy-owned viewport (D8). A surface the app
    /// set none on keeps `None`.
    geometry: Option<[i32; 4]>,
}

impl ClientXdgSurface {
    /// A held-back surface whose role is not yet known (D4): it carries no
    /// layer surface, has nothing upstream and no window geometry.
    pub(super) fn held_back(
        routing: Rc<Routing>,
        wl_surface: Rc<WlSurface>,
        xdg_surface: Rc<XdgSurface>,
        xdg_wm_base: Rc<XdgWmBase>,
    ) -> Self {
        Self {
            routing,
            wl_surface,
            xdg_surface,
            xdg_wm_base,
            has_server: false,
            geometry: None,
        }
    }

    /// The panel window's current layer surface when this surface is the
    /// panel window (D4, D11): the routing's live reference, so popups and
    /// acks stay on the surface that is actually docked across a re-dock.
    fn panel_layer_surface(&self) -> Option<Rc<ZwlrLayerSurfaceV1>> {
        if !self.routing.is_panel_surface(self.wl_surface.unique_id()) {
            return None;
        }
        self.routing.panel_layer_surface()
    }

    /// Forwards this surface and its new toplevel to the compositor unchanged
    /// (D4: `get_toplevel` while the slot is full).
    fn forward(&mut self, id: &Rc<XdgToplevel>) {
        self.xdg_wm_base
            .send_get_xdg_surface(&self.xdg_surface, &self.wl_surface);
        self.has_server = true;
        self.xdg_surface.send_get_toplevel(id);
    }

    /// Turns this surface into the panel's layer surface (D4: `get_toplevel`
    /// while the slot is empty). Copied from wl-paper's `paper.rs`
    /// `ClientXdgSurface::handle_get_toplevel` (GPL-3.0); the geometry comes
    /// from pinwin's settings, and the layer surface's creation and handler
    /// live in [`super::dock`] (D11).
    fn take_slot(&mut self, layer_shell: &Rc<ZwlrLayerShellV1>, id: &Rc<XdgToplevel>) {
        let geometry = self.routing.geometry();
        if id.version() >= XdgToplevel::MSG__WM_CAPABILITIES__SINCE {
            id.send_wm_capabilities(&[]);
        }
        // The toplevel stays local: the app sees a normal window, but every
        // request on this role is answered by the proxy (D4).
        id.set_forward_to_server(false);
        id.set_handler(ClientXdgToplevel::new(
            Rc::clone(&self.routing),
            id,
            id.unique_id(),
        ));
        let window = dock::PanelWindow {
            surface: Rc::clone(&self.wl_surface),
            xdg_surface: Rc::clone(&self.xdg_surface),
            toplevel: Rc::clone(id),
        };
        // D11: the settings' output pins the panel when the proxy has bound
        // it; otherwise the compositor chooses.
        dock::dock_panel_window(&self.routing, layer_shell, &window, &geometry);
        // D9, D11: the reserve surface is created with the panel window and
        // owns the exclusive zone the panel surface does not claim, pinned
        // to the panel's output when one is set.
        dock::start_reserve(
            &self.routing,
            &geometry,
            layer_of(self.routing.settings().layer()),
            self.routing.pinned_output().as_ref(),
        );
        // D9: the surface watch learns which output the compositor docked
        // the panel on; the D7 commits handler sits on top of it and sees
        // each pending-state request and the app's commit.
        self.wl_surface
            .set_handler(PanelSurfaceCommits::new(&self.routing, &self.wl_surface));
        // A window geometry the app set before its toplevel crops from here.
        apply_steady_crop(&self.routing, self.geometry);
    }
}

impl XdgSurfaceHandler for ClientXdgSurface {
    fn handle_destroy(&mut self, _slf: &Rc<XdgSurface>) {
        self.xdg_surface.unset_handler();
        if self.has_server {
            self.xdg_surface.send_destroy();
            self.has_server = false;
        } else {
            self.xdg_surface.delete_id();
        }
    }

    fn handle_get_toplevel(&mut self, _slf: &Rc<XdgSurface>, id: &Rc<XdgToplevel>) {
        let Some(layer_shell) = self.routing.layer_shell() else {
            self.forward(id);
            return;
        };
        let owner = id.core().client();
        if self.routing.route(id.unique_id(), owner.as_ref()) == Route::PassedThrough {
            self.forward(id);
            return;
        }
        // D9: validate before applying. A geometry the output cannot hold
        // never docks — the window passes through as a normal window and the
        // slot stays empty (spec "Validate a change before applying it").
        if let Err(error) = self
            .routing
            .geometry()
            .validate(self.routing.known_output())
        {
            eprintln!(
                "pinwin: the panel geometry cannot apply: {error}; the window opens as a normal window"
            );
            self.routing.release(id.unique_id());
            self.forward(id);
            return;
        }
        self.take_slot(&layer_shell, id);
    }

    fn handle_set_window_geometry(
        &mut self,
        _slf: &Rc<XdgSurface>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
        // The window geometry drives the popup anchor offset (D5) and the
        // panel window's viewport crop (D8); every surface keeps
        // what the app asked for, and a held surface sends nothing upstream
        // yet.
        self.geometry = Some([x, y, width, height]);
        if self.has_server {
            self.xdg_surface
                .send_set_window_geometry(x, y, width, height);
        }
        // D5/D8: the panel window's buffer is cropped to the window geometry
        // through the proxy-owned viewport, so a client-side title bar and
        // shadow stay outside the panel. The values are double-buffered
        // viewport state: they apply with the app's next commit (D7 is row
        // 8.1).
        if self.panel_layer_surface().is_some() {
            apply_steady_crop(&self.routing, self.geometry);
        }
    }

    fn handle_get_popup(
        &mut self,
        _slf: &Rc<XdgSurface>,
        id: &Rc<XdgPopup>,
        parent: Option<&Rc<XdgSurface>>,
        positioner: &Rc<XdgPositioner>,
    ) {
        // The popup's parent decides its route (D5): a popup of the panel
        // window becomes a popup of the panel's layer surface, moved by the
        // window geometry, and every other popup is forwarded unchanged.
        // Copied from wl-paper's `paper.rs`
        // `ClientXdgSurface::handle_get_popup` (GPL-3.0); the geometry the
        // offset moves by is the one [`Self::handle_set_window_geometry`]
        // keeps.
        if !self.has_server {
            // The upstream popup is created from this surface's upstream
            // `xdg_surface`, so the held-back surface goes first.
            self.xdg_wm_base
                .send_get_xdg_surface(&self.xdg_surface, &self.wl_surface);
            self.has_server = true;
        }
        if let Some([x, y, width, height]) = self.geometry {
            self.xdg_surface
                .send_set_window_geometry(x, y, width, height);
        }
        let mut layer_surface = None;
        let mut proxy_parent = parent;
        let mut parent_geometry = None;
        if let Some(parent) = parent
            && let Ok(parent_handler) = parent.try_get_handler_ref::<ClientXdgSurface>()
            && let Some(parent_layer) = parent_handler.panel_layer_surface()
        {
            layer_surface = Some(parent_layer);
            proxy_parent = None;
            parent_geometry = parent_handler.geometry;
        }
        let anchor = positioner
            .try_get_handler_ref::<ClientXdgPositioner>()
            .ok()
            .and_then(|handler| handler.anchor_rect());
        if let Some(geometry) = parent_geometry
            && let Some([ax, ay, aw, ah]) = anchor
        {
            let [x, y, width, height] = offset_anchor_rect([ax, ay, aw, ah], Some(geometry));
            positioner.send_set_anchor_rect(x, y, width, height);
        }
        self.xdg_surface
            .send_get_popup(id, proxy_parent, positioner);
        if parent_geometry.is_some()
            && let Some([ax, ay, aw, ah]) = anchor
        {
            // The upstream positioner keeps the app's rectangle: the offset
            // was only for this popup's creation (wl-paper).
            positioner.send_set_anchor_rect(ax, ay, aw, ah);
        }
        if let Some(layer_surface) = layer_surface {
            layer_surface.send_get_popup(id);
        }
        if let Some(geometry) = parent_geometry {
            id.set_handler(ClientXdgPopup::new(id, geometry));
        }
    }

    fn handle_ack_configure(&mut self, _slf: &Rc<XdgSurface>, serial: u32) {
        if let Some(layer) = self.panel_layer_surface() {
            // The app acks the layer surface's configure through its local
            // xdg_surface (D4); after a re-dock it reaches the live surface
            // (D11).
            layer.send_ack_configure(serial);
        } else if self.has_server {
            self.xdg_surface.send_ack_configure(serial);
        }
    }
}
