//! The panel's geometry: the output edge it docks to, its resolved size
//! across that edge, the four directional gutters, the reserve mode and the
//! held gap (replace-terminal-with-app-host D9).
//!
//! This is one pure module where the terminal's `layout.rs` and
//! `surfaces/gap.rs` were two: it computes the layer surface's anchors,
//! margins and size, the strip a pushing panel reserves and how the held gap
//! moves, and it names no Wayland object (`port-to-rust` D10), so every
//! decision is unit tested with no compositor.
//!
//! The reservation rules, stated once: pushing moves the gap to the panel's
//! own strip, `near + across + far`; covering leaves the gap where the last
//! pushing setting put it — an empty strip when none ever applied — and
//! draws over the tiled windows; an edge change always moves the gap to the
//! new edge.

use std::num::NonZeroU32;

/// The output edge a panel docks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// Whether the size across this edge is a width (a side edge) or a height
    /// (a top or bottom edge).
    #[must_use]
    pub const fn across_is_width(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// The edge opposite this one.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    /// The anchors of a surface that fills the output's length at this edge:
    /// the edge itself plus both ends of its length.
    #[must_use]
    pub const fn anchors(self) -> Anchors {
        Anchors::at(self)
    }
}

/// The output edges a layer surface anchors to.
///
/// A panel and its reserve both anchor to the panel's edge plus both ends of
/// that edge, so the anchored set is every edge but the opposite one: the
/// across-edge size is then explicit and the compositor fills the length
/// between the ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchors {
    far: Edge,
}

impl Anchors {
    /// The anchors at `edge`: the edge itself plus both ends of its length.
    #[must_use]
    pub const fn at(edge: Edge) -> Self {
        Self {
            far: edge.opposite(),
        }
    }

    /// Whether a surface anchored here sticks to `edge`.
    #[must_use]
    pub const fn contains(self, edge: Edge) -> bool {
        match self.far {
            Edge::Left => !matches!(edge, Edge::Left),
            Edge::Right => !matches!(edge, Edge::Right),
            Edge::Top => !matches!(edge, Edge::Top),
            Edge::Bottom => !matches!(edge, Edge::Bottom),
        }
    }
}

/// The four directional gutters in logical pixels, named after the output
/// edges they measure from (spec "Directional gutters and docking
/// geometry").
///
/// A gutter may be negative: then the panel extends beyond that output edge,
/// or the strip ends before the panel's far side. Fields are private, so only
/// [`Gutters::new`] builds a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gutters {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
}

impl Gutters {
    #[must_use]
    pub const fn new(left: i32, right: i32, top: i32, bottom: i32) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }

    /// The near gutter: the one on the panel's edge.
    #[must_use]
    pub const fn near(self, edge: Edge) -> i32 {
        match edge {
            Edge::Left => self.left,
            Edge::Right => self.right,
            Edge::Top => self.top,
            Edge::Bottom => self.bottom,
        }
    }

    /// The far gutter: the one on the edge opposite the panel's.
    #[must_use]
    pub const fn far(self, edge: Edge) -> i32 {
        self.near(edge.opposite())
    }

    /// The side gutter at the start of the panel's length: Top on a side
    /// edge, where the length runs down the output, and Left on a top or
    /// bottom edge, where it runs across.
    #[must_use]
    pub const fn along_start(self, edge: Edge) -> i32 {
        match edge {
            Edge::Left | Edge::Right => self.top,
            Edge::Top | Edge::Bottom => self.left,
        }
    }

    /// The side gutter at the end of the panel's length: Bottom on a side
    /// edge, Right on a top or bottom edge.
    #[must_use]
    pub const fn along_end(self, edge: Edge) -> i32 {
        match edge {
            Edge::Left | Edge::Right => self.bottom,
            Edge::Top | Edge::Bottom => self.right,
        }
    }
}

/// The layer surface's margins, in logical pixels, named after the output
/// edges they measure from. A negative margin moves the panel past its edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Margins {
    top: i32,
    right: i32,
    bottom: i32,
    left: i32,
}

