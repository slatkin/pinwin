//! The width tween's frame log on the panel thread (replace-gtk-with-wayland
//! D7): it records one timestamp per frame while `PINWIN_FRAMELOG=1` and
//! prints one summary line per tween to stderr. The line format is stable —
//! prefix, field names, `source=` suffix — so scripts parsing earlier
//! releases' lines still work.
//!
//! Two time sources, one recorder. When the compositor offers the
//! presentation-time protocol, the recorder takes each frame's timestamp
//! from the `wp_presentation_feedback.presented` event — the time the frame
//! actually reached the screen, combined from the `tv_sec_hi`, `tv_sec_lo`
//! and `tv_nsec` fields into microseconds. Without the protocol, it takes
//! the `wl_surface.frame` callback's compositor event time. The summary
//! line names its source in a `source=` suffix.
//!
//! The recorder is a pure struct whose enablement is injected as a bool
//! ([`FrameLog::begin`]), so the tests never touch the environment; the
//! production caller reads `PINWIN_FRAMELOG` once per tween through
//! [`FrameLog::enabled`]. The driver in [`super::tween`] owns the recorder
//! across the tween's lifecycle: created at the begin, fed on each frame,
//! printed once at every stop point — a frame finish, the watchdog, a
//! cancel, a retarget's stop relay.
//!
//! The protocol plumbing below the recorder — binding the optional
//! `wp_presentation` global like the viewporter ([`Presentation`]) and the
//! `Dispatch` impls for the manager and its feedback objects — is
//! compile-only here: it runs only in a live session, where the niri check
//! covers the summary line. A `discarded` feedback records nothing, and a
//! feedback's generation tag ([`TweenDriver`](super::tween::TweenDriver)
//! generations) keeps a late `presented` from a tween that already stopped
//! or was retargeted out of the next tween's log.
//!
//! The recorder's arithmetic is checked throughout and casts no numeric
//! type with `as`: integer-to-float conversions go through
//! `num_traits::cast`, which is total in these directions, so the
//! dead fallbacks never fire.

use std::io::Write;

use num_traits::ToPrimitive;
use smithay_client_toolkit::globals::GlobalData;
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};

use crate::guard::guard;

use super::state::PanelState;

/// Which clock feeds a frame log: the presentation-time protocol's
/// presented timestamps, or the frame callbacks' compositor event times
/// when the compositor offers no presentation-time global.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameSource {
    /// `wp_presentation_feedback.presented`: the time each frame reached
    /// the screen.
    Presentation,
    /// The `wl_surface.frame` callback's compositor event time, in
    /// milliseconds: when the compositor asked for the next frame.
    Callback,
}

impl FrameSource {
    /// The summary line's `source=` suffix value, stable text a script can
    /// parse.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            FrameSource::Presentation => "presentation",
            FrameSource::Callback => "callback",
        }
    }
}

/// The gap summary a full frame log prints: the frame count, the mean,
/// p95 (nearest rank over the sorted gaps) and maximum gap, in ms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GapSummary {
    /// The recorded frames; the gaps are one fewer.
    pub(crate) frames: usize,
    /// The mean gap, in ms.
    pub(crate) mean: f64,
    /// The nearest-rank p95 gap, in ms.
    pub(crate) p95: f64,
    /// The maximum gap, in ms.
    pub(crate) max: f64,
}

/// The env-gated frame log for one tween: records one timestamp per frame
/// and prints one gap summary at the tween's stop. Off unless
/// `PINWIN_FRAMELOG=1`, so the per-frame cost when off is one `Option`
/// check in the driver.
#[derive(Debug)]
pub(crate) struct FrameLog {
    /// The clock the timestamps came from, fixed at the tween's begin.
    source: FrameSource,
    /// The frames' timestamps, in microseconds of the source's clock.
    /// Monotone within one tween: both sources count up, and a backwards
    /// sample only clamps its own gap to zero.
    times: Vec<u64>,
}

impl FrameLog {
    /// Whether the frame log is on for this run: `PINWIN_FRAMELOG=1`, read
    /// once per tween, not per frame. Tests inject their enablement
    /// through [`FrameLog::begin`] instead of touching the environment.
    #[must_use]
    pub(crate) fn enabled() -> bool {
        std::env::var("PINWIN_FRAMELOG").is_ok_and(|v| v == "1")
    }

