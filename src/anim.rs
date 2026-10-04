//! The animated width transition (port-to-rust D3): one frame-clock tick
//! callback eases the visible panel's width and the reservation's exclusive
//! zone together, and a watchdog snaps to the final layout if frames stop.
//! Ported from `src/glue_anim.c` (the add-animated-width design D2, D4-D6 it
//! cites is archived under `openspec/changes/archive/`).
//!
//! Everything runs on the GTK thread; the state is one struct, so a frame
//! allocates nothing. The easing and the per-tick step are pure and unit
//! tested here; the GTK pieces (tick callback, watchdog, the
//! `gtk-enable-animations` setting) cannot run without a display.
//!
//! The pty read drain bounds itself while a tween runs (`src/pty.c` read
//! `glue_anim_active()` directly; here [`Anim`] owns a flag for
//! `Pty::set_tween_active`, and the glue relays every state change through the
//! `set_tween_active` hook the panel wires up).
//!
//! Panics must never cross back into GTK/glib (D5): the tick and watchdog
//! closures run their bodies under `catch_unwind`, latching a poisoned flag.
//! Row 4.2 grows this local guard into the shared helper.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk4::glib;
use gtk4::prelude::*;

use crate::layout::Side;

/// Ease-out cubic: close to niri's critically damped window-resize spring
/// (`glue_anim.c`).
pub fn ease(t: f64) -> f64 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// The outcome of one tween tick, in frame-clock microseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    /// Keep easing; the tween's eased width is the payload.
    Frame(i32),
    /// The duration is spent: the tween stops at its target.
    Finished,
}

/// The tween's pure state and step, GTK-free so tests can drive it (`glue_anim.c`
/// `Anim` minus the widget and source ids).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tween {
    active: bool,
    from_px: i32,
    to_px: i32,
    cur_px: i32,
    /// `None` until the first tick stamps it (`a.t0_us == 0` in the C).
    t0_us: Option<i64>,
    dur_us: i64,
}

impl Tween {
    /// Start, or retarget, a tween from `from_px` to `to_px` over
    /// `duration_ms`. The caller clamps the duration (`pinwin_api.c` clamps
    /// to 1000 ms before the apply reaches the glue).
    pub fn begin(from_px: i32, to_px: i32, duration_ms: u32) -> Self {
        Tween {
            active: true,
            from_px,
            to_px,
            cur_px: from_px,
            t0_us: None,
            dur_us: i64::from(duration_ms) * 1000,
        }
    }

    /// Whether the tween is running (`a.active`).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Advance to `now_us`. The first call stamps the tween's start time, so
    /// the first frame is exactly `from_px`.
    pub fn advance(&mut self, now_us: i64) -> Advance {
        let t0 = *self.t0_us.get_or_insert(now_us);
        let t = (now_us - t0) as f64 / self.dur_us as f64;
        if t >= 1.0 {
            return Advance::Finished;
        }
        let t = t.max(0.0);
        self.cur_px = self.from_px + ((self.to_px - self.from_px) as f64 * ease(t)).round() as i32;
        Advance::Frame(self.cur_px)
    }

    /// Reset to inactive (`anim_stop`'s state half).
    pub fn stop(&mut self) {
        *self = Tween::default();
    }
}

/// What the tween drives on the glue side. The closures must not call back
/// into [`Anim`] methods synchronously: the frame/finish paths hand the glue
/// everything it needs (`on_frame` carries the eased width, `on_finish` runs
/// after the tween state is already reset), so no re-entrant borrow happens.
pub struct AnimHooks {
    /// One eased frame at `px`: apply the panel width and both surfaces for
    /// that width (`glue_apply_geometry` from the tick).
    pub on_frame: Box<dyn FnMut(i32)>,
    /// The tween stopped for any reason (finish, watchdog, cancel, retarget):
    /// drop the tween frame cache, fire the deferred terminal grid resize and
    /// clear the pty's tween flag (`anim_stop`).
    pub on_stop: Box<dyn FnMut()>,
    /// The tween ended at its exact target: apply the geometry through the
    /// same path as a non-animated apply, now reading the applied width
    /// (`anim_finish`'s `glue_apply_geometry`).
    pub on_finish: Box<dyn FnMut()>,
}

