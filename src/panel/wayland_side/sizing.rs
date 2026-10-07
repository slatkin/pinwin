//! The terminal grid's size decision for the panel thread
//! (replace-gtk-with-wayland D3): the rows derive from the latest configure
//! height, the columns from the applied layout, and the grid and pty size
//! push only on a layout apply or a configure with a new height. Pure and
//! GTK-free, so the decisions are unit tested here without a compositor
//! (`port-to-rust` D10), like [`crate::layout`].
//!
//! The pty resize itself is a callback the caller supplies: the panel thread
//! applies the winsize to the host's master fd, and a test records the pushes
//! instead (`port-to-rust` D10 — no display, no real pty).
//!
//! The cell metrics are a startup input (replace-gtk-with-wayland D3): the
//! start command carries them — measured before the panel thread binds its
//! surfaces — so [`Sizing::new`] requires
//! them and a metrics-less state is unrepresentable. A sizing that silently
//! derived nothing would never size the pty, which is the bug this guards
//! against.
//!
//! The device-pixel sizes the panel reports to the host
//! (device-pixel-cell-reports D1) are pure conversions here too: the
//! reported device cell and the size-report mapping, both on the scale
//! helpers of [`super::buffers`], so the reports round exactly as the
//! drawn grid's device size does.

use std::num::NonZeroU16;

use crate::layout::CellSize;

use super::buffers::{FractionalScale, device_size};

/// The grid one push carries: the applied columns, the rows derived from the
/// latest configure height, and the cell metrics the winsize's pixel fields
/// and the terminal's push both need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    cols: u16,
    rows: u32,
    cell: CellSize,
}

impl Grid {
    fn new(cols: u16, rows: u32, cell: CellSize) -> Self {
        Grid { cols, rows, cell }
    }

    /// The applied column count.
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// The rows derived from the latest configure height.
    #[must_use]
    pub const fn rows(&self) -> u32 {
        self.rows
    }

    /// The cell's horizontal pitch in logical pixels.
    #[must_use]
    pub const fn cell_width(&self) -> i32 {
        self.cell.width().get()
    }

    /// The cell's vertical pitch in logical pixels.
    #[must_use]
    pub const fn cell_height(&self) -> i32 {
        self.cell.height().get()
    }
}

/// Whether the panel has seen a configure yet, and which height it named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// No configure has arrived: the next configure applies the startup
    /// layout.
    AwaitingHeight,
    /// The latest configure's height, in surface-local logical pixels.
    Running { height: u32 },
}

/// The grid size state of one panel (D3): the applied columns, the cell
/// metrics and the last pushed grid. The panel thread owns one; the decisions
/// here are pure so tests drive them without a compositor.
#[derive(Clone, Copy, Debug)]
pub struct Sizing {
    cols: NonZeroU16,
    cell: CellSize,
    stage: Stage,
    pushed: Option<Grid>,
    /// The columns of the grid the terminal runs: updated only when a push
    /// actually lands, so a deferred apply's recorded columns do not move
    /// it. The tween's wide draw reads it.
    live: NonZeroU16,
    /// The defer mode (row 6.2): while a width tween runs, the pushes wait
    /// for the tween's finish. Default off, so the row 3.3 and 3.5
    /// decisions are unchanged.
    deferred: bool,
}

impl Sizing {
    /// The state before the first configure: the startup layout's columns
    /// and the startup cell metrics the start command carries (row 4.6
    /// computes them before the thread binds its surfaces). The metrics are
    /// required: without them no configure could ever derive a grid, which
    /// would leave the panel silently unmapped and the pty unsized.
    #[must_use]
    pub fn new(cols: NonZeroU16, cell: CellSize) -> Self {
        Sizing {
            cols,
            cell,
            stage: Stage::AwaitingHeight,
            pushed: None,
            live: cols,
            deferred: false,
        }
    }