    /// The log a new tween begins with: `None` when disabled, else a log
    /// fed from the presentation-time protocol when the compositor bound
    /// that global and from the frame callbacks otherwise. `presentation`
    /// is the caller's read of the session, so this stays a pure function
    /// the tests drive (`port-to-rust` D10).
    #[must_use]
    pub(crate) fn begin(enabled: bool, presentation: bool) -> Option<Self> {
        if !enabled {
            return None;
        }
        let source = if presentation {
            FrameSource::Presentation
        } else {
            FrameSource::Callback
        };
        Some(FrameLog {
            source,
            times: Vec::new(),
        })
    }

    /// One frame callback's compositor event time, in milliseconds. A log
    /// fed from the presentation timestamps records nothing here.
    pub(crate) fn record_callback(&mut self, time_ms: u32) {
        if self.source != FrameSource::Callback {
            return;
        }
        // Milliseconds to microseconds: a u32 ms value times 1000 cannot
        // overflow u64.
        self.times.push(u64::from(time_ms) * 1000);
    }

    /// One `wp_presentation_feedback.presented` timestamp, combined from
    /// the event's `tv_sec_hi`, `tv_sec_lo` and `tv_nsec` fields. A log fed
    /// from the frame callbacks records nothing here.
    pub(crate) fn record_presented(&mut self, tv_sec_hi: u32, tv_sec_lo: u32, tv_nsec: u32) {
        if self.source != FrameSource::Presentation {
            return;
        }
        let Some(timestamp) = presentation_us(tv_sec_hi, tv_sec_lo, tv_nsec) else {
            return;
        };
        self.times.push(timestamp);
    }

    /// `(frame count, mean, p95, max)` gap in ms over the recorded frames;
    /// `None` with fewer than two frames, where no gap exists. The gaps are
    /// the differences of successive timestamps, a backwards sample
    /// clamped to zero; the p95 is the nearest-rank order statistic over
    /// the sorted gaps.
    #[must_use]
    pub(crate) fn summary(&self) -> Option<GapSummary> {
        if self.times.len() < 2 {
            return None;
        }
        let mut gaps: Vec<f64> = self
            .times
            .windows(2)
            .map(|window| gap_ms(window[0], window[1]))
            .collect();
        gaps.sort_by(f64::total_cmp);
        let mean = gaps.iter().sum::<f64>() / to_f64(gaps.len());
        let p95 = *gaps.get(p95_rank(gaps.len()))?;
        let max = *gaps.last()?;
        Some(GapSummary {
            frames: self.times.len(),
            mean,
            p95,
            max,
        })
    }

    /// The one summary line per tween, or `None` when the log holds fewer
    /// than two frames. The prefix and field names are stable so parsing
    /// scripts keep working; the `source=` suffix names the clock.
    #[must_use]
    pub(crate) fn summary_line(&self) -> Option<String> {
        let summary = self.summary()?;
        Some(format!(
            "pinwin: tween frame log: frames={} mean={:.2}ms p95={:.2}ms max={:.2}ms source={}",
            summary.frames,
            summary.mean,
            summary.p95,
            summary.max,
            self.source.label(),
        ))
    }

    /// Print the summary line on stderr. A failed write is dropped, never
    /// a panic back into calloop (D5).
    pub(crate) fn print(&self) {
        if let Some(line) = self.summary_line() {
            let _ = writeln!(std::io::stderr(), "{line}");
        }
    }
}

/// One presentation timestamp, combined from the `presented` event's
/// `tv_sec_hi`, `tv_sec_lo` and `tv_nsec` fields into microseconds of the
/// presentation clock: whole seconds are the 64-bit `tv_sec_hi`:`tv_sec_lo`
/// pair, the fraction the nanoseconds. `None` when the seconds do not fit
/// the nanosecond product — a timestamp no microsecond value holds.
#[must_use]
pub(crate) fn presentation_us(tv_sec_hi: u32, tv_sec_lo: u32, tv_nsec: u32) -> Option<u64> {
    let secs = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
    let nanos = secs
        .checked_mul(1_000_000_000)?
        .checked_add(u64::from(tv_nsec))?;
    // Nanoseconds to microseconds truncates at most 999 ns, well under the
    // resolution the summary's two decimal places in ms keep.
    Some(nanos / 1000)
}

/// The gap between two timestamps, in ms: a backwards sample — a wrapped
/// u32 millisecond callback clock, or an out-of-order presentation —
/// clamps to zero.
#[must_use]
fn gap_ms(from_us: u64, to_us: u64) -> f64 {
    to_f64(to_us.saturating_sub(from_us)) / 1000.0
}

/// An integer to `f64` without an `as` cast. `num_traits`' conversion is
/// total in this direction — every u64/usize round-trips to the nearest
/// f64 — so the fallback is dead.
#[must_use]
fn to_f64<T: num_traits::NumCast + ToPrimitive>(value: T) -> f64 {
    num_traits::cast(value).unwrap_or(0.0)
}

