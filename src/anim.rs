//! The animated width transition's pure core (port-to-rust D3): the easing
//! and the per-tick step. Ported from `src/glue_anim.c` (the
//! add-animated-width design D2, D4-D6 it cites is archived under
//! `openspec/changes/archive/`). The driver — frame callbacks, the
//! running-tween state and the frame log — lives in the proxy's tween step
//! (`src/proxy/tween.rs`, replace-terminal-with-app-host D15;
//! `replace-gtk-with-wayland` D7): the daemon reads no desktop animation
//! setting — the requested duration is the only control.

/// Ease-out cubic, close to a critically damped spring (`glue_anim.c`), in
/// millionths: `t` and the result are fractions of one scaled by
/// `1_000_000`, so the step needs no lossy float-to-int conversion.
fn ease_millionths(t: i64) -> i64 {
    const ONE: i64 = 1_000_000;
    let u = ONE - t;
    // `u` is at most one million, so `u * u * u` is at most 1e18, which fits
    // in an i64.
    ONE - u * u * u / (ONE * ONE)
}

/// Round a value in millionths to the nearest whole unit, halves away from
/// zero like `f64::round`.
fn round_millionths(scaled: i64) -> i64 {
    const HALF: i64 = 500_000;
    const ONE: i64 = 1_000_000;
    if scaled >= 0 {
        (scaled + HALF) / ONE
    } else {
        (scaled - HALF) / ONE
    }
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

    /// Advance to `now_us`. The first call stamps the tween's start time, so
    /// the first frame is exactly `from_px`.
    pub fn advance(&mut self, now_us: i64) -> Advance {
        let t0 = *self.t0_us.get_or_insert(now_us);
        let elapsed = now_us.saturating_sub(t0);
        if elapsed >= self.dur_us {
            return Advance::Finished;
        }
        // The caller clamps the duration to 1000 ms, so its microseconds fit
        // an i32, and the elapsed time is at most that duration.
        let duration = i32::try_from(self.dur_us).unwrap_or(i32::MAX);
        let elapsed = i32::try_from(elapsed.max(0)).unwrap_or(duration);
        // The eased fraction in millionths, then the eased pixel delta. The
        // delta stays between zero and `to_px - from_px`, so its conversion
        // back to i32 cannot fail.
        let t = i64::from(elapsed) * 1_000_000 / i64::from(duration);
        let delta = i64::from(self.to_px - self.from_px) * ease_millionths(t);
        let delta = round_millionths(delta);
        let delta = i32::try_from(delta).unwrap_or(self.to_px - self.from_px);
        self.cur_px = self.from_px + delta;
        Advance::Frame(self.cur_px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_matches_the_c_ease_out_cubic() {
        assert_eq!(ease_millionths(0), 0);
        assert_eq!(ease_millionths(1_000_000), 1_000_000);
        assert_eq!(ease_millionths(500_000), 875_000);
        assert_eq!(ease_millionths(250_000), 578_125);
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