impl Margins {
    #[must_use]
    pub const fn new(top: i32, right: i32, bottom: i32, left: i32) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    #[must_use]
    pub const fn top(self) -> i32 {
        self.top
    }

    #[must_use]
    pub const fn right(self) -> i32 {
        self.right
    }

    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.bottom
    }

    #[must_use]
    pub const fn left(self) -> i32 {
        self.left
    }
}

/// What the panel does with the strip it covers (spec "Reserve space so tiles
/// start beside the panel").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReserveMode {
    /// Push: hold the strip at this panel's own size, so tiles start beside it.
    Push,
    /// Cover: leave the strip where the last pushing setting put it.
    Cover,
}

/// A percent of the output's length across the panel's edge, from 1 to 100
/// (spec "Size values").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Percent(NonZeroU32);

impl Percent {
    /// A percent from 1 to 100; zero and anything above 100 is `None`.
    #[must_use]
    pub fn new(percent: u32) -> Option<Self> {
        NonZeroU32::new(percent)
            .filter(|value| value.get() <= 100)
            .map(Self)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// A panel's size setting: a whole number of logical pixels, or a percent of
/// the output's width on a side edge and of its height on a top or bottom
/// edge (spec "Size values").
///
/// Both forms carry their invariant, so a zero size and a percent above 100
/// are unrepresentable (`port-to-rust` D6); the percent resolves to pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// A whole number of logical pixels, at least one.
    Pixels(NonZeroU32),
    /// A percent of the output's length across the panel's edge.
    Percent(Percent),
}

impl Size {
    /// A size of `px` logical pixels; zero is `None`.
    #[must_use]
    pub fn pixels(px: u32) -> Option<Self> {
        NonZeroU32::new(px).map(Self::Pixels)
    }

    /// A size of `percent` of the output; anything outside 1..=100 is `None`.
    #[must_use]
    pub fn percent(percent: u32) -> Option<Self> {
        Percent::new(percent).map(Self::Percent)
    }
}

/// A panel's resolved geometry: the edge it docks to, its size across that
/// edge in logical pixels, the four gutters and the reserve mode.
///
/// The size across the edge is the panel's width on a side edge and its
/// height on a top or bottom edge; the length along the edge is what the
/// compositor fills between the two end anchors, minus the side gutters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    edge: Edge,
    across: u32,
    gutters: Gutters,
    reserve: ReserveMode,
}

impl Geometry {
    /// A geometry of `across` logical pixels on `edge`, inset by `gutters`.
    #[must_use]
    pub const fn new(edge: Edge, across: u32, gutters: Gutters, reserve: ReserveMode) -> Self {
        Self {
            edge,
            across,
            gutters,
            reserve,
        }
    }

    #[must_use]
    pub const fn edge(self) -> Edge {
        self.edge
    }

    /// The size across the panel's edge, in logical pixels: its width on a
    /// side edge and its height on a top or bottom edge.
    #[must_use]
    pub const fn across(self) -> u32 {
        self.across
    }

    #[must_use]
    pub const fn gutters(self) -> Gutters {
        self.gutters
    }

    #[must_use]
    pub const fn reserve(self) -> ReserveMode {
        self.reserve
    }

    /// The panel surface's anchors: its edge plus both ends of that edge.
    #[must_use]
    pub const fn anchors(self) -> Anchors {
        self.edge.anchors()
    }

    /// The panel surface's margins: the near gutter insets the panel from its
    /// edge and the side gutters inset its length at the ends.
    #[must_use]
    pub const fn margins(self) -> Margins {
        let gutters = self.gutters;
        match self.edge {
            Edge::Left => Margins {
                top: gutters.top,
                right: 0,
                bottom: gutters.bottom,
                left: gutters.left,
            },
            Edge::Right => Margins {
                top: gutters.top,
                right: gutters.right,
                bottom: gutters.bottom,
                left: 0,
            },
            Edge::Top => Margins {
                top: gutters.top,
                right: gutters.right,
                bottom: 0,
                left: gutters.left,
            },
            Edge::Bottom => Margins {
                top: 0,
                right: gutters.right,
                bottom: gutters.bottom,
                left: gutters.left,
            },
        }
    }

