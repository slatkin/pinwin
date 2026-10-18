//! The proxy's own globals and the output-size tracking behind percent
//! geometry (replace-terminal-with-app-host D9, D12): the upstream objects a
//! panel needs besides the layer shell — `wl_compositor`, `wl_shm` and
//! `wp_viewporter` for the reserve surface and the crop — and the monitor
//! size a percent size resolves against.
//!
//! The proxy binds every `wl_output` on its own registry and, when the
//! compositor offers it, a `zxdg_output_v1` for each. The size follows D9's
//! precedence: the `zxdg_output_v1.logical_size` when it is announced, the
//! `wl_output` mode divided by its scale as the fallback. Which output the
//! panel actually sits on is learned from `wl_surface.enter` on the panel
//! window: the event names the app's own `wl_output`, which is correlated to
//! the proxy's bind through the global name both registries share.
//!
//! A percent size resolves against the panel's output size, and a panel
//! whose size needs that output does not dock until the size is known (see
//! [`super::dock::size`]). The proxy never invents an output size: a percent
//! panel waits for the compositor to report one, and an absolute-pixel panel
//! docks without it.
//!
//! The proxy-owned output objects here have no client on the far side, so
//! their event handlers observe and never forward: the generated default
//! handlers would only fail their `try_send_*` and log.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::{Rc, Weak};

use wl_proxy::object::{Object, ObjectCoreApi};
use wl_proxy::protocols::ObjectInterface;
use wl_proxy::protocols::viewporter::wp_viewporter::WpViewporter;
use wl_proxy::protocols::wayland::wl_compositor::WlCompositor;
use wl_proxy::protocols::wayland::wl_output::{WlOutput, WlOutputHandler, WlOutputMode};
use wl_proxy::protocols::wayland::wl_registry::{WlRegistry, WlRegistryHandler};
use wl_proxy::protocols::wayland::wl_shm::WlShm;
use wl_proxy::protocols::wayland::wl_subcompositor::WlSubcompositor;
use wl_proxy::protocols::wayland::wl_surface::{WlSurface, WlSurfaceHandler};
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wl_proxy::protocols::xdg_decoration_unstable_v1::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wl_proxy::protocols::xdg_output_unstable_v1::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wl_proxy::protocols::xdg_output_unstable_v1::zxdg_output_v1::{
    ZxdgOutputV1, ZxdgOutputV1Handler,
};

use super::ring::RingSlot;
use super::routing::Routing;
use super::viewport::ViewportState;
use crate::geometry::OutputSize;

/// The protocol versions the generated bindings know; a compositor that
/// announces more gets the known version, as the layer-shell bind already
/// does. `wl_output` at 4 also carries the `name` event, which the
/// `set-output` flow of row 6.5 matches panels against.
const COMPOSITOR_VERSION: u32 = 7;
const SHM_VERSION: u32 = 3;
const SUBCOMPOSITOR_VERSION: u32 = 1;
const VIEWPORTER_VERSION: u32 = 1;
const OUTPUT_VERSION: u32 = 4;
const XDG_OUTPUT_MANAGER_VERSION: u32 = 3;

/// The logical size implied by a `wl_output` mode and scale (D9's fallback
/// when the compositor has no `zxdg_output_v1`): the mode's pixel size
/// divided by the integer scale, floored. `None` when either is missing or
/// the division collapses to nothing (`port-to-rust` D10).
fn logical_from_mode(mode: Option<(i32, i32)>, scale: i32) -> Option<OutputSize> {
    let (width, height) = mode?;
    if scale <= 0 {
        return None;
    }
    OutputSize::new(
        u32::try_from(width / scale).ok()?,
        u32::try_from(height / scale).ok()?,
    )
}

/// The size an output tracks, by D9's precedence: the `zxdg_output_v1`
/// logical size when it has been announced, the mode-and-scale fallback
/// otherwise (`port-to-rust` D10).
fn effective_output_size(
    logical: Option<OutputSize>,
    mode: Option<(i32, i32)>,
    scale: i32,
) -> Option<OutputSize> {
    logical.or_else(|| logical_from_mode(mode, scale))
}