    /// The defer mode (row 6.2): while a width tween runs, the grid and pty
    /// push waits for the tween's finish — an animated apply and a mid-tween
    /// configure record their inputs (the columns and the height) without
    /// deriving or pushing, and the finish pushes once from the latest of
    /// both. The same gate the GTK path's `apply_size` applies with
    /// `!anim.active()`, moved onto the sizing the push runs through.
    pub fn defer_pushes(&mut self, deferred: bool) {
        self.deferred = deferred;
    }

    /// The columns of the grid the terminal runs: the last pushed grid's,
    /// or the startup layout's before the first push. The tween's wide draw
    /// reads this — the grid on screen during a tween is the last pushed
    /// one, which the deferred applies have not changed.
    #[must_use]
    pub fn live_cols(&self) -> NonZeroU16 {
        self.live
    }

    /// The latest configure height, in logical pixels (D3); `None` before
    /// the first configure. The tween's wide draw reads it for the crop's
    /// height.
    #[must_use]
    pub fn height(&self) -> Option<u32> {
        match self.stage {
            Stage::AwaitingHeight => None,
            Stage::Running { height } => Some(height),
        }
    }

    /// The state right after the startup apply pushed the startup grid at
    /// `height` — a running panel thread's steady state between configures.
    /// Replaying the real path with a sink for the push keeps this
    /// constructor honest: the recorded pushed grid is the one the real
    /// decision produces.
    #[must_use]
    pub fn started(cols: NonZeroU16, cell: CellSize, height: u32) -> Self {
        let mut sizing = Self::new(cols, cell);
        let mut sink = |_: Grid| {};
        sizing.configure(height, &mut sink);
        sizing
    }

    /// One layer-surface configure at `height` (D3). The first configure
    /// applies the startup layout — the pushed grid is still empty, so the
    /// derivation below pushes it — and every later configure pushes only
    /// when its height is new and the derived grid differs from the pushed
    /// one. A configure that repeats the previous height never pushes, however
    /// often the compositor sends it.
    pub fn configure(&mut self, height: u32, push: &mut dyn FnMut(Grid)) {
        if let Stage::Running { height: previous } = self.stage
            && previous == height
        {
            return;
        }
        let Some(rows) = rows_for_height(height, self.cell) else {
            return;
        };
        self.stage = Stage::Running { height };
        if self.deferred {
            // The tween's finish derives and pushes once from the latest
            // height (row 6.2); this configure only records the input.
            return;
        }
        let grid = Grid::new(self.cols.get(), rows, self.cell);
        if self.pushed != Some(grid) {
            push(grid);
            self.pushed = Some(grid);
            self.live = NonZeroU16::new(grid.cols).unwrap_or(self.live);
        }
    }

    /// A layout apply of a layout with `cols` columns (D3): the columns come
    /// from the layout, the rows stay derived from the latest configure
    /// height, and the push happens only when the derived grid differs from
    /// the pushed one. Before the first configure there is no height to
    /// derive from, so the apply only records the columns; the next configure
    /// derives with them.
    pub fn apply_columns(&mut self, cols: NonZeroU16, push: &mut dyn FnMut(Grid)) {
        self.cols = cols;
        if self.deferred {
            // The same defer as `configure`'s: the finish derives and
            // pushes once from the recorded columns and height (row 6.2).
            return;
        }
        let Stage::Running { height } = self.stage else {
            return;
        };
        let Some(rows) = rows_for_height(height, self.cell) else {
            return;
        };
        let grid = Grid::new(cols.get(), rows, self.cell);
        if self.pushed != Some(grid) {
            push(grid);
            self.pushed = Some(grid);
            self.live = NonZeroU16::new(grid.cols).unwrap_or(self.live);
        }
    }
}

/// The rows for a configure height: the height divided by the cell height,
/// at least one complete row — the same floor the GTK path's
/// `apply_size_to` applies. `None` when the cell height is not a positive
/// pixel count.
#[must_use]
pub fn rows_for_height(height: u32, cell: CellSize) -> Option<u32> {
    let cell_height = u32::try_from(cell.height().get()).ok()?;
    Some((height / cell_height).max(1))
}