    /// The panel surface's size: this geometry's size on the panel's edge and
    /// 0 on the other axis, which the compositor fills between the two end
    /// anchors.
    #[must_use]
    pub const fn size(self) -> (u32, u32) {
        self.size_at(self.across)
    }

    /// The panel surface's size of a panel `across` pixels into its edge, for
    /// the frames of a width animation.
    #[must_use]
    pub const fn size_at(self, across: u32) -> (u32, u32) {
        if self.edge.across_is_width() {
            (across, 0)
        } else {
            (0, across)
        }
    }

    /// The strip this geometry covers at its edge: `near + across + far`, or
    /// `None` on the overflow that validation rejects.
    #[must_use]
    pub fn strip(self) -> Option<i32> {
        self.strip_at(self.across)
    }

    /// The strip covered by a panel `across` pixels into its edge, for the
    /// frames of a width animation.
    #[must_use]
    pub fn strip_at(self, across: u32) -> Option<i32> {
        let sum = i64::from(self.gutters.near(self.edge))
            + i64::from(across)
            + i64::from(self.gutters.far(self.edge));
        i32::try_from(sum).ok()
    }
}

/// The gap the reserve surface holds: its edge and exclusive zone in pixels.
/// A zone of zero reserves nothing, which is a covering start's held strip.
///
/// Fields are private, so the gap moves only through [`HeldGap::start`],
/// [`HeldGap::after_publish`] and [`HeldGap::reserve`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldGap {
    edge: Edge,
    zone: i32,
}

impl HeldGap {
    /// The gap a panel starts with: a pushing geometry reserves its own strip
    /// from the first frame; a covering start holds an empty strip on its edge.
    #[must_use]
    pub fn start(geometry: Geometry) -> Self {
        let zone = match geometry.reserve() {
            ReserveMode::Push => geometry.strip().unwrap_or(0),
            ReserveMode::Cover => 0,
        };
        Self {
            edge: geometry.edge(),
            zone,
        }
    }

    /// The gap after a validated apply: a pushing geometry resets both the
    /// edge and the zone to its own strip; a covering geometry never touches
    /// the zone and moves the edge only when the panel changed edges — the
    /// gap follows the panel to the new edge at the last pushing size.
    #[must_use]
    pub fn after_publish(self, geometry: Geometry) -> Self {
        let zone = match geometry.reserve() {
            ReserveMode::Push => geometry.strip().unwrap_or(self.zone),
            ReserveMode::Cover => self.zone,
        };
        Self {
            edge: geometry.edge(),
            zone,
        }
    }

    /// The gap the reserve surface draws for one frame. While an apply has
    /// flagged the gap as tweening — the one case where a pushing target's
    /// strip differs from the strip the gap rests at — the gap tweens with the
    /// panel; otherwise it holds the strip the last apply left.
    #[must_use]
    pub fn reserve(self, gap_tweening: bool, geometry: Geometry, across_px: u32) -> Self {
        if gap_tweening
            && geometry.reserve() == ReserveMode::Push
            && let Some(zone) = geometry.strip_at(across_px)
        {
            return Self {
                edge: geometry.edge(),
                zone,
            };
        }
        self
    }

    /// Whether an animated apply tweens the gap with the panel: only a pushing
    /// target whose strip differs from the strip the gap rests at right now.
    /// The caller reads that strip before the apply moves the held gap, so a
    /// covering excursion's shrink-back holds the gap still.
    #[must_use]
    pub fn tweens_with_panel(
        gap_rests_at: i32,
        animate: bool,
        geometry: Geometry,
        across_px: u32,
    ) -> bool {
        animate
            && geometry.reserve() == ReserveMode::Push
            && geometry
                .strip_at(across_px)
                .is_some_and(|strip| strip != gap_rests_at)
    }

