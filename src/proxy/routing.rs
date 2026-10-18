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
//! `handle_ack_configure` and `ProxyLayerSurface::handle_configure`. The slot
//! test and the pass-through path are pinwin's; the layer surface and its
//! `closed` handling live in [`super::dock`] (D11).
//!
//! `_slf` names the handler parameters the proxy's own object would carry; a
//! handler that needs its own object keeps it in a `Weak` field, so the
//! display, registry, wm-base, toplevel and layer-surface handlers close no
//! reference cycle.
//!
//! The layer surface is the minimal one rows 6.2 to 6.4 run with: anchors,
//! size, margins, layer and keyboard mode from [`PanelSettings`], an
//! exclusive zone of -1 (D9: the reserve surface owns the strip), and a
//! percent size resolved against the panel's output size, tracked from
//! `zxdg_output_v1`. A percent panel whose output size is not known yet
//! holds its window back instead of docking at an invented size
//! ([`super::dock::size`]); an absolute-pixel panel docks without waiting.
//! The reserve is created and destroyed with the panel window
//! ([`super::reserve`]), and the panel window's buffer is cropped through a
//! proxy-owned `wp_viewport`
//! ([`super::viewport`]). A popup of the panel window becomes a popup of its
//! layer surface with the anchor rectangle moved by the window geometry
//! ([`super::popup`]), and each decoration object is routed by its toplevel
//! ([`super::decoration`], D5).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use wl_proxy::client::Client;
use wl_proxy::object::{ObjectCoreApi, ObjectUtils};
use wl_proxy::protocols::viewporter::wp_viewport::WpViewport;
use wl_proxy::protocols::wayland::wl_output::WlOutput;
use wl_proxy::protocols::wayland::wl_surface::WlSurface;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
use wl_proxy::protocols::xdg_decoration_unstable_v1::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wl_proxy::protocols::xdg_shell::xdg_surface::XdgSurface;
use wl_proxy::protocols::xdg_shell::xdg_toplevel::XdgToplevel;

use super::apply;
use super::dock;
use super::dock::size::DockDecision;
use super::output::Globals;
use super::reserve::Reserve;
use super::slot::{Route, Slot};
use super::viewport::{StaticCrop, write_crop};
use crate::config::{FocusRing, Keyboard, Layer, PanelSettings};
use crate::geometry::{Geometry, HeldGap, OutputSize, Size};

mod xdg_surface;

pub(super) use xdg_surface::ClientXdgSurface;

/// How long a percent panel's first dock waits for its output size before
/// taking the first tracked size instead (D9). A normal compositor reports
/// its outputs with the registry roundtrip, so this only bounds a missing or
/// late announcement.
const DOCK_WAIT: Duration = Duration::from_millis(500);

/// A panel window whose first dock waits for an output size (D9): the app
/// surface whose handler holds the waiting toplevel, the slot's window id,
/// and when the wait gives up.
struct PendingDock {
    xdg_surface: Weak<XdgSurface>,
    window: u64,
    deadline: Instant,
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
    /// surface is built from, changed in place by a live apply.
    settings: RefCell<PanelSettings>,
    /// The panel's shown/hidden state (row 8.2, D6): kept here so it survives
    /// a second `take_slot` and every re-dock; the status reply reads it.
    pub(super) hidden: super::hide::Hidden,
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
    /// The panel window whose first dock waits for an output size (D9).
    pending_dock: RefCell<Option<PendingDock>>,
}

