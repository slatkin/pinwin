//! pinwin's layout core: the docking side, the applied column count, the
//! four directional gutters and the per-layout push/cover choice, plus
//! checked geometry validation and the pty read loop's yield decision.
//!
//! This is the Rust port of `options.c` (port-to-rust D3). It is GTK-free and
//! depends on nothing but `std`, so a layout can be validated with no display
//! (D10). The types encode the invariants the C core checked at runtime: the
//! side is an enum, the column count and the accent width are non-zero, and
//! the cell and output dimensions are non-zero (port-to-rust D6).

use std::fmt;
use std::num::{NonZeroI32, NonZeroU16};

/// The edge a panel docks against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Whether a layout pushes tiled windows aside or covers them in place
/// (overlay-expand D1).
///
/// The choice travels with the layout, so a host describes its small size as
/// pushing and its big size as covering. Pushing is the default, so callers
/// that never opt in keep today's behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// Move the compositor gap to this layout's own geometry: tiled windows
    /// reflow beside the panel.
    Push,
    /// Leave the gap exactly where the last pushing layout put it and draw
    /// the panel over the tiled windows. Side switches are never covering.
    Cover,
}

/// A full layout: where the panel docks, how many terminal columns it is
/// wide, its four directional gutters in logical pixels, and whether it
/// pushes tiles aside or covers them in place.
///
/// The gutters may be negative (they move the panel edge past its output
/// edge). Fields are private so the invariant lives in the field types and
/// only [`Layout::new`] builds a value.
///
/// [`Layout::new`] builds a pushing layout, the backward-compatible default:
/// the panel moves the compositor gap to its own geometry and tiles reflow.
/// Covering is opt-in per layout via [`Layout::covering`]: the panel draws
/// over the tiles and the gap stays at the last pushing layout's strip, so a
/// same-side expand or shrink moves no windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    side: Side,
    cols: NonZeroU16,
    top: i32,
    bottom: i32,
    left: i32,
    right: i32,
    coverage: Coverage,
}

impl Layout {
    /// Build a layout. `cols` is the panel width in terminal columns; the
    /// gutters are in logical pixels. The layout pushes tiles aside; opt in
    /// to covering with [`Layout::covering`].
    pub const fn new(
        side: Side,
        cols: NonZeroU16,
        top: i32,
        bottom: i32,
        left: i32,
        right: i32,
    ) -> Self {
        Self {
            side,
            cols,
            top,
            bottom,
            left,
            right,
            coverage: Coverage::Push,
        }
    }

    /// Opt this layout in to covering: the panel draws over the tiled
    /// windows and the compositor gap stays at the last pushing layout's
    /// strip. A side switch is never a covering move.
    pub const fn covering(mut self) -> Self {
        self.coverage = Coverage::Cover;
        self
    }

    pub const fn side(&self) -> Side {
        self.side
    }

    pub const fn cols(&self) -> NonZeroU16 {
        self.cols
    }

    pub const fn top(&self) -> i32 {
        self.top
    }

    pub const fn bottom(&self) -> i32 {
        self.bottom
    }

    pub const fn left(&self) -> i32 {
        self.left
    }

    pub const fn right(&self) -> i32 {
        self.right
    }

    /// The layout's push/cover choice.
    pub const fn coverage(&self) -> Coverage {
        self.coverage
    }

    /// Horizontal placement for the docking side, with checked arithmetic.
    ///
    /// `panel_width` is the applied column count times the cell width.
    /// `edge_margin` is the visible panel's margin on its docking edge (Left
    /// when docked left, Right when docked right); `reservation` is
    /// `left + panel_width + right`, the strip reserved at that edge.
    pub fn side_geometry(&self, panel_width: i64) -> Result<SideGeometry, InvalidLayout> {
        if !(0..=i32::MAX as i64).contains(&panel_width) {
            return Err(InvalidLayout::Overflow);
        }
        let sum = self.left as i64 + panel_width + self.right as i64;
        let reservation = i32::try_from(sum).map_err(|_| InvalidLayout::Overflow)?;
        let edge_margin = match self.side {
            Side::Left => self.left,
            Side::Right => self.right,
        };
        Ok(SideGeometry {
            edge_margin,
            reservation,
        })
    }