/// One proxy-bound `wl_output` and its size state: the current mode and
/// scale from `wl_output`, the logical size from its `zxdg_output_v1`.
pub(super) struct OutputTracker {
    /// The global name the output was bound under — the join key to the
    /// app's own `wl_output` binds, which carry the same names.
    global_name: u32,
    wl_output: Rc<WlOutput>,
    mode: Cell<Option<(i32, i32)>>,
    scale: Cell<i32>,
    logical: Cell<Option<OutputSize>>,
    /// Whether a size-affecting event has arrived since the last `done`, so
    /// the `done` that closes the batch is what re-applies the geometry.
    dirty: Cell<bool>,
    /// Whether this output already has its `zxdg_output_v1`.
    xdg_created: Cell<bool>,
    /// The compositor's name for the output (e.g. `DP-2`), kept for the
    /// `set-output` flow of row 6.5.
    output_name: RefCell<Option<String>>,
}

impl OutputTracker {
    fn new(global_name: u32, wl_output: Rc<WlOutput>) -> Self {
        Self {
            global_name,
            wl_output,
            mode: Cell::new(None),
            scale: Cell::new(1),
            logical: Cell::new(None),
            dirty: Cell::new(false),
            xdg_created: Cell::new(false),
            output_name: RefCell::new(None),
        }
    }

    /// The output's current size, by D9's precedence.
    fn size(&self) -> Option<OutputSize> {
        effective_output_size(self.logical.get(), self.mode.get(), self.scale.get())
    }

    /// The compositor's name for the output, when announced — from the
    /// `wl_output.name` event or the `zxdg_output_v1.name` event (D11): the
    /// `set-output` flow matches panels against it.
    fn name(&self) -> Option<String> {
        self.output_name.borrow().clone()
    }

    /// Creates the output's `zxdg_output_v1` once, when the manager is
    /// available, so `logical_size` can replace the mode-and-scale fallback.
    fn ensure_xdg_output(
        self: &Rc<Self>,
        manager: &Rc<ZxdgOutputManagerV1>,
        routing: &Rc<Routing>,
    ) {
        if self.xdg_created.get() {
            return;
        }
        self.xdg_created.set(true);
        let xdg_output = manager.new_send_get_xdg_output(&self.wl_output);
        xdg_output.set_handler(ProxyZxdgOutput {
            routing: Rc::clone(routing),
            tracker: Rc::clone(self),
        });
    }
}

/// The proxy-side globals and per-output state one panel's routing shares
/// (D9, D12). Filled by the proxy's own registry ([`ProxyRegistry`]) as the
/// compositor announces globals, and read by the routing when it builds the
/// panel's layer surface, the reserve and the crop.
pub(super) struct Globals {
    compositor: RefCell<Option<Rc<WlCompositor>>>,
    shm: RefCell<Option<Rc<WlShm>>>,
    subcompositor: RefCell<Option<Rc<WlSubcompositor>>>,
    viewporter: RefCell<Option<Rc<WpViewporter>>>,
    output_manager: RefCell<Option<Rc<ZxdgOutputManagerV1>>>,
    /// Every `wl_output` the proxy bound, by global name.
    outputs: RefCell<BTreeMap<u32, Rc<OutputTracker>>>,
    /// The app's `wl_output` binds by object id, mapped to the global name
    /// they were bound under — the join that turns a `wl_surface.enter` on
    /// the panel window into the output it sits on.
    client_output_names: RefCell<HashMap<u64, u32>>,
    /// The global name of the output the panel surface entered.
    panel_output: Cell<Option<u32>>,
    /// Surfaces the app put its own `wp_viewport` on (D8): the proxy must
    /// not create a second viewport on any of them, and on the panel surface
    /// it must not crop over the app's mapping either.
    viewport_surfaces: RefCell<HashSet<u64>>,
    /// The panel window's focus ring and the configured size its bars follow
    /// (row 9.1, D10); the ring is created with the first configure and
    /// destroyed with the panel window.
    ring: RingSlot,
    /// The panel surface's viewport inputs (row 10.1, D8): the app's own
    /// viewports, the window geometry, the compositor's preferred scale, the
    /// buffer sizes the crop is clamped to, and what the proxy wrote last.
    viewport: ViewportState,
    /// The panel's animated width transition (row 10.1, D8): the running
    /// tween and the serials the proxy mints for its own configures.
    tween: super::tween::Slot,
}