/// The tween state and hooks, shared between [`Anim`] and the GTK closures
/// that outlive any borrow of it. The mirror flags live here too, so the stop
/// paths the closures drive can update them without a second handle.
struct AnimInner {
    tween: RefCell<Tween>,
    hooks: RefCell<AnimHooks>,
    tick: std::cell::Cell<Option<gtk4::TickCallbackId>>,
    watchdog: std::cell::Cell<Option<glib::SourceId>>,
    /// Mirrors `tween.is_active()` so the pty read drain can bound itself
    /// without touching the GTK state (D4). Written only through
    /// [`set_tween_flag`], next to the tween-state writes it mirrors.
    tween_flag: Arc<AtomicBool>,
    /// Latched when a tick, watchdog or hook body panicked (D5).
    poisoned: Arc<AtomicBool>,
}

/// The one place [`AnimInner::tween_flag`] changes, called exactly where the
/// tween state it mirrors is written, so the mirror cannot diverge: `begin`
/// sets it with the new tween, `stop_inner` and `stop_inner_quiet` clear it
/// with the reset.
fn set_tween_flag(inner: &AnimInner, active: bool) {
    inner.tween_flag.store(active, Ordering::Relaxed);
}

/// The animated width transition for one panel. Lives on the GTK thread (D4).
pub struct Anim {
    inner: Rc<AnimInner>,
}

impl Anim {
    /// Build an idle tween with the glue hooks attached.
    pub fn new(hooks: AnimHooks) -> Self {
        Anim {
            inner: Rc::new(AnimInner {
                tween: RefCell::new(Tween::default()),
                hooks: RefCell::new(hooks),
                tick: std::cell::Cell::new(None),
                watchdog: std::cell::Cell::new(None),
                tween_flag: Arc::new(AtomicBool::new(false)),
                poisoned: Arc::new(AtomicBool::new(false)),
            }),
        }
    }

    /// Whether a tween is running (`glue_anim_active`).
    pub fn active(&self) -> bool {
        self.inner.tween_flag.load(Ordering::Relaxed)
    }

    /// The tween flag to relay into `Pty::set_tween_active`. The glue relays
    /// every state change through its `set_tween_active` hook; this shared
    /// flag is the value that relay carries.
    pub fn tween_flag(&self) -> Arc<AtomicBool> {
        self.inner.tween_flag.clone()
    }

    /// Whether a tick or watchdog body panicked (D5).
    pub fn poisoned(&self) -> bool {
        self.inner.poisoned.load(Ordering::Relaxed)
    }

    /// The panel's current pixel width: the animated width while a tween runs,
    /// else the applied width (`panel_px`); the glue passes `cols * cell_w`.
    pub fn current_px(&self, applied_px: i32) -> i32 {
        let tween = self.inner.tween.borrow();
        if tween.is_active() {
            tween.cur_px
        } else {
            applied_px
        }
    }

    /// Horizontal shift that keeps the grid against the docked edge while the
    /// surface is wider or narrower than the grid; zero when not animating
    /// (`glue_anim_draw_offset`). `area_width` is the drawing area's width,
    /// `None` when the glue has no live area; `grid_px` is `cols * cell_w`.
    pub fn draw_offset(&self, side: Side, area_width: Option<i32>, grid_px: i32) -> i32 {
        if !self.inner.tween.borrow().is_active() || side != Side::Right || area_width.is_none() {
            return 0;
        }
        area_width.unwrap_or(0) - grid_px
    }