/// The nearest-rank p95 index over `n` sorted gaps: the 1-based rank
/// `ceil(0.95 * n)`, computed in exact integer arithmetic as
/// `ceil(19n/20) = (19n + 19) / 20`, then 0-based — the integer form
/// cannot drift with an f64 rounding error.
#[must_use]
fn p95_rank(n: usize) -> usize {
    let rank = n.saturating_mul(19).saturating_add(19) / 20;
    rank.saturating_sub(1).min(n.saturating_sub(1))
}

/// The optional `wp_presentation` global of one bound session, the frame
/// log's presentation-time source (replace-gtk-with-wayland D7). `None`
/// inside when the compositor does not offer the protocol: the frame log degrades to the
/// frame callbacks' times, never the start (D1's optional-globals rule).
#[derive(Debug, Default)]
pub(crate) struct Presentation {
    manager: Option<WpPresentation>,
}

impl Presentation {
    /// Bind the optional presentation-time global (D1): a missing or
    /// unbindable global is a degradation, never a start failure.
    pub(crate) fn bind(globals: &GlobalList, qh: &QueueHandle<PanelState>) -> Self {
        Presentation {
            manager: globals.bind(qh, 1..=1, GlobalData).ok(),
        }
    }

    /// Whether the compositor offered the protocol: the frame log's source
    /// decision at the tween's begin.
    #[must_use]
    pub(crate) fn is_bound(&self) -> bool {
        self.manager.is_some()
    }

    /// Request presentation feedback for the surface's current content
    /// submission — one call per committed tween frame, right beside the
    /// next `wl_surface.frame` request. The feedback carries the tween's
    /// generation as its user data, so the dispatch can drop a late
    /// `presented` from a tween that already stopped.
    pub(crate) fn feedback(
        &self,
        surface: &WlSurface,
        qh: &QueueHandle<PanelState>,
        generation: u64,
    ) {
        if let Some(manager) = &self.manager {
            manager.feedback(surface, qh, generation);
        }
    }
}

/// The presentation-time manager's dispatch: it carries one event,
/// `clock_id`, sent once at the bind to name the clock the presented
/// timestamps count in. niri reports `CLOCK_MONOTONIC`, the clock the
/// frame callbacks' event times count in too, so both sources of one
/// recorder are comparable and the value needs no recording.
impl Dispatch<WpPresentation, GlobalData> for PanelState {
    fn event(
        _state: &mut PanelState,
        _proxy: &WpPresentation,
        _event: wayland_protocols::wp::presentation_time::client::wp_presentation::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qh: &QueueHandle<PanelState>,
    ) {
    }
}