impl Globals {
    pub(super) fn new() -> Self {
        Self {
            compositor: RefCell::new(None),
            shm: RefCell::new(None),
            subcompositor: RefCell::new(None),
            viewporter: RefCell::new(None),
            output_manager: RefCell::new(None),
            outputs: RefCell::new(BTreeMap::new()),
            client_output_names: RefCell::new(HashMap::new()),
            panel_output: Cell::new(None),
            viewport_surfaces: RefCell::new(HashSet::new()),
            ring: RingSlot::new(),
            viewport: ViewportState::new(),
            tween: super::tween::Slot::new(),
        }
    }

    /// The panel's width tween, if one runs (row 10.1, D8).
    pub(super) fn tween(&self) -> &super::tween::Slot {
        &self.tween
    }

    pub(super) fn compositor(&self) -> Option<Rc<WlCompositor>> {
        self.compositor.borrow().clone()
    }

    pub(super) fn shm(&self) -> Option<Rc<WlShm>> {
        self.shm.borrow().clone()
    }

    /// The compositor's `wl_subcompositor`, which the focus ring's four
    /// sub-surfaces of the panel surface are created from (row 9.1, D10).
    pub(super) fn subcompositor(&self) -> Option<Rc<WlSubcompositor>> {
        self.subcompositor.borrow().clone()
    }

    /// The panel window's focus-ring slot (row 9.1, D10).
    pub(super) fn ring(&self) -> &RingSlot {
        &self.ring
    }

    /// The panel surface's viewport inputs (row 10.1, D8).
    pub(super) fn viewport(&self) -> &ViewportState {
        &self.viewport
    }

    pub(super) fn viewporter(&self) -> Option<Rc<WpViewporter>> {
        self.viewporter.borrow().clone()
    }

    fn set_compositor(&self, compositor: Rc<WlCompositor>) {
        *self.compositor.borrow_mut() = Some(compositor);
    }

    fn set_shm(&self, shm: Rc<WlShm>) {
        *self.shm.borrow_mut() = Some(shm);
    }

    fn set_subcompositor(&self, subcompositor: Rc<WlSubcompositor>) {
        *self.subcompositor.borrow_mut() = Some(subcompositor);
    }

    fn set_viewporter(&self, viewporter: Rc<WpViewporter>) {
        *self.viewporter.borrow_mut() = Some(viewporter);
    }

    fn set_output_manager(&self, manager: Rc<ZxdgOutputManagerV1>) {
        *self.output_manager.borrow_mut() = Some(manager);
    }

    /// Gives each already-bound output its `zxdg_output_v1`, for when the
    /// manager's global arrives after the outputs it serves.
    fn attach_xdg_outputs(&self, manager: &Rc<ZxdgOutputManagerV1>, routing: &Rc<Routing>) {
        for tracker in self.outputs.borrow().values() {
            tracker.ensure_xdg_output(manager, routing);
        }
    }

    fn add_output(&self, global_name: u32, tracker: Rc<OutputTracker>) {
        self.outputs.borrow_mut().insert(global_name, tracker);
    }