    /// False when the `gtk-enable-animations` setting is off (`glue_anim_allowed`).
    pub fn allowed() -> bool {
        gtk4::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations())
    }

    /// Start, or retarget from the current width, a tween on `widget`'s frame
    /// clock (`glue_anim_begin`). A running tween is stopped first, which
    /// fires the `on_stop` hook exactly as the C did.
    pub fn begin(
        &self,
        widget: &impl IsA<gtk4::Widget>,
        from_px: i32,
        to_px: i32,
        duration_ms: u32,
    ) {
        self.stop(false, false);
        self.begin_state(from_px, to_px, duration_ms);

        let inner = self.inner.clone();
        let tick = widget.add_tick_callback(move |_widget, clock| {
            let now = clock.frame_time();
            let action = guarded(&inner.poisoned, || inner.tween.borrow_mut().advance(now));
            match action {
                Some(Advance::Frame(px)) => {
                    guarded(&inner.poisoned, || (inner.hooks.borrow_mut().on_frame)(px));
                    glib::ControlFlow::Continue
                }
                Some(Advance::Finished) => {
                    Anim::stop_inner(&inner, true, false);
                    guarded(&inner.poisoned, || (inner.hooks.borrow_mut().on_finish)());
                    glib::ControlFlow::Break
                }
                // A panic latched the poison flag: drop the tween quietly so no
                // further glue code runs (D5).
                None => {
                    Anim::stop_inner_quiet(&inner);
                    glib::ControlFlow::Break
                }
            }
        });
        self.inner.tick.set(Some(tick));

        // The watchdog snaps to the final layout if frames stop; the C allows
        // 100 ms of slack past the duration.
        let inner = self.inner.clone();
        let watchdog = glib::timeout_add_local(
            std::time::Duration::from_millis(u64::from(duration_ms) + 100),
            move || {
                Anim::stop_inner(&inner, false, true);
                guarded(&inner.poisoned, || (inner.hooks.borrow_mut().on_finish)());
                glib::ControlFlow::Break
            },
        );
        self.inner.watchdog.set(Some(watchdog));
    }

    /// The GTK-free state half of [`Anim::begin`]: stage the tween and raise
    /// its mirror flag. `begin` runs this before registering the GTK sources;
    /// tests drive it directly to exercise the flag without a display.
    fn begin_state(&self, from_px: i32, to_px: i32, duration_ms: u32) {
        *self.inner.tween.borrow_mut() = Tween::begin(from_px, to_px, duration_ms);
        set_tween_flag(&self.inner, true);
    }

    /// Drop the tick and watchdog and stop the tween (`glue_anim_cancel`).
    /// The `on_stop` hook fires whether or not a tween was running, exactly
    /// like the C's unconditional cache drop and deferred-grid fire.
    pub fn cancel(&self) {
        self.stop(false, false);
    }

    /// `anim_stop(in_tick, in_watchdog)`: remove the sources the caller is
    /// not inside of, reset the state, fire the stop hook.
    fn stop(&self, in_tick: bool, in_watchdog: bool) {
        Anim::stop_inner(&self.inner, in_tick, in_watchdog);
    }

    /// The shared-state half of [`Anim::stop`], callable from the closures
    /// that hold only `inner`.
    fn stop_inner(inner: &AnimInner, in_tick: bool, in_watchdog: bool) {
        if !in_tick {
            if let Some(tick) = inner.tick.take() {
                tick.remove();
            }
        } else {
            inner.tick.set(None);
        }
        if !in_watchdog {
            if let Some(watchdog) = inner.watchdog.take() {
                watchdog.remove();
            }
        } else {
            inner.watchdog.set(None);
        }
        inner.tween.borrow_mut().stop();
        // The flag mirrors the tween state, so the reset clears it here — not
        // only in `begin` — or `active()` latches true and the deferred grid
        // resize idles forever.
        set_tween_flag(inner, false);
        // The hook runs under the D5 guard like `on_finish`; the temporary
        // hook borrow ends with the guarded statement, matching the frame and
        // finish paths' borrow shape.
        guarded(&inner.poisoned, || (inner.hooks.borrow_mut().on_stop)());
    }

    /// The panic half of [`Anim::stop_inner`]: tear the sources down without
    /// running any more glue code (D5).
    fn stop_inner_quiet(inner: &AnimInner) {
        if let Some(tick) = inner.tick.take() {
            tick.remove();
        }
        if let Some(watchdog) = inner.watchdog.take() {
            watchdog.remove();
        }
        inner.tween.borrow_mut().stop();
        set_tween_flag(inner, false);
    }
}

