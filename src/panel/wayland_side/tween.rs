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
//! `PINWIN_FRAMELOG` (row 6.3) rides on the driver: a tween's begin takes
//! the frame log it will feed ([`FrameLog`](super::frame_log::FrameLog),
//! built by the caller from the environment and the session's
//! presentation-time binding), the frames feed it, and every stop point —
//! a frame finish, the watchdog, a cancel, a retarget's stop relay —
//! prints its one summary line, exactly where the GTK path's `stop_inner`
//! printed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};

use crate::anim::{Advance, Tween};
use crate::guard::guard;
use crate::layout::Layout;

use super::frame_log::FrameLog;
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
    /// The running tween's frame log, `Some` exactly while a tween runs
    /// and `PINWIN_FRAMELOG=1` (row 6.3): created at the begin, fed on
    /// each frame, printed and dropped at the first stop point.
    frame_log: Option<FrameLog>,
    /// The generation the running tween's presentation feedbacks carry:
    /// incremented at every begin, so a late `presented` from a tween that
    /// already stopped or was retargeted cannot land in the next tween's
    /// log.
    generation: u64,
    /// The pty read source's tween flag (row 8.1): whether a tween runs,
    /// shared with the calloop pty source so the drain's budget applies
    /// only while a tween runs. Owned here — not mirrored from outside —
    /// so the flag is set and cleared exactly where the driver begins and
    /// stops a tween, and a missed stop cannot throttle the drain forever.
    tween_active: Arc<AtomicBool>,
}

impl TweenDriver {
    /// Start, or retarget, a tween from `from_px` to `to_px` over
    /// `duration_ms` ([`crate::anim::Anim::begin`]'s state half): a running
    /// tween stops first, and the caller relays the stop unconditionally,
    /// exactly as the GTK path's `on_stop` runs on every begin. A zero
    /// duration stages no tween — the caller snaps through the plain apply
    /// path. The caller clamps the duration to 1000 ms, as `pinwin_api.c`
    /// did before the apply reached the glue. Crate-internal like its
    /// frame-log parameter.
    pub(crate) fn begin(
        &mut self,
        from_px: i32,
        to_px: i32,
        duration_ms: u32,
        now: Instant,
        frame_log: Option<FrameLog>,
    ) -> TweenBegin {
        // A begin over a running tween is a retarget: the previous tween
        // stops first and prints its summary, exactly as the GTK begin's
        // unconditional stop did. With no tween running there is no log
        // left to print — every stop path already took it.
        self.stop_log();
        self.running = None;
        // The frame feedbacks carry the generation they were requested
        // under, so the new tween's log starts clean.
        self.generation = self.generation.wrapping_add(1);
        let outcome = if duration_ms == 0 {
            // A snap holds no log: nothing runs to feed one.
            TweenBegin::Snap
        } else {
            match now.checked_add(Duration::from_millis(
                u64::from(duration_ms) + WATCHDOG_SLACK_MS,
            )) {
                // A deadline beyond the monotonic clock's range cannot be
                // armed; a snap is always a valid outcome.
                None => TweenBegin::Snap,
                Some(deadline) => {
                    self.running = Some(Running {
                        tween: Tween::begin(from_px, to_px, duration_ms),
                        deadline,
                    });
                    self.frame_log = frame_log;
                    TweenBegin::Run { deadline }
                }
            }
        };
        // The pty read source's tween flag (row 8.1): a begin that staged a
        // tween sets it, a retarget over a running tween keeps it set — the
        // old tween's stop and the new one's begin land in the same store —
        // and a begin that snapped clears it. The drain's budget follows
        // the driver's own state.
        self.tween_active
            .store(self.running.is_some(), Ordering::Relaxed);
        outcome
    }

