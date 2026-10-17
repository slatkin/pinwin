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
//! `handle_ack_configure`, `ProxyZwlrLayerSurfaceV1::handle_configure` and
//! `handle_closed` are the source. The slot test and the pass-through path
//! are pinwin's.
//!
//! The handler parameters the proxy's own object would carry are unused and
//! named `_slf`: a handler that held its own object in an `Rc` would close a
//! reference cycle, so the display, registry and wm-base handlers keep a
//! `Weak` instead and the surface and toplevel handlers keep the objects
//! they act on.
//!
//! The layer surface here is the minimal one rows 6.2 and 6.3 run with:
//! anchors, size, margins, layer and keyboard mode from [`PanelSettings`],
//! and a percent size resolved against a placeholder 1920x1080 output until
//! row 6.4 reads the real output size from `zxdg_output_v1`. Popups and
//! decorations are row 6.3: a panel window's popups cannot forward yet
//! because its `xdg_surface` has no upstream object, so this row leaves the
//! default handler to drop them, while a passed-through window's popups use
//! the default forward path.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use wl_proxy::object::{Object, ObjectCoreApi, ObjectRcUtils};
use wl_proxy::protocols::ObjectInterface;
use wl_proxy::protocols::wayland::wl_display::{WlDisplay, WlDisplayHandler};
use wl_proxy::protocols::wayland::wl_registry::{WlRegistry, WlRegistryHandler};
use wl_proxy::protocols::wayland::wl_surface::WlSurface;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::{
    ZwlrLayerShellV1, ZwlrLayerShellV1Layer,
};
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::{
    ZwlrLayerSurfaceV1, ZwlrLayerSurfaceV1Anchor, ZwlrLayerSurfaceV1Handler,
    ZwlrLayerSurfaceV1KeyboardInteractivity,
};
use wl_proxy::protocols::xdg_shell::xdg_surface::{XdgSurface, XdgSurfaceHandler};
use wl_proxy::protocols::xdg_shell::xdg_toplevel::{
    XdgToplevel, XdgToplevelHandler, XdgToplevelState,
};
use wl_proxy::protocols::xdg_shell::xdg_wm_base::{XdgWmBase, XdgWmBaseHandler};

use super::slot::{Route, Slot};
use crate::config::{Keyboard, Layer, PanelSettings};
use crate::geometry::{Anchors, Edge, Geometry, OutputSize};

/// The layer-shell namespace of a panel surface. One namespace keeps every
/// panel out of the special namespaces compositors reserve for their own
/// panels.
const NAMESPACE: &str = "pinwin";

/// The output size a percent panel resolves against until row 6.4 reads the
/// real one from `zxdg_output_v1`: a layer surface anchored on only its near
/// edge must still carry a size across that edge, and 1920x1080 turns the
/// built-in 25% default into a 480 pixel panel. Row 6.4 replaces this with
/// the panel's output size and removes the constants.
const PLACEHOLDER_OUTPUT_WIDTH: u32 = 1920;
const PLACEHOLDER_OUTPUT_HEIGHT: u32 = 1080;

/// The routing state one panel shares between all its app connections (D4):
/// the slot, the settings the panel's layer surface is built from, and the
/// compositor's layer-shell object once the proxy's registry found it. One
/// state per panel, not per connection, so a second connection sees the
/// window the first one created.
pub(super) struct Routing {
    settings: PanelSettings,
    slot: Cell<Slot>,
    layer_shell: RefCell<Option<Rc<ZwlrLayerShellV1>>>,
}

impl Routing {
    /// The routing state for a panel with `settings`.
    pub(super) fn new(settings: PanelSettings) -> Rc<Self> {
        Rc::new(Self {
            settings,
            slot: Cell::new(Slot::new()),
            layer_shell: RefCell::new(None),
        })
    }

    /// Whether the panel holds a window now: the status reply's `has_window`
    /// (spec "Status").
    pub(super) fn has_window(&self) -> bool {
        self.slot.get().has_window()
    }

    /// The panel's settings, which its layer surface is built from.
    fn settings(&self) -> &PanelSettings {
        &self.settings
    }

    /// Routes a new toplevel through the slot (D4).
    fn route(&self, window: u64) -> Route {
        let mut slot = self.slot.get();
        let route = slot.route(window);
        self.slot.set(slot);
        route
    }

    /// Empties the slot when `window` is its own (D4).
    fn release(&self, window: u64) {
        let mut slot = self.slot.get();
        slot.release(window);
        self.slot.set(slot);
    }

    /// The layer-shell object the proxy bound, once the registry announced
    /// it. The daemon's start check requires it (spec "Wayland layer-shell
    /// is required"); a connection that outruns the global gets a normal
    /// window instead of a panel.
    fn layer_shell(&self) -> Option<Rc<ZwlrLayerShellV1>> {
        self.layer_shell.borrow().clone()
    }

