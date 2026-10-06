//! The width tween's driver on the panel thread (replace-gtk-with-wayland
//! row 6.1, design decision 7): the GTK-free twin of [`crate::anim::Anim`],
//! driven by the compositor's `wl_surface.frame` callbacks instead of a GTK
//! tick callback and watched by a calloop timer instead of a `GLib` timeout.
//!
//! The tween step, the ease and the retarget rules stay in
//! [`crate::anim::Tween`]; this module adds the running-tween state around
//! it: the target width the stop paths restore, the watchdog deadline at the
//! duration plus 100 ms (the GTK path's rule), and the arming bookkeeping
//! the run loop drives. Every decision is a pure step the tests drive
//! without a display (`port-to-rust` D10); the Wayland calls — the frame
//! callback requests and the per-frame commits — stay in the thin glue of
//! the caller: [`arm_watchdog`] arms the timer from the run loop, and row
//! 6.2 wires `CompositorHandler::frame` and the animated apply's begin to
//! the driver.
//!
//! The GTK path's [`crate::anim::AnimHooks`] closures do not carry over: the
//! driver lives inside the panel state, and its frame, stop and finish
//! actions need `&mut PanelState` (the surfaces, the grid sizing), which a
//! closure stored beside the driver cannot reach. The hook semantics ride on
//! the returned steps instead: [`FrameStep::Frame`] is `on_frame`,
//! [`FrameStep::Finished`] and [`WatchdogStep::Expired`] are the stop relay
//! followed by `on_finish`, and every [`TweenDriver::begin`] and
//! [`TweenDriver::cancel`] relays the stop unconditionally, exactly as the
//! GTK path's `anim_stop` half runs on every begin and cancel.
//!
//! Panics never cross back into calloop (D5): the watchdog's timer callback
//! runs its finish action under the shared [`crate::guard`]; the tween-state
//! half of the decision is plain field operations that cannot panic, like
//! the GTK watchdog's unguarded `stop_inner`.
//!
//! `PINWIN_FRAMELOG` (row 6.3) is not wired here; the GTK path's frame log
//! in [`crate::anim`] stays as it is until then.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};

use crate::anim::{Advance, Tween};
use crate::guard::guard;

use super::state::PanelState;

/// The watchdog's slack past the duration, the GTK path's rule
/// ([`crate::anim::Anim::begin`]): a tween whose frame callbacks stall still
/// snaps to its target 100 ms after it was due.
const WATCHDOG_SLACK_MS: u64 = 100;

/// What one `wl_surface.frame` callback leaves for the glue to do. The GTK
/// path's `on_frame` hook is [`FrameStep::Frame`]; its stop-then-finish pair
/// is [`FrameStep::Finished`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameStep {
    /// No tween runs: the frame arrived after the tween stopped, and the
    /// glue requests no further callback.
    Idle,
    /// An eased frame at `px`: the glue commits that width and requests the
    /// next frame callback (`on_frame`).
    Frame(i32),
    /// The duration is spent: the tween stopped at its target `px`. The glue
    /// relays the stop, applies the final geometry and requests no further
    /// callback (`on_stop` then `on_finish`).
    Finished(i32),
}

/// What one watchdog timer fire leaves for the glue to do. The GTK path's
/// watchdog stops the tween and applies the final geometry; the stop relay
/// runs before the finish, as `on_stop` then `on_finish` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchdogStep {
    /// No tween runs: the fire removes the timer.
    Idle,
    /// A tween runs and its deadline is still ahead: the timer re-arms for
    /// the remainder.
    Pending {
        /// The time from the fire to the tween's deadline.
        remaining: Duration,
    },
    /// The deadline passed while the tween ran: the tween stopped, and the
    /// glue applies the target width `px` through the finish path.
    Expired(i32),
}

/// What [`TweenDriver::begin`] staged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TweenBegin {
    /// No tween: a zero duration snaps, and the caller applies the layout
    /// directly through the plain apply path, like the GTK publish path's
    /// `!animate` branch. A running tween, if any, was stopped first.
    Snap,
    /// A tween runs; the run loop arms the watchdog timer at this deadline.
    Run {
        /// The begin instant plus the duration plus the slack.
        deadline: Instant,
    },
}

/// One running tween: the GTK-free tween step and the deadline the watchdog
/// snaps at. Constructed only by [`TweenDriver::begin`], so a running tween
/// always carries its deadline (`port-to-rust` D6: the invariant lives in
/// the field's type, not a check).
#[derive(Debug)]
struct Running {
    tween: Tween,
    /// The watchdog deadline: the begin instant plus the duration plus
    /// [`WATCHDOG_SLACK_MS`].
    deadline: Instant,
}

