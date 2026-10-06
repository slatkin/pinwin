//! The startup watchdog (replace-gtk-with-wayland D2/D3): two calloop timers
//! on the panel thread watching the start handshake, the wayland twin of the
//! GTK side's single repeating `timeout_add_local` watchdog
//! ([`super::super::gtk_side`]).
//!
//! The two timers keep two separate duties apart. The poll is a repeating
//! 200 ms tick, like the GTK side's: while the handshake is still pending, a
//! latched shared flag (a panic somewhere in the startup path, D5) reports
//! `Internal`, and the poll removes itself once the handshake resolves. A
//! slow start is never a failure: start resolution depends on compositor
//! events after the bind and commit (D3) — the panel's first
//! `wl_surface.enter`, then the output's xdg-output logical size — and a
//! busy or cold compositor can take longer than any short bound. The hard
//! deadline is the one-shot backstop for a start that never completes: a
//! compositor that never sends the output's logical size. It reports
//! `NoDisplay`, the "no display" case of a thread whose loop returned before
//! the panel went live, with the same bound an apply waits for its reply
//! ([`APPLY_WAIT`]).
//!
//! A fire that reports also tears the panel down — the same teardown the
//! teardown command and the closed command channel take — so the failed
//! panel leaves the screen at once and no later configure can push a pty
//! size. Panics never cross back into calloop (D5): each fire runs under
//! [`guard_always`], the relay variant, because a latched flag is exactly
//! the case the watchdog must still report.

use std::time::Duration;

use calloop::timer::TimeoutAction;

use crate::guard::guard_always;

use super::super::handshake::{APPLY_WAIT, StartOutcome};
use super::state::PanelState;

/// The poll's bound: the 200 ms the GTK side's startup watchdog polls with
/// (`gtk_side.rs`'s `timeout_add_local`).
pub(crate) const START_WATCHDOG: Duration = Duration::from_millis(200);

/// The hard deadline's bound: the same five seconds an apply waits for its
/// bounded reply ([`APPLY_WAIT`], D5's wedge bound). A start still pending
/// that long is a compositor that never completed the output description,
/// not a slow one — the poll exists so that slowness alone never fails.
pub(crate) const DEADLINE: Duration = APPLY_WAIT;

/// What one watchdog tick decides. The pure decision the timers' callbacks
/// run; the tests pin every case display-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tick {
    /// Keep polling: the start is still pending and nothing failed yet.
    Continue,
    /// The timer removes itself: the handshake resolved, nothing to report.
    Done,
    /// Report the outcome, tear the panel down and end the loop.
    Report(StartOutcome),
}

/// The poll tick's decision: a resolved handshake removes the timer, a
/// pending one keeps polling unless the shared latch is set — a panic in the
/// startup path reports `Internal` like the GTK side's watchdog's `Err` arm.
pub(crate) fn poll_tick(resolved: bool, poisoned: bool) -> Tick {
    if resolved {
        Tick::Done
    } else if poisoned {
        Tick::Report(StartOutcome::Internal)
    } else {
        Tick::Continue
    }
}

/// The hard deadline's decision: a start still pending at the deadline never
/// completed the output description (D3), so it reports `NoDisplay`; a
/// resolved one reports nothing. The deadline never keeps polling — it is
/// one-shot.
pub(crate) fn deadline_tick(resolved: bool) -> Tick {
    if resolved {
        Tick::Done
    } else {
        Tick::Report(StartOutcome::NoDisplay)
    }
}

/// The poll timer's callback: one decision, then re-arm at the poll's bound
/// or remove the timer.
pub(crate) fn on_poll_tick(state: &mut PanelState) -> TimeoutAction {
    let resolved = state.handshake.resolved();
    let poisoned = state.poisoned.is_poisoned();
    match poll_tick(resolved, poisoned) {
        Tick::Continue => TimeoutAction::ToDuration(START_WATCHDOG),
        // Nothing to report; the timer removes itself.
        Tick::Done => TimeoutAction::Drop,
        Tick::Report(outcome) => {
            fire(state, outcome);
            TimeoutAction::Drop
        }
    }
}

