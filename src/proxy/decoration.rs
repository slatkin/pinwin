//! The per-window decoration routing (replace-terminal-with-app-host D5):
//! every `zxdg_toplevel_decoration_v1` object is handled by its toplevel.
//!
//! The panel window's decoration is answered locally with server-side mode,
//! so the app draws no title bar and the compositor draws none on a layer
//! surface, and it is answered again on every later `set_mode` and
//! `unset_mode`. A passed-through window's decoration is forwarded to the
//! compositor's real `zxdg_decoration_manager_v1`, which the proxy binds,
//! or — when the compositor has none — answered client-side by the proxy's
//! synthetic manager, which is what the app does without the global. An app
//! can create the decoration object before its window enters or leaves the
//! slot, so the route is read from the toplevel's state at the time of each
//! request and never cached at creation.
//!
//! The shape of the local answers is wl-paper's `apps/wl-paper/src/paper.rs`
//! `ClientZxdgToplevelDecorationV1` (<https://github.com/mahkoh/wl-proxy>,
//! GPL-3.0, the licence of this crate); the routing differs from wl-paper,
//! which answers server-side for every window and hides the compositor's real
//! manager. The decision itself is [`route_decoration`], a pure function of
//! the toplevel's state and the manager's presence (`port-to-rust` D10), so
//! its matrix is unit tested without a compositor.

use std::rc::{Rc, Weak};

use wl_proxy::object::ObjectCoreApi;
use wl_proxy::protocols::xdg_decoration_unstable_v1::zxdg_decoration_manager_v1::{
    ZxdgDecorationManagerV1, ZxdgDecorationManagerV1Handler,
};
use wl_proxy::protocols::xdg_decoration_unstable_v1::zxdg_toplevel_decoration_v1::{
    ZxdgToplevelDecorationV1, ZxdgToplevelDecorationV1Handler, ZxdgToplevelDecorationV1Mode,
};
use wl_proxy::protocols::xdg_shell::xdg_toplevel::XdgToplevel;

use super::routing::Routing;
use super::window::is_panel_window;

/// What a decoration object becomes (D5): answered by the proxy with
/// server-side mode, forwarded to the compositor's manager, or answered by
/// the proxy with client-side mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecorationAction {
    /// The proxy answers the object itself with server-side mode: the panel
    /// window's decoration, whatever manager the app used.
    AnswerServerSide,
    /// The object goes to the compositor's real manager unchanged.
    Forward,
    /// The proxy answers the object itself with client-side mode: a
    /// passed-through window's decoration out of the synthetic manager, which
    /// is what the app does without the global.
    AnswerClientSide,
}

/// The route of one decoration object (D5): the panel window always gets the
/// proxy's server-side answer, a passed-through window is forwarded when the
/// compositor has a real manager and answered client-side when only the
/// proxy's synthetic one exists.
pub(super) fn route_decoration(is_panel_window: bool, manager_present: bool) -> DecorationAction {
    if is_panel_window {
        DecorationAction::AnswerServerSide
    } else if manager_present {
        DecorationAction::Forward
    } else {
        DecorationAction::AnswerClientSide
    }
}

/// The app's handler for the compositor's real decoration manager (D5): the
/// bind itself is forwarded, so the app sees the compositor's manager, and
/// each decoration object it creates is routed by its toplevel. `destroy`
/// keeps the default forward path.
pub(super) struct ClientDecorationManager {
    routing: Rc<Routing>,
}

impl ClientDecorationManager {
    /// The handler of one app's bind of the compositor's manager (D5).
    pub(super) fn new(routing: Rc<Routing>) -> Self {
        Self { routing }
    }
}

impl ZxdgDecorationManagerV1Handler for ClientDecorationManager {
    fn handle_get_toplevel_decoration(
        &mut self,
        _slf: &Rc<ZxdgDecorationManagerV1>,
        id: &Rc<ZxdgToplevelDecorationV1>,
        toplevel: &Rc<XdgToplevel>,
    ) {
        // The manager is read at the time of the request, not at the bind:
        // the routing state is the one the whole panel shares (D5).
        route_object(self.routing.decoration_manager().as_ref(), id, toplevel);
    }
}

/// The app's handler for the proxy's synthetic decoration manager (D5): the
/// compositor has no manager, so the bind is never forwarded and every
/// decoration object is answered by the proxy. `destroy` frees the app's id,
/// which no upstream object would otherwise do.
pub(super) struct ClientSyntheticDecorationManager {
    /// The manager this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    manager: Weak<ZxdgDecorationManagerV1>,
}

impl ClientSyntheticDecorationManager {
    /// The handler of the synthetic manager `manager` (D5).
    pub(super) fn new(manager: &Rc<ZxdgDecorationManagerV1>) -> Self {
        Self {
            manager: Rc::downgrade(manager),
        }
    }
}

impl ZxdgDecorationManagerV1Handler for ClientSyntheticDecorationManager {
    fn handle_destroy(&mut self, _slf: &Rc<ZxdgDecorationManagerV1>) {
        if let Some(manager) = self.manager.upgrade() {
            manager.delete_id();
        }
    }

