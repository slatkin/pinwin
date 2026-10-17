//! The animated width transition's state machine (replace-terminal-with-app-host
//! row 10.1, D8): the grow that waits for the app's wide buffer, the shrink
//! that crops the buffer the app already has, and the per-frame values the
//! proxy commits.
//!
//! D8 keeps the method the terminal panel used and the step of
//! [`anim`](super::anim):
//!
//! - **Grow**: the proxy configures the app once to the target width and
//!   holds the current visible width until the app commits a buffer whose
//!   reported size reaches the target — checked at the commit, because a
//!   toolkit reports its new window geometry after its attach (D8 finding 4).
//!   The wait is bounded at 500 ms, after which the panel snaps. Every frame
//!   then crops the docked-edge part of the wide buffer.
//! - **Shrink**: the app keeps the wide buffer it has and the proxy crops it
//!   down each frame; at the end the proxy configures the app to the target
//!   width.
//!
//! Each frame carries the layer surface's size, the margins, the reserve
//! surface's new zone when the gap moves, and the visible [fraction](Fraction)
//! the viewport composition crops to. The driver ([`drive`]) applies them
//! through the commit coordinator (D7) and runs the frame clock.
//!
//! "One size per tween" (D8): while the tween runs, the compositor's
//! intermediate layer-surface configures are acked by the proxy itself and
//! never forwarded ([`route_configure`]), so the app gets exactly one new
//! window size for the whole tween. That configure carries a serial the proxy
//! minted from its own range ([`PROXY_SERIAL_BASE`]), and the app's ack of it
//! is swallowed ([`swallow_ack`]) because the layer surface never saw it.
//!
//! The machine does not read a clock: [`Tween::step`] takes the microseconds
//! since the tween began, so a test drives it without a display
//! (`port-to-rust` D10). Row 10.2 adds the end and interrupt rules (retarget,
//! the 1000 ms clamp, `tween_ms` and the stalled clock) on top of these
//! phases.

use super::anim::{Advance, Tween as Anim};
use crate::geometry::{Geometry, HeldGap, Margins};

use super::compose::Fraction;

#[path = "tween/drive.rs"]
mod drive;

pub(super) use drive::{Slot, advance, is_running, on_app_commit, start, stop};

/// The first serial of the configures the proxy mints itself (D8). A
/// compositor's layer-surface configure serials are small, so an ack at or
/// above this base answers the proxy, not the compositor.
pub(super) const PROXY_SERIAL_BASE: u32 = 1 << 31;

/// How long a grow waits for the app's wide buffer before it snaps (D8): the
/// spec's "Grow waits for the app" bound.
pub(super) const GROW_TIMEOUT_US: i64 = 500_000;

/// Whether an app ack answers a configure the proxy minted itself (D8): the
/// layer surface never saw that serial, so the ack must not be forwarded.
pub(super) fn swallow_ack(serial: u32) -> bool {
    serial >= PROXY_SERIAL_BASE
}

/// What the proxy does with one compositor layer-surface configure (D8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigureRoute {
    /// A tween runs: the proxy acks the intermediate configure itself and
    /// forwards nothing, so the app gets one new size for the whole tween.
    AckOnly,
    /// The configure goes to the app as its window configure.
    Forward,
}

/// The route of one compositor configure (D8).
pub(super) fn route_configure(tweening: bool) -> ConfigureRoute {
    if tweening {
        ConfigureRoute::AckOnly
    } else {
        ConfigureRoute::Forward
    }
}

/// Where the tween stands (D8). Row 10.2 adds the interrupt and retarget
/// transitions between these phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// A grow: the app was configured to the target and the panel holds its
    /// current width until the app commits a wide buffer.
    Waiting,
    /// The eased width moves from the start length to the target.
    Running,
    /// The eased width reached the target; the driver applies this frame and
    /// drops the tween.
    Finished,
}

/// What one [`Tween::step`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// The grow still waits for the app's wide buffer: nothing changes.
    Held,
    /// A frame to apply at the eased width.
    Frame(Frame),
    /// The last frame: the eased width reached the target exactly.
    Finished(Frame),
}

/// One frame's values (D8): what the driver sends before its commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Frame {
    visible: u32,
    layer: (u32, u32),
    margins: Margins,
    gap: HeldGap,
    fraction: Fraction,
}