    /// Validate the layout against the output and font metrics, all in
    /// logical pixels. Both choices reject checked-arithmetic overflow and
    /// vertical space below one complete terminal row
    /// (`output_height - top - bottom < cell_height`); the rest splits by
    /// choice (overlay-expand D4):
    ///
    /// Pushing keeps today's checks: a reservation sum below zero
    /// (`left + panel_width + right`) and a reservation that leaves no output
    /// width for other windows.
    ///
    /// Covering never checks the untouched gap: it only rejects a visible
    /// panel wider than the output.
    pub fn validate(&self, cell: CellSize, output: OutputSize) -> Result<(), InvalidLayout> {
        let panel_width = self.cols.get() as i64 * cell.width().get() as i64;
        // Both choices place the visible panel by its own edge gutter, so
        // both reject the same checked-arithmetic overflow.
        let geometry = self.side_geometry(panel_width)?;
        match self.coverage {
            Coverage::Push => {
                if geometry.reservation() < 0 {
                    return Err(InvalidLayout::NoReserve);
                }
                if geometry.reservation() >= output.width().get() {
                    return Err(InvalidLayout::NoWidth);
                }
            }
            Coverage::Cover => {
                // The held gap is not this layout's business; only the
                // visible panel must fit the output.
                if panel_width > output.width().get() as i64 {
                    return Err(InvalidLayout::NoWidth);
                }
            }
        }
        let vertical = self.top as i64 + self.bottom as i64;
        if vertical > output.height().get() as i64 - cell.height().get() as i64 {
            return Err(InvalidLayout::NoRow);
        }
        Ok(())
    }
}

/// Horizontal placement produced by [`Layout::side_geometry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SideGeometry {
    edge_margin: i32,
    reservation: i32,
}

impl SideGeometry {
    pub const fn edge_margin(&self) -> i32 {
        self.edge_margin
    }

    pub const fn reservation(&self) -> i32 {
        self.reservation
    }
}

/// A terminal cell's pixel size; both dimensions are at least one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSize {
    width: NonZeroI32,
    height: NonZeroI32,
}

impl CellSize {
    pub const fn new(width: i32, height: i32) -> Option<Self> {
        match (NonZeroI32::new(width), NonZeroI32::new(height)) {
            (Some(width), Some(height)) => Some(Self { width, height }),
            _ => None,
        }
    }

    pub const fn width(&self) -> NonZeroI32 {
        self.width
    }

    pub const fn height(&self) -> NonZeroI32 {
        self.height
    }
}

/// A monitor's logical-pixel size; both dimensions are at least one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputSize {
    width: NonZeroI32,
    height: NonZeroI32,
}

impl OutputSize {
    pub const fn new(width: i32, height: i32) -> Option<Self> {
        match (NonZeroI32::new(width), NonZeroI32::new(height)) {
            (Some(width), Some(height)) => Some(Self { width, height }),
            _ => None,
        }
    }

    pub const fn width(&self) -> NonZeroI32 {
        self.width
    }

    pub const fn height(&self) -> NonZeroI32 {
        self.height
    }
}

/// A layout the panel refuses.
///
/// The C core's `PINWIN_GEOM_ERR_*` causes minus the ones the argument types
/// make unrepresentable: an unknown side, a zero column count, non-positive
/// metrics and a panel width beyond `i32` are no longer runtime errors
/// (port-to-rust D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidLayout {
    /// Checked arithmetic overflowed while placing the panel.
    Overflow,
    /// `left + panel_width + right` is below zero.
    NoReserve,
    /// Pushing: the reservation leaves no output width for other windows.
    /// Covering: the visible panel is wider than the output.
    NoWidth,
    /// Vertical space is below one complete terminal row.
    NoRow,
}

impl fmt::Display for InvalidLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("layout geometry overflows"),
            Self::NoReserve => f.write_str("layout reservation is negative"),
            Self::NoWidth => f.write_str("layout leaves no output width beside the panel"),
            Self::NoRow => f.write_str("layout leaves no complete terminal row"),
        }
    }
}