    /// The edge the reserve surface anchors to.
    #[must_use]
    pub const fn edge(self) -> Edge {
        self.edge
    }

    /// The exclusive zone the reserve surface carries.
    #[must_use]
    pub const fn zone(self) -> i32 {
        self.zone
    }

    /// The reserve surface's anchors: its edge plus both ends of that edge, so
    /// the strip covers the output's full length even when side gutters inset
    /// the panel.
    #[must_use]
    pub const fn anchors(self) -> Anchors {
        self.edge.anchors()
    }

    /// The reserve surface's size: the held zone on the panel's edge axis and 0
    /// on the other, which the compositor fills between the ends. A zero zone
    /// gives a zero size, so a gap that reserves nothing shows nothing.
    #[must_use]
    pub fn size(self) -> (u32, u32) {
        let zone = self.zone.max(0).unsigned_abs();
        if self.edge.across_is_width() {
            (zone, 0)
        } else {
            (0, zone)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(edge: Edge, across: u32, gutters: Gutters) -> Geometry {
        Geometry::new(edge, across, gutters, ReserveMode::Push)
    }

    fn covering(edge: Edge, across: u32, gutters: Gutters) -> Geometry {
        Geometry::new(edge, across, gutters, ReserveMode::Cover)
    }

    fn held(gap: HeldGap) -> (Edge, i32) {
        (gap.edge(), gap.zone())
    }

    #[test]
    fn left_docking() {
        let docked = geometry(Edge::Left, 320, Gutters::new(8, 12, 0, 0));
        assert_eq!(
            docked.margins(),
            Margins::new(0, 0, 0, 8),
            "the panel starts 8 pixels from the left output edge"
        );
        assert_eq!(
            docked.strip(),
            Some(340),
            "the strip reserves 8 + 320 + 12 at the left edge"
        );
        assert_eq!(docked.size(), (320, 0));
        assert_eq!(
            docked.anchors(),
            Anchors::at(Edge::Left),
            "the panel anchors to its edge and both ends of its length"
        );
        assert!(docked.anchors().contains(Edge::Top));
        assert!(docked.anchors().contains(Edge::Bottom));
        assert!(!docked.anchors().contains(Edge::Right));
    }

    #[test]
    fn right_docking() {
        let docked = geometry(Edge::Right, 320, Gutters::new(8, 12, 0, 0));
        assert_eq!(
            docked.margins(),
            Margins::new(0, 12, 0, 0),
            "the panel ends 12 pixels before the right output edge"
        );
        assert_eq!(docked.strip(), Some(340));

        // A pushing apply releases the left reservation: the held gap moves
        // to the right edge at the pushing size resolved there.
        let left = HeldGap::start(geometry(Edge::Left, 320, Gutters::new(8, 12, 0, 0)));
        assert_eq!(held(left), (Edge::Left, 340));
        assert_eq!(held(left.after_publish(docked)), (Edge::Right, 340));
    }

    #[test]
    fn top_docking() {
        let docked = geometry(Edge::Top, 400, Gutters::new(20, 30, 10, 6));
        assert_eq!(
            docked.margins(),
            Margins::new(10, 30, 0, 20),
            "10 below the top edge, 20 from the left edge and 30 before the right edge"
        );
        assert_eq!(docked.size(), (0, 400), "the panel is 400 pixels high");
        assert_eq!(
            docked.strip(),
            Some(416),
            "the strip reserves 10 + 400 + 6 at the top edge"
        );

        // The reserved strip spans the output's full length: the reserve
        // surface anchors to the top edge and both ends with no margins, so
        // the side gutters inset only the panel and the strip is 416 deep
        // across the full output width.
        let gap = HeldGap::start(docked);
        assert!(gap.anchors().contains(Edge::Top));
        assert!(gap.anchors().contains(Edge::Left));
        assert!(gap.anchors().contains(Edge::Right));
        assert!(!gap.anchors().contains(Edge::Bottom));
        assert_eq!(gap.size(), (0, 416));
        assert_eq!(held(gap), (Edge::Top, 416));
    }

    #[test]
    fn vertical_insets() {
        let docked = geometry(Edge::Left, 320, Gutters::new(8, 12, 24, 10));
        assert_eq!(
            docked.margins(),
            Margins::new(24, 0, 10, 8),
            "the panel begins 24 below the output top and ends 10 above its bottom"
        );
        assert_eq!(
            docked.size(),
            (320, 0),
            "the height is what the insets leave"
        );
        assert_eq!(
            docked.strip(),
            Some(340),
            "the side gutters leave the reservation a full-height strip"
        );
        assert_eq!(
            HeldGap::start(docked).size(),
            (340, 0),
            "the strip is 340 deep and spans the full output height"
        );
    }

    #[test]
    fn negative_gutter() {
        let docked = geometry(Edge::Left, 320, Gutters::new(-40, 12, 0, 0));
        assert_eq!(
            docked.margins(),
            Margins::new(0, 0, 0, -40),
            "the panel starts 40 pixels beyond the left output edge"
        );
        assert_eq!(
            docked.strip(),
            Some(292),
            "the reservation is -40 + 320 + 12"
        );
    }

    #[test]
    fn covering_layout_holds_the_gap() {
        let push = geometry(Edge::Left, 320, Gutters::new(8, 12, 0, 0));
        let gap = HeldGap::start(push);
        assert_eq!(held(gap), (Edge::Left, 340));

        let cover = covering(Edge::Left, 1000, Gutters::new(8, 12, 0, 0));
        assert_eq!(cover.size(), (1000, 0), "the panel is 1000 pixels wide");
        assert_eq!(
            held(gap.after_publish(cover)),
            (Edge::Left, 340),
            "a covering apply leaves the reserved strip alone"
        );
    }

    #[test]
    fn side_switch_moves_the_gap() {
        // The pushing size is `25%` of a 1920 pixel wide output, which is 480
        // pixels; the held gap keeps the resolved pixels, not the percent.
        let push = geometry(Edge::Right, 480, Gutters::new(0, 0, 0, 0));
        let gap = HeldGap::start(push);
        assert_eq!(held(gap), (Edge::Right, 480));

        let moved = gap.after_publish(covering(Edge::Left, 480, Gutters::new(0, 0, 0, 0)));
        assert_eq!(
            held(moved),
            (Edge::Left, 480),
            "a covering side switch moves the reservation to the new edge"
        );
    }

    #[test]
    fn the_reserve_draws_from_the_held_gap_not_the_geometry() {
        // A pushing 360 pixel panel with a 12 pixel far gutter holds 372.
        let gap = HeldGap::start(geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0)));
        assert_eq!(held(gap), (Edge::Left, 372));

        // A covering geometry on the same edge with a tween idle draws the
        // held strip, not its own wider strip.
        let cover = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(gap.reserve(false, cover, 1080)), (Edge::Left, 372));