impl Frame {
    /// The layer surface's size for `set_size`: the visible length across
    /// the edge and 0 along it, which the compositor fills between the end
    /// anchors.
    pub(super) const fn layer_size(self) -> (u32, u32) {
        self.layer
    }

    /// The layer surface's margins, unchanged by a width tween but re-sent
    /// with each frame as D8 lists it.
    pub(super) const fn margins(self) -> Margins {
        self.margins
    }

    /// The gap the reserve surface carries this frame (D8, D9): it follows a
    /// pushing panel and holds still for a covering one.
    pub(super) const fn gap(self) -> HeldGap {
        self.gap
    }
}

/// One running width tween (D8). Built only by [`Tween::start`], so the
/// phase and the eased step always agree (`port-to-rust` D6).
#[derive(Clone, Copy, Debug)]
pub(super) struct Tween {
    /// The target geometry: the edge, the target across and the gutters the
    /// frames resolve their margins and strip against.
    geometry: Geometry,
    from: u32,
    to: u32,
    /// The app surface's extent the visible fraction is taken over (D8):
    /// the window geometry the app reports, its own viewport destination, or
    /// the size the panel configures. Width first, height second — the
    /// surface-local order the proxy's window geometry, viewport destination
    /// and panel size all carry; [`Tween::across`] picks the axis by the
    /// edge, so a top or bottom panel crops against the height.
    surface: (u32, u32),
    phase: Phase,
    anim: Anim,
    /// The gap the panel held before the change: a covering target keeps it
    /// and a pushing one moves it.
    resting_gap: HeldGap,
    /// Whether the gap moves with the panel (D9): only a pushing target whose
    /// strip differs from the resting strip.
    gap_tweening: bool,
    visible: u32,
}

impl Tween {
    /// The tween of a change from `from` to `geometry.across()` over
    /// `duration_ms`. `surface` is the app's current extent in surface-local
    /// pixels, width first, the source of the visible fraction's denominator.
    /// `grow` starts in the wait for the app's wide buffer; a shrink starts
    /// moving at once.
    /// `None` for a change with no distance, no duration or a size the
    /// easing's integer step cannot carry.
    pub(super) fn start(
        geometry: Geometry,
        from: u32,
        surface: (u32, u32),
        resting_gap: HeldGap,
        grow: bool,
        duration_ms: u32,
    ) -> Option<Self> {
        let to = geometry.across();
        if from == to || duration_ms == 0 {
            return None;
        }
        let anim = Anim::begin(
            i32::try_from(from).ok()?,
            i32::try_from(to).ok()?,
            duration_ms,
        );
        Some(Self {
            geometry,
            from,
            to,
            surface: (surface.0.max(1), surface.1.max(1)),
            phase: if grow { Phase::Waiting } else { Phase::Running },
            anim,
            resting_gap,
            gap_tweening: HeldGap::tweens_with_panel(resting_gap.zone(), true, geometry, to),
            visible: from,
        })
    }

    /// The target geometry the tween resolves its frames against.
    pub(super) const fn geometry(self) -> Geometry {
        self.geometry
    }

    /// The length across the edge the panel is moving to. Rows 10.2's
    /// retarget reads it.
    #[cfg(test)]
    pub(super) const fn target(self) -> u32 {
        self.to
    }

    /// Where the tween stands (D8).
    #[cfg(test)]
    pub(super) const fn phase(self) -> Phase {
        self.phase
    }

    /// Whether the tween still owns the frames: it runs while it waits for
    /// the app's wide buffer, moves and has not yet applied its last frame.
    pub(super) const fn is_tweening(self) -> bool {
        !matches!(self.phase, Phase::Finished)
    }

    /// Whether the change makes the panel narrower.
    pub(super) const fn is_shrink(self) -> bool {
        self.to < self.from
    }

    /// The app surface's extent the composition crops (D8), while no window
    /// geometry reports a fresh one.
    pub(super) const fn surface(self) -> (u32, u32) {
        self.surface
    }

    /// The app surface's length across the tween axis (D8): the width on a
    /// left or right panel, the height on a top or bottom one.
    const fn across(self) -> u32 {
        if self.geometry.edge().across_is_width() {
            self.surface.0
        } else {
            self.surface.1
        }
    }