/// The presentation feedback's dispatch: each `presented` event is one
/// frame's on-screen timestamp, recorded into the running tween's frame
/// log; `discarded` — the compositor replaced the frame before it reached
/// the screen — records nothing, and neither does `sync_output`. The user
/// data is the tween generation the feedback was requested for.
impl Dispatch<WpPresentationFeedback, u64> for PanelState {
    fn event(
        state: &mut PanelState,
        _proxy: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        data: &u64,
        _conn: &Connection,
        _qh: &QueueHandle<PanelState>,
    ) {
        if let wp_presentation_feedback::Event::Presented {
            tv_sec_hi,
            tv_sec_lo,
            tv_nsec,
            ..
        } = event
        {
            // The timestamp is a checked combination and a vector push,
            // neither of which can panic, but it runs inside the panel's
            // dispatch, so it stays inside the guard like every other
            // handler body (D5).
            let poisoned = state.poisoned.clone();
            let _ = guard(&poisoned, || {
                state
                    .tween
                    .record_presented(*data, tv_sec_hi, tv_sec_lo, tv_nsec);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The summary math: mean, p95 (nearest rank) and max over the gaps
    /// between successive frame times, in ms.
    #[test]
    fn the_summary_reports_mean_p95_and_max_gaps() {
        let mut log = FrameLog::begin(true, false).expect("an enabled callback log");
        // Gaps of 4, 8, 16, 32, 40 ms.
        for t in [0, 4_000, 12_000, 28_000, 60_000, 100_000] {
            log.record_callback(u32::try_from(t / 1000).expect("test range"));
        }
        let summary = log.summary().expect("six frames make five gaps");
        assert_eq!(summary.frames, 6);
        assert_eq!(summary.mean, 20.0);
        assert_eq!(summary.p95, 40.0);
        assert_eq!(summary.max, 40.0);
    }

    /// A summary needs two frames: one frame alone has no gap, and a
    /// backwards sample clamps its gap to zero rather than going negative.
    #[test]
    fn the_summary_needs_two_frames_and_clamps_backwards_gaps() {
        let mut log = FrameLog::begin(true, false).expect("an enabled callback log");
        log.record_callback(2_000);
        assert!(log.summary().is_none(), "one frame has no gap");
        assert!(log.summary_line().is_none(), "and prints nothing");
        log.record_callback(1_000);
        let summary = log.summary().expect("two frames make one gap");
        assert_eq!(summary.frames, 2);
        assert_eq!(summary.mean, 0.0);
        assert_eq!(summary.p95, 0.0);
        assert_eq!(summary.max, 0.0);
    }

    /// The line format is stable — the prefix and field names — and names
    /// the clock in the source suffix a script can parse.
    #[test]
    fn the_summary_line_keeps_the_format_and_names_the_source() {
        let mut log = FrameLog::begin(true, false).expect("an enabled callback log");
        for t in [0, 4_000, 12_000] {
            log.record_callback(u32::try_from(t / 1000).expect("test range"));
        }
        assert_eq!(
            log.summary_line().expect("three frames make two gaps"),
            "pinwin: tween frame log: frames=3 mean=6.00ms p95=8.00ms max=8.00ms source=callback"
        );

        let mut presented = FrameLog::begin(true, true).expect("an enabled presentation log");
        presented.record_presented(0, 0, 0);
        presented.record_presented(0, 0, 8_000_000);
        assert_eq!(
            presented.summary_line().expect("two frames make one gap"),
            "pinwin: tween frame log: frames=2 mean=8.00ms p95=8.00ms max=8.00ms source=presentation"
        );
    }

    /// A disabled recorder does not exist: `begin(false, ..)` is `None`, so
    /// nothing records and nothing prints.
    #[test]
    fn a_disabled_recorder_records_and_prints_nothing() {
        assert!(FrameLog::begin(false, true).is_none());
        assert!(FrameLog::begin(false, false).is_none());
    }

    /// The source switch (replace-gtk-with-wayland D7): the
    /// presentation-time protocol wins
    /// when the compositor offers it, the frame callbacks otherwise, and
    /// each recorder ignores the other source's feed.
    #[test]
    fn the_source_follows_the_protocol_and_ignores_the_other_feed() {
        let mut presented = FrameLog::begin(true, true).expect("an enabled log");
        assert_eq!(presented.source, FrameSource::Presentation);
        presented.record_callback(5_000);
        assert!(
            presented.summary_line().is_none(),
            "callback times do not feed it"
        );

        let mut callbacks = FrameLog::begin(true, false).expect("an enabled log");
        assert_eq!(callbacks.source, FrameSource::Callback);
        callbacks.record_presented(0, 1, 0);
        assert!(
            callbacks.summary_line().is_none(),
            "presented times do not feed it"
        );
    }

    /// The presentation timestamp combiner: the 64-bit seconds pair and the
    /// nanoseconds become a monotone microsecond value, and a seconds count
    /// whose nanosecond product overflows u64 is refused rather than
    /// truncated.
    #[test]
    fn the_presentation_timestamp_combines_and_refuses_overflow() {
        // Half a second past the epoch of the clock.
        assert_eq!(presentation_us(0, 0, 500_000_000), Some(500_000));
        // Seconds split across the hi and lo halves: 1 << 32 seconds.
        assert_eq!(presentation_us(1, 0, 0), Some(4_294_967_296_000_000));
        assert_eq!(presentation_us(0, 5, 500_000_000), Some(5_500_000));
        // Truncation only below the microsecond: 999 ns rounds to 0 us.
        assert_eq!(presentation_us(0, 0, 999), Some(0));

        // A seconds count past u64 nanoseconds is refused.
        assert_eq!(presentation_us(u32::MAX, u32::MAX, 0), None);
    }

    /// The nearest-rank p95 index in integer math matches the rank the f64
    /// form computed across sample counts, including the exact-multiple
    /// counts where the f64 error could drift.
    #[test]
    fn the_p95_rank_is_the_nearest_rank_over_the_sorted_gaps() {
        // n gaps, 1-based rank ceil(0.95n), 0-based index one less.
        assert_eq!(p95_rank(1), 0);
        assert_eq!(p95_rank(2), 1);
        assert_eq!(p95_rank(5), 4, "ceil(4.75) = 5");
        assert_eq!(p95_rank(20), 18, "ceil(19) = 19, not past it");
        assert_eq!(p95_rank(40), 37);
        assert_eq!(p95_rank(100), 94);
    }
}