/// The reported device cell size (device-pixel-cell-reports D1): each
/// dimension of the logical cell scaled with
/// [`FractionalScale::scale_dimension`] — the rounding the drawn grid's
/// device size uses, so the report and the draw never disagree by
/// rounding path. `None` when a dimension is not a non-negative pixel
/// count, scales to zero, or the scaled cell does not fit `i32`.
#[must_use]
pub fn device_cell(cell: CellSize, scale: FractionalScale) -> Option<CellSize> {
    let width = scaled_dimension(cell.width().get(), scale)?;
    let height = scaled_dimension(cell.height().get(), scale)?;
    CellSize::new(width, height)
}

/// One logical pixel count scaled to device pixels and back into the
/// cell dimensions' `i32` (device-pixel-cell-reports D1).
fn scaled_dimension(logical: i32, scale: FractionalScale) -> Option<i32> {
    let device = scale.scale_dimension(u32::try_from(logical).ok()?)?;
    i32::try_from(device).ok()
}

/// The device-pixel values the panel reports to the host
/// (device-pixel-cell-reports D1): the terminal size queries' cell pixels
/// and the pty winsize's pixel fields. The column and row counts pass
/// through unchanged; the cell pixels are the logical cell scaled per
/// dimension, and the grid pixels are the logical grid size scaled — the
/// drawn grid's device size, which the cell pixels times the counts need
/// not equal at a fractional scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceSizeReport {
    cols: u16,
    rows: u16,
    cell_width: u32,
    cell_height: u32,
    grid_width: u32,
    grid_height: u32,
}

impl DeviceSizeReport {
    /// The reported column count: the layout's logical one.
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// The reported row count: the logical one.
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// The cell's width in device pixels: the `CSI 16 t` value.
    #[must_use]
    pub const fn cell_width(&self) -> u32 {
        self.cell_width
    }

    /// The cell's height in device pixels: the `CSI 16 t` value.
    #[must_use]
    pub const fn cell_height(&self) -> u32 {
        self.cell_height
    }

    /// The grid's width in device pixels: the drawn panel's width, the
    /// `CSI 14 t` and winsize value.
    #[must_use]
    pub const fn grid_width(&self) -> u32 {
        self.grid_width
    }

    /// The grid's height in device pixels: the drawn panel's height, the
    /// `CSI 14 t` and winsize value.
    #[must_use]
    pub const fn grid_height(&self) -> u32 {
        self.grid_height
    }
}

