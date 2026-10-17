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
use wl_proxy::object::Object;
use wl_proxy::protocols::xdg_shell::xdg_toplevel::{XdgToplevel, XdgToplevelHandler};

use super::routing::Routing;

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
}

impl ClientXdgToplevel {
    /// The handler of the toplevel `toplevel` of the panel named by `window`,
    /// whose routing state is `routing`.
    pub(super) fn new(routing: Rc<Routing>, toplevel: &Rc<XdgToplevel>, window: u64) -> Self {
        Self {
            routing,
            toplevel: Rc::downgrade(toplevel),
            window,
        }
    }
}

impl XdgToplevelHandler for ClientXdgToplevel {
    fn handle_destroy(&mut self, _slf: &Rc<XdgToplevel>) {
        // The routing holds this layer surface and its handler holds this
        // toplevel: unset the handler here, as wl-paper's does, so the two
        // objects cannot keep each other alive past the app's destroy.
        if let Some(toplevel) = self.toplevel.upgrade() {
            toplevel.unset_handler();
        }
        // D11: destroy the panel's current layer surface — a `set-output` or
        // a `closed` may have replaced the one this window docked with.
        if let Some(layer_surface) = self.routing.panel_layer_surface() {
            layer_surface.send_destroy();
        }
        // D9: the reserve surface is destroyed with the panel window it was
        // created for, which also frees the slot and the strip.
        self.routing.on_panel_window_destroyed(self.window);
    }
}

/// A client connection went away without destroying its toplevel (row 7.3,
/// D3, D4): when it owned the panel's slot, the window's layer surface and
/// reserve go with it and the next window can take the panel.
pub(super) fn client_disconnected(routing: &Routing, client: &Weak<Client>) {
    let Some(window) = routing.client_window(client) else {
        return;
    };
    if let Some(layer_surface) = routing.panel_layer_surface() {
        layer_surface.send_destroy();
    }
    routing.on_panel_window_destroyed(window);
}