    /// Seeds one tracked output with a known size, for the display-free tests
    /// of the code that resolves against it (`port-to-rust` D10). The object
    /// is minted in a client-side state and never sent anywhere.
    #[cfg(test)]
    pub(super) fn install_test_output(&self, global: u32, size: OutputSize) {
        let state = wl_proxy::state::State::builder(wl_proxy::baseline::Baseline::V6)
            .without_server()
            .build()
            .expect("a client-side state needs no compositor");
        let wl_output = state.create_object::<WlOutput>(OUTPUT_VERSION);
        let tracker = Rc::new(OutputTracker::new(global, wl_output));
        tracker.logical.set(Some(size));
        self.add_output(global, tracker);
    }

    /// Forgets the output bound under `global`, when one is tracked (D11):
    /// the compositor removed the global, so the object is dead and its name
    /// must stop answering `set-output` (spec "Unknown output") and must
    /// stop pinning new panels. The app `wl_output` binds that named the
    /// global are dropped with it, so a stale `enter` no longer claims a
    /// size the proxy does not track, and the panel's output reverts to the
    /// compositor's choice. Returns the dead `wl_output`, for the caller to
    /// release.
    fn remove_output(&self, global: u32) -> Option<Rc<WlOutput>> {
        let tracker = self.outputs.borrow_mut().remove(&global)?;
        self.client_output_names
            .borrow_mut()
            .retain(|_, bound| *bound != global);
        if self.panel_output.get() == Some(global) {
            self.panel_output.set(None);
        }
        Some(Rc::clone(&tracker.wl_output))
    }

    /// Records which global name an app `wl_output` bind came from.
    pub(super) fn record_client_output(&self, global_name: u32, client_id: u64) {
        self.client_output_names
            .borrow_mut()
            .insert(client_id, global_name);
    }

    /// Records the output the panel surface entered, from the app's own
    /// `wl_output` object. Returns whether the output is one the proxy
    /// tracks and so has a size for.
    pub(super) fn set_panel_output(&self, client_id: u64) -> bool {
        let Some(global_name) = self.client_output_names.borrow().get(&client_id).copied() else {
            return false;
        };
        self.panel_output.set(Some(global_name));
        true
    }

    /// The global name of the output the panel is docked to.
    pub(super) fn panel_output(&self) -> Option<u32> {
        self.panel_output.get()
    }

    /// The global name of the tracked output whose compositor name is
    /// `name` (D11, spec "Unknown output"). An output that has not
    /// announced a name yet matches nothing, and neither does one the
    /// compositor has removed ([`Globals::remove_output`]).
    pub(super) fn named_output(&self, name: &str) -> Option<u32> {
        self.outputs
            .borrow()
            .values()
            .find(|tracker| tracker.name().as_deref() == Some(name))
            .map(|tracker| tracker.global_name)
    }

    /// The proxy-bound `wl_output` of the global named `global` (D11): the
    /// object a layer surface is pinned with.
    pub(super) fn output_by_global(&self, global: u32) -> Option<Rc<WlOutput>> {
        self.outputs
            .borrow()
            .get(&global)
            .map(|tracker| Rc::clone(&tracker.wl_output))
    }

    /// The tracked size of the output bound under `global` (D11).
    pub(super) fn output_size(&self, global: u32) -> Option<OutputSize> {
        self.outputs
            .borrow()
            .get(&global)
            .and_then(|tracker| tracker.size())
    }

    /// Each tracked output's global name and the compositor name it has
    /// announced, if any (D11): the input of the `set-output` decision.
    pub(super) fn tracked_output_names(&self) -> Vec<(u32, Option<String>)> {
        self.outputs
            .borrow()
            .values()
            .map(|tracker| (tracker.global_name, tracker.name()))
            .collect()
    }

    /// The global name of a proxy-bound `wl_output` object (D11): the join
    /// that turns the reserve surface's `enter` into the output the strip
    /// sits on. The `enter` names one of the proxy's own `wl_output`
    /// objects, and a `wl_output` the proxy forwarded for an app bind is
    /// one of them (both live on the proxy's connection), so the app-bind
    /// table answers first and the proxy's own binds answer second.
    pub(super) fn global_of_output(&self, output: &Rc<WlOutput>) -> Option<u32> {
        let id = output.unique_id();
        if let Some(global) = self.client_output_names.borrow().get(&id).copied() {
            return Some(global);
        }
        self.outputs
            .borrow()
            .values()
            .find(|tracker| tracker.wl_output.unique_id() == id)
            .map(|tracker| tracker.global_name)
    }

