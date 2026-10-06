//! The panel thread's xdg-activation binding (replace-gtk-with-wayland D4,
//! row 7.2): the optional `xdg_activation_v1` global, the pure focus
//! decision and the focus command's activation of the panel surface.
//!
//! The global is optional (D1): a compositor without xdg-activation degrades
//! the focus request to a no-op, never the start. The request is
//! fire-and-forget at the protocol level — the compositor's choice is
//! invisible to the client — so the command answers `Ok(())` whether it
//! activated, skipped for the keyboard mode, or found no global.
//!
//! The binding is a direct `globals.bind` of the raw protocol object, not
//! the toolkit's [`smithay_client_toolkit::activation::ActivationState`]:
//! pinwin never mints tokens (the host supplies one), so the toolkit's
//! token-request machinery and its `ActivationHandler` obligation would only
//! add surface. The protocol has no events, so the user data is empty.
//!
//! The live activation itself is compile-only until the niri verification
//! (row 10.1); the decision and the command path are tested without a
//! display (`port-to-rust` D10).

use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

use crate::activation::ActivationToken;
use crate::layout::Keyboard;

use super::state::PanelState;

/// The user data of the bound `xdg_activation_v1` object: the protocol has
/// no events, so nothing is stored and nothing is dispatched.
#[derive(Debug)]
struct ActivationData;

impl Dispatch2<XdgActivationV1, PanelState> for ActivationData {
    fn event(
        &self,
        _state: &mut PanelState,
        _proxy: &XdgActivationV1,
        _event: <XdgActivationV1 as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<PanelState>,
    ) {
        // `xdg_activation_v1` has no events.
    }
}

/// The bound xdg-activation global of one panel thread session (D4). `None`
/// when the compositor offers no `xdg_activation_v1` global.
pub(crate) struct Activation {
    activation: Option<XdgActivationV1>,
}

impl Activation {
    /// Bind `xdg_activation_v1` (version 1 only), or hold `None` when the
    /// global is missing — an optional global (D1).
    pub(crate) fn bind(globals: &GlobalList, qh: &QueueHandle<PanelState>) -> Self {
        Activation {
            activation: globals.bind(qh, 1..=1, ActivationData).ok(),
        }
    }

    /// Whether the global is bound: the input of the pure focus decision.
    pub(crate) fn is_bound(&self) -> bool {
        self.activation.is_some()
    }

    /// Pass `token` to the compositor as an xdg-activation request for
    /// `surface`. Nothing replies: the compositor's choice is invisible to
    /// the client (D4).
    fn activate(&self, surface: &WlSurface, token: &ActivationToken) {
        let Some(activation) = &self.activation else {
            return;
        };
        activation.activate(token.as_str().to_owned(), surface);
    }
}

/// What the focus command does with its token (D4, row 7.2): activate the
/// panel surface, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusAction {
    /// Pass the token to the compositor for the panel surface.
    Activate,
    /// Do nothing: the keyboard mode is not `on-demand`, or the session has
    /// no xdg-activation global.
    Nothing,
}

/// The pure focus decision (row 7.2): only an `on-demand` panel with a bound
/// xdg-activation global activates; every other combination is a no-op, the
/// way the host's mode short-circuit and the optional global degrade it.
pub(crate) fn focus_action(keyboard: Keyboard, activation: bool) -> FocusAction {
    if keyboard == Keyboard::OnDemand && activation {
        FocusAction::Activate
    } else {
        FocusAction::Nothing
    }
}

/// The focus command's body: run the pure decision against the session, and
/// activate the panel surface when it says so. The command answers `Ok(())`
/// either way (D4) — its caller sends the reply.
pub(crate) fn on_focus(state: &mut PanelState, token: &ActivationToken) {
    let Some(session) = state.session.as_ref() else {
        // The session is not bound yet (or is torn down): no surface, so
        // nothing to activate.
        return;
    };
    if focus_action(state.startup.keyboard, session.activation.is_bound()) != FocusAction::Activate
    {
        return;
    }
    let Some(surfaces) = session.surfaces.as_ref() else {
        // The surfaces are gone (the watchdog tore the panel down): nothing
        // to activate.
        return;
    };
    session
        .activation
        .activate(surfaces.panel_wl_surface(), token);
}

#[cfg(test)]
mod tests {
    use super::super::super::handshake::Handshake;
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::layout::{CellSize, Layout, Side};
    use crate::panel::wayland_side::Inner;
    use std::num::NonZeroU16;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, mpsc};

    /// A valid test token.
    fn token() -> ActivationToken {
        ActivationToken::new("pinwin-test-token").expect("test token is valid")
    }

    /// The decision table of row 7.2: only `on-demand` with a bound global
    /// activates; the other modes are no-ops with the global, and
    /// `on-demand` without the global is a no-op too.
    #[test]
    fn only_on_demand_with_a_bound_global_activates() {
        for (keyboard, bound, expected) in [
            (Keyboard::OnDemand, true, FocusAction::Activate),
            (Keyboard::OnDemand, false, FocusAction::Nothing),
            (Keyboard::None, true, FocusAction::Nothing),
            (Keyboard::Exclusive, true, FocusAction::Nothing),
            (Keyboard::None, false, FocusAction::Nothing),
            (Keyboard::Exclusive, false, FocusAction::Nothing),
        ] {
            assert_eq!(focus_action(keyboard, bound), expected);
        }
    }

    /// A focus command against a headless state (no session) is a no-op that
    /// panics nowhere: the reply path itself is covered in `commands`.
    #[test]
    fn a_focus_command_without_a_session_is_a_noop() {
        let (tx, _rx) = mpsc::channel();
        let mut state = PanelState::headless(
            Handshake::new(tx),
            GuardPoisoned::new(),
            Arc::new(Inner {
                id: 0,
                poisoned: GuardPoisoned::new(),
                live: AtomicBool::new(true),
                keyboard: Keyboard::OnDemand,
            }),
            super::super::super::Startup {
                fd: -1,
                layout: Layout::new(
                    Side::Left,
                    NonZeroU16::new(40).expect("test columns"),
                    0,
                    0,
                    0,
                    0,
                ),
                keyboard: Keyboard::OnDemand,
                accent: None,
            },
            CellSize::new(9, 16).expect("test cell size is non-zero"),
        );
        on_focus(&mut state, &token());
    }
}