    /// Keeps the layer-shell object the proxy registry bound.
    fn set_layer_shell(&self, layer_shell: Rc<ZwlrLayerShellV1>) {
        *self.layer_shell.borrow_mut() = Some(layer_shell);
    }

    /// The panel surface's geometry: its anchors, margins and size across
    /// its edge from the settings (D4, D9). Row 6.4 resolves a percent
    /// against the real output and extends this with the reserve surface.
    fn geometry(&self) -> Geometry {
        let settings = self.settings();
        // The placeholder is a whole output, so its two dimensions are
        // valid by construction.
        let output = OutputSize::new(PLACEHOLDER_OUTPUT_WIDTH, PLACEHOLDER_OUTPUT_HEIGHT)
            .expect("the placeholder output has a width and a height");
        let across = settings.size().resolve(settings.edge(), output);
        Geometry::new(
            settings.edge(),
            across,
            settings.gutters(),
            settings.reserve(),
        )
    }
}

/// One app connection's `wl_display` handler (D4): it creates the proxy's own
/// registry upstream once, so the proxy can bind `zwlr_layer_shell_v1`, and it
/// installs a [`ClientRegistry`] on every registry the app asks for.
pub(super) struct ClientDisplay {
    routing: Rc<Routing>,
    /// The client's display, kept weak so the handler does not keep the
    /// object alive; it outlives the handler because the client owns it.
    display: Weak<WlDisplay>,
    /// Whether the proxy's own registry has been created. The app may call
    /// `get_registry` more than once; one proxy registry serves the panel.
    init: bool,
}

impl ClientDisplay {
    /// The display handler of one app connection, whose display is `display`.
    pub(super) fn new(routing: Rc<Routing>, display: &Rc<WlDisplay>) -> Self {
        Self {
            routing,
            display: Rc::downgrade(display),
            init: false,
        }
    }
}

impl WlDisplayHandler for ClientDisplay {
    fn handle_get_registry(&mut self, _slf: &Rc<WlDisplay>, registry: &Rc<WlRegistry>) {
        let Some(display) = self.display.upgrade() else {
            return;
        };
        if !self.init {
            self.init = true;
            // The proxy's registry is created before the app's, so the
            // compositor's globals for it arrive first and the layer-shell
            // object is bound before the app can create a toplevel.
            let wl_registry = display.create_child::<WlRegistry>();
            wl_registry.set_handler(ProxyRegistry {
                routing: Rc::clone(&self.routing),
                registry: Rc::downgrade(&wl_registry),
            });
            display.send_get_registry(&wl_registry);
        }
        display.send_get_registry(registry);
        registry.set_handler(ClientRegistry {
            routing: Rc::clone(&self.routing),
            registry: Rc::downgrade(registry),
        });
    }
}

/// The proxy's own upstream registry handler (D4): it binds the layer-shell
/// object and nothing else. The app never sees this registry.
struct ProxyRegistry {
    routing: Rc<Routing>,
    /// The registry this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    registry: Weak<WlRegistry>,
}

impl WlRegistryHandler for ProxyRegistry {
    fn handle_global(
        &mut self,
        _slf: &Rc<WlRegistry>,
        name: u32,
        interface: ObjectInterface,
        version: u32,
    ) {
        if interface != ObjectInterface::ZwlrLayerShellV1 {
            return;
        }
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        // Version 5 is the interface's XML version and the one wl-paper
        // binds; a compositor that implements less gets its version.
        let layer_shell = registry
            .state()
            .create_object::<ZwlrLayerShellV1>(version.min(5));
        let concrete = Rc::clone(&layer_shell);
        let bound: Rc<dyn Object> = concrete;
        registry.send_bind(name, bound);
        self.routing.set_layer_shell(layer_shell);
    }
}

/// The app's registry handler (D4): it installs the xdg-shell handler on the
/// app's `xdg_wm_base` bind and forwards every other bind unchanged.
struct ClientRegistry {
    routing: Rc<Routing>,
    /// The app's registry, kept weak so the handler does not keep its own
    /// object alive.
    registry: Weak<WlRegistry>,
}

impl WlRegistryHandler for ClientRegistry {
    fn handle_bind(&mut self, _slf: &Rc<WlRegistry>, name: u32, id: Rc<dyn Object>) {
        if id.interface() == ObjectInterface::XdgWmBase {
            let xdg_wm_base = id.downcast::<XdgWmBase>();
            xdg_wm_base.set_handler(ClientXdgWmBase {
                routing: Rc::clone(&self.routing),
                xdg_wm_base: Rc::downgrade(&xdg_wm_base),
            });
        }
        if let Some(registry) = self.registry.upgrade() {
            registry.send_bind(name, id);
        }
    }
}