    /// The visible fraction of the app surface the viewport composition
    /// crops to now (D8): the eased length over the surface's length across
    /// the edge.
    pub(super) fn fraction(self) -> Fraction {
        Fraction::of(self.visible, self.across()).unwrap_or_else(Fraction::whole)
    }

    /// The gap the reserve holds once the tween ends: a pushing target's own
    /// strip at the target length, a covering target's resting gap.
    pub(super) fn final_gap(self) -> HeldGap {
        self.resting_gap
            .reserve(self.gap_tweening, self.geometry, self.to)
    }

    /// One step at `elapsed_us` microseconds after the tween began (D8).
    ///
    /// A waiting grow holds for at most [`GROW_TIMEOUT_US`] and then snaps to
    /// the target. A running tween eases with [`crate::anim`] and finishes
    /// exactly on the target, so the last frame's length is the target and
    /// nothing re-rounds it.
    pub(super) fn step(&mut self, elapsed_us: i64) -> Step {
        match self.phase {
            Phase::Waiting => {
                if elapsed_us >= GROW_TIMEOUT_US {
                    self.visible = self.to;
                    self.phase = Phase::Finished;
                    return Step::Finished(self.frame());
                }
                Step::Held
            }
            Phase::Running => match self.anim.advance(elapsed_us) {
                Advance::Frame(px) => {
                    self.visible = px
                        .max(0)
                        .unsigned_abs()
                        .clamp(self.from.min(self.to), self.from.max(self.to));
                    Step::Frame(self.frame())
                }
                Advance::Finished => {
                    self.visible = self.to;
                    self.phase = Phase::Finished;
                    Step::Finished(self.frame())
                }
            },
            Phase::Finished => Step::Finished(self.frame()),
        }
    }

    /// The app committed a surface: a waiting grow starts moving once the
    /// length the app reports across the tween axis reaches the target (D8
    /// finding 4). `false` for a shrink or a grow that already waits no
    /// more.
    pub(super) fn app_commit(&mut self, reported: Option<u32>) -> bool {
        if self.phase != Phase::Waiting {
            return false;
        }
        let Some(reported) = reported else {
            return false;
        };
        if reported < self.to {
            return false;
        }
        let reported = reported.max(1);
        if self.geometry.edge().across_is_width() {
            self.surface.0 = reported;
        } else {
            self.surface.1 = reported;
        }
        self.phase = Phase::Running;
        true
    }