/// The size-report mapping (device-pixel-cell-reports D1): the logical
/// counts and cell plus the resolved scale, mapped onto the device-pixel
/// values the panel reports. `None` when the logical grid does not fit
/// `u32` — a grid no buffer could hold — or a scaled dimension does not
/// fit.
#[must_use]
pub fn device_size_report(
    cols: u16,
    rows: u16,
    cell: CellSize,
    scale: FractionalScale,
) -> Option<DeviceSizeReport> {
    let logical_width = u32::try_from(cell.width().get()).ok()?;
    let logical_height = u32::try_from(cell.height().get()).ok()?;
    let device_cell = device_cell(cell, scale)?;
    let cell_width = u32::try_from(device_cell.width().get()).ok()?;
    let cell_height = u32::try_from(device_cell.height().get()).ok()?;
    // The logical grid size, not the device cell times the counts: the
    // drawn grid's device size is the scaled logical one (D1).
    let logical_grid_width = u32::try_from(u64::from(cols) * u64::from(logical_width)).ok()?;
    let logical_grid_height = u32::try_from(u64::from(rows) * u64::from(logical_height)).ok()?;
    let (grid_width, grid_height) = device_size((logical_grid_width, logical_grid_height), scale)?;
    Some(DeviceSizeReport {
        cols,
        rows,
        cell_width,
        cell_height,
        grid_width,
        grid_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cell(width: i32, height: i32) -> CellSize {
        CellSize::new(width, height).expect("test cell size is non-zero")
    }

    fn cols(count: u16) -> NonZeroU16 {
        NonZeroU16::new(count).expect("test column count is non-zero")
    }

    /// The push recorder the tests feed where the panel thread would apply
    /// the pty winsize (`port-to-rust` D10). Each configure call builds its
    /// own push closure over the recorder, the way the panel thread builds
    /// its winsize closure over the pty fd.
    #[derive(Default)]
    struct Recorder {
        grids: Vec<Grid>,
    }

    impl Recorder {
        fn push(&mut self, grid: Grid) {
            self.grids.push(grid);
        }
    }

    /// The row 3.3 scenario: three configures with one height, then one with
    /// a new height. The pty receives exactly one resize, for the new
    /// height — the repeated configures at the startup height push nothing.
    #[test]
    fn three_configures_with_one_height_then_a_new_height_resize_once() {
        let mut sizing = Sizing::started(cols(40), cell(9, 16), 1080);
        let mut pushed = Recorder::default();

        sizing.configure(1080, &mut |grid| pushed.push(grid));
        sizing.configure(1080, &mut |grid| pushed.push(grid));
        sizing.configure(1080, &mut |grid| pushed.push(grid));
        assert!(
            pushed.grids.is_empty(),
            "repeated configures at the startup height push nothing"
        );

        sizing.configure(1040, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1, "exactly one resize");
        let grid = pushed.grids[0];
        assert_eq!(grid.cols(), 40, "the columns come from the layout");
        assert_eq!(grid.rows(), 1040 / 16, "the rows come from the new height");
    }

    /// The first configure applies the startup layout: the pushed grid is
    /// still empty, so the derivation pushes once; further configures at the
    /// same height push nothing.
    #[test]
    fn the_first_configure_applies_the_startup_grid() {
        let mut sizing = Sizing::new(cols(40), cell(9, 16));
        let mut pushed = Recorder::default();

        sizing.configure(1080, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1, "the startup apply pushes");
        assert_eq!(pushed.grids[0].rows(), 1080 / 16);

        sizing.configure(1080, &mut |grid| pushed.push(grid));
        sizing.configure(1080, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1, "repeated configures push nothing");
    }

    /// A configure whose new height derives the same row count pushes
    /// nothing: the grid and the pty change only when the grid changes (D3's
    /// one source for the pty size).
    #[test]
    fn a_height_change_within_one_row_pushes_nothing() {
        let mut sizing = Sizing::started(cols(40), cell(9, 16), 1080);
        let mut pushed = Recorder::default();

        // 1080 and 1087 both hold 67 rows at a 16 px cell.
        sizing.configure(1087, &mut |grid| pushed.push(grid));
        assert!(pushed.grids.is_empty(), "the grid is unchanged");
    }

    /// A layout apply pushes when the derived grid changes, and a repeated
    /// apply of the same columns does not.
    #[test]
    fn a_layout_apply_pushes_when_the_grid_changes() {
        let mut sizing = Sizing::started(cols(40), cell(9, 16), 1080);
        let mut pushed = Recorder::default();

        sizing.apply_columns(cols(60), &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1, "the widened layout pushes");
        assert_eq!(pushed.grids[0].cols(), 60);
        assert_eq!(pushed.grids[0].rows(), 1080 / 16, "the rows stay");

        sizing.apply_columns(cols(60), &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1, "the same columns push nothing");
    }

    /// A layout apply before the first configure only records the columns:
    /// there is no height to derive from, and the next configure derives
    /// with the applied ones.
    #[test]
    fn an_apply_before_the_first_configure_records_the_columns() {
        let mut sizing = Sizing::new(cols(40), cell(9, 16));
        let mut pushed = Recorder::default();

        sizing.apply_columns(cols(60), &mut |grid| pushed.push(grid));
        assert!(pushed.grids.is_empty(), "no height, no push");

        sizing.configure(1080, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1, "the next configure derives");
        assert_eq!(pushed.grids[0].cols(), 60, "with the applied columns");
    }

    /// The rows floor at one complete row, like the GTK path's
    /// `apply_size_to`; a non-positive cell height derives nothing.
    #[test]
    fn rows_floor_at_one_and_reject_a_degenerate_cell() {
        assert_eq!(rows_for_height(5, cell(9, 16)), Some(1));
        assert_eq!(rows_for_height(16, cell(9, 16)), Some(1));
        assert_eq!(rows_for_height(17, cell(9, 16)), Some(1));
        assert_eq!(rows_for_height(0, cell(9, 16)), Some(1));
        assert_eq!(
            rows_for_height(1080, cell(9, -16)),
            None,
            "a negative cell height derives nothing"
        );
    }

    /// `started` records the pushed grid the real decision produces, so the
    /// steady-state tests start from the thread's actual post-startup state.
    #[test]
    fn started_matches_the_startup_apply() {
        let started = Sizing::started(cols(40), cell(9, 16), 1080);
        let mut from_scratch = Sizing::new(cols(40), cell(9, 16));
        let mut pushed = Recorder::default();
        from_scratch.configure(1080, &mut |grid| pushed.push(grid));

        assert_eq!(started.pushed, from_scratch.pushed);
        assert_eq!(pushed.grids.len(), 1);
    }

    /// The defer mode (row 6.2): while it is on, an animated apply and a
    /// mid-tween configure record their inputs without deriving or pushing,
    /// and the finish's plain `apply_columns` pushes once from the latest
    /// of both — the new columns, the rows of the latest configure height.
    #[test]
    fn the_defer_mode_records_inputs_and_the_finish_pushes_once() {
        let mut sizing = Sizing::started(cols(40), cell(9, 16), 1080);
        let mut pushed = Recorder::default();

        sizing.defer_pushes(true);
        sizing.apply_columns(cols(120), &mut |grid| pushed.push(grid));
        assert!(pushed.grids.is_empty(), "a deferred apply pushes nothing");
        sizing.configure(1040, &mut |grid| pushed.push(grid));
        assert!(
            pushed.grids.is_empty(),
            "a deferred configure pushes nothing"
        );

        // The live columns stay the last pushed ones while the pushes wait.
        assert_eq!(sizing.live_cols(), cols(40));
        assert_eq!(sizing.height(), Some(1040), "the height is recorded");

        // The finish lifts the defer and pushes once.
        sizing.defer_pushes(false);
        sizing.apply_columns(cols(120), &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1, "the finish pushes once");
        assert_eq!(pushed.grids[0].cols(), 120, "the tweened columns");
        assert_eq!(
            pushed.grids[0].rows(),
            1040 / 16,
            "the latest height's rows"
        );
        assert_eq!(sizing.live_cols(), cols(120));

        // A configure repeating the pushed grid still pushes nothing.
        sizing.configure(1040, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1);
    }

    /// The live columns before the first push are the recorded ones, and a
    /// deferred apply that cannot derive (no height yet) still records.
    #[test]
    fn live_cols_follow_the_pushed_grid_and_the_record_before_it() {
        let mut sizing = Sizing::new(cols(40), cell(9, 16));
        assert_eq!(sizing.live_cols(), cols(40));
        assert_eq!(sizing.height(), None, "no configure, no height");

        sizing.defer_pushes(true);
        sizing.apply_columns(cols(60), &mut |_grid| panic!("no push"));
        assert_eq!(
            sizing.live_cols(),
            cols(40),
            "the push is still the old one"
        );

        sizing.configure(1080, &mut |_grid| panic!("no push"));
        sizing.defer_pushes(false);
        let mut pushed = Recorder::default();
        sizing.apply_columns(cols(60), &mut |grid| pushed.push(grid));
        assert_eq!(pushed.grids.len(), 1, "the finish derives and pushes");
        assert_eq!(pushed.grids[0].cols(), 60);
    }

    /// The row 1.1 scenario at the user's laptop scale: a 9 logical pixel
    /// wide cell at 1.8 reports 16 device pixels (9 × 1.8 = 16.2, rounded),
    /// and the drawn grid's device size is the scaled logical one — 40
    /// columns of 9 logical pixels are 360 logical, 648 device.
    #[test]
    fn the_1_8_scale_reports_a_9_pixel_cell_as_16_device_pixels() {
        let scale = FractionalScale::from_120ths(216);
        let device = device_cell(cell(9, 16), scale).expect("the cell scales");
        assert_eq!(device.width().get(), 16, "9 × 1.8 = 16.2, rounded down");
        assert_eq!(device.height().get(), 29, "16 × 1.8 = 28.8, rounded up");

        let grid = device_size((40 * 9, 24 * 16), scale).expect("the grid scales");
        assert_eq!(grid, (648, 691), "the scaled logical grid size");
    }

    /// Scale 1 is the identity: the reported cell and the drawn grid's
    /// device size equal their logical sizes.
    #[test]
    fn scale_1_leaves_the_cell_and_grid_sizes_unchanged() {
        let scale = FractionalScale::from_120ths(120);
        let device = device_cell(cell(9, 16), scale).expect("the cell scales");
        assert_eq!((device.width().get(), device.height().get()), (9, 16));
        assert_eq!(
            device_size((40 * 9, 24 * 16), scale),
            Some((40 * 9, 24 * 16)),
        );
    }

    /// Non-exact products round exactly as `scale_dimension` does, per
    /// dimension — including a half-way product, which rounds up.
    #[test]
    fn non_exact_products_round_as_scale_dimension_does() {
        let scale = FractionalScale::from_120ths(180);
        for logical in [7_u32, 11, 9, 16, 3] {
            let expected = scale
                .scale_dimension(logical)
                .expect("the test dimension scales");
            let logical = i32::try_from(logical).expect("the test dimension fits i32");
            let device = device_cell(cell(logical, logical), scale).expect("the test cell scales");
            assert_eq!(
                device.width().get(),
                i32::try_from(expected).expect("the scaled test dimension fits i32")
            );
            assert_eq!(
                device.height().get(),
                i32::try_from(expected).expect("the scaled test dimension fits i32")
            );
        }
    }

    /// The size-report mapping (row 1.2): the column and row counts pass
    /// through unchanged, while the cell pixels and the grid pixels come
    /// from the device conversion, at scale 1 and at 1.8.
    #[test]
    fn the_size_report_passes_the_counts_through_and_scales_the_pixels() {
        let exact = device_size_report(40, 24, cell(9, 16), FractionalScale::from_120ths(120))
            .expect("the report maps");
        assert_eq!((exact.cols(), exact.rows()), (40, 24));
        assert_eq!(exact.cell_width(), 9);
        assert_eq!(exact.cell_height(), 16);
        assert_eq!(exact.grid_width(), 40 * 9);
        assert_eq!(exact.grid_height(), 24 * 16);

        let fractional = device_size_report(40, 24, cell(9, 16), FractionalScale::from_120ths(216))
            .expect("the report maps");
        assert_eq!(
            (fractional.cols(), fractional.rows()),
            (40, 24),
            "the counts stay the logical ones"
        );
        assert_eq!(fractional.cell_width(), 16, "the CSI 16 t cell width");
        assert_eq!(fractional.cell_height(), 29, "the CSI 16 t cell height");
        assert_eq!(fractional.grid_width(), 648, "the CSI 14 t grid width");
        assert_eq!(fractional.grid_height(), 691, "the CSI 14 t grid height");
    }

    /// The grid pixels are the scaled logical grid size, not the device
    /// cell times the counts: at 1.8 the two differ (D1) — 40 × 16 = 640
    /// against the drawn grid's 648.
    #[test]
    fn the_grid_pixels_are_the_drawn_size_not_the_cell_pixels_times_the_counts() {
        let report = device_size_report(40, 24, cell(9, 16), FractionalScale::from_120ths(216))
            .expect("the report maps");
        assert_eq!(report.grid_width(), 648);
        assert_ne!(
            report.grid_width(),
            u32::from(report.cols()) * report.cell_width(),
            "the cell pixels times the columns is not the drawn width"
        );
    }
}