/// The app's `xdg_wm_base` handler (D4): it holds every new `xdg_surface`
/// back from the compositor until the surface's role is known.
struct ClientXdgWmBase {
    routing: Rc<Routing>,
    /// The wm-base object this handler answers, kept weak so the handler
    /// does not keep its own object alive.
    xdg_wm_base: Weak<XdgWmBase>,
}

impl XdgWmBaseHandler for ClientXdgWmBase {
    fn handle_get_xdg_surface(
        &mut self,
        _slf: &Rc<XdgWmBase>,
        id: &Rc<XdgSurface>,
        surface: &Rc<WlSurface>,
    ) {
        let Some(xdg_wm_base) = self.xdg_wm_base.upgrade() else {
            return;
        };
        let xdg_surface = ClientXdgSurface {
            routing: Rc::clone(&self.routing),
            wl_surface: Rc::clone(surface),
            xdg_surface: Rc::clone(id),
            xdg_wm_base,
            layer_surface: None,
            has_server: false,
        };
        id.set_handler(xdg_surface);
    }
}

/// One app `xdg_surface` (D4): held back until its toplevel role appears,
/// then either the panel's layer surface or a forwarded normal window.
struct ClientXdgSurface {
    routing: Rc<Routing>,
    wl_surface: Rc<WlSurface>,
    xdg_surface: Rc<XdgSurface>,
    xdg_wm_base: Rc<XdgWmBase>,
    /// The layer surface while this `xdg_surface` is the panel's window.
    layer_surface: Option<Rc<ZwlrLayerSurfaceV1>>,
    /// Whether the compositor has this `xdg_surface` as an upstream object:
    /// false while the surface is held back, true after a pass-through
    /// forward.
    has_server: bool,
}

impl ClientXdgSurface {
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
    /// from pinwin's settings.
    fn take_slot(&mut self, layer_shell: &Rc<ZwlrLayerShellV1>, id: &Rc<XdgToplevel>) {
        let geometry = self.routing.geometry();
        let layer_surface = layer_shell.create_child::<ZwlrLayerSurfaceV1>();
        layer_shell.send_get_layer_surface(
            &layer_surface,
            &self.wl_surface,
            None,
            layer_of(self.routing.settings().layer()),
            NAMESPACE,
        );
        let (width, height) = geometry.size();
        layer_surface.send_set_size(width, height);
        let margins = geometry.margins();
        layer_surface.send_set_margin(
            margins.top(),
            margins.right(),
            margins.bottom(),
            margins.left(),
        );
        layer_surface.send_set_anchor(anchor_of(geometry.anchors()));
        layer_surface
            .send_set_keyboard_interactivity(keyboard_of(self.routing.settings().keyboard()));
        if id.version() >= XdgToplevel::MSG__WM_CAPABILITIES__SINCE {
            id.send_wm_capabilities(&[]);
        }
        // The toplevel stays local: the app sees a normal window, but every
        // request on this role is answered by the proxy (D4).
        id.set_forward_to_server(false);
        id.set_handler(ClientXdgToplevel {
            routing: Rc::clone(&self.routing),
            layer_surface: Rc::clone(&layer_surface),
            window: id.unique_id(),
        });
        layer_surface.set_handler(ProxyLayerSurface {
            xdg_surface: Rc::clone(&self.xdg_surface),
            xdg_toplevel: Rc::clone(id),
        });
        self.layer_surface = Some(layer_surface);
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
        if self.routing.route(id.unique_id()) == Route::PassedThrough {
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
        // The panel surface's window geometry is kept for the viewport crop
        // (D8) by row 6.4; a held surface has no upstream object to tell.
        if self.has_server {
            self.xdg_surface
                .send_set_window_geometry(x, y, width, height);
        }
    }

    fn handle_ack_configure(&mut self, _slf: &Rc<XdgSurface>, serial: u32) {
        if let Some(layer) = &self.layer_surface {
            // The app acks the layer surface's configure through its local
            // xdg_surface (D4).
            layer.send_ack_configure(serial);
        } else if self.has_server {
            self.xdg_surface.send_ack_configure(serial);
        }
    }
}

/// The panel's local `xdg_toplevel` handler (D4): the app destroying the
/// panel window destroys the layer surface, empties the slot and releases
/// the strip.
struct ClientXdgToplevel {
    routing: Rc<Routing>,
    layer_surface: Rc<ZwlrLayerSurfaceV1>,
    /// The panel window's unique id, which empties the slot when the app
    /// destroys it.
    window: u64,
}

impl XdgToplevelHandler for ClientXdgToplevel {
    fn handle_destroy(&mut self, _slf: &Rc<XdgToplevel>) {
        self.layer_surface.send_destroy();
        self.routing.release(self.window);
    }
}

/// The proxy's layer surface (D4): the compositor's configure becomes the
/// app's `xdg_toplevel.configure` and `xdg_surface.configure` with the same
/// serial; `closed` becomes the app's `close`. Copied from wl-paper's
/// `paper.rs` `ProxyZwlrLayerSurfaceV1` (GPL-3.0).
struct ProxyLayerSurface {
    xdg_surface: Rc<XdgSurface>,
    xdg_toplevel: Rc<XdgToplevel>,
}

impl ZwlrLayerSurfaceV1Handler for ProxyLayerSurface {
    fn delete_id(&mut self, _slf: &Rc<ZwlrLayerSurfaceV1>) {
        // Break the handler cycle between the panel toplevel and its layer
        // surface, then release the app's local toplevel id; the layer
        // surface object drops with this handler.
        self.xdg_toplevel.unset_handler();
        self.xdg_toplevel.delete_id();
    }

