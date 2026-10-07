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

use std::num::NonZeroU16;

use crate::layout::CellSize;

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
}
