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
//! The handler parameters the proxy's own object would carry are named
//! `_slf`, the trait's name with the unused-prefix the codebase's lints
//! require, and a handler that needs its own object reaches it through a
//! `Weak` field instead of the parameter: a handler that held its own object
//! in an `Rc` would close a reference cycle, so the display, registry,
//! wm-base, toplevel and layer-surface handlers keep their own object weak
//! and keep the other objects they act on.
//!
//! The layer surface here is the minimal one rows 6.2 and 6.3 run with:
//! anchors, size, margins, layer and keyboard mode from [`PanelSettings`],
//! and a percent size resolved against a placeholder 1920x1080 output until
//! row 6.4 reads the real output size from `zxdg_output_v1`. Popups and
//! decorations are row 6.3 (D5): a popup of the panel window becomes a popup
//! of its layer surface with the anchor rectangle moved by the window
//! geometry ([`super::popup`]), and each decoration object is routed by its
//! toplevel — answered server-side for the panel window, forwarded to the
//! compositor's real manager or answered client-side for a passed-through
//! window ([`super::decoration`]).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use wl_proxy::object::{Object, ObjectCoreApi, ObjectRcUtils, ObjectUtils};
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
use wl_proxy::protocols::xdg_decoration_unstable_v1::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wl_proxy::protocols::xdg_shell::xdg_popup::XdgPopup;
use wl_proxy::protocols::xdg_shell::xdg_positioner::XdgPositioner;
use wl_proxy::protocols::xdg_shell::xdg_surface::{XdgSurface, XdgSurfaceHandler};
use wl_proxy::protocols::xdg_shell::xdg_toplevel::{
    XdgToplevel, XdgToplevelHandler, XdgToplevelState,
};
use wl_proxy::protocols::xdg_shell::xdg_wm_base::{XdgWmBase, XdgWmBaseHandler};

use super::decoration::{ClientDecorationManager, ClientSyntheticDecorationManager};
use super::popup::{ClientXdgPopup, ClientXdgPositioner, offset_anchor_rect};
use super::slot::{Route, Slot};
use crate::config::{Keyboard, Layer, PanelSettings};
use crate::geometry::{Anchors, Edge, Geometry, OutputSize};

/// Whether `toplevel` is the panel's window (D5): only the slot's toplevel
/// carries a handler, a passed-through window uses the default forward path,
/// and a toplevel whose panel window ended has its handler unset. This is
/// what routes a decoration object at the time of each request.
pub(super) fn is_panel_window(toplevel: &XdgToplevel) -> bool {
    toplevel.try_get_handler_ref::<ClientXdgToplevel>().is_ok()
}

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

/// The global name the proxy's synthetic decoration manager is announced
/// under (D5): a compositor's global names start at 1, so the app's bind of
/// name 0 can only be the proxy's own manager and is never forwarded
/// upstream.
const SYNTHETIC_GLOBAL_NAME: u32 = 0;

/// The version the synthetic decoration manager is announced with: version 1
/// is the interface's only version.
const SYNTHETIC_DECORATION_VERSION: u32 = 1;

/// The routing state one panel shares between all its app connections (D4):
/// the slot, the settings the panel's layer surface is built from, and the
/// compositor's layer-shell object once the proxy's registry found it. One
/// state per panel, not per connection, so a second connection sees the
/// window the first one created.
pub(super) struct Routing {
    settings: PanelSettings,
    slot: Cell<Slot>,
    layer_shell: RefCell<Option<Rc<ZwlrLayerShellV1>>>,
    /// The compositor's `zxdg_decoration_manager_v1`, bound by the proxy's
    /// own registry (D5): a passed-through window's decoration object is
    /// forwarded to it. `None` while the compositor announces none, which is
    /// when the app's registries hear the proxy's synthetic one instead.
    decoration_manager: RefCell<Option<Rc<ZxdgDecorationManagerV1>>>,
}

