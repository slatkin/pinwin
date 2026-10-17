//! The panel window's local toplevel and the slot's owner
//! (replace-terminal-with-app-host D3, D4, row 7.3): the `xdg_toplevel`
//! handler that tears the panel window down, and the cleanup of a client
//! that disconnects without destroying its toplevel.
//!
//! The slot is empty or holds one window ([`super::slot`]); a connection can
//! go away without destroying that window, because a dying client is not
//! asked to destroy its objects (D3). [`client_disconnected`] clears the slot
//! only for the connection that took it, so another connection's window
//! stays the panel and a later window can take the panel again.

use std::rc::{Rc, Weak};

use wl_proxy::client::Client;
use wl_proxy::object::{Object, ObjectUtils};
use wl_proxy::protocols::xdg_shell::xdg_toplevel::{XdgToplevel, XdgToplevelHandler};

use super::ring;
use super::routing::Routing;
use crate::daemon::panel::AppId;

/// Whether `toplevel` is the panel's window (D5): only the slot's toplevel
/// carries a handler, a passed-through window uses the default forward path,
/// and a toplevel whose panel window ended has its handler unset. This is
/// what routes a decoration object at the time of each request.
pub(super) fn is_panel_window(toplevel: &XdgToplevel) -> bool {
    toplevel.try_get_handler_ref::<ClientXdgToplevel>().is_ok()
}

/// The panel's local `xdg_toplevel` handler (D4): the app destroying the
/// panel window destroys the layer surface, empties the slot and releases
/// the strip.
pub(super) struct ClientXdgToplevel {
    routing: Rc<Routing>,
    /// The toplevel this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    toplevel: Weak<XdgToplevel>,
    /// The panel window's unique id, which empties the slot when the app
    /// destroys it.
    window: u64,
    /// The hosted app's `app_id`, which the app's `set_app_id` records for the
    /// tray icon (row 11.1, D14).
    app_id: AppId,
}

impl ClientXdgToplevel {
    /// The handler of the toplevel `toplevel` of the panel named by `window`,
    /// whose routing state is `routing`, recording the app's id in `app_id`.
    pub(super) fn new(
        routing: Rc<Routing>,
        toplevel: &Rc<XdgToplevel>,
        window: u64,
        app_id: AppId,
    ) -> Self {
        Self {
            routing,
            toplevel: Rc::downgrade(toplevel),
            window,
            app_id,
        }
    }
}

impl XdgToplevelHandler for ClientXdgToplevel {
    /// The app names itself (row 11.1, D14): the panel window stays local, so
    /// this request reaches no compositor, but the tray names the oldest
    /// panel's app. A later id replaces the earlier one.
    fn handle_set_app_id(&mut self, _slf: &Rc<XdgToplevel>, app_id: &str) {
        self.app_id.set(app_id);
    }
    fn handle_destroy(&mut self, _slf: &Rc<XdgToplevel>) {
        // The routing holds this layer surface and its handler holds this
        // toplevel: unset the handler here, as wl-paper's does, so the two
        // objects cannot keep each other alive past the app's destroy.
        if let Some(toplevel) = self.toplevel.upgrade() {
            toplevel.unset_handler();
        }
        teardown(&self.routing, self.window);
    }
}

/// End the panel window `window`: its layer surface, reserve and focus ring
/// go with it and the slot is free.
fn teardown(routing: &Routing, window: u64) {
    // D11: destroy the panel's current layer surface — a `set-output` or a
    // `closed` may have replaced the one this window docked with.
    if let Some(layer_surface) = routing.panel_layer_surface() {
        layer_surface.send_destroy();
    }
    // D9: the reserve surface is destroyed with the panel window it was
    // created for, which also frees the slot and the strip; D10: the focus
    // ring's sub-surfaces go with the window they were laid over.
    ring::window_destroyed(routing);
    routing.on_panel_window_destroyed(window);
}

/// A client connection went away without destroying its toplevel (row 7.3,
/// D3, D4): when it owned the panel's slot, the window's layer surface and
/// reserve go with it and the next window can take the panel.
pub(super) fn client_disconnected(routing: &Routing, client: &Weak<Client>) {
    let Some(window) = routing.client_window(client) else {
        return;
    };
    teardown(routing, window);
}
