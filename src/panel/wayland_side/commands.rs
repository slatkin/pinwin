//! The panel thread's command-channel handling (replace-gtk-with-wayland D2):
//! the apply, focus and teardown commands the host posts and the closed
//! command channel, split from the surface-event handlers in
//! [`super::state`] so both stay small. The teardown and the closed channel
//! are stop relays (D5): they end the loop and drop the surfaces even on a
//! latched flag, or a dead panel would strand the host's drop for the whole
//! reply bound and leak the thread and its Wayland connection.

use calloop::channel::Event;

use crate::guard::{Poisoned, guard, guard_always};

use super::state::PanelState;

/// The command channel's callback (D5 boundary): a callback is a boundary
/// like a GTK closure, so ordinary commands run under the shared [`guard`]
/// and a latched panel runs no more glue code. The stop relays — a teardown
/// and a closed command channel — must run even on a latched flag (D5):
/// skipping them would drop the teardown's reply (the host's drop waits the
/// whole reply bound) and never end the loop, leaking the thread and its
/// Wayland connection.
pub(crate) fn on_command_event(state: &mut PanelState, event: Event<super::PanelCommand>) {
    // The latch is cloned first: the guard's borrow and the command
    // handling must not alias the same `PanelState`.
    let poisoned = state.poisoned.clone();
    match event {
        // The teardown is a stop relay, not ordinary glue (D5):
        // `guard_always` runs it even on a latched flag.
        Event::Msg(super::PanelCommand::Teardown { reply }) => {
            let _ = guard_always(&poisoned, || {
                handle_command(state, super::PanelCommand::Teardown { reply });
            });
        }
        Event::Msg(command) => {
            let _ = guard(&poisoned, || handle_command(state, command));
        }
        // Every sender is gone (the host dropped the handle without a
        // teardown): end the thread the same way a teardown does. A plain
        // store cannot panic, so it needs no guard at all.
        Event::Closed => state.tear_down(),
    }
}

/// One posted command (D2): the guarded arms mirror the GTK side's
/// dispatched glue, each answering through the bounded reply the command
/// carried.
fn handle_command(state: &mut PanelState, command: super::PanelCommand) {
    match command {
        super::PanelCommand::Apply {
            layout,
            duration_ms,
            reply,
        } => {
            let _ = reply.send(state.apply(layout, duration_ms));
        }
        super::PanelCommand::Focus { reply } => {
            let _ = reply.send(focus_without_activation());
        }
        super::PanelCommand::Teardown { reply } => {
            // A stop relay, not ordinary glue (D5): the reply and the loop's
            // end must happen even on a latched flag, or a dead panel
            // strands the host's drop for the whole reply bound.
            state.tear_down();
            let scratch = Poisoned::new();
            let _ = guard_always(&scratch, || {
                let _ = reply.send(());
            });
        }
    }
}

/// The focus answer while the thread has no xdg-activation request yet (row
/// 7.2 replaces this with the activation call of decision 4): the same
/// not-live lifecycle state as the pre-3.5 apply answer.
fn focus_without_activation() -> super::super::handshake::FocusOutcome {
    super::super::handshake::FocusOutcome::NotLive
}

#[cfg(test)]
mod tests {
    use super::super::{PanelCommand, Startup};
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::layout::{CellSize, Keyboard, Layout, Side};
    use crate::panel::handshake::Handshake;
    use std::num::NonZeroU16;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    /// A startup for the tests; the thread does not touch the pty fd until
    /// the surfaces push a grid, so a placeholder fd is fine here.
    fn startup() -> Startup {
        Startup {
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
        }
    }

    /// The startup cell metrics the tests carry, the way a start command
    /// would (D3): required, because a metrics-less state never pushes.
    fn cell() -> CellSize {
        CellSize::new(9, 16).expect("test cell size is non-zero")
    }

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<super::super::Inner> {
        Arc::new(super::super::Inner {
            id: 0,
            poisoned: GuardPoisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        })
    }

    fn headless_state(handshake: Handshake) -> PanelState {
        PanelState::headless(handshake, Poisoned::new(), live_inner(), startup(), cell())
    }

    /// An apply posted to a thread whose session is not bound — no resolved
    /// output to validate against — is answered `NotLive` through the same
    /// bounded reply (`NotRunning`), never a hang.
    #[test]
    fn an_apply_without_a_session_is_not_running() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        handle_command(
            &mut state,
            PanelCommand::Apply {
                layout: startup().layout,
                duration_ms: 0,
                reply: reply_tx,
            },
        );
        assert_eq!(
            crate::panel::handshake::wait_for_apply(&reply_rx, std::time::Duration::from_secs(1)),
            Err(crate::panel::PinwinError::NotRunning)
        );
        assert!(!state.done, "an apply does not end the thread");
    }

    /// A focus request posted to a thread without an activation path is
    /// answered `NotLive` through the same bounded reply (`NotRunning`).
    #[test]
    fn a_focus_request_without_an_activation_path_is_not_running() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        handle_command(&mut state, PanelCommand::Focus { reply: reply_tx });
        assert_eq!(
            crate::panel::handshake::wait_for_focus(&reply_rx, std::time::Duration::from_secs(1)),
            Err(crate::panel::PinwinError::NotRunning)
        );
    }

    /// A teardown command tears the panel down — the surfaces drop and the
    /// loop state ends — and answers through the bounded reply, even on a
    /// latched flag (D5's stop relay): the host's drop must not strand.
    /// The test drives the real callback path ([`on_command_event`], outer
    /// guard included) — a `handle_command` call alone would skip the
    /// short-circuiting outer guard that hid this relay once.
    #[test]
    fn a_teardown_command_ends_the_thread_state() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        state.poisoned = Poisoned::latched();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        on_command_event(
            &mut state,
            Event::Msg(PanelCommand::Teardown { reply: reply_tx }),
        );
        assert!(state.done, "the loop ends");
        assert_eq!(
            reply_rx.recv_timeout(std::time::Duration::from_secs(1)),
            Ok(()),
            "the teardown replies even on a latched flag"
        );
    }

    /// A closed command channel tears the panel down even on a latched flag
    /// (D5's stop relay): a swallowed `Closed` would leak the thread and its
    /// Wayland connection after a plain handle drop.
    #[test]
    fn a_closed_command_channel_ends_the_thread_even_when_latched() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        state.poisoned = Poisoned::latched();
        on_command_event(&mut state, Event::Closed);
        assert!(state.done, "the closed channel ends the thread");
    }

    /// An ordinary command stays under the short-circuiting guard (D5): on
    /// a latched flag it never runs and its reply channel closes empty,
    /// while the loop keeps going.
    #[test]
    fn a_latched_flag_drops_an_ordinary_command() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        state.poisoned = Poisoned::latched();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        on_command_event(
            &mut state,
            Event::Msg(PanelCommand::Apply {
                layout: startup().layout,
                duration_ms: 0,
                reply: reply_tx,
            }),
        );
        assert!(!state.done, "an ordinary command does not end the thread");
        assert_eq!(
            reply_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Disconnected),
            "the guarded command never ran"
        );
    }
}
