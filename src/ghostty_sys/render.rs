//! Incremental render state: cells, styles, cursor and colors
//! (`ghostty/vt/render.h`).

use std::os::raw::{c_int, c_void};

use super::style::GhosttyColorRgb;
use super::terminal::GhosttyTerminal;
use super::{GhosttyAllocator, GhosttyResult};

/// Opaque handle to a render state instance (`GhosttyRenderState`,
/// render.h).
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyRenderState(pub *mut c_void);

/// Opaque handle to a render-state row iterator
/// (`GhosttyRenderStateRowIterator`, render.h).
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyRenderStateRowIterator(pub *mut c_void);

/// Opaque handle to render-state row cells (`GhosttyRenderStateRowCells`,
/// render.h).
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyRenderStateRowCells(pub *mut c_void);

/// A render-state query id (`GhosttyRenderStateData`, render.h).
pub type GhosttyRenderStateData = c_int;

pub const GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR: GhosttyRenderStateData = 4;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR: GhosttyRenderStateData = 18;
pub const GHOSTTY_RENDER_STATE_DATA_COLORS: GhosttyRenderStateData = 19;

/// A row query id (`GhosttyRenderStateRowData`, render.h).
pub type GhosttyRenderStateRowData = c_int;

pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS: GhosttyRenderStateRowData = 3;
/// The row's viewport Y position; one of the two data paths the rejected
/// `libghostty-vt` crate could not reach against the pin (D2).
pub const GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y: GhosttyRenderStateRowData = 6;

/// A row-cells query id (`GhosttyRenderStateRowCellsData`, render.h).
pub type GhosttyRenderStateRowCellsData = c_int;

pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW: GhosttyRenderStateRowCellsData = 1;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE: GhosttyRenderStateRowCellsData = 2;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN: GhosttyRenderStateRowCellsData = 3;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF: GhosttyRenderStateRowCellsData = 4;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR: GhosttyRenderStateRowCellsData = 5;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR: GhosttyRenderStateRowCellsData = 6;

/// The shape drawn for the cursor (`GhosttyRenderStateCursorVisualStyle`,
/// render.h).
pub type GhosttyRenderStateCursorVisualStyle = c_int;

pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR: GhosttyRenderStateCursorVisualStyle = 0;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK: GhosttyRenderStateCursorVisualStyle = 1;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE: GhosttyRenderStateCursorVisualStyle =
    2;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW:
    GhosttyRenderStateCursorVisualStyle = 3;

/// The cursor state (`GhosttyRenderStateCursor`, render.h). A sized struct:
/// set `size` to `size_of::<GhosttyRenderStateCursor>()` first.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyRenderStateCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: GhosttyRenderStateCursorVisualStyle,
}

/// The combined default colors (`GhosttyRenderStateColors`, render.h). A
/// sized struct: set `size` to `size_of::<GhosttyRenderStateColors>()` first.
/// This is the other data path the rejected crate could not reach against the
/// pin (D2): `ghostty_render_state_colors_get` was removed upstream.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyRenderStateColors {
    pub size: usize,
    pub background: GhosttyColorRgb,
    pub foreground: GhosttyColorRgb,
    pub cursor: GhosttyColorRgb,
    pub cursor_has_value: bool,
    pub palette: [GhosttyColorRgb; 256],
}

unsafe extern "C" {
    /// Create a render state with `allocator`, or the default allocator when
    /// it is NULL (render.h).
    pub fn ghostty_render_state_new(
        allocator: *const GhosttyAllocator,
        state: *mut GhosttyRenderState,
    ) -> GhosttyResult;

    /// Refresh `state` from `terminal` (render.h).
    pub fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;

    /// Clear the dirty state after a frame was consumed (render.h).
    pub fn ghostty_render_state_clean(state: GhosttyRenderState) -> GhosttyResult;

    /// Query render state; `out`'s type depends on `data` (render.h).
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        data: GhosttyRenderStateData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Create a row iterator with `allocator`, or the default allocator when
    /// it is NULL (render.h).
    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> GhosttyResult;

    /// Advance the row iterator; false at the end of the grid (render.h).
    pub fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;

    /// Query the current row; `out`'s type depends on `data` (render.h).
    pub fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: GhosttyRenderStateRowData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Create row cells with `allocator`, or the default allocator when it is
    /// NULL (render.h).
    pub fn ghostty_render_state_row_cells_new(
        allocator: *const GhosttyAllocator,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> GhosttyResult;

    /// Advance the cell iterator; false at the end of the row (render.h).
    pub fn ghostty_render_state_row_cells_next(cells: GhosttyRenderStateRowCells) -> bool;

    /// Query the current cell; `out`'s type depends on `data` (render.h).
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: GhosttyRenderStateRowCellsData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Free a render state created with `ghostty_render_state_new` (render.h).
    pub fn ghostty_render_state_free(state: GhosttyRenderState);

    /// Free a row iterator created with `ghostty_render_state_row_iterator_new`
    /// (render.h).
    pub fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);

    /// Free row cells created with `ghostty_render_state_row_cells_new`
    /// (render.h).
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);
}