/// The watchdog timer source the run loop currently has armed for a running
/// tween, with the deadline it was armed at: the run loop replaces it when a
/// retarget moves the deadline ([`TweenDriver::watchdog_to_arm`]). The token
/// is `None` when the loop refused to insert the timer and the tween runs
/// unwatched.
#[derive(Debug)]
struct Armed {
    token: Option<RegistrationToken>,
    deadline: Instant,
}

/// The width tween's state on the panel thread: the one running tween, if
/// any, and the watchdog timer's arming bookkeeping. Lives on
/// [`PanelState`](super::state::PanelState) beside the session; the driver's
/// decisions are pure and unit tested here (`port-to-rust` D10).
#[derive(Default, Debug)]
pub struct TweenDriver {
    running: Option<Running>,
    armed: Option<Armed>,
}

impl TweenDriver {
    /// Start, or retarget, a tween from `from_px` to `to_px` over
    /// `duration_ms` ([`crate::anim::Anim::begin`]'s state half): a running
    /// tween stops first, and the caller relays the stop unconditionally,
    /// exactly as the GTK path's `on_stop` runs on every begin. A zero
    /// duration stages no tween — the caller snaps through the plain apply
    /// path. The caller clamps the duration to 1000 ms, as `pinwin_api.c`
    /// did before the apply reached the glue.
    pub fn begin(
        &mut self,
        from_px: i32,
        to_px: i32,
        duration_ms: u32,
        now: Instant,
    ) -> TweenBegin {
        self.running = None;
        if duration_ms == 0 {
            return TweenBegin::Snap;
        }
        let Some(deadline) = now.checked_add(Duration::from_millis(
            u64::from(duration_ms) + WATCHDOG_SLACK_MS,
        )) else {
            // A deadline beyond the monotonic clock's range cannot be armed;
            // a snap is always a valid outcome.
            return TweenBegin::Snap;
        };
        self.running = Some(Running {
            tween: Tween::begin(from_px, to_px, duration_ms),
            deadline,
        });
        TweenBegin::Run { deadline }
    }

    /// Whether a tween is running.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.running.is_some()
    }

    /// The panel's current pixel width: the running tween's eased width,
    /// else the applied width ([`crate::anim::Anim::current_px`]). A retarget
    /// reads this as its `from_px`.
    #[must_use]
    pub fn current_px(&self, applied_px: i32) -> i32 {
        self.running
            .as_ref()
            .map_or(applied_px, |running| running.tween.current_px())
    }

    /// One frame callback's step (row 6.1): the compositor's event time in
    /// milliseconds drives [`Tween::advance`], whose clock is microseconds.
    pub fn frame(&mut self, now_ms: u32) -> FrameStep {
        // The compositor's event time is u32 milliseconds and wraps about
        // every 49.7 days; a tween spanning a wrap eases at its start until
        // the watchdog snaps it.
        let frame_us = i64::from(now_ms) * 1000;
        match self.running.take() {
            None => FrameStep::Idle,
            Some(mut running) => match running.tween.advance(frame_us) {
                Advance::Frame(px) => {
                    self.running = Some(running);
                    FrameStep::Frame(px)
                }
                Advance::Finished => FrameStep::Finished(running.tween.target_px()),
            },
        }
    }

    /// Cancel the running tween, if any ([`crate::anim::Anim::cancel`]'s
    /// unconditional stop): the caller relays the stop whether or not a
    /// tween ran, exactly like the GTK path's unconditional cache drop and
    /// deferred-grid fire. The armed watchdog timer, if any, fires once more
    /// and removes itself.
    pub fn cancel(&mut self) {
        self.running = None;
    }

    /// The watchdog timer fire's decision: a tween still short of its
    /// deadline re-arms the timer for the remainder, a tween at or past it
    /// stops with the target width ([`WatchdogStep::Expired`]), and no tween
    /// means the fire removes the timer. The state half is plain field
    /// operations that cannot panic, like the GTK watchdog's unguarded
    /// `stop_inner`.
    pub fn watchdog(&mut self, now: Instant) -> WatchdogStep {
        let Some(running) = self.running.take() else {
            // The tween is already gone (it finished, was canceled or
            // expired earlier): the fire drops the timer and clears the
            // arming note.
            self.armed = None;
            return WatchdogStep::Idle;
        };
        if now < running.deadline {
            let remaining = running.deadline - now;
            self.running = Some(running);
            return WatchdogStep::Pending { remaining };
        }
        self.armed = None;
        WatchdogStep::Expired(running.tween.target_px())
    }

    /// The deadline the run loop must arm the watchdog timer at, if the
    /// running tween's deadline is not the one the live timer is already
    /// armed at: a first begin arms, a retarget to a different deadline
    /// re-arms, and an armed timer at the same deadline serves as it is.
    #[must_use]
    pub fn watchdog_to_arm(&self) -> Option<Instant> {
        let deadline = self.running.as_ref()?.deadline;
        if self
            .armed
            .as_ref()
            .is_some_and(|armed| armed.deadline == deadline)
        {
            return None;
        }
        Some(deadline)
    }

    /// Take the live watchdog timer's registration token, so the run loop
    /// can remove it before re-arming at a moved deadline. `None` when no
    /// timer is armed — including one whose insertion failed.
    pub fn take_watchdog_token(&mut self) -> Option<RegistrationToken> {
        self.armed.take().and_then(|armed| armed.token)
    }

    /// Note the timer the run loop armed for `deadline`, if any: the
    /// bookkeeping the next [`TweenDriver::watchdog_to_arm`] reads.
    pub fn note_watchdog_armed(&mut self, token: Option<RegistrationToken>, deadline: Instant) {
        self.armed = Some(Armed { token, deadline });
    }
}