impl std::error::Error for InvalidLayout {}

/// Focus accent: a stroke around the whole window while the panel holds
/// keyboard focus (layer surfaces get no compositor focus ring, so the panel
/// marks focus itself).
///
/// The colour bytes are always valid and `width` is the stroke width in pixels
/// (1..=65535). An accent is passed as `Option<Accent>`, so "disabled" is the
/// absence of the value (port-to-rust D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accent {
    rgb: [u8; 3],
    width: NonZeroU16,
}

impl Accent {
    pub const fn new(rgb: [u8; 3], width: NonZeroU16) -> Self {
        Self { rgb, width }
    }

    pub const fn rgb(&self) -> [u8; 3] {
        self.rgb
    }

    pub const fn width(&self) -> NonZeroU16 {
        self.width
    }
}

/// The panel's keyboard interactivity mode, fixed at start time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyboard {
    None,
    OnDemand,
    Exclusive,
}

impl Keyboard {
    /// The mode named by `on-demand`, `exclusive` or `none`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "on-demand" => Some(Self::OnDemand),
            "exclusive" => Some(Self::Exclusive),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// The pty read loop's yield decision for one main-loop dispatch.
///
/// A `budget_us` of zero or less means unbounded (no tween running): never
/// yield. Otherwise yield once the dispatch has consumed `budget_us` of wall
/// time since `started_us`, so an image burst cannot starve the frame clock
/// mid-tween. Ports `pinwin_pty_yield` from `options.c`.
pub const fn pty_yield(started_us: i64, now_us: i64, budget_us: i64) -> bool {
    budget_us > 0 && now_us.saturating_sub(started_us) >= budget_us
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_parse_accepts_the_three_modes_and_nothing_else() {
        assert_eq!(Keyboard::parse("on-demand"), Some(Keyboard::OnDemand));
        assert_eq!(Keyboard::parse("exclusive"), Some(Keyboard::Exclusive));
        assert_eq!(Keyboard::parse("none"), Some(Keyboard::None));
        assert_eq!(Keyboard::parse(""), None);
        assert_eq!(Keyboard::parse("Exclusive"), None);
    }

    fn layout(cols: u16, top: i32, bottom: i32, left: i32, right: i32) -> Layout {
        Layout::new(
            Side::Left,
            NonZeroU16::new(cols).expect("test column count is non-zero"),
            top,
            bottom,
            left,
            right,
        )
    }

    fn covering(cols: u16, top: i32, bottom: i32, left: i32, right: i32) -> Layout {
        layout(cols, top, bottom, left, right).covering()
    }

    fn cell(width: i32, height: i32) -> CellSize {
        CellSize::new(width, height).expect("test cell size is non-zero")
    }

    fn output(width: i32, height: i32) -> OutputSize {
        OutputSize::new(width, height).expect("test output size is non-zero")
    }

    #[test]
    fn side_geometry_margins_follow_the_docking_side() {
        let left = Layout::new(Side::Left, NonZeroU16::new(60).unwrap(), 0, 0, 10, 20);
        let geometry = left.side_geometry(100).expect("geometry fits");
        assert_eq!(geometry.edge_margin(), 10);
        assert_eq!(geometry.reservation(), 130);

        let right = Layout::new(Side::Right, NonZeroU16::new(60).unwrap(), 0, 0, 10, 20);
        let geometry = right.side_geometry(100).expect("geometry fits");
        assert_eq!(geometry.edge_margin(), 20);
        assert_eq!(geometry.reservation(), 130);
    }

    #[test]
    fn side_geometry_rejects_bad_width_and_overflow() {
        let base = layout(60, 0, 0, 0, 0);
        assert_eq!(base.side_geometry(-1), Err(InvalidLayout::Overflow));
        assert_eq!(
            base.side_geometry(i32::MAX as i64 + 1),
            Err(InvalidLayout::Overflow)
        );

        let gutters = layout(60, 0, 0, i32::MAX, i32::MAX);
        assert_eq!(gutters.side_geometry(0), Err(InvalidLayout::Overflow));
    }

    #[test]
    fn validate_accepts_a_sane_layout() {
        assert_eq!(
            layout(60, 0, 0, 0, 12).validate(cell(8, 16), output(1920, 1080)),
            Ok(())
        );
    }

    #[test]
    fn layout_validation_causes() {
        struct Case {
            what: &'static str,
            layout: Layout,
            cell: CellSize,
            output: OutputSize,
            expected: InvalidLayout,
        }

        let cases = [
            Case {
                what: "column width overflows i32",
                layout: layout(65535, 0, 0, 0, 0),
                cell: cell(65536, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::Overflow,
            },
            Case {
                what: "gutter sum overflows i32",
                layout: layout(1, 0, 0, i32::MAX, i32::MAX),
                cell: cell(1, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::Overflow,
            },
            Case {
                what: "reservation is below zero",
                layout: layout(1, 0, 0, -100, -100),
                cell: cell(1, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::NoReserve,
            },
            Case {
                what: "reservation leaves no output width",
                layout: layout(600, 0, 0, 0, 0),
                cell: cell(1, 16),
                output: output(600, 1080),
                expected: InvalidLayout::NoWidth,
            },
            Case {
                what: "vertical space below one row",
                layout: layout(1, 1000, 1000, 0, 0),
                cell: cell(1, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::NoRow,
            },
        ];

        for case in cases {
            assert_eq!(
                case.layout.validate(case.cell, case.output),
                Err(case.expected),
                "{}",
                case.what
            );
        }
    }

    #[test]
    fn new_builds_a_pushing_layout_and_covering_opts_in() {
        let base = layout(60, 0, 0, 0, 12);
        assert_eq!(base.coverage(), Coverage::Push);

        let covered = base.covering();
        assert_eq!(covered.coverage(), Coverage::Cover);
        // Only the choice changes; the geometry fields stay as built.
        assert_eq!(covered.side(), base.side());
        assert_eq!(covered.cols(), base.cols());
        assert_eq!(covered.top(), base.top());
        assert_eq!(covered.bottom(), base.bottom());
        assert_eq!(covered.left(), base.left());
        assert_eq!(covered.right(), base.right());
    }

    #[test]
    fn covering_validation_rejects_a_panel_wider_than_the_output() {
        // The 600-column panel is exactly the output width: still visible,
        // so covering accepts it where pushing would reject the reservation.
        assert_eq!(
            covering(600, 0, 0, 0, 0).validate(cell(1, 16), output(600, 1080)),
            Ok(())
        );
        // One column wider than the output is rejected.
        assert_eq!(
            covering(601, 0, 0, 0, 0).validate(cell(1, 16), output(600, 1080)),
            Err(InvalidLayout::NoWidth)
        );
    }

    #[test]
    fn covering_validation_never_checks_the_gap() {
        // A negative reservation sum is only a pushing failure: a covering
        // layout leaves the held gap untouched, so it must validate.
        let layout = layout(1, 0, 0, -100, -100);
        assert_eq!(
            layout.validate(cell(1, 16), output(1920, 1080)),
            Err(InvalidLayout::NoReserve)
        );
        assert_eq!(
            layout.covering().validate(cell(1, 16), output(1920, 1080)),
            Ok(())
        );
        // Structural overflow is still rejected for covering.
        assert_eq!(
            covering(1, 0, 0, i32::MAX, i32::MAX).validate(cell(1, 16), output(1920, 1080)),
            Err(InvalidLayout::Overflow)
        );
    }

    #[test]
    fn covering_validation_rejects_insufficient_vertical_space() {
        assert_eq!(
            covering(1, 1000, 1000, 0, 0).validate(cell(1, 16), output(1920, 1080)),
            Err(InvalidLayout::NoRow)
        );
    }

    #[test]
    fn pty_yield_never_yields_without_a_budget() {
        assert!(!pty_yield(1000, 1_001_000, 0));
        assert!(!pty_yield(1000, 1_001_000, -1));
    }

    #[test]
    fn pty_yield_yields_at_and_past_the_budget() {
        assert!(!pty_yield(1000, 4999, 4000));
        assert!(pty_yield(1000, 5000, 4000));
        assert!(pty_yield(1000, 999_999, 4000));
    }
}
