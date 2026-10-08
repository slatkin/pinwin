use std::num::NonZeroU16;
use std::sync::mpsc;

use crate::guard::Poisoned;
use crate::layout::{CellSize, Keyboard, Side};
use crate::panel::handshake::Handshake;

use super::super::{Inner, Startup};

use super::super::state::FrameOp;
use super::super::tween_draw::on_tween_frame;
use super::*;

/// A startup for the tests; the thread does not touch the pty fd until
/// the surfaces push a grid, so a placeholder fd is fine here.
fn startup() -> Startup {
    Startup::new(
        -1,
        Layout::new(
            Side::Left,
            NonZeroU16::new(40).expect("test columns"),
            0,
            0,
            0,
            0,
        ),
        Keyboard::OnDemand,
        None,
    )
}

/// A live handle state like a started panel's, for the thread-side
/// tests.
fn live_inner() -> Arc<Inner> {
    Arc::new(Inner {
        poisoned: Poisoned::new(),
        live: AtomicBool::new(true),
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
        driver.begin(0, 100, 1000, t0, None),
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

/// A tween frame requests its next callback before the commit it belongs
/// to (replace-gtk-with-wayland D7): a `wl_surface.frame` request binds to
/// the commit that
/// follows it, so a frame that commits first and requests second binds its
/// callback to a commit no tween frame makes — no callback ever fires, and
/// the watchdog snaps the tween to its target, which is what the live niri
/// session showed for both directions. The recording seam captures the
/// issue order of the two requests before either reads the session, so the
/// headless state observes it (`port-to-rust` D10). The finish commits the
/// target once and requests nothing after it.
#[test]
fn a_tween_frame_requests_its_callback_before_its_commit() {
    let (tx, _rx) = mpsc::channel();
    let mut state = headless_state(Handshake::new(tx));
    state.tween.begin(360, 1080, 200, Instant::now(), None);

    // Two eased frames: the begin callback's step and one eased step, each
    // requesting its callback before its commit.
    on_tween_frame(&mut state, 0);
    on_tween_frame(&mut state, 100);
    assert_eq!(
        state.frame_ops.borrow().as_slice(),
        [
            FrameOp::RequestFrame,
            FrameOp::CommitFrame,
            FrameOp::RequestFrame,
            FrameOp::CommitFrame,
        ]
        .as_slice(),
        "the request precedes the commit it belongs to"
    );

    // The finish: the final frame commits at the target, and no callback
    // is requested after it — the tween is over.
    on_tween_frame(&mut state, 200);
    assert_eq!(
        state.frame_ops.borrow().as_slice(),
        [
            FrameOp::RequestFrame,
            FrameOp::CommitFrame,
            FrameOp::RequestFrame,
            FrameOp::CommitFrame,
            FrameOp::CommitFrame,
        ]
        .as_slice(),
        "the finish commits without requesting a callback"
    );
}

/// A retarget mid-tween starts from the current width: the caller reads
/// [`TweenDriver::current_px`] and begins from it, and the new tween's
/// first frame holds that width. The deadline moves with the retarget.
#[test]
fn a_retarget_starts_from_the_current_width() {
    let mut driver = TweenDriver::default();
    driver.begin(40, 120, 1000, Instant::now(), None);
    let _ = driver.frame(0);
    // t = 0.5: 40 + round(80 * 0.875) = 110.
    let _ = driver.frame(500);
    assert_eq!(driver.current_px(40), 110);

    // The retarget: the same `begin`, from the current width.
    let base = Instant::now();
    assert_eq!(
        driver.begin(110, 200, 200, base, None),
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
    driver.begin(40, 120, 200, t0, None);
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
    driver.begin(40, 120, 200, Instant::now(), None);
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
    assert_eq!(
        driver.begin(40, 120, 0, Instant::now(), None),
        TweenBegin::Snap
    );
    assert!(!driver.is_active());
    assert_eq!(driver.frame(10), FrameStep::Idle);
    assert_eq!(driver.watchdog(Instant::now()), WatchdogStep::Idle);

    // A zero duration while a tween runs stops it.
    driver.begin(40, 120, 200, Instant::now(), None);
    assert!(driver.is_active());
    assert_eq!(
        driver.begin(40, 120, 0, Instant::now(), None),
        TweenBegin::Snap
    );
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
/// finish's geometry write itself is exercised only on niri.
#[test]
fn the_armed_watchdog_stops_a_stalled_tween() {
    let (tx, _rx) = mpsc::channel();
    let mut state = headless_state(Handshake::new(tx));
    // A begin whose deadline is already in the past, so the timer fires
    // on the first dispatch.
    assert!(matches!(
        state.tween.begin(40, 120, 1, past(500), None),
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
    state.tween.begin(40, 120, 10_000, now, None);
    let long_deadline = state
        .tween
        .watchdog_to_arm()
        .expect("a first deadline to arm");

    let mut event_loop = calloop::EventLoop::<PanelState>::try_new().expect("test loop");
    arm_watchdog(&event_loop.handle(), &mut state);
    assert_eq!(state.tween.watchdog_to_arm(), None, "the timer is armed");

    // The retarget: a one-millisecond tween begun in the past, whose
    // deadline is far ahead of the first timer's.
    state.tween.begin(120, 360, 1, past(500), None);
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
    state.tween.begin(0, 100, 1, past(500), None);
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

/// The animate decision read against the applied layout: a positive
/// duration between layouts that match in side and left and right gutters
/// animates; a zero duration, a side switch and a gutter change snap.
#[test]
fn the_animate_decision_follows_the_gtk_rule() {
    fn layout(side: Side, cols: u16, left: i32, right: i32) -> Layout {
        Layout::new(
            side,
            std::num::NonZeroU16::new(cols).expect("test columns"),
            0,
            0,
            left,
            right,
        )
    }
    let applied = layout(Side::Left, 40, 0, 12);

    assert!(
        should_animate(200, &applied, &applied),
        "an identity applies"
    );
    assert!(
        should_animate(200, &applied, &layout(Side::Left, 120, 0, 12)),
        "a column change at the same side and gutters animates"
    );
    assert!(
        should_animate(200, &applied, &layout(Side::Left, 120, 0, 12).covering()),
        "a covering-only change animates so the gap can ease back"
    );
    assert!(
        !should_animate(0, &applied, &layout(Side::Left, 120, 0, 12)),
        "a zero duration snaps"
    );
    assert!(
        !should_animate(200, &applied, &layout(Side::Right, 120, 0, 12)),
        "a side switch snaps"
    );
    assert!(
        !should_animate(200, &applied, &layout(Side::Left, 120, 4, 12)),
        "a left-gutter change snaps"
    );
    assert!(
        !should_animate(200, &applied, &layout(Side::Left, 120, 0, 4)),
        "a right-gutter change snaps"
    );
}

/// One print per tween (replace-gtk-with-wayland D7): a running tween
/// holds its log
/// exactly between its begin and its first stop, and every stop path —
/// a frame finish, the watchdog, a cancel, a retarget — takes it and
/// prints its one summary, so no later stop prints again.
#[test]
fn the_frame_log_prints_once_per_tween() {
    let mut driver = TweenDriver::default();
    let enabled = || FrameLog::begin(true, false).expect("an enabled log");

    // The frame finish: the log dies with the tween's last frame.
    driver.begin(0, 100, 1000, Instant::now(), Some(enabled()));
    assert!(
        driver.frame_log.is_some(),
        "the running tween holds the log"
    );
    let _ = driver.frame(0);
    assert!(driver.frame_log.is_some(), "an eased frame keeps the log");
    assert_eq!(driver.frame(1000), FrameStep::Finished(100));
    assert!(driver.frame_log.is_none(), "the finish printed the log");
    driver.cancel();
    assert!(driver.frame_log.is_none(), "no second print is left");

    // The watchdog's expiry, at the duration plus the 100 ms slack.
    let t0 = Instant::now();
    driver.begin(0, 100, 200, t0, Some(enabled()));
    assert_eq!(
        driver.watchdog(t0 + Duration::from_millis(300)),
        WatchdogStep::Expired(100)
    );
    assert!(driver.frame_log.is_none());

    // A cancel.
    driver.begin(0, 100, 200, Instant::now(), Some(enabled()));
    driver.cancel();
    assert!(driver.frame_log.is_none());

    // A retarget's stop relay: the old log printed once, the new
    // tween's log starts fresh and empty.
    driver.begin(0, 100, 1000, Instant::now(), Some(enabled()));
    let _ = driver.frame(0);
    driver.begin(50, 200, 500, Instant::now(), Some(enabled()));
    // The retargeted tween's log is its own: empty, so it prints
    // nothing yet — an old tween's samples would already summarize.
    assert!(
        driver
            .frame_log
            .as_ref()
            .and_then(FrameLog::summary_line)
            .is_none(),
        "the retargeted tween's log starts empty"
    );
}

/// A disabled recorder does not exist: the driver runs its tween with
/// no log at all, and no stop path has anything to print.
#[test]
fn a_disabled_recorder_runs_no_log() {
    let mut driver = TweenDriver::default();
    driver.begin(0, 100, 1000, Instant::now(), FrameLog::begin(false, true));
    assert!(driver.frame_log.is_none(), "nothing was built");
    let _ = driver.frame(0);
    assert!(driver.frame_log.is_none(), "nothing recorded");
    assert_eq!(driver.frame(1000), FrameStep::Finished(100));
    assert!(driver.frame_log.is_none());
}

/// The presentation feed lands in the running tween's log only under
/// its own generation: the presented handler records the on-screen
/// timestamps while the tween runs, and a sample tagged with an older
/// generation — a tween that already stopped or was retargeted — is
/// dropped instead of polluting the next tween's log.
#[test]
fn the_presentation_feed_follows_the_generation() {
    let mut driver = TweenDriver::default();
    driver.begin(
        0,
        100,
        1000,
        Instant::now(),
        Some(FrameLog::begin(true, true).expect("an enabled log")),
    );
    let generation = driver.generation();
    driver.record_presented(generation, 0, 0, 1_000_000);
    driver.record_presented(generation, 0, 0, 9_000_000);
    assert_eq!(
        driver
            .frame_log
            .as_ref()
            .and_then(FrameLog::summary_line)
            .as_deref(),
        Some(
            "pinwin: tween frame log: frames=2 mean=8.00ms p95=8.00ms max=8.00ms source=presentation"
        ),
        "both on-screen timestamps landed"
    );

    // The retarget: a new generation, so the old tween's late sample
    // is dropped and the new tween's log counts only its own feed.
    driver.begin(
        50,
        200,
        500,
        Instant::now(),
        Some(FrameLog::begin(true, true).expect("an enabled log")),
    );
    driver.record_presented(generation, 0, 0, 20_000_000);
    driver.record_presented(driver.generation(), 0, 0, 1_000_000);
    driver.record_presented(driver.generation(), 0, 0, 9_000_000);
    assert_eq!(
        driver
            .frame_log
            .as_ref()
            .and_then(FrameLog::summary_line)
            .as_deref(),
        Some(
            "pinwin: tween frame log: frames=2 mean=8.00ms p95=8.00ms max=8.00ms source=presentation"
        ),
        "the stale generation is dropped, the running one records"
    );
}

/// The frame-log gate (replace-gtk-with-wayland D7): a begin whose log is
/// `Some` carries it — the caller requests presentation feedback only then
/// — and a begin without one holds none; the stop takes the log away.
#[test]
fn the_driver_holds_a_frame_log_only_while_one_was_begun() {
    let mut driver = TweenDriver::default();
    assert!(!driver.has_log(), "no tween, no log");

    let t0 = Instant::now();
    let _ = driver.begin(0, 100, 100, t0, None);
    assert!(!driver.has_log(), "a begin without a log holds none");

    let log = FrameLog::begin(true, false).expect("an enabled callback log");
    let _ = driver.begin(0, 100, 100, t0, Some(log));
    assert!(driver.has_log(), "the begun log is held");

    driver.cancel();
    assert!(!driver.has_log(), "the stop took the log away");
}

/// The pty read source's tween flag (replace-gtk-with-wayland D2): a begin
/// that stages a tween sets it and the finish frame clears it — the
/// drain's budget follows the driver's own state.
#[test]
fn the_tween_flag_sets_at_the_begin_and_clears_at_the_finish() {
    let mut driver = TweenDriver::default();
    assert!(
        !driver.tween_flag().load(Ordering::Relaxed),
        "no tween, no flag"
    );
    driver.begin(0, 100, 1000, Instant::now(), None);
    assert!(
        driver.tween_flag().load(Ordering::Relaxed),
        "a begin sets it"
    );
    // The first frame stamps the tween's clock start.
    let _ = driver.frame(0);
    assert_eq!(driver.frame(1000), FrameStep::Finished(100));
    assert!(
        !driver.tween_flag().load(Ordering::Relaxed),
        "the finish clears it"
    );
}

/// The watchdog's expiry is a stop relay too: the flag clears with the
/// tween it snapped.
#[test]
fn the_tween_flag_clears_at_the_watchdogs_expiry() {
    let mut driver = TweenDriver::default();
    // A begin whose instant is already 2000 ms past: the deadline is long
    // gone, so the next fire expires the tween.
    driver.begin(0, 100, 1000, past(2000), None);
    assert!(driver.tween_flag().load(Ordering::Relaxed));
    assert_eq!(driver.watchdog(Instant::now()), WatchdogStep::Expired(100));
    assert!(
        !driver.tween_flag().load(Ordering::Relaxed),
        "the expiry clears it"
    );
}

/// The cancel is a stop relay whether or not a tween ran, and a snap begin
/// — a zero duration — sets nothing.
#[test]
fn the_tween_flag_clears_at_the_cancel_and_sets_nothing_on_a_snap() {
    let mut driver = TweenDriver::default();
    driver.begin(0, 100, 1000, Instant::now(), None);
    driver.cancel();
    assert!(
        !driver.tween_flag().load(Ordering::Relaxed),
        "the cancel clears it"
    );

    assert_eq!(
        driver.begin(0, 100, 0, Instant::now(), None),
        TweenBegin::Snap
    );
    assert!(
        !driver.tween_flag().load(Ordering::Relaxed),
        "a snap sets nothing"
    );
}

/// A retarget keeps the flag set: the old tween's stop and the new one's
/// begin land in the same store, so the drain's budget never drops between
/// two tweens.
#[test]
fn the_tween_flag_keeps_set_through_a_retarget() {
    let mut driver = TweenDriver::default();
    driver.begin(0, 100, 1000, Instant::now(), None);
    driver.begin(100, 200, 1000, Instant::now(), None);
    assert!(driver.is_active(), "the retarget staged a tween");
    assert!(
        driver.tween_flag().load(Ordering::Relaxed),
        "the flag survives the retarget"
    );
}
