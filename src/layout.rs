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
///
/// The reservation rules, stated once here:
///
/// * Pushing moves the compositor gap to the layout's own strip,
///   `left + panel width + right`, so tiled windows reflow beside the panel.
/// * Covering leaves the gap exactly where the last pushing layout put it —
///   an empty strip when no pushing layout has ever applied — and draws the
///   panel over the tiled windows, so a same-side expand or shrink moves no
///   windows.
/// * A side switch always moves the gap to the new side at the last pushing
///   width, covering or not: the shuffle belongs to whoever moved the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// Push: move the compositor gap to this layout's own strip.
    Push,
    /// Cover: draw the panel over the tiled windows and leave the gap alone.
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
/// [`Layout::new`] builds a pushing layout, the backward-compatible default.
/// Covering is opt-in per layout via [`Layout::covering`]; see [`Coverage`]
/// for what each choice does to the compositor gap and the reservation.
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
    #[must_use]
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
    #[must_use]
    pub const fn covering(mut self) -> Self {
        self.coverage = Coverage::Cover;
        self
    }

    #[must_use]
    pub const fn side(&self) -> Side {
        self.side
    }

    #[must_use]
    pub const fn cols(&self) -> NonZeroU16 {
        self.cols
    }

    #[must_use]
    pub const fn top(&self) -> i32 {
        self.top
    }

    #[must_use]
    pub const fn bottom(&self) -> i32 {
        self.bottom
    }

    #[must_use]
    pub const fn left(&self) -> i32 {
        self.left
    }

    #[must_use]
    pub const fn right(&self) -> i32 {
        self.right
    }

    /// The layout's push/cover choice.
    #[must_use]
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
        if !(0..=i64::from(i32::MAX)).contains(&panel_width) {
            return Err(InvalidLayout::Overflow);
        }
        let sum = i64::from(self.left) + panel_width + i64::from(self.right);
        let Ok(reservation) = i32::try_from(sum) else {
            return Err(InvalidLayout::Overflow);
        };
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
        let panel_width = i64::from(self.cols.get()) * i64::from(cell.width().get());
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
                if panel_width > i64::from(output.width().get()) {
                    return Err(InvalidLayout::NoWidth);
                }
            }
        }
        let vertical = i64::from(self.top) + i64::from(self.bottom);
        if vertical > i64::from(output.height().get()) - i64::from(cell.height().get()) {
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
    #[must_use]
    pub const fn edge_margin(&self) -> i32 {
        self.edge_margin
    }

    #[must_use]
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
    #[must_use]
    pub const fn new(width: i32, height: i32) -> Option<Self> {
        match (NonZeroI32::new(width), NonZeroI32::new(height)) {
            (Some(width), Some(height)) => Some(Self { width, height }),
            _ => None,
        }
    }

    #[must_use]
    pub const fn width(&self) -> NonZeroI32 {
        self.width
    }

    #[must_use]
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
    #[must_use]
    pub const fn new(width: i32, height: i32) -> Option<Self> {
        match (NonZeroI32::new(width), NonZeroI32::new(height)) {
            (Some(width), Some(height)) => Some(Self { width, height }),
            _ => None,
        }
    }

    #[must_use]
    pub const fn width(&self) -> NonZeroI32 {
        self.width
    }

    #[must_use]
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
    #[must_use]
    pub const fn new(rgb: [u8; 3], width: NonZeroU16) -> Self {
        Self { rgb, width }
    }

    #[must_use]
    pub const fn rgb(&self) -> [u8; 3] {
        self.rgb
    }

    #[must_use]
    pub const fn width(&self) -> NonZeroU16 {
        self.width
    }
}

/// The panel's keyboard interactivity mode, fixed at start time: no runtime
/// override exists. In `on-demand` mode the panel keeps and gives up the
/// keyboard by the click rules; a toggle shows the panel in `on-demand`
/// interactivity, so a compositor that focuses a newly mapped surface gives
/// it the keyboard (replace-gtk-with-wayland D4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyboard {
    None,
    OnDemand,
    Exclusive,
}

impl Keyboard {
    /// The mode named by `on-demand`, `exclusive` or `none`.
    #[must_use]
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
#[must_use]
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
            base.side_geometry(i64::from(i32::MAX) + 1),
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
            covering: bool,
            cell: CellSize,
            output: OutputSize,
            expected: InvalidLayout,
        }

        let cases = [
            Case {
                what: "column width overflows i32",
                layout: layout(65535, 0, 0, 0, 0),
                covering: false,
                cell: cell(65536, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::Overflow,
            },
            Case {
                what: "column width overflows i32, covering",
                layout: layout(65535, 0, 0, 0, 0),
                covering: true,
                cell: cell(65536, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::Overflow,
            },
            Case {
                what: "gutter sum overflows i32",
                layout: layout(1, 0, 0, i32::MAX, i32::MAX),
                covering: false,
                cell: cell(1, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::Overflow,
            },
            Case {
                what: "gutter sum overflows i32, covering",
                layout: layout(1, 0, 0, i32::MAX, i32::MAX),
                covering: true,
                cell: cell(1, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::Overflow,
            },
            Case {
                what: "reservation is below zero",
                layout: layout(1, 0, 0, -100, -100),
                covering: false,
                cell: cell(1, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::NoReserve,
            },
            Case {
                what: "reservation leaves no output width",
                layout: layout(600, 0, 0, 0, 0),
                covering: false,
                cell: cell(1, 16),
                output: output(600, 1080),
                expected: InvalidLayout::NoWidth,
            },
            Case {
                what: "vertical space below one row",
                layout: layout(1, 1000, 1000, 0, 0),
                covering: false,
                cell: cell(1, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::NoRow,
            },
            Case {
                what: "vertical space below one row, covering",
                layout: layout(1, 1000, 1000, 0, 0),
                covering: true,
                cell: cell(1, 16),
                output: output(1920, 1080),
                expected: InvalidLayout::NoRow,
            },
        ];

        for case in cases {
            let layout = if case.covering {
                case.layout.covering()
            } else {
                case.layout
            };
            assert_eq!(
                layout.validate(case.cell, case.output),
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
        assert_eq!(base.covering().coverage(), Coverage::Cover);
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
        // A negative reservation sum is only a pushing failure (the table's
        // NoReserve row): a covering layout leaves the held gap untouched, so
        // it must validate.
        assert_eq!(
            covering(1, 0, 0, -100, -100).validate(cell(1, 16), output(1920, 1080)),
            Ok(())
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