    /// The panel output's current size, when it is known.
    pub(super) fn panel_output_size(&self) -> Option<OutputSize> {
        let global_name = self.panel_output.get()?;
        self.outputs
            .borrow()
            .get(&global_name)
            .and_then(|tracker| tracker.size())
    }

    /// The size of the only tracked output, when the compositor has exactly
    /// one: a panel the compositor chooses an output for can only land on
    /// that output, so its percent size is already decidable (D9).
    pub(super) fn sole_output_size(&self) -> Option<OutputSize> {
        let outputs = self.outputs.borrow();
        let mut remaining = outputs.values();
        let only = remaining.next()?;
        if remaining.next().is_some() {
            return None;
        }
        only.size()
    }

    /// The first known output size in global-name order, when one is known:
    /// the last resort for a first dock whose own output is still unknown.
    pub(super) fn first_known_output_size(&self) -> Option<OutputSize> {
        self.outputs
            .borrow()
            .values()
            .find_map(|tracker| tracker.size())
    }

    /// Whether the surface named `surface_id` carries an app viewport (D8).
    pub(super) fn has_app_viewport(&self, surface_id: u64) -> bool {
        self.viewport_surfaces.borrow().contains(&surface_id)
    }

    pub(super) fn add_app_viewport(&self, surface_id: u64) {
        self.viewport_surfaces.borrow_mut().insert(surface_id);
    }

    pub(super) fn remove_app_viewport(&self, surface_id: u64) {
        self.viewport_surfaces.borrow_mut().remove(&surface_id);
    }
}

/// The proxy's own upstream registry (D4, D9): it binds the layer-shell and
/// decoration objects the routing serves, plus the globals the reserve and
/// the crop are built from — `wl_compositor`, `wl_shm`, `wp_viewporter`,
/// `zxdg_output_manager_v1` and every `wl_output`. The app never sees this
/// registry.
pub(super) struct ProxyRegistry {
    routing: Rc<Routing>,
    globals: Rc<Globals>,
    /// The registry this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    registry: Weak<WlRegistry>,
}

impl ProxyRegistry {
    /// The proxy's registry handler for one app connection's display.
    pub(super) fn new(
        routing: &Rc<Routing>,
        globals: &Rc<Globals>,
        registry: &Rc<WlRegistry>,
    ) -> Self {
        Self {
            routing: Rc::clone(routing),
            globals: Rc::clone(globals),
            registry: Rc::downgrade(registry),
        }
    }
}

