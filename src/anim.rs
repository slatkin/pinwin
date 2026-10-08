//! The animated width transition's pure core (port-to-rust D3): the easing
//! and the per-tick step. Ported from `src/glue_anim.c` (the
//! add-animated-width design D2, D4-D6 it cites is archived under
//! `openspec/changes/archive/`). The panel thread's driver — frame
//! callbacks, a calloop watchdog, the running-tween state and the frame log
//! — lives in `src/panel/wayland_side/tween.rs`
//! (`replace-gtk-with-wayland` D7): the library reads no desktop animation
//! setting — the host's duration is the only control.

/// Ease-out cubic: close to niri's critically damped window-resize spring
/// (`glue_anim.c`).
#[must_use]
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

/// The tween's pure state and step, display-free so tests can drive it
/// (`glue_anim.c` `Anim` minus the widget and source ids).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tween {
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
    #[must_use]
    pub fn begin(from_px: i32, to_px: i32, duration_ms: u32) -> Self {
        Tween {
            from_px,
            to_px,
            cur_px: from_px,
            t0_us: None,
            dur_us: i64::from(duration_ms) * 1000,
        }
    }

    /// The tween's current eased width: `from_px` until the first tick, then
    /// the last eased frame (`a.cur_px`). The wayland driver's retargets
    /// read it as their `from_px`.
    #[must_use]
    pub fn current_px(&self) -> i32 {
        self.cur_px
    }

    /// The tween's target width in pixels (`a.to_px`): what the stop paths
    /// apply when the tween finishes.
    #[must_use]
    pub fn target_px(&self) -> i32 {
        self.to_px
    }

    /// Advance to `now_us`. The first call stamps the tween's start time, so
    /// the first frame is exactly `from_px`.
    // Approved per-instance (#13): microsecond timestamps are tiny against
    // f64's 52-bit mantissa; the ratio is clamped to [0, 1] below; the pixel
    // delta is rounded and screen-bounded, so the float-to-int step cannot
    // meaningfully truncate.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "tween timings are small; pixel delta is rounded and screen-bounded"
    )]
    pub fn advance(&mut self, now_us: i64) -> Advance {
        let t0 = *self.t0_us.get_or_insert(now_us);
        let t = (now_us - t0) as f64 / self.dur_us as f64;
        if t >= 1.0 {
            return Advance::Finished;
        }
        let t = t.max(0.0);
        self.cur_px =
            self.from_px + (f64::from(self.to_px - self.from_px) * ease(t)).round() as i32;
        Advance::Frame(self.cur_px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