/// The hard deadline's callback: one shot, whatever it finds.
pub(crate) fn on_deadline_tick(state: &mut PanelState) -> TimeoutAction {
    match deadline_tick(state.handshake.resolved()) {
        Tick::Report(outcome) => {
            fire(state, outcome);
            TimeoutAction::Drop
        }
        // The deadline is one-shot: it removes itself even when the
        // handshake resolved first and there is nothing to report.
        Tick::Done | Tick::Continue => TimeoutAction::Drop,
    }
}

/// One watchdog fire: report the outcome first — the one-shot handshake
/// drops it when the start already resolved — then tear the panel down, the
/// same teardown the teardown command and the closed command channel take:
/// the surfaces drop, no configure can push a pty size afterwards, and the
/// loop ends. Runs under [`guard_always`], not [`guard`]: a latched flag is
/// exactly the case the watchdog must still report (D5).
fn fire(state: &mut PanelState, outcome: StartOutcome) {
    let poisoned = state.poisoned.clone();
    let _ = guard_always(&poisoned, || {
        state.handshake.report(outcome);
        state.tear_down();
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    use calloop::timer::Timer;

    use crate::guard::Poisoned;
    use crate::layout::{CellSize, Keyboard, Side};
    use crate::panel::handshake::{Handshake, wait_for_start};
    use crate::panel::{PinwinError, Startup};

    use super::super::Inner;

    use super::*;

    /// A startup for the tests; the thread does not touch the pty fd until
    /// the surfaces push a grid, so a placeholder fd is fine here.
    fn startup() -> Startup {
        Startup {
            fd: -1,
            layout: crate::layout::Layout::new(
                Side::Left,
                std::num::NonZeroU16::new(40).expect("test columns"),
                0,
                0,
                0,
                0,
            ),
            keyboard: Keyboard::OnDemand,
            accent: None,
        }
    }

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<Inner> {
        Arc::new(Inner {
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        })
    }

    fn headless_state(handshake: Handshake) -> PanelState {
        PanelState::headless(
            handshake,
            Poisoned::new(),
            live_inner(),
            startup(),
            CellSize::new(9, 16).expect("test cell size is non-zero"),
        )
    }

    /// The poll tick's decisions (the GTK side's repeating watchdog): a
    /// pending, healthy start keeps polling and reports nothing, a pending
    /// one on a latched flag reports `Internal`, a resolved one reports
    /// nothing and removes the timer.
    #[test]
    fn the_poll_tick_only_fails_a_pending_latched_start() {
        assert_eq!(poll_tick(false, false), Tick::Continue);
        assert_eq!(poll_tick(false, true), Tick::Report(StartOutcome::Internal));
        assert_eq!(poll_tick(true, false), Tick::Done);
        assert_eq!(poll_tick(true, true), Tick::Done);
    }

    /// The hard deadline's decisions: a start still pending at the deadline
    /// never completed the output description and reports `NoDisplay`, a
    /// resolved one reports nothing and removes the timer.
    #[test]
    fn the_deadline_tick_fails_only_a_pending_start() {
        assert_eq!(deadline_tick(false), Tick::Report(StartOutcome::NoDisplay));
        assert_eq!(deadline_tick(true), Tick::Done);
    }

    /// The poll tick's bound: the same 200 ms the GTK side polls with.
    #[test]
    fn the_poll_keeps_the_gtk_side_bound() {
        assert_eq!(START_WATCHDOG, Duration::from_millis(200));
        // The hard deadline reuses the apply reply's wedge bound.
        assert_eq!(DEADLINE, Duration::from_secs(5));
    }

    /// The poll bound the callbacks re-arm with, driven through a real
    /// calloop loop: the test re-arms at its own short bound, the way the
    /// production wiring re-arms at [`START_WATCHDOG`].
    const TEST_POLL: Duration = Duration::from_millis(5);

    /// Drive the poll callback through a real calloop loop and timer
    /// source, the wiring [`super::super::run_loop`] uses, without a display
    /// (`port-to-rust` D10). The test re-arms the timer at its own short
    /// bound so a slow start can be watched for several ticks quickly.
    fn drive_poll_ticks(state: &mut PanelState, rounds: usize) {
        let mut event_loop = calloop::EventLoop::<PanelState>::try_new().expect("test loop");
        event_loop
            .handle()
            .insert_source(
                Timer::from_duration(TEST_POLL),
                |_, &mut (), state: &mut PanelState| {
                    match on_poll_tick(state) {
                        // Re-arm at the test's short bound instead of the
                        // production one.
                        TimeoutAction::ToDuration(_) => TimeoutAction::ToDuration(TEST_POLL),
                        other => other,
                    }
                },
            )
            .expect("insert the poll timer");
        for _ in 0..rounds {
            event_loop
                .dispatch(Some(TEST_POLL * 2), state)
                .expect("dispatch");
        }
    }

    /// Drive the hard deadline's callback through a real calloop loop, the
    /// wiring [`super::super::run_loop`] uses, at a test-short bound.
    fn drive_deadline_tick(state: &mut PanelState) {
        let mut event_loop = calloop::EventLoop::<PanelState>::try_new().expect("test loop");
        event_loop
            .handle()
            .insert_source(
                Timer::from_duration(TEST_POLL),
                |_, &mut (), state: &mut PanelState| on_deadline_tick(state),
            )
            .expect("insert the deadline timer");
        event_loop
            .dispatch(Some(TEST_POLL * 2), state)
            .expect("dispatch");
    }

    /// A slow start does not fail: a pending, healthy handshake polled
    /// repeatedly through a real loop stays pending — the poll reports
    /// nothing and keeps the loop running (the old one-shot watchdog failed
    /// exactly here).
    #[test]
    fn a_slow_start_is_never_failed_by_the_poll() {
        let (tx, rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        drive_poll_ticks(&mut state, 5);
        assert!(!state.handshake.resolved(), "the start is still pending");
        assert!(rx.try_recv().is_err(), "the poll added no report");
        assert!(!state.done, "the loop keeps running");
    }

    /// A latched flag at a poll tick reports `Internal` through the waiting
    /// host and tears the panel down: the loop state ends, the way a
    /// teardown ends it.
    #[test]
    fn a_latched_poll_tick_fails_and_tears_the_panel_down() {
        let (tx, rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        state.poisoned = Poisoned::latched();
        drive_poll_ticks(&mut state, 3);
        assert_eq!(wait_for_start(&rx), Err(PinwinError::Internal));
        assert!(state.done, "the fire tore the panel down");
    }

    /// A start still pending at the hard deadline reports `NoDisplay` — the
    /// compositor never completed the output description — and ends the
    /// loop state: the report does not leave the loop dispatching a dead
    /// panel.
    #[test]
    fn a_pending_start_at_the_deadline_reports_no_display_and_ends_the_loop() {
        let (tx, rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        drive_deadline_tick(&mut state);
        assert_eq!(wait_for_start(&rx), Err(PinwinError::NoDisplay));
        assert!(state.done, "the fire ended the loop state");
    }

    /// A start that resolved before the deadline gets no second report and
    /// keeps running: the deadline removes itself without tearing down.
    #[test]
    fn a_resolved_start_at_the_deadline_is_left_alone() {
        let (tx, rx) = mpsc::channel();
        let handshake = Handshake::new(tx);
        handshake.report(StartOutcome::Started);
        let mut state = headless_state(handshake);
        drive_deadline_tick(&mut state);
        assert_eq!(
            rx.try_recv().expect("the start result"),
            StartOutcome::Started
        );
        assert!(rx.try_recv().is_err(), "the deadline adds no report");
        assert!(!state.done, "a resolved start keeps running");
    }
}