impl WlRegistryHandler for ProxyRegistry {
    fn handle_global(
        &mut self,
        _slf: &Rc<WlRegistry>,
        name: u32,
        interface: ObjectInterface,
        version: u32,
    ) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        if interface == ObjectInterface::ZwlrLayerShellV1 {
            // Version 5 is the interface's XML version and the one wl-paper
            // binds; a compositor that implements less gets its version.
            let layer_shell = registry
                .state()
                .create_object::<ZwlrLayerShellV1>(version.min(5));
            let concrete = Rc::clone(&layer_shell);
            let bound: Rc<dyn Object> = concrete;
            registry.send_bind(name, bound);
            self.routing.set_layer_shell(layer_shell);
        } else if interface == ObjectInterface::ZxdgDecorationManagerV1 {
            // D5: the compositor's real decoration manager is kept, so a
            // passed-through window's decoration object can be forwarded to
            // it. Version 1 is the interface's only version.
            let manager = registry
                .state()
                .create_object::<ZxdgDecorationManagerV1>(version.min(1));
            let concrete = Rc::clone(&manager);
            let bound: Rc<dyn Object> = concrete;
            registry.send_bind(name, bound);
            self.routing.set_decoration_manager(manager);
        } else if interface == ObjectInterface::WlCompositor {
            let compositor = registry
                .state()
                .create_object::<WlCompositor>(version.min(COMPOSITOR_VERSION));
            let concrete = Rc::clone(&compositor);
            let bound: Rc<dyn Object> = concrete;
            registry.send_bind(name, bound);
            self.globals.set_compositor(compositor);
        } else if interface == ObjectInterface::WlShm {
            let shm = registry
                .state()
                .create_object::<WlShm>(version.min(SHM_VERSION));
            let concrete = Rc::clone(&shm);
            let bound: Rc<dyn Object> = concrete;
            registry.send_bind(name, bound);
            self.globals.set_shm(shm);
        } else if interface == ObjectInterface::WlSubcompositor {
            // D10: the focus ring's bars are sub-surfaces of the panel
            // surface, created from the proxy's own bind so the app's
            // object list never carries them.
            let subcompositor = registry
                .state()
                .create_object::<WlSubcompositor>(version.min(SUBCOMPOSITOR_VERSION));
            let concrete = Rc::clone(&subcompositor);
            let bound: Rc<dyn Object> = concrete;
            registry.send_bind(name, bound);
            self.globals.set_subcompositor(subcompositor);
        } else if interface == ObjectInterface::WpViewporter {
            // D8: the crop viewport is created from this viewporter.
            let viewporter = registry
                .state()
                .create_object::<WpViewporter>(version.min(VIEWPORTER_VERSION));
            let concrete = Rc::clone(&viewporter);
            let bound: Rc<dyn Object> = concrete;
            registry.send_bind(name, bound);
            self.globals.set_viewporter(viewporter);
        } else if interface == ObjectInterface::ZxdgOutputManagerV1 {
            let manager = registry
                .state()
                .create_object::<ZxdgOutputManagerV1>(version.min(XDG_OUTPUT_MANAGER_VERSION));
            let concrete = Rc::clone(&manager);
            let bound: Rc<dyn Object> = concrete;
            registry.send_bind(name, bound);
            self.globals.set_output_manager(Rc::clone(&manager));
            self.globals.attach_xdg_outputs(&manager, &self.routing);
        } else if interface == ObjectInterface::WlOutput {
            let wl_output = registry
                .state()
                .create_object::<WlOutput>(version.min(OUTPUT_VERSION));
            let concrete = Rc::clone(&wl_output);
            let bound: Rc<dyn Object> = concrete;
            registry.send_bind(name, bound);
            let tracker = Rc::new(OutputTracker::new(name, Rc::clone(&wl_output)));
            wl_output.set_handler(ProxyOutput {
                routing: Rc::clone(&self.routing),
                tracker: Rc::clone(&tracker),
            });
            self.globals.add_output(name, Rc::clone(&tracker));
            if let Some(manager) = self.globals.output_manager.borrow().as_ref() {
                tracker.ensure_xdg_output(manager, &self.routing);
            }
        }
    }

    fn handle_global_remove(&mut self, _slf: &Rc<WlRegistry>, name: u32) {
        // Only the outputs carry state that outlives the announcement (D11):
        // a removed output must stop answering `set-output` and stop pinning
        // new panels (spec "Unknown output"). The globals without
        // bookkeeping — the layer shell among them — keep their objects
        // until the connection ends, and the removals the app hears about
        // are answered by its own registry's handler, which forwards them.
        let Some(wl_output) = self.globals.remove_output(name) else {
            return;
        };
        // D9: a removal can leave a sole tracked output behind, which is the
        // size a waiting first dock was missing.
        super::routing::resume_pending(&self.routing);
        // The client destroys the object its bind created (the contract of
        // `wl_registry.global_remove`); `release` is `wl_output`'s
        // destructor since version 3, and the version the proxy bound is
        // the object's.
        if wl_output.version() >= WlOutput::MSG__RELEASE__SINCE {
            wl_output.send_release();
        }
    }
}