    fn handle_get_toplevel_decoration(
        &mut self,
        _slf: &Rc<ZxdgDecorationManagerV1>,
        id: &Rc<ZxdgToplevelDecorationV1>,
        toplevel: &Rc<XdgToplevel>,
    ) {
        route_object(None, id, toplevel);
    }
}

/// Routes one decoration object by its toplevel's state at the time of the
/// request (D5). `real` is the compositor's manager when there is one, so
/// [`DecorationAction::Forward`] forwards to it; without one the decision
/// falls to [`DecorationAction::AnswerClientSide`] and the arm stays empty,
/// because there is no manager to forward to.
fn route_object(
    real: Option<&Rc<ZxdgDecorationManagerV1>>,
    id: &Rc<ZxdgToplevelDecorationV1>,
    toplevel: &Rc<XdgToplevel>,
) {
    match route_decoration(is_panel_window(toplevel), real.is_some()) {
        DecorationAction::AnswerServerSide => {
            answer_toplevel_decoration(id, ZxdgToplevelDecorationV1Mode::SERVER_SIDE);
        }
        DecorationAction::Forward => {
            if let Some(manager) = real {
                manager.send_get_toplevel_decoration(id, toplevel);
            }
        }
        DecorationAction::AnswerClientSide => {
            answer_toplevel_decoration(id, ZxdgToplevelDecorationV1Mode::CLIENT_SIDE);
        }
    }
}

/// Answers a decoration object locally (D5): the mode goes out at once, and
/// the object answers with the same mode again on every later `set_mode` and
/// `unset_mode`, so the app cannot talk the panel window back into a title
/// bar.
fn answer_toplevel_decoration(
    id: &Rc<ZxdgToplevelDecorationV1>,
    mode: ZxdgToplevelDecorationV1Mode,
) {
    id.set_handler(ClientToplevelDecoration {
        decoration: Rc::downgrade(id),
        mode,
    });
    id.send_configure(mode);
}

/// The app's decoration object the proxy answers itself (D5): it holds the
/// mode every answer carries. Copied from wl-paper's `paper.rs`
/// `ClientZxdgToplevelDecorationV1` (GPL-3.0), which answers server-side for
/// every window; pinwin's holds the mode the routing picked.
struct ClientToplevelDecoration {
    /// The decoration this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    decoration: Weak<ZxdgToplevelDecorationV1>,
    mode: ZxdgToplevelDecorationV1Mode,
}

impl ZxdgToplevelDecorationV1Handler for ClientToplevelDecoration {
    fn handle_destroy(&mut self, _slf: &Rc<ZxdgToplevelDecorationV1>) {
        if let Some(decoration) = self.decoration.upgrade() {
            decoration.delete_id();
        }
    }

    fn handle_set_mode(
        &mut self,
        _slf: &Rc<ZxdgToplevelDecorationV1>,
        _mode: ZxdgToplevelDecorationV1Mode,
    ) {
        if let Some(decoration) = self.decoration.upgrade() {
            decoration.send_configure(self.mode);
        }
    }

    fn handle_unset_mode(&mut self, _slf: &Rc<ZxdgToplevelDecorationV1>) {
        if let Some(decoration) = self.decoration.upgrade() {
            decoration.send_configure(self.mode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DecorationAction::{AnswerClientSide, AnswerServerSide, Forward};
    use super::route_decoration;
    use crate::proxy::slot::{Route, Slot};

    /// The panel window's decoration is answered server-side whether the
    /// compositor has a real manager or only the proxy's synthetic one (D5,
    /// spec "No decorations on the panel window").
    #[test]
    fn the_panel_window_answers_server_side() {
        assert_eq!(route_decoration(true, true), AnswerServerSide);
        assert_eq!(route_decoration(true, false), AnswerServerSide);
    }

    /// A passed-through window's decoration is forwarded when the compositor
    /// has a real manager, so it gets the decorations it would have without
    /// pinwin (spec "Dialog keeps its decorations").
    #[test]
    fn a_passed_through_window_forwards_to_a_real_manager() {
        assert_eq!(route_decoration(false, true), Forward);
    }

    /// A passed-through window's decoration is answered client-side when the
    /// compositor has no manager, which is what the app does without the
    /// global (D5).
    #[test]
    fn a_passed_through_window_answers_client_side_without_a_manager() {
        assert_eq!(route_decoration(false, false), AnswerClientSide);
    }

    /// The route is read from the toplevel's state at the time of the request
    /// (D5): the slot decides which window is the panel's, a window that
    /// arrived while the slot was full never joins it, and the slot's release
    /// frees the name without promoting the passed-through window, so its
    /// next decoration request routes the same way.
    #[test]
    fn the_decision_follows_the_slot_at_request_time() {
        let mut slot = Slot::new();
        assert_eq!(slot.route(7), Route::Panel);
        assert_eq!(slot.route(8), Route::PassedThrough);
        assert_eq!(
            route_decoration(slot.window() == Some(7), true),
            AnswerServerSide
        );
        assert_eq!(route_decoration(slot.window() == Some(8), true), Forward);
        assert!(!slot.release(8));
        assert_eq!(slot.window(), Some(7));
        assert_eq!(
            route_decoration(slot.window() == Some(7), true),
            AnswerServerSide
        );
        assert_eq!(route_decoration(slot.window() == Some(8), true), Forward);
    }
}