/// The run loop's watchdog arming (row 6.1): when a running tween's deadline
/// is not the one the live timer is armed at, remove the stale timer and arm
/// a fresh one-shot timer at the deadline. The timer's callback is
/// [`on_watchdog_tick`]; a fire that finds the tween gone or expired returns
/// `Drop` and removes the timer, so the arming runs again only when a new
/// deadline appears. A timer the loop refused to insert is noted as armed
/// without one: the tween's frames still drive it, and the watchdog is only
/// the backstop against a compositor that stops delivering frames.
pub(crate) fn arm_watchdog(handle: &LoopHandle<'_, PanelState>, state: &mut PanelState) {
    let Some(deadline) = state.tween.watchdog_to_arm() else {
        return;
    };
    if let Some(token) = state.tween.take_watchdog_token() {
        handle.remove(token);
    }
    let inserted = handle.insert_source(
        Timer::from_deadline(deadline),
        |_, &mut (), state: &mut PanelState| on_watchdog_tick(state),
    );
    match inserted {
        Ok(token) => state.tween.note_watchdog_armed(Some(token), deadline),
        Err(_insert) => state.tween.note_watchdog_armed(None, deadline),
    }
}

/// The watchdog timer's callback (D5 boundary): the tween-state half of the
/// decision is plain field operations that cannot panic, so it runs like the
/// GTK watchdog's unguarded `stop_inner` — even a latched panel ends its
/// tween; the finish action is ordinary glue and stays under the shared
/// guard, so a latched panel runs no more of it.
pub(crate) fn on_watchdog_tick(state: &mut PanelState) -> TimeoutAction {
    match state.tween.watchdog(Instant::now()) {
        WatchdogStep::Idle => TimeoutAction::Drop,
        WatchdogStep::Pending { remaining } => TimeoutAction::ToDuration(remaining),
        WatchdogStep::Expired(target_px) => {
            let poisoned = state.poisoned.clone();
            let _ = guard(&poisoned, || state.tween_finished(target_px));
            TimeoutAction::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    use crate::guard::Poisoned;
    use crate::layout::{CellSize, Keyboard, Layout, Side};
    use crate::panel::handshake::Handshake;

    use super::super::{Inner, Startup};

    use super::*;

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

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<Inner> {
        Arc::new(Inner {
            id: 0,
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

    /// An instant some milliseconds before now, so a test's deadlines are
    /// already past and its timers fire on the first dispatch.
    fn past(ms: u64) -> Instant {
        Instant::now()
            .checked_sub(Duration::from_millis(ms))
            .expect("the monotonic clock is past boot")
    }

    /// A scripted sequence of frame times eases to the target: every eased
    /// width matches [`crate::anim::ease`], the last callback finishes at
    /// the target, and a frame after the stop does nothing.
    #[test]
    fn scripted_frames_ease_to_the_target() {
        let mut driver = TweenDriver::default();
        let t0 = Instant::now();
        assert_eq!(
            driver.begin(0, 100, 1000, t0),
            TweenBegin::Run {
                deadline: t0 + Duration::from_millis(1100)
            },
            "the watchdog bound is the duration plus 100 ms"
        );
        // The first frame stamps the tween's start and holds `from_px`.
        assert_eq!(driver.frame(0), FrameStep::Frame(0));
        // t = 0.25: ease = 1 - 0.75^3 = 0.578125, so 58 px.
        assert_eq!(driver.frame(250), FrameStep::Frame(58));
        // t = 0.5: ease = 0.875, so 88 px (rounded half away from zero).
        assert_eq!(driver.frame(500), FrameStep::Frame(88));
        // t = 0.75: ease = 0.984375, so 98 px.
        assert_eq!(driver.frame(750), FrameStep::Frame(98));
        // The duration is spent: the finish carries the target width.
        assert_eq!(driver.frame(1000), FrameStep::Finished(100));
        assert!(!driver.is_active());
        // A frame after the stop does nothing.
        assert_eq!(driver.frame(2000), FrameStep::Idle);
    }

    /// A retarget mid-tween starts from the current width: the caller reads
    /// [`TweenDriver::current_px`] and begins from it, and the new tween's
    /// first frame holds that width. The deadline moves with the retarget.
    #[test]
    fn a_retarget_starts_from_the_current_width() {
        let mut driver = TweenDriver::default();
        driver.begin(40, 120, 1000, Instant::now());
        let _ = driver.frame(0);
        // t = 0.5: 40 + round(80 * 0.875) = 110.
        let _ = driver.frame(500);
        assert_eq!(driver.current_px(40), 110);

        // The retarget: the same `begin`, from the current width.
        let base = Instant::now();
        assert_eq!(
            driver.begin(110, 200, 200, base),
            TweenBegin::Run {
                deadline: base + Duration::from_millis(300)
            },
            "the retarget's deadline replaces the old one"
        );
        assert!(driver.is_active());
        // The first frame of the new tween is its start width.
        assert_eq!(driver.frame(0), FrameStep::Frame(110));
        assert_eq!(driver.current_px(40), 110);
    }

    /// The watchdog stops a tween whose frame callbacks never arrive and
    /// applies the target: before the deadline the fire re-arms, at it the
    /// tween stops with the target width, and both a later frame and a later
    /// fire do nothing.
    #[test]
    fn the_watchdog_stops_a_stalled_tween_at_the_target() {
        let mut driver = TweenDriver::default();
        let t0 = Instant::now();
        driver.begin(40, 120, 200, t0);
        assert!(driver.is_active());
        // Just short of the deadline the tween keeps running and re-arms.
        assert_eq!(
            driver.watchdog(t0 + Duration::from_millis(299)),
            WatchdogStep::Pending {
                remaining: Duration::from_millis(1)
            }
        );
        assert!(driver.is_active());
        // At the deadline the tween stops and the target width is the
        // finish payload.
        assert_eq!(
            driver.watchdog(t0 + Duration::from_millis(300)),
            WatchdogStep::Expired(120)
        );
        assert!(!driver.is_active());
        // A late frame and a late fire do nothing.
        assert_eq!(driver.frame(100_000), FrameStep::Idle);
        assert_eq!(
            driver.watchdog(t0 + Duration::from_millis(400)),
            WatchdogStep::Idle
        );
    }

    /// Cancel stops the tween: a later frame and fire do nothing, and a
    /// cancel with nothing running is a no-op on the state.
    #[test]
    fn cancel_stops_the_tween() {
        let mut driver = TweenDriver::default();
        driver.begin(40, 120, 200, Instant::now());
        assert!(driver.is_active());
        driver.cancel();
        assert!(!driver.is_active());
        assert_eq!(driver.frame(10), FrameStep::Idle);
        assert_eq!(driver.watchdog(Instant::now()), WatchdogStep::Idle);
        // Cancel again with nothing running: still fine.
        driver.cancel();
        assert!(!driver.is_active());
    }

    /// A zero duration snaps with no tween: the caller applies directly
    /// through the plain apply path. A zero duration while a tween runs
    /// stops it first, like `Anim::begin`'s unconditional stop.
    #[test]
    fn a_zero_duration_snaps_with_no_tween() {
        let mut driver = TweenDriver::default();
        assert_eq!(driver.begin(40, 120, 0, Instant::now()), TweenBegin::Snap);
        assert!(!driver.is_active());
        assert_eq!(driver.frame(10), FrameStep::Idle);
        assert_eq!(driver.watchdog(Instant::now()), WatchdogStep::Idle);

        // A zero duration while a tween runs stops it.
        driver.begin(40, 120, 200, Instant::now());
        assert!(driver.is_active());
        assert_eq!(driver.begin(40, 120, 0, Instant::now()), TweenBegin::Snap);
        assert!(!driver.is_active());
    }

    /// An idle driver reports the applied width, like
    /// [`crate::anim::Anim::current_px`].
    #[test]
    fn an_idle_driver_reports_the_applied_width() {
        let driver = TweenDriver::default();
        assert!(!driver.is_active());
        assert_eq!(driver.current_px(320), 320);
    }

    /// The armed watchdog timer stops a stalled tween through a real calloop
    /// loop (`port-to-rust` D10): the timer fires at the deadline and the
    /// driver's state ends. The headless state has no session, so the
    /// finish's geometry write itself is exercised only on niri (row 10.1).
    #[test]
    fn the_armed_watchdog_stops_a_stalled_tween() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        // A begin whose deadline is already in the past, so the timer fires
        // on the first dispatch.
        assert!(matches!(
            state.tween.begin(40, 120, 1, past(500)),
            TweenBegin::Run { .. }
        ));
        assert!(state.tween.is_active());

        let mut event_loop = calloop::EventLoop::<PanelState>::try_new().expect("test loop");
        arm_watchdog(&event_loop.handle(), &mut state);
        assert_eq!(state.tween.watchdog_to_arm(), None, "the timer is armed");

        event_loop
            .dispatch(Some(Duration::from_millis(50)), &mut state)
            .expect("dispatch");
        assert!(!state.tween.is_active(), "the watchdog stopped the tween");
        // The fired timer dropped itself: no re-arm is pending, and a second
        // dispatch does nothing further.
        assert_eq!(state.tween.watchdog_to_arm(), None);
        event_loop
            .dispatch(Some(Duration::from_millis(10)), &mut state)
            .expect("dispatch");
        assert!(!state.tween.is_active());
    }

    /// A retarget to an earlier deadline re-arms: the run loop removes the
    /// stale timer and arms at the new deadline, so the shorter tween's
    /// watchdog fires at its own deadline, not at the stale one.
    #[test]
    fn a_retarget_rearms_the_watchdog_at_the_new_deadline() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        let now = Instant::now();
        state.tween.begin(40, 120, 10_000, now);
        let long_deadline = state
            .tween
            .watchdog_to_arm()
            .expect("a first deadline to arm");

        let mut event_loop = calloop::EventLoop::<PanelState>::try_new().expect("test loop");
        arm_watchdog(&event_loop.handle(), &mut state);
        assert_eq!(state.tween.watchdog_to_arm(), None, "the timer is armed");

        // The retarget: a one-millisecond tween begun in the past, whose
        // deadline is far ahead of the first timer's.
        state.tween.begin(120, 360, 1, past(500));
        let short_deadline = state
            .tween
            .watchdog_to_arm()
            .expect("the moved deadline to arm");
        assert!(
            short_deadline < long_deadline,
            "the retarget's deadline is earlier"
        );
        // The re-arm removes the stale timer and arms at the new deadline.
        arm_watchdog(&event_loop.handle(), &mut state);
        assert_eq!(state.tween.watchdog_to_arm(), None);

        event_loop
            .dispatch(Some(Duration::from_millis(50)), &mut state)
            .expect("dispatch");
        assert!(
            !state.tween.is_active(),
            "the retargeted tween's watchdog fired"
        );
    }

    /// An armed timer whose tween finished through its frames first fires
    /// into an idle driver and drops itself: no finish runs twice.
    #[test]
    fn an_armed_timer_after_a_frame_finish_fires_into_idle() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        // A begin whose deadline is already in the past, so the armed timer
        // would fire on the first dispatch if the tween were still running.
        state.tween.begin(0, 100, 1, past(500));
        let mut event_loop = calloop::EventLoop::<PanelState>::try_new().expect("test loop");
        arm_watchdog(&event_loop.handle(), &mut state);

        // The frames finish the tween before the timer can fire.
        let _ = state.tween.frame(0);
        assert_eq!(state.tween.frame(5), FrameStep::Finished(100));
        assert!(!state.tween.is_active());

        event_loop
            .dispatch(Some(Duration::from_millis(20)), &mut state)
            .expect("dispatch");
        assert!(!state.tween.is_active(), "the fire found no tween");
        assert_eq!(state.tween.watchdog_to_arm(), None, "the timer dropped");
    }

    /// An idle driver arms nothing, and a dispatch with no tween changes
    /// nothing about the loop.
    #[test]
    fn an_idle_driver_arms_nothing() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        let mut event_loop = calloop::EventLoop::<PanelState>::try_new().expect("test loop");
        arm_watchdog(&event_loop.handle(), &mut state);
        assert_eq!(state.tween.watchdog_to_arm(), None);
        assert!(!state.tween.is_active());

        event_loop
            .dispatch(Some(Duration::from_millis(10)), &mut state)
            .expect("dispatch");
        assert!(!state.tween.is_active());
        assert!(!state.done, "an idle watchdog does not end the loop");
    }
}