/// The proxy's handler for one bound `wl_output` (D9): it records the
/// current mode, the scale and the compositor's name, and marks the batch
/// done so the routing can re-apply a percent geometry against the fresh
/// size. The object is proxy-owned, so no event is forwarded.
struct ProxyOutput {
    routing: Rc<Routing>,
    tracker: Rc<OutputTracker>,
}

impl WlOutputHandler for ProxyOutput {
    fn handle_mode(
        &mut self,
        _slf: &Rc<WlOutput>,
        flags: WlOutputMode,
        width: i32,
        height: i32,
        _refresh: i32,
    ) {
        if flags.contains(WlOutputMode::CURRENT) {
            self.tracker.mode.set(Some((width, height)));
            self.tracker.dirty.set(true);
        }
    }

    fn handle_scale(&mut self, _slf: &Rc<WlOutput>, factor: i32) {
        self.tracker.scale.set(factor);
        self.tracker.dirty.set(true);
    }

    fn handle_done(&mut self, _slf: &Rc<WlOutput>) {
        if self.tracker.dirty.replace(false) {
            self.routing.on_output_size_ready(self.tracker.global_name);
        }
    }

    fn handle_name(&mut self, _slf: &Rc<WlOutput>, name: &str) {
        *self.tracker.output_name.borrow_mut() = Some(name.to_owned());
    }
}

/// The proxy's handler for one output's `zxdg_output_v1` (D9): the logical
/// size it announces wins over the mode-and-scale fallback. The object is
/// proxy-owned, so no event is forwarded.
struct ProxyZxdgOutput {
    routing: Rc<Routing>,
    tracker: Rc<OutputTracker>,
}

impl ZxdgOutputV1Handler for ProxyZxdgOutput {
    fn handle_logical_size(&mut self, _slf: &Rc<ZxdgOutputV1>, width: i32, height: i32) {
        let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
            return;
        };
        if let Some(size) = OutputSize::new(width, height) {
            self.tracker.logical.set(Some(size));
            self.tracker.dirty.set(true);
        }
    }

    fn handle_done(&mut self, _slf: &Rc<ZxdgOutputV1>) {
        if self.tracker.dirty.replace(false) {
            self.routing.on_output_size_ready(self.tracker.global_name);
        }
    }

    fn handle_name(&mut self, _slf: &Rc<ZxdgOutputV1>, name: &str) {
        let mut output_name = self.tracker.output_name.borrow_mut();
        if output_name.is_none() {
            *output_name = Some(name.to_owned());
        }
    }
}

/// The panel window's `wl_surface` handler (D9): it watches `enter`, which
/// names the output the compositor docked the panel on, and forwards
/// everything else by the generated defaults.
pub(super) struct PanelSurfaceWatch {
    routing: Rc<Routing>,
    /// The watched surface, kept weak so the handler does not keep its own
    /// object alive (the routing's handler convention).
    surface: Weak<WlSurface>,
}

impl PanelSurfaceWatch {
    /// The watch for the panel's own surface, installed when the surface
    /// takes the slot.
    pub(super) fn new(routing: &Rc<Routing>, surface: &Rc<WlSurface>) -> Self {
        Self {
            routing: Rc::clone(routing),
            surface: Rc::downgrade(surface),
        }
    }
}

impl WlSurfaceHandler for PanelSurfaceWatch {
    fn handle_enter(&mut self, _slf: &Rc<WlSurface>, output: &Rc<WlOutput>) {
        // The app's `wl_output` maps to the proxy's tracker through the
        // global name (D9); a known size re-applies the geometry at once.
        self.routing.on_panel_entered(output.unique_id());
        // Forward as the default handler would: the app still needs the
        // event and the client-scoped object it names.
        let Some(surface) = self.surface.upgrade() else {
            return;
        };
        if let (Some(surface_client), Some(output_client)) =
            (surface.client_id(), output.client_id())
            && surface_client != output_client
        {
            return;
        }
        let _ = surface.try_send_enter(output);
    }
}

#[cfg(test)]
mod tests;
