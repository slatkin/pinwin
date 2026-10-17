//! The panel's geometry: the output edge it docks to, its resolved size
//! across that edge, the four directional gutters, the reserve mode and the
//! held gap (replace-terminal-with-app-host D9).
//!
//! This is one pure module where the terminal's `layout.rs` and
//! `surfaces/gap.rs` were two: it computes the layer surface's anchors,
//! margins and size, the strip a pushing panel reserves and how the held gap
//! moves, it resolves a percent size against the output and validates a
//! change before it applies, and it names no Wayland object (`port-to-rust`
//! D10), so every decision is unit tested with no compositor.
//!
//! The reservation rules, stated once: pushing moves the gap to the panel's
//! own strip, `near + across + far`; covering leaves the gap where the last
//! pushing setting put it — an empty strip when none ever applied — and
//! draws over the tiled windows; an edge change always moves the gap to the
//! new edge.

use std::fmt;
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
    // Row 6.6's apply path rebuilds the margins from a changed settings
    // merge; until then the struct literal in [`Geometry::margins`] is the
    // only construction.
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

    /// The size in logical pixels on `edge` of `output` (spec "Size
    /// values"): a pixel size as it is, a percent of the output's width on a
    /// side edge and of its height on a top or bottom edge.
    ///
    /// A percent rounds down and never resolves below one pixel, because
    /// every size is at least one pixel: 1% of a 60 pixel output is 1, not
    /// the zero that [`Size::pixels`] refuses.
    #[must_use]
    pub const fn resolve(self, edge: Edge, output: OutputSize) -> u32 {
        match self {
            Self::Pixels(px) => px.get(),
            Self::Percent(percent) => {
                let across = output.across(edge);
                let rate = percent.get();
                // `across * rate / 100`, floored, in 32 bits: the whole
                // hundreds never exceed `across`, and the remainder's
                // product stays below ten thousand.
                let hundreds = across / 100 * rate;
                let remainder = across % 100 * rate / 100;
                let resolved = hundreds + remainder;
                if resolved < 1 { 1 } else { resolved }
            }
        }
    }
}

/// A monitor's logical size in pixels, at least one along each axis.
///
/// The proxy reads it from `zxdg_output_v1` (or the `wl_output` mode and
/// scale when that protocol is missing) on the panel's upstream connection
/// and passes the two numbers here, so this module stays display-free
/// (`port-to-rust` D10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputSize {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl OutputSize {
    /// An output `width` by `height` logical pixels; zero along either axis
    /// is `None`.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            width: NonZeroU32::new(width)?,
            height: NonZeroU32::new(height)?,
        })
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        self.width.get()
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        self.height.get()
    }

    /// The output's dimension across a panel on `edge`: its width on a side
    /// edge and its height on a top or bottom edge.
    #[must_use]
    pub const fn across(self, edge: Edge) -> u32 {
        if edge.across_is_width() {
            self.width.get()
        } else {
            self.height.get()
        }
    }

    /// The output's dimension along a panel on `edge`: its height on a side
    /// edge and its width on a top or bottom edge.
    #[must_use]
    pub const fn length(self, edge: Edge) -> u32 {
        if edge.across_is_width() {
            self.height.get()
        } else {
            self.width.get()
        }
    }
}

/// A geometry the panel refuses (spec "Validate a change before applying
/// it").
///
/// The argument types make the remaining failures unrepresentable: a zero
/// size, an unknown edge and a percent outside 1..=100 cannot exist at run
/// time (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidLayout {
    /// Checked arithmetic overflowed while sizing the panel.
    Overflow,
    /// A pushing panel's strip `near + across + far` is below zero.
    NoReserve,
    /// A pushing strip leaves no output space for other windows, or a
    /// covering panel is wider than its output.
    NoWidth,
    /// The output's length at the panel's edge, less the two side gutters,
    /// is below one pixel.
    NoLength,
}

impl fmt::Display for InvalidLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("layout geometry overflows"),
            Self::NoReserve => f.write_str("layout reservation is negative"),
            Self::NoWidth => f.write_str("layout leaves no output space beside the panel"),
            Self::NoLength => f.write_str("layout leaves less than one pixel of length"),
        }
    }
}

impl std::error::Error for InvalidLayout {}

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
    // Read back by row 6.6's apply path, which diffs the live geometry
    // against the requested one before it re-applies.
    #[must_use]
    pub const fn across(self) -> u32 {
        self.across
    }

    // Read back by row 6.6's apply path, which diffs the live geometry
    // against the requested one before it re-applies.
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

    /// Whether this geometry can apply to `output` (spec "Validate a change
    /// before applying it").
    ///
    /// Both reserve modes refuse checked-arithmetic overflow in the strip and
    /// a visible length below one pixel. Pushing also refuses a negative
    /// strip and a strip that leaves no output space for other windows.
    /// Covering refuses only a panel wider than the output: the held strip
    /// is the last pushing setting's, not this panel's, so it cannot fail a
    /// covering change.
    pub fn validate(self, output: OutputSize) -> Result<(), InvalidLayout> {
        let strip = self.strip().ok_or(InvalidLayout::Overflow)?;
        if self.visible_length(output) < 1 {
            return Err(InvalidLayout::NoLength);
        }
        match self.reserve {
            ReserveMode::Push => {
                if strip < 0 {
                    return Err(InvalidLayout::NoReserve);
                }
                if i64::from(strip) >= i64::from(output.across(self.edge)) {
                    return Err(InvalidLayout::NoWidth);
                }
            }
            ReserveMode::Cover if self.across > output.across(self.edge) => {
                return Err(InvalidLayout::NoWidth);
            }
            ReserveMode::Cover => {}
        }
        Ok(())
    }

    /// The panel's visible length on `output`: the output's length at the
    /// panel's edge, less the two side gutters. Validation refuses a value
    /// below one pixel.
    fn visible_length(self, output: OutputSize) -> i64 {
        i64::from(output.length(self.edge))
            - i64::from(self.gutters.along_start(self.edge))
            - i64::from(self.gutters.along_end(self.edge))
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
    // Called per frame by the width tween's frame step (row 10.x, D8).
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
    // Called by the width tween's start rule (row 10.x, D8).
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
    // Read by the tween's per-frame apply, which re-anchors the reserve as
    // the gap moves (row 10.x, D8).
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

// The path is explicit because `tests/tween_crop.rs` includes this file with
// `#[path]`, and a module loaded that way searches its own children in the
// included file's directory rather than beside it (row 3.3).
#[cfg(test)]
#[path = "geometry/tests.rs"]
mod tests;