impl Routing {
    /// The routing state for a panel with `settings`.
    pub(super) fn new(settings: PanelSettings) -> Rc<Self> {
        Rc::new(Self {
            settings,
            slot: Cell::new(Slot::new()),
            layer_shell: RefCell::new(None),
            decoration_manager: RefCell::new(None),
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
    fn set_decoration_manager(&self, manager: Rc<ZxdgDecorationManagerV1>) {
        *self.decoration_manager.borrow_mut() = Some(manager);
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
            announced_synthetic_manager: false,
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
        }
    }
}

/// The app's registry handler (D4, D5): it announces the proxy's synthetic
/// decoration manager when the compositor has none, installs the xdg-shell
/// handler on the app's `xdg_wm_base` bind and the per-window decoration
/// handler on its decoration manager bind, and forwards every other bind
/// unchanged.
struct ClientRegistry {
    routing: Rc<Routing>,
    /// The app's registry, kept weak so the handler does not keep its own
    /// object alive.
    registry: Weak<WlRegistry>,
    /// Whether this registry already heard the proxy's synthetic decoration
    /// manager (D5): one announcement per registry, and only while the
    /// compositor has no real one.
    announced_synthetic_manager: bool,
}

impl WlRegistryHandler for ClientRegistry {
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
        if !self.announced_synthetic_manager && !self.routing.has_decoration_manager() {
            self.announced_synthetic_manager = true;
            // D5: without the compositor's manager the proxy offers its own,
            // so a panel window is still told server-side decorations. The
            // app's first global cannot arrive before the proxy's registry
            // saw the compositor's whole list — both lists are answered in
            // the order their `get_registry` requests were sent — so the
            // decision is made on the complete list.
            registry.send_global(
                SYNTHETIC_GLOBAL_NAME,
                ObjectInterface::ZxdgDecorationManagerV1,
                SYNTHETIC_DECORATION_VERSION,
            );
        }
        registry.send_global(name, interface, version);
    }

    fn handle_bind(&mut self, _slf: &Rc<WlRegistry>, name: u32, id: Rc<dyn Object>) {
        if name == SYNTHETIC_GLOBAL_NAME {
            if id.interface() == ObjectInterface::ZxdgDecorationManagerV1 {
                // The proxy's synthetic manager (D5): its decoration objects
                // are answered by the proxy, so the bind is never forwarded.
                let manager = id.downcast::<ZxdgDecorationManagerV1>();
                manager.set_handler(ClientSyntheticDecorationManager::new(&manager));
            } else {
                // A bind of the proxy's own name for another interface names
                // no compositor global: the object stays local instead of
                // erroring upstream.
                id.set_forward_to_server(false);
            }
            return;
        }
        if id.interface() == ObjectInterface::XdgWmBase {
            let xdg_wm_base = id.downcast::<XdgWmBase>();
            xdg_wm_base.set_handler(ClientXdgWmBase {
                routing: Rc::clone(&self.routing),
                xdg_wm_base: Rc::downgrade(&xdg_wm_base),
            });
        }
        if id.interface() == ObjectInterface::ZxdgDecorationManagerV1 {
            // D5: the compositor's real manager is kept, and each decoration
            // object it creates is routed by its toplevel.
            let manager = id.downcast::<ZxdgDecorationManagerV1>();
            manager.set_handler(ClientDecorationManager::new(Rc::clone(&self.routing)));
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
    fn handle_create_positioner(&mut self, _slf: &Rc<XdgWmBase>, id: &Rc<XdgPositioner>) {
        // The popup offset needs the anchor rectangle the app set (D5): the
        // handler records it while the request itself is forwarded (wl-paper).
        id.set_handler(ClientXdgPositioner::new(id));
        let Some(xdg_wm_base) = self.xdg_wm_base.upgrade() else {
            return;
        };
        xdg_wm_base.send_create_positioner(id);
    }

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
            geometry: None,
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
    /// The window geometry the app set on this surface, `[x, y, width,
    /// height]` (D5): a popup of the panel window offsets its anchor
    /// rectangle by the panel window's one, and row 6.4 crops the panel
    /// window's buffer to it (D8). A surface the app set none on keeps
    /// `None`.
    geometry: Option<[i32; 4]>,
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
            toplevel: Rc::downgrade(id),
            window: id.unique_id(),
        });
        layer_surface.set_handler(ProxyLayerSurface {
            xdg_surface: Rc::clone(&self.xdg_surface),
            xdg_toplevel: Rc::clone(id),
            layer_surface: Rc::downgrade(&layer_surface),
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
        // The window geometry drives the popup anchor offset (D5) and, from
        // row 6.4, the panel window's viewport crop (D8); every surface keeps
        // what the app asked for, and a held surface sends nothing upstream
        // yet.
        self.geometry = Some([x, y, width, height]);
        if self.has_server {
            self.xdg_surface
                .send_set_window_geometry(x, y, width, height);
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
            && let Some(parent_layer) = &parent_handler.layer_surface
        {
            layer_surface = Some(Rc::clone(parent_layer));
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
    /// The toplevel this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    toplevel: Weak<XdgToplevel>,
    /// The panel window's unique id, which empties the slot when the app
    /// destroys it.
    window: u64,
}

impl XdgToplevelHandler for ClientXdgToplevel {
    fn handle_destroy(&mut self, _slf: &Rc<XdgToplevel>) {
        // The handler holds the layer surface, whose own handler holds this
        // toplevel: unset it here, as wl-paper's does, so the two objects
        // cannot keep each other alive past the app's destroy.
        if let Some(toplevel) = self.toplevel.upgrade() {
            toplevel.unset_handler();
        }
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
    /// The layer surface this handler answers, kept weak so the handler does
    /// not keep its own object alive.
    layer_surface: Weak<ZwlrLayerSurfaceV1>,
}

impl ZwlrLayerSurfaceV1Handler for ProxyLayerSurface {
    fn delete_id(&mut self, _slf: &Rc<ZwlrLayerSurfaceV1>) {
        // Drop this handler first: it holds the app's toplevel and the
        // panel's xdg_surface, so unsetting it breaks the handler cycle
        // (wl-paper), and then free the app's toplevel id, which died with
        // the layer surface the compositor destroyed.
        if let Some(layer_surface) = self.layer_surface.upgrade() {
            layer_surface.unset_handler();
        }
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