        // A pushing geometry with no tween running agrees with the held gap:
        // the apply has already reset it to the geometry's own strip.
        let push = geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(gap.reserve(false, push, 360)), (Edge::Left, 372));
    }

    #[test]
    fn a_flagged_gap_tween_moves_the_gap_with_the_panel() {
        let gap = HeldGap::start(geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0)));
        assert_eq!(held(gap), (Edge::Left, 372));

        // Mid-tween at a 700 pixel panel: the gap shows the strip at the
        // eased width, 0 + 700 + 12.
        let tweening = geometry(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(gap.reserve(true, tweening, 700)), (Edge::Left, 712));

        // The gap follows the tweening geometry's edge.
        let tweening = geometry(Edge::Right, 1080, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(gap.reserve(true, tweening, 700)), (Edge::Right, 712));
    }

    #[test]
    fn a_covering_tween_holds_the_gap() {
        let gap = HeldGap::start(geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0)));
        let tweening = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
        assert_eq!(
            held(gap.reserve(true, tweening, 700)),
            (Edge::Left, 372),
            "a covering target never moves the gap, flag or not"
        );
    }

    #[test]
    fn the_gap_tween_decision_compares_against_the_strip_the_gap_rests_at() {
        // Pushing 360 -> 1080 pixels: the gap rests at 372 and the target
        // strip is 1092, so the gap tweens.
        let expanding = geometry(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
        assert!(HeldGap::tweens_with_panel(372, true, expanding, 1080));

        // Shrinking a covering excursion back to the original pushing width:
        // the target strip is the strip the gap rests at, so it holds still.
        let shrinking_back = geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0));
        assert!(!HeldGap::tweens_with_panel(372, true, shrinking_back, 360));

        // A covering target never tweens the gap.
        let cover = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
        assert!(!HeldGap::tweens_with_panel(372, true, cover, 1080));

        // A snap apply (zero duration, no animation, a side or gutter change)
        // applies the strip in one step.
        assert!(!HeldGap::tweens_with_panel(372, false, expanding, 1080));

        // A pushing target whose strip the gap already rests at does not tween.
        assert!(!HeldGap::tweens_with_panel(1092, true, expanding, 1080));
    }

    #[test]
    fn the_starting_held_gap_follows_the_reserve_mode() {
        let push = geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(HeldGap::start(push)), (Edge::Left, 372));

        // A covering start reserves nothing: no earlier pushing width to hold.
        let cover = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(HeldGap::start(cover)), (Edge::Left, 0));
    }

    #[test]
    fn the_held_gap_follows_a_publish_by_reserve_mode() {
        let gap = HeldGap::start(geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0)));
        assert_eq!(held(gap), (Edge::Left, 372));

        // A pushing publish on the same edge resets the zone to its own strip.
        let wider = geometry(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(gap.after_publish(wider)), (Edge::Left, 1092));

        // A pushing publish on the other edge moves both, at the strip that
        // edge resolves.
        let other = geometry(Edge::Right, 360, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(gap.after_publish(other)), (Edge::Right, 372));

        // A covering publish on the same edge touches nothing.
        let cover = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(gap.after_publish(cover)), (Edge::Left, 372));

        // A covering publish on the other edge moves only the edge.
        let cover = covering(Edge::Right, 1080, Gutters::new(0, 12, 0, 0));
        assert_eq!(held(gap.after_publish(cover)), (Edge::Right, 372));
    }

    #[test]
    fn a_size_is_whole_pixels_or_one_to_a_hundred_percent() {
        assert_eq!(Size::pixels(0), None);
        assert_eq!(Size::percent(0), None);
        assert_eq!(Size::percent(101), None);
        assert_eq!(Percent::new(25).map(Percent::get), Some(25));
        assert!(matches!(
            Size::pixels(320),
            Some(Size::Pixels(px)) if px.get() == 320
        ));
        assert!(matches!(
            Size::percent(25),
            Some(Size::Percent(percent)) if percent.get() == 25
        ));
    }

    #[test]
    fn the_gutters_follow_the_screen_directions() {
        let gutters = Gutters::new(8, 12, 24, 10);
        assert_eq!(gutters.near(Edge::Left), 8);
        assert_eq!(gutters.far(Edge::Left), 12);
        assert_eq!(gutters.near(Edge::Right), 12);
        assert_eq!(gutters.far(Edge::Right), 8);
        assert_eq!(gutters.near(Edge::Top), 24);
        assert_eq!(gutters.far(Edge::Top), 10);
        assert_eq!(gutters.near(Edge::Bottom), 10);
        assert_eq!(gutters.far(Edge::Bottom), 24);

        // The length runs down the output on a side edge and across it on a
        // top or bottom edge, so its ends are Top/Bottom and Left/Right.
        assert_eq!(gutters.along_start(Edge::Left), 24);
        assert_eq!(gutters.along_end(Edge::Left), 10);
        assert_eq!(gutters.along_start(Edge::Top), 8);
        assert_eq!(gutters.along_end(Edge::Top), 12);
    }
}