impl Routing {
    /// The routing state for a panel with `settings`.
    pub(super) fn new(settings: PanelSettings) -> Rc<Self> {
        let output = settings.output().map(str::to_owned);
        let hidden = settings.hidden();
        Rc::new_cyclic(|self_ref| Self {
            self_weak: Weak::clone(self_ref),
            settings: RefCell::new(settings),
            hidden: super::hide::Hidden::new(hidden),
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
            pending_dock: RefCell::new(None),
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

    /// The panel's live layer, read without cloning the settings.
    pub(super) fn layer(&self) -> Layer {
        self.settings.borrow().layer()
    }

    /// The panel's live keyboard mode, read without cloning the settings.
    pub(super) fn keyboard(&self) -> Keyboard {
        self.settings.borrow().keyboard()
    }

    /// The panel's live focus ring, read without cloning the settings.
    pub(super) fn focus_ring(&self) -> FocusRing {
        self.settings.borrow().focus_ring()
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
        PanelSettings {
            output: self.output(),
            ..self.settings()
        }
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

    /// The output size the first dock resolves a percent size against, when
    /// it is known (D9): the pinned output's, the output the panel entered,
    /// or a sole tracked output's. `None` leaves the compositor's choice
    /// open, so a percent panel waits for it.
    pub(super) fn dock_output(&self) -> Option<OutputSize> {
        self.pinned_output()
            .and_then(|(_, global)| self.globals.output_size(global))
            .or_else(|| self.globals.panel_output_size())
            .or_else(|| self.globals.sole_output_size())
    }

    /// What the panel's first dock does about its size (D4, D9): an
    /// absolute-pixel size docks at once, and a percent size waits until
    /// [`Routing::dock_output`] names the output it resolves against.
    pub(super) fn dock_decision(&self) -> DockDecision {
        super::dock::size::decide(&self.settings(), self.dock_output())
    }

    /// The decision a timed-out wait falls back to (D9): any tracked
    /// output's known size, in global-name order. `None` when no output has
    /// reported one, which forwards the window instead of inventing a size.
    pub(super) fn fallback_dock_decision(&self) -> Option<DockDecision> {
        self.globals
            .first_known_output_size()
            .map(|output| super::dock::size::decide(&self.settings(), Some(output)))
    }

    /// The panel surface's geometry, when its size resolves now: its
    /// anchors, margins and size across its edge from the settings, with a
    /// percent resolved against the panel's output size (D4, D9).
    pub(super) fn geometry(&self) -> Option<Geometry> {
        match self.dock_decision() {
            DockDecision::Ready { geometry, .. } => Some(geometry),
            DockDecision::Wait => None,
        }
    }

    /// Records that the panel window's first dock waits for its output size
    /// (D9): the surface handler holds the toplevel, and the panel loop or an
    /// output event resumes it.
    pub(super) fn defer_dock(&self, xdg_surface: &Rc<XdgSurface>, window: u64) {
        *self.pending_dock.borrow_mut() = Some(PendingDock {
            xdg_surface: Rc::downgrade(xdg_surface),
            window,
            deadline: Instant::now() + DOCK_WAIT,
        });
    }

    /// The waiting dock's surface, window and deadline, for the resume path.
    pub(super) fn pending_snapshot(&self) -> Option<(Weak<XdgSurface>, u64, Instant)> {
        self.pending_dock.borrow().as_ref().map(|pending| {
            (
                Weak::clone(&pending.xdg_surface),
                pending.window,
                pending.deadline,
            )
        })
    }

    /// Forgets the waiting dock of `window`, when it is that window's (D9).
    pub(super) fn clear_pending_dock(&self, window: u64) {
        let mut pending = self.pending_dock.borrow_mut();
        if pending
            .as_ref()
            .is_some_and(|pending| pending.window == window)
        {
            *pending = None;
        }
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

    /// An output's size is now known or changed (D9): a waiting first dock
    /// resumes when the size decides it, and a docked panel whose output
    /// changed takes one re-apply pass.
    pub(super) fn on_output_size_ready(self: &Rc<Self>, global_name: u32) {
        resume_pending(self);
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
        resume_pending(self);
        apply::reapply(self);
        dock::follow_reserve(self);
    }

    /// Ends the panel window's footprint (D4, D9, D11): the slot empties,
    /// the reserve and the hidden buffers go, and the surface references clear.
    pub(super) fn on_panel_window_destroyed(&self, window: u64) {
        self.release(window);
        self.clear_pending_dock(window);
        self.slot_owner.borrow_mut().take();
        self.hidden.reset();
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
        self.globals.tween().reset();
        self.proxy_viewport.borrow_mut().take();
        *self.held_gap.borrow_mut() = None;
        self.last_resolved_across.set(None);
    }
}

/// Resumes a first dock that waits for its output size (D9): one attempt per
/// lane that can decide it — a size arrival, the panel's `enter`, an output
/// removal, and the panel loop's tick, which also carries the wait's
/// deadline. A wait still open does nothing.
pub(super) fn resume_pending(routing: &Rc<Routing>) {
    let Some((xdg_surface, window, deadline)) = routing.pending_snapshot() else {
        return;
    };
    let Some(xdg_surface) = xdg_surface.upgrade() else {
        routing.clear_pending_dock(window);
        return;
    };
    let Ok(mut handler) = xdg_surface.try_get_handler_mut::<ClientXdgSurface>() else {
        routing.clear_pending_dock(window);
        return;
    };
    handler.resume(routing, Instant::now() >= deadline);
}