    /// This frame's values (D8).
    fn frame(self) -> Frame {
        Frame {
            visible: self.visible,
            layer: self.geometry.size_at(self.visible),
            margins: self.geometry.margins(),
            gap: self
                .resting_gap
                .reserve(self.gap_tweening, self.geometry, self.visible),
            fraction: self.fraction(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Edge, Gutters, ReserveMode};

    /// A panel geometry of `across` logical pixels on `edge`, with no
    /// gutters (D9).
    fn geometry(edge: Edge, across: u32, reserve: ReserveMode) -> Geometry {
        Geometry::new(edge, across, Gutters::new(0, 0, 0, 0), reserve)
    }

    /// A pushing geometry of `across` pixels on `edge`.
    fn push(edge: Edge, across: u32) -> Geometry {
        geometry(edge, across, ReserveMode::Push)
    }

    /// A covering geometry of `across` pixels on `edge`.
    fn cover(edge: Edge, across: u32) -> Geometry {
        geometry(edge, across, ReserveMode::Cover)
    }

    /// The app surface `across` pixels across the tween axis on `edge`, with
    /// 1080 along it: the width-first pair the driver hands the tween (D8).
    fn surface(edge: Edge, across: u32) -> (u32, u32) {
        if edge.across_is_width() {
            (across, 1080)
        } else {
            (1080, across)
        }
    }

    /// A 200 ms grow from `from` to `to` on `edge`, with the app surface at
    /// its old length.
    fn growing(edge: Edge, from: u32, to: u32) -> Tween {
        Tween::start(
            push(edge, to),
            from,
            surface(edge, from),
            HeldGap::start(push(edge, from)),
            true,
            200,
        )
        .expect("a 200 ms grow")
    }

    /// A 200 ms shrink from `from` to `to` on `edge`, with the app surface
    /// still at the wide length.
    fn shrinking(edge: Edge, from: u32, to: u32) -> Tween {
        Tween::start(
            push(edge, to),
            from,
            surface(edge, from),
            HeldGap::start(push(edge, from)),
            false,
            200,
        )
        .expect("a 200 ms shrink")
    }

    /// The frame of a step, panicking when the step held.
    fn frame(step: Step) -> Frame {
        match step {
            Step::Frame(frame) | Step::Finished(frame) => frame,
            Step::Held => panic!("the step holds no frame"),
        }
    }

    /// Grows and shrinks keep the panel at its start length while a grow
    /// waits, then ease to the target. Both directions end exactly on the
    /// target, on a left and on a top panel (D8, D9).
    #[test]
    fn grow_and_shrink_end_on_the_target_on_both_axes() {
        for edge in [Edge::Left, Edge::Top] {
            let mut grow = growing(edge, 300, 450);
            assert_eq!(grow.step(0), Step::Held, "the grow holds first");
            assert!(grow.app_commit(Some(450)));
            let first = frame(grow.step(0));
            assert_eq!(first.visible, 300, "the first frame is the start");
            let last = frame(grow.step(200_000));
            assert_eq!(last.visible, 450, "the grow ends on the target on {edge:?}");
            assert_eq!(
                last.layer,
                if edge.across_is_width() {
                    (450, 0)
                } else {
                    (0, 450)
                },
                "the layer size across the edge on {edge:?}"
            );

            let mut shrink = shrinking(edge, 450, 300);
            let _ = shrink.step(0);
            let last = frame(shrink.step(200_000));
            assert_eq!(
                last.visible, 300,
                "the shrink ends on the target on {edge:?}"
            );
            assert_eq!(shrink.phase(), Phase::Finished);
        }
    }

    /// The driver hands the tween a width-first surface pair, so a top or
    /// bottom panel takes its length across the edge from the height: the
    /// first frame of a shrink and of a grow crops against the panel's own
    /// length, not the output's width (D8, spec "Content does not slide").
    #[test]
    fn a_top_panel_reads_the_across_length_from_the_height() {
        for edge in [Edge::Top, Edge::Bottom] {
            let mut shrink = shrinking(edge, 450, 300);
            let first = frame(shrink.step(0));
            assert_eq!(first.visible, 450);
            assert_eq!(
                first.fraction,
                Fraction::of(450, 450).expect("the whole height"),
                "the first shrink frame crops the height on {edge:?}"
            );
            // t = 0.5 -> ease(0.5) = 0.875 -> 450 - round(150 * 0.875).
            let mid = frame(shrink.step(100_000));
            assert_eq!(mid.visible, 319);
            assert_eq!(
                mid.fraction,
                Fraction::of(319, 450).expect("the docked part of the height"),
                "the mid shrink frame crops the height on {edge:?}"
            );

            let mut grow = growing(edge, 300, 450);
            assert_eq!(grow.step(0), Step::Held);
            assert!(grow.app_commit(Some(450)));
            assert_eq!(
                grow.surface,
                surface(edge, 450),
                "the wide surface keeps the width first on {edge:?}"
            );
            let first = frame(grow.step(0));
            assert_eq!(
                first.fraction,
                Fraction::of(300, 450).expect("the start part of the height"),
                "the first grow frame crops the height on {edge:?}"
            );
            let last = frame(grow.step(200_000));
            assert_eq!(
                last.fraction,
                Fraction::of(450, 450).expect("the whole height"),
                "the grow ends on the whole height on {edge:?}"
            );
            assert_eq!(
                last.layer,
                (0, 450),
                "the layer size across the edge on {edge:?}"
            );
        }
    }

    /// A grow holds the panel's current length until the app commits a
    /// buffer whose reported length reaches the target (D8 finding 4), and
    /// then eases from that length.
    #[test]
    fn a_grow_starts_only_at_a_commit_that_reports_the_target() {
        let mut tween = growing(Edge::Left, 400, 600);
        assert_eq!(tween.phase(), Phase::Waiting);
        assert!(!tween.app_commit(Some(575)), "an intermediate size waits");
        assert!(!tween.app_commit(None), "no report waits");
        assert_eq!(tween.step(0), Step::Held);
        assert_eq!(tween.step(499_999), Step::Held, "still inside the bound");
        assert!(tween.app_commit(Some(600)), "the target starts the grow");
        assert_eq!(tween.phase(), Phase::Running);
        assert_eq!(tween.surface, (600, 1080), "the wide surface is the crop");
        let first = frame(tween.step(0));
        assert_eq!(first.visible, 400);
        assert_eq!(first.fraction, Fraction::of(400, 600).expect("a fraction"));
        // t = 0.5 -> ease(0.5) = 0.875 -> 400 + round(200 * 0.875).
        let mid = frame(tween.step(100_000));
        assert_eq!(mid.visible, 575);
        let last = frame(tween.step(200_000));
        assert_eq!(last.visible, 600);
        assert_eq!(
            last.fraction,
            Fraction::of(600, 600).expect("the whole wide surface")
        );
    }

    /// A grow that does not see the wide buffer within 500 ms snaps to the
    /// target (D8, spec "Grow waits for the app").
    #[test]
    fn a_grow_snaps_after_the_500_ms_bound() {
        let mut tween = growing(Edge::Left, 400, 600);
        assert_eq!(tween.step(0), Step::Held);
        assert_eq!(tween.step(GROW_TIMEOUT_US - 1), Step::Held);
        let last = frame(tween.step(GROW_TIMEOUT_US));
        assert_eq!(last.visible, 600, "the snap lands on the target");
        assert_eq!(tween.phase(), Phase::Finished);
        assert!(
            !tween.app_commit(Some(600)),
            "a snapped grow takes no commit"
        );
    }

    /// A shrink keeps the app's wide buffer and crops the docked part of it
    /// down each frame, so the content stays against the edge (D8, spec
    /// "Content does not slide").
    #[test]
    fn a_shrink_crops_the_wide_buffer_from_the_docked_edge() {
        let mut tween = shrinking(Edge::Right, 600, 400);
        let first = frame(tween.step(0));
        assert_eq!(first.visible, 600);
        assert_eq!(
            first.fraction,
            Fraction::of(600, 600).expect("the whole wide surface")
        );
        // t = 0.5 -> ease = 0.875 -> 600 - 175.
        let mid = frame(tween.step(100_000));
        assert_eq!(mid.visible, 425);
        assert_eq!(
            mid.fraction,
            Fraction::of(425, 600).expect("the docked part of the wide surface")
        );
        assert_eq!(mid.layer, (425, 0), "the panel's own width follows");
        let last = frame(tween.step(200_000));
        assert_eq!(last.visible, 400);
        assert_eq!(
            last.fraction,
            Fraction::of(400, 600).expect("the last crop keeps the right edge")
        );
        assert_eq!(tween.phase(), Phase::Finished);
        assert!(tween.is_shrink());
    }

    /// A pushing target moves the strip with the panel (D8, spec "Animate on
    /// request"): every frame's gap is the strip at that frame's length.
    #[test]
    fn a_pushing_gap_moves_with_the_panel() {
        let mut tween = growing(Edge::Left, 400, 600);
        let _ = tween.step(0);
        assert!(tween.app_commit(Some(600)));
        let _ = tween.step(0);
        let mid = frame(tween.step(100_000));
        assert_eq!(mid.visible, 575);
        assert_eq!(mid.gap.zone(), 575, "the strip is the visible length");
        let last = frame(tween.step(200_000));
        assert_eq!(last.gap.zone(), 600, "the strip ends on the target");
    }

    /// A covering target holds the strip still (D8, spec "Covering expand
    /// holds tiles still"): the gap rests at the last pushing strip in every
    /// frame, and the panel grows over it.
    #[test]
    fn a_covering_gap_holds_still() {
        let resting = HeldGap::start(push(Edge::Left, 400));
        let mut tween = Tween::start(cover(Edge::Left, 600), 400, (400, 1080), resting, true, 200)
            .expect("a covering grow");
        let _ = tween.step(0);
        assert!(tween.app_commit(Some(600)));
        let _ = tween.step(0);
        let mid = frame(tween.step(100_000));
        assert_eq!(mid.visible, 575, "the panel still grows");
        assert_eq!(mid.gap.zone(), 400, "the strip holds still");
        assert_eq!(mid.gap.edge(), Edge::Left);
        let last = frame(tween.step(200_000));
        assert_eq!(last.visible, 600);
        assert_eq!(last.gap.zone(), 400, "the final frame leaves the strip");
    }

    /// While a tween runs every compositor configure is acked by the proxy
    /// and none reaches the app, so the app gets one new size for the whole
    /// tween (D8 finding 3, spec "One new size for the app").
    #[test]
    fn a_tween_forwards_one_app_size() {
        let mut tween = growing(Edge::Left, 400, 600);
        // The one configure the proxy sends when the grow starts.
        let mut app_sizes = vec![tween.target()];
        assert_eq!(
            route_configure(tween.is_tweening()),
            ConfigureRoute::AckOnly,
            "the waiting grow swallows the compositor's configures"
        );
        assert!(tween.app_commit(Some(600)));
        for elapsed in [0, 50_000, 100_000, 150_000] {
            let _ = tween.step(elapsed);
            if route_configure(tween.is_tweening()) == ConfigureRoute::Forward {
                app_sizes.push(tween.target());
            }
        }
        let _ = tween.step(200_000);
        assert_eq!(tween.phase(), Phase::Finished);
        assert_eq!(
            route_configure(tween.is_tweening()),
            ConfigureRoute::Forward,
            "the steady path forwards again after the tween"
        );
        assert_eq!(app_sizes, [600], "exactly one size for the whole tween");
    }

    /// A frame queued for a tween the slot has since cleared is stale and
    /// does nothing, so it cannot re-send its layer state over a hide, a
    /// re-dock or a newer tween; a frame of the tween the slot still owns
    /// applies (D8).
    #[test]
    fn a_cleared_tween_drops_the_frames_it_queued() {
        let slot = Slot::new();
        let queued = slot.generation();
        assert!(slot.is_current(queued), "the tween's frame applies");
        slot.clear();
        assert!(!slot.is_current(queued), "a cleared tween's frame is stale");
        slot.clear();
        assert!(
            !slot.is_current(queued),
            "a second clear keeps the old frame stale"
        );
    }

    /// A serial the proxy minted is swallowed, and a compositor's is not
    /// (D8 finding 3).
    #[test]
    fn the_app_ack_of_a_minted_configure_is_not_forwarded() {
        assert!(swallow_ack(PROXY_SERIAL_BASE));
        assert!(swallow_ack(PROXY_SERIAL_BASE + 7));
        assert!(!swallow_ack(0));
        assert!(!swallow_ack(1024));
    }

    /// The tween's held gap at the end is the state a plain apply would
    /// leave: a pushing target's own strip, a covering target's resting gap
    /// (spec "Exact final width").
    #[test]
    fn the_final_gap_matches_a_plain_apply() {
        let pushing = Tween::start(
            push(Edge::Left, 600),
            400,
            (400, 1080),
            HeldGap::start(push(Edge::Left, 400)),
            true,
            200,
        )
        .expect("a pushing grow");
        assert_eq!(
            pushing.final_gap(),
            HeldGap::start(push(Edge::Left, 400)).after_publish(push(Edge::Left, 600))
        );
        let resting = HeldGap::start(push(Edge::Left, 400));
        let covering = Tween::start(cover(Edge::Left, 600), 400, (400, 1080), resting, true, 200)
            .expect("a covering grow");
        assert_eq!(covering.final_gap(), resting);
    }

    /// No duration and no distance start no tween (D8: a snap), and an app
    /// surface of no pixels never divides the fraction.
    #[test]
    fn a_change_that_cannot_animate_starts_no_tween() {
        assert!(
            Tween::start(
                push(Edge::Left, 600),
                400,
                (400, 1080),
                HeldGap::start(push(Edge::Left, 400)),
                true,
                0
            )
            .is_none()
        );
        assert!(
            Tween::start(
                push(Edge::Left, 400),
                400,
                (400, 1080),
                HeldGap::start(push(Edge::Left, 400)),
                true,
                200
            )
            .is_none()
        );
        let tween = Tween::start(
            push(Edge::Left, 600),
            400,
            (0, 0),
            HeldGap::start(push(Edge::Left, 400)),
            false,
            200,
        )
        .expect("a shrink");
        assert_eq!(
            tween.fraction(),
            Fraction::of(400, 1).expect("the surface is at least one pixel")
        );
    }
}