    fn handle_configure(
        &mut self,
        _slf: &Rc<ZwlrLayerSurfaceV1>,
        serial: u32,
        width: u32,
        height: u32,
    ) {
        let states = toplevel_states(&self.xdg_toplevel);
        if self.xdg_toplevel.version() >= XdgToplevel::MSG__CONFIGURE_BOUNDS__SINCE {
            self.xdg_toplevel
                .send_configure_bounds(width.cast_signed(), height.cast_signed());
        }
        self.xdg_toplevel
            .send_configure(width.cast_signed(), height.cast_signed(), &states);
        self.xdg_surface.send_configure(serial);
    }

    fn handle_closed(&mut self, _slf: &Rc<ZwlrLayerSurfaceV1>) {
        self.xdg_toplevel.send_close();
    }
}

/// The tiled and constrained states the panel window is configured with (D4,
/// copied from wl-paper): the app may lay its content out as a tiled window,
/// and each state is only sent when its version supports it. The states are
/// a native-endian `u32` array, as the protocol requires.
fn toplevel_states(toplevel: &XdgToplevel) -> Vec<u8> {
    // Eight states at four bytes each: the tiled and constrained sets.
    let mut states = Vec::with_capacity(8 * 4);
    if toplevel.version() >= XdgToplevel::ENM__STATE_TILED_TOP__SINCE {
        for state in [
            XdgToplevelState::TILED_LEFT,
            XdgToplevelState::TILED_TOP,
            XdgToplevelState::TILED_RIGHT,
            XdgToplevelState::TILED_BOTTOM,
        ] {
            states.extend_from_slice(&state.0.to_ne_bytes());
        }
    }
    if toplevel.version() >= XdgToplevel::ENM__STATE_CONSTRAINED_TOP__SINCE {
        for state in [
            XdgToplevelState::CONSTRAINED_LEFT,
            XdgToplevelState::CONSTRAINED_TOP,
            XdgToplevelState::CONSTRAINED_RIGHT,
            XdgToplevelState::CONSTRAINED_BOTTOM,
        ] {
            states.extend_from_slice(&state.0.to_ne_bytes());
        }
    }
    states
}

/// The protocol anchor bits of a panel's anchors (D9).
const fn anchor_of(anchors: Anchors) -> ZwlrLayerSurfaceV1Anchor {
    let mut anchor = ZwlrLayerSurfaceV1Anchor::empty();
    if anchors.contains(Edge::Left) {
        anchor = anchor.union(ZwlrLayerSurfaceV1Anchor::LEFT);
    }
    if anchors.contains(Edge::Right) {
        anchor = anchor.union(ZwlrLayerSurfaceV1Anchor::RIGHT);
    }
    if anchors.contains(Edge::Top) {
        anchor = anchor.union(ZwlrLayerSurfaceV1Anchor::TOP);
    }
    if anchors.contains(Edge::Bottom) {
        anchor = anchor.union(ZwlrLayerSurfaceV1Anchor::BOTTOM);
    }
    anchor
}

/// The protocol layer of a panel's layer setting.
const fn layer_of(layer: Layer) -> ZwlrLayerShellV1Layer {
    match layer {
        Layer::Background => ZwlrLayerShellV1Layer::BACKGROUND,
        Layer::Bottom => ZwlrLayerShellV1Layer::BOTTOM,
        Layer::Top => ZwlrLayerShellV1Layer::TOP,
        Layer::Overlay => ZwlrLayerShellV1Layer::OVERLAY,
    }
}

/// The protocol keyboard mode of a panel's keyboard setting.
const fn keyboard_of(keyboard: Keyboard) -> ZwlrLayerSurfaceV1KeyboardInteractivity {
    match keyboard {
        Keyboard::None => ZwlrLayerSurfaceV1KeyboardInteractivity::NONE,
        Keyboard::OnDemand => ZwlrLayerSurfaceV1KeyboardInteractivity::ON_DEMAND,
        Keyboard::Exclusive => ZwlrLayerSurfaceV1KeyboardInteractivity::EXCLUSIVE,
    }
}