    /// Whether a tween is running.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.running.is_some()
    }

    /// The pty read source's tween flag (row 8.1): the `Arc` the calloop
    /// pty source reads its drain budget from, owned here so the flag is
    /// set and cleared exactly where the driver begins and stops a tween.
    #[must_use]
    pub(crate) fn tween_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.tween_active)
    }

    /// Whether the running tween carries a frame log (row 6.3): the caller
    /// requests presentation feedback only for a tween that feeds one, so
    /// an unlogged tween's samples are never requested just to be dropped
    /// by the generation filter.
    #[must_use]
    pub(crate) fn has_log(&self) -> bool {
        self.frame_log.is_some()
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
    /// The frame log's callback source records the same time for every
    /// frame while a log exists — the finishing one included, exactly as
    /// the GTK tick recorded before advancing (row 6.3).
    pub fn frame(&mut self, now_ms: u32) -> FrameStep {
        // The compositor's event time is u32 milliseconds and wraps about
        // every 49.7 days; a tween spanning a wrap eases at its start until
        // the watchdog snaps it.
        let frame_us = i64::from(now_ms) * 1000;
        if let Some(log) = self.frame_log.as_mut() {
            log.record_callback(now_ms);
        }
        match self.running.take() {
            None => FrameStep::Idle,
            Some(mut running) => match running.tween.advance(frame_us) {
                Advance::Frame(px) => {
                    self.running = Some(running);
                    FrameStep::Frame(px)
                }
                Advance::Finished => {
                    self.stop_log();
                    // The pty read source's flag (row 8.1): the finish is a
                    // stop relay, so the drain's budget ends with the tween.
                    self.tween_active.store(false, Ordering::Relaxed);
                    FrameStep::Finished(running.tween.target_px())
                }
            },
        }
    }

    /// Cancel the running tween, if any ([`crate::anim::Anim::cancel`]'s
    /// unconditional stop): the caller relays the stop whether or not a
    /// tween ran, exactly like the GTK path's unconditional cache drop and
    /// deferred-grid fire. The armed watchdog timer, if any, fires once more
    /// and removes itself. The running tween's frame log, if any, prints
    /// its summary here, as the GTK stop did.
    pub fn cancel(&mut self) {
        self.running = None;
        self.stop_log();
        // The pty read source's flag (row 8.1): the cancel is a stop relay,
        // whether or not a tween ran.
        self.tween_active.store(false, Ordering::Relaxed);
    }

    /// The generation the running tween's presentation feedbacks carry
    /// (row 6.3): the user data a `wp_presentation.feedback` request tags
    /// its `presented` events with, so the dispatch can drop a sample from
    /// a tween that already stopped or was retargeted.
    #[must_use]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// One `wp_presentation_feedback.presented` timestamp (row 6.3): the
    /// time the frame reached the screen, for a log fed from the
    /// presentation-time protocol. Samples from an older generation — a
    /// tween that already stopped or was retargeted — are dropped, and a
    /// log fed from the frame callbacks records nothing here.
    pub(crate) fn record_presented(
        &mut self,
        generation: u64,
        tv_sec_hi: u32,
        tv_sec_lo: u32,
        tv_nsec: u32,
    ) {
        if generation != self.generation {
            return;
        }
        if let Some(log) = self.frame_log.as_mut() {
            log.record_presented(tv_sec_hi, tv_sec_lo, tv_nsec);
        }
    }

    /// Print and drop the running tween's frame log: the one summary line
    /// per tween, at every stop point — a frame finish, the watchdog, a
    /// cancel, a retarget's stop relay — exactly where the GTK path's
    /// `stop_inner` printed ([`crate::anim::FrameLog`]). A stopped tween
    /// leaves no log behind, so no later stop prints again.
    fn stop_log(&mut self) {
        if let Some(log) = self.frame_log.take() {
            log.print();
        }
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
        self.stop_log();
        // The pty read source's flag (row 8.1): the expiry is a stop relay.
        self.tween_active.store(false, Ordering::Relaxed);
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

/// Whether a layout apply animates (row 6.2): the GTK path's
/// [`crate::surfaces::should_animate`] rule, read against the applied
/// layout — a positive duration between layouts that match in side and
/// left and right gutters animates, because those move the reservation's
/// side; a covering-only change animates so a pushing retarget at the same
/// width can ease its gap back (overlay-expand D3). The duration clamp to
/// 1000 ms happens here, at the begin, as `pinwin_api.c` did upstream. The
/// library reads no desktop animation setting: the host's duration is the
/// only control.
#[must_use]
pub fn should_animate(duration_ms: u32, applied: &Layout, requested: &Layout) -> bool {
    duration_ms > 0
        && requested.side() == applied.side()
        && requested.left() == applied.left()
        && requested.right() == applied.right()
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
            let fd = state.startup.fd;
            let _ = guard(&poisoned, || {
                state.tween_finished(target_px, &mut |grid| {
                    super::state::apply_pty_size(fd, grid);
                });
            });
            TimeoutAction::Drop
        }
    }
}

#[cfg(test)]
mod tests;