/// The local D5 guard, matching `term::callbacks` and `pty` until row 4.2
/// lifts the shared helper: run `body`, latching `poisoned` and returning
/// `None` when it panics.
fn guarded<T>(poisoned: &AtomicBool, body: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => Some(value),
        Err(_) => {
            poisoned.store(true, Ordering::Relaxed);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hooks that record what the tween drove.
    struct Recording {
        frames: Vec<i32>,
        stops: usize,
        finishes: usize,
    }

    impl Recording {
        fn new() -> Rc<RefCell<Self>> {
            Rc::new(RefCell::new(Recording {
                frames: Vec::new(),
                stops: 0,
                finishes: 0,
            }))
        }

        fn hooks(recording: &Rc<RefCell<Self>>) -> AnimHooks {
            let frames = recording.clone();
            let stops = recording.clone();
            let finishes = recording.clone();
            AnimHooks {
                on_frame: Box::new(move |px| frames.borrow_mut().frames.push(px)),
                on_stop: Box::new(move || stops.borrow_mut().stops += 1),
                on_finish: Box::new(move || finishes.borrow_mut().finishes += 1),
            }
        }
    }

    #[test]
    fn ease_matches_the_c_ease_out_cubic() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(0.5), 0.875);
        assert!((ease(0.25) - (1.0 - 0.75f64.powi(3))).abs() < 1e-12);
    }

    #[test]
    fn first_tick_stamps_the_start_and_holds_from() {
        let mut tween = Tween::begin(40, 120, 1000);
        assert_eq!(tween.advance(5_000_000), Advance::Frame(40));
        // A second tick measures from the first stamp, not from `now`:
        // t = 0.5, ease = 0.875, 40 + round(80 * 0.875) = 110.
        assert_eq!(tween.advance(5_500_000), Advance::Frame(110));
    }

    #[test]
    fn tick_eases_and_rounds_like_lround() {
        let mut tween = Tween::begin(0, 100, 1000);
        let _ = tween.advance(0);
        // ease(0.5) = 0.875 -> 87.5 -> 88 (round half away from zero).
        assert_eq!(tween.advance(500_000), Advance::Frame(88));
    }

    #[test]
    fn tick_finishes_at_and_past_the_duration() {
        let mut tween = Tween::begin(0, 100, 1000);
        let _ = tween.advance(0);
        // Just short of the duration the ease is nearly one, so the width is
        // already the target rounded.
        assert_eq!(tween.advance(999_999), Advance::Frame(100));
        assert_eq!(tween.advance(1_000_000), Advance::Finished);
        assert_eq!(tween.advance(10_000_000), Advance::Finished);
    }

    /// A backwards frame clock cannot run the tween negative: the step clamps
    /// at the start, like the C's `if (t < 0.0) t = 0.0`.
    #[test]
    fn tick_clamps_a_backwards_clock_at_the_start() {
        let mut tween = Tween::begin(40, 120, 1000);
        let _ = tween.advance(5_000_000);
        assert_eq!(tween.advance(4_000_000), Advance::Frame(40));
    }

    #[test]
    fn stop_resets_the_tween() {
        let mut tween = Tween::begin(0, 100, 1000);
        let _ = tween.advance(100_000);
        tween.stop();
        assert_eq!(tween, Tween::default());
        assert!(!tween.is_active());
    }

    /// An idle tween reports the applied width, no offset and no activity.
    #[test]
    fn idle_anim_reports_the_applied_width() {
        let anim = Anim::new(Recording::hooks(&Recording::new()));
        assert!(!anim.active());
        assert!(!anim.poisoned());
        assert_eq!(anim.current_px(320), 320);
        assert_eq!(anim.draw_offset(Side::Right, Some(400), 320), 0);
        assert_eq!(anim.draw_offset(Side::Left, Some(400), 320), 0);
    }

    /// A started tween drives the eased width and the draw offset, without
    /// touching GTK: the test seeds the tween through `begin`'s state half.
    #[test]
    fn active_anim_reports_the_eased_width_and_offset() {
        let recording = Recording::new();
        let anim = Anim::new(Recording::hooks(&recording));
        anim.begin_state(320, 480, 1000);

        assert!(anim.active());
        assert_eq!(anim.current_px(320), 320);
        // A right-docked panel whose surface is wider than the grid shifts the
        // drawing by the difference; left-docked gets none.
        assert_eq!(anim.draw_offset(Side::Right, Some(480), 320), 160);
        assert_eq!(anim.draw_offset(Side::Left, Some(480), 320), 0);
        assert_eq!(anim.draw_offset(Side::Right, None, 320), 0);

        // The tween itself eases: 320 + round(160 * 0.875) = 460 at t = 0.5.
        let mut tween = Tween::begin(320, 480, 1000);
        let _ = tween.advance(0);
        assert_eq!(tween.advance(500_000), Advance::Frame(460));
    }

    /// Cancel stops the tween and fires the stop hook even when nothing ran,
    /// like the C's unconditional `anim_stop` half.
    #[test]
    fn cancel_fires_the_stop_hook_and_resets() {
        let recording = Recording::new();
        let anim = Anim::new(Recording::hooks(&recording));
        anim.cancel();
        assert!(!anim.active());
        let recording = recording.borrow();
        assert_eq!(recording.stops, 1);
        assert_eq!(recording.finishes, 0);
        assert!(recording.frames.is_empty());
    }

    /// The mirror flag follows the tween state through every stop path: a
    /// latched flag would keep the pty drain throttled and the deferred grid
    /// resize idling forever after the tween ended.
    #[test]
    fn the_tween_flag_clears_on_every_stop_path() {
        let recording = Recording::new();
        let anim = Anim::new(Recording::hooks(&recording));

        // begin's state half raises the flag...
        anim.begin_state(320, 480, 1000);
        assert!(anim.active());
        // ...the tick's finish path (stop from inside the tick) clears it.
        Anim::stop_inner(&anim.inner, true, false);
        assert!(!anim.active());

        anim.begin_state(320, 480, 1000);
        assert!(anim.active());
        // The watchdog path (stop from inside the watchdog).
        Anim::stop_inner(&anim.inner, false, true);
        assert!(!anim.active());

        anim.begin_state(320, 480, 1000);
        assert!(anim.active());
        // An external cancel.
        anim.cancel();
        assert!(!anim.active());

        anim.begin_state(320, 480, 1000);
        assert!(anim.active());
        // The quiet panic path runs no hooks and still clears the flag.
        Anim::stop_inner_quiet(&anim.inner);
        assert!(!anim.active());
        assert_eq!(recording.borrow().stops, 3);
    }

    /// A panicking stop hook cannot cross back into GTK (D5): the guarded
    /// `on_stop` call latches the poison flag instead.
    #[test]
    fn a_panicking_stop_hook_latches_the_poison_flag() {
        let anim = Anim::new(AnimHooks {
            on_frame: Box::new(|_| {}),
            on_stop: Box::new(|| panic!("hook blew up")),
            on_finish: Box::new(|| {}),
        });
        anim.begin_state(320, 480, 1000);
        anim.cancel();
        assert!(!anim.active());
        assert!(anim.poisoned());
    }
}
