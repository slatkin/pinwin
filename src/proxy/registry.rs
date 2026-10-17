//! The registry layer between the app's connections and the proxy
//! (replace-terminal-with-app-host D4, D5): the display handler that stands
//! the proxy's own upstream registry up, the app's registry handler that
//! forwards the compositor's globals and announces the proxy's synthetic
//! decoration manager, and the `xdg_wm_base` handler that holds every new
//! `xdg_surface` back until its role is known.
//!
//! Every handler here is installed from a bind or a display call, so this
//! module is where the app's view of the compositor's global list is shaped:
//! the proxy's registry binds what the panel needs on its own ([`super::output`]),
//! the app's registry hears everything else unchanged.

use std::rc::{Rc, Weak};

use wl_proxy::object::{Object, ObjectCoreApi, ObjectRcUtils};
use wl_proxy::protocols::ObjectInterface;
use wl_proxy::protocols::fractional_scale_v1::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wl_proxy::protocols::linux_dmabuf_v1::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wl_proxy::protocols::linux_drm_syncobj_v1::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1;
use wl_proxy::protocols::viewporter::wp_viewporter::WpViewporter;
use wl_proxy::protocols::wayland::wl_display::{WlDisplay, WlDisplayHandler};
use wl_proxy::protocols::wayland::wl_registry::{WlRegistry, WlRegistryHandler};
use wl_proxy::protocols::wayland::wl_seat::WlSeat;
use wl_proxy::protocols::wayland::wl_shm::WlShm;
use wl_proxy::protocols::wayland::wl_surface::WlSurface;
use wl_proxy::protocols::xdg_decoration_unstable_v1::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wl_proxy::protocols::xdg_shell::xdg_positioner::XdgPositioner;
use wl_proxy::protocols::xdg_shell::xdg_surface::XdgSurface;
use wl_proxy::protocols::xdg_shell::xdg_wm_base::{XdgWmBase, XdgWmBaseHandler};

use super::buffers::{ClientDmabuf, ClientWlShm};
use super::decoration::{ClientDecorationManager, ClientSyntheticDecorationManager};
use super::output::ProxyRegistry;
use super::popup::ClientXdgPositioner;
use super::ring::ClientWlSeat;
use super::routing::{ClientXdgSurface, Routing};
use super::syncobj::ClientSyncobjManager;
use super::viewport::{ClientFractionalScaleManager, ClientWpViewporter};
use crate::daemon::panel::AppId;

/// The global name the proxy's synthetic decoration manager is announced
/// under (D5): a compositor's global names start at 1, so the app's bind of
/// name 0 can only be the proxy's own manager and is never forwarded
/// upstream.
const SYNTHETIC_GLOBAL_NAME: u32 = 0;

/// The version the synthetic decoration manager is announced with: version 1
/// is the interface's only version.
const SYNTHETIC_DECORATION_VERSION: u32 = 1;

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
    /// The hosted app's `app_id`, which every surface handler carries to the
    /// panel window's toplevel (row 11.1, D14).
    app_id: AppId,
}

impl ClientDisplay {
    /// The display handler of one app connection, whose display is `display`.
    pub(super) fn new(routing: Rc<Routing>, display: &Rc<WlDisplay>, app_id: AppId) -> Self {
        Self {
            routing,
            display: Rc::downgrade(display),
            init: false,
            app_id,
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
            wl_registry.set_handler(ProxyRegistry::new(
                &self.routing,
                &self.routing.globals(),
                &wl_registry,
            ));
            display.send_get_registry(&wl_registry);
        }
        display.send_get_registry(registry);
        registry.set_handler(ClientRegistry {
            routing: Rc::clone(&self.routing),
            registry: Rc::downgrade(registry),
            announced_synthetic_manager: false,
            app_id: self.app_id.clone(),
        });
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
    /// The hosted app's `app_id`, passed on to every wm-base handler (row
    /// 11.1, D14).
    app_id: AppId,
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
                app_id: self.app_id.clone(),
            });
        }
        if id.interface() == ObjectInterface::ZxdgDecorationManagerV1 {
            // D5: the compositor's real manager is kept, and each decoration
            // object it creates is routed by its toplevel.
            let manager = id.downcast::<ZxdgDecorationManagerV1>();
            manager.set_handler(ClientDecorationManager::new(Rc::clone(&self.routing)));
        }
        if id.interface() == ObjectInterface::WpViewporter {
            // D8: the app's viewporter is mirrored — every viewport it
            // creates passes through with its values, and the panel
            // surface's crop decision is made per viewport.
            let viewporter = id.downcast::<WpViewporter>();
            viewporter.set_handler(ClientWpViewporter::new(&viewporter, &self.routing));
        }
        if id.interface() == ObjectInterface::WpFractionalScaleManagerV1 {
            // D8: a surface may have only one `wp_fractional_scale_v1`, so
            // the proxy watches the app's object for the panel surface's
            // preferred scale instead of asking for one itself.
            let manager = id.downcast::<WpFractionalScaleManagerV1>();
            manager.set_handler(ClientFractionalScaleManager::new(&manager, &self.routing));
        }
        if id.interface() == ObjectInterface::WlShm {
            // D8: the crop is clamped to the buffer the panel surface
            // attached, and a `wl_shm` buffer's size is only in the request
            // that creates it.
            let shm = id.downcast::<WlShm>();
            shm.set_handler(ClientWlShm::new(&shm, &self.routing.globals()));
        }
        if id.interface() == ObjectInterface::ZwpLinuxDmabufV1 {
            // D8: a dmabuf buffer's size is in its params' create request,
            // for the clients that draw from the GPU.
            let dmabuf = id.downcast::<ZwpLinuxDmabufV1>();
            dmabuf.set_handler(ClientDmabuf::new(&dmabuf, &self.routing.globals()));
        }
        if id.interface() == ObjectInterface::WpLinuxDrmSyncobjManagerV1 {
            // D6: the app's explicit-sync surfaces are observed, so the
            // panel surface's acquire and release points can be held while
            // it is hidden and emitted with a buffer-carrying commit.
            let manager = id.downcast::<WpLinuxDrmSyncobjManagerV1>();
            manager.set_handler(ClientSyncobjManager::new(
                Rc::clone(&self.routing),
                &manager,
            ));
        }
        if id.interface() == ObjectInterface::WlSeat {
            // D10: the proxy watches the keyboard focus of every keyboard the
            // app asks the seat for, which is what shows the focus ring, and
            // forwards the bind unchanged.
            let seat = id.downcast::<WlSeat>();
            seat.set_handler(ClientWlSeat::new(&self.routing, &seat));
        }
        if id.interface() == ObjectInterface::WlOutput {
            // D9: the app's `wl_output` binds carry the same global names the
            // proxy's own registry bound, so a `wl_surface.enter` on the
            // panel window can be correlated to the panel's output.
            self.routing
                .globals()
                .record_client_output(name, id.unique_id());
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
    /// The hosted app's `app_id`, passed on to every held-back surface (row
    /// 11.1, D14).
    app_id: AppId,
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
        id.set_handler(ClientXdgSurface::held_back(
            Rc::clone(&self.routing),
            Rc::clone(surface),
            Rc::clone(id),
            xdg_wm_base,
            self.app_id.clone(),
        ));
    }
}
