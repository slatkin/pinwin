//! The frame protocol half of pinwin's terminal core (port-to-rust D3):
//! starting and ending a frame, walking the terminal's cells (with grapheme
//! merging and constraint widths), the cursor and default colours, and the
//! kitty image placements (design D4). Ported from `src/cells.zig` and the
//! frame half of `src/pinwin.h`.
//!
//! The shared terminal handles live on [`Terminal`]; the panel thread's
//! renderer drives these methods and paints the plain Rust types below. No
//! toolkit or
//! cairo type appears here.

use std::mem;
use std::ptr;

use super::{Handles, Terminal};
use crate::ghostty_sys::GHOSTTY_SUCCESS;
use crate::ghostty_sys::kitty::GhosttyKittyGraphics;
use crate::ghostty_sys::render::{
    GHOSTTY_RENDER_STATE_DATA_COLORS, GHOSTTY_RENDER_STATE_DATA_CURSOR,
    GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
    GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y, GhosttyRenderStateColors, GhosttyRenderStateCursor,
    GhosttyRenderStateRowCells, ghostty_render_state_clean, ghostty_render_state_get,
    ghostty_render_state_row_cells_get, ghostty_render_state_row_cells_next,
    ghostty_render_state_row_get, ghostty_render_state_row_iterator_next,
    ghostty_render_state_update,
};
use crate::ghostty_sys::screen::{
    GHOSTTY_CELL_DATA_WIDE, GHOSTTY_CELL_WIDE_NARROW, GhosttyCellWide, ghostty_cell_get,
};
use crate::ghostty_sys::style::{GHOSTTY_STYLE_COLOR_RGB, GhosttyColorRgb, GhosttyStyle};

mod dirty;
pub use dirty::FrameDirty;
mod graphemes;
pub(crate) use graphemes::first_codepoint;
mod images;
#[cfg(test)]
pub(crate) use images::mbv_replay_bytes;
mod types;

/// The kitty placeholder codepoint: a cell carrying it means "draw the image
/// here", with the image id in the cell's foreground colour.
const PLACEHOLDER: u32 = 0x0010_EEEE;

pub use types::{CELL_TEXT_CAP, Cell, Colors, Cursor, CursorStyle, Image, Rgb, StyleFlags, Wide};

/// The frame's lifecycle state, grouped so [`FrameState`] stays readable
/// (`struct_excessive_bools`: adjacent flags invite swapped writes). The
/// row-walk flag stays on [`FrameState`] itself: it belongs to the row
/// iterator, not the frame lifecycle.
#[derive(Default)]
pub(crate) struct FrameFlags {
    open: bool,
    has_pending: bool,
    images_started: bool,
}
#[derive(Default)]
pub(crate) struct FrameState {
    flags: FrameFlags,
    last_emitted_cp: u32,
    pending_cell: Cell,
    in_row: bool,
    cell_x: i32,
    cell_y: i32,
    cursor: Cursor,
    foreground: Rgb,
    background: Rgb,
    graphics: Option<GhosttyKittyGraphics>,
    placeholders: images::PlaceholderMap,
    /// The global dirty state the render state reported at `frame_begin`.
    /// `update` only updates the dirty state, it never unsets
    /// it; `frame_end`'s clean unsets both layers after a consumed frame.
    dirty: FrameDirty,
    /// The viewport rows whose dirty flag the render state reported at
    /// `frame_begin`, in walk order. The flag is conservative: a
    /// row may be listed without its content changing.
    dirty_rows: Vec<i32>,
    /// Whether the render state carries any kitty placement at all: an
    /// image placement changing does not necessarily mark any row
    /// dirty, so the frame gate escalates every frame with placements to a
    /// full redraw.
    has_images: bool,
    /// When set, the frame walk visits only these viewport rows — the
    /// painter's partial repaint draws the dirty rows and their
    /// glyph-spill neighbours, and every cell pass shares the walk. Cleared
    /// by `frame_begin`, kept by `frame_rewind` (every pass of one frame
    /// walks the same rows).
    row_filter: Option<Vec<i32>>,
}

/// The frame API on [`Terminal`]. Every draw call draws a complete frame by
/// default: a frame that skipped the cells it thinks are unchanged would
/// leave those areas blank, because the caller paints the panel's background
/// first. The Wayland painter keeps its canvas across frames instead, so it
/// can consult the dirty data — [`Terminal::frame_dirty`],
/// [`Terminal::frame_dirty_rows`] — and repaint only the changed rows.
impl Terminal {
    /// Refresh the render state and start a frame. Returns false when the
    /// terminal does not exist or its render state cannot be refreshed, in
    /// which case no cells should be drawn.
    pub fn frame_begin(&mut self) -> bool {
        let Some(handles) = self.handles.as_mut() else {
            return false;
        };
        begin(&mut self.frame, handles)
    }

    /// Revisit the captured cells after painting all backgrounds, so glyphs may
    /// extend into neighboring cells without being covered by their
    /// backgrounds.
    pub fn frame_rewind(&mut self) {
        let Some(handles) = self.handles.as_mut() else {
            return;
        };
        rewind(&mut self.frame, handles);
    }

    /// The next raw cell of the frame, after grapheme joining. `None` at the
    /// end of the grid.
    pub fn cell_next(&mut self) -> Option<Cell> {
        let handles = self.handles.as_mut()?;
        cell_next(&mut self.frame, handles)
    }

    /// The cursor, if the frame has one that is visible with a viewport
    /// position.
    #[must_use]
    pub fn cursor(&self) -> Option<Cursor> {
        (self.frame.flags.open && self.frame.cursor.has_value).then_some(self.frame.cursor)
    }

    /// The terminal's default background and foreground colours.
    #[must_use]
    pub fn colors(&self) -> Colors {
        Colors {
            background: self.frame.background,
            foreground: self.frame.foreground,
        }
    }

    /// The next kitty graphics placement of the frame, or `None` at the end.
    pub fn image_next(&mut self) -> Option<Image> {
        let cell_w = self.cell_w();
        let cell_h = self.cell_h();
        // The shared scale note, the same one `size_report` answers from
        // (device-pixel-image-size D1): the drawn size and the reported size
        // can never use different scales.
        let scale_120 = self.ctx.scale_120.get();
        let handles = self.handles.as_mut()?;
        images::image_next(&mut self.frame, handles, cell_w, cell_h, scale_120)
    }

    /// Finish the frame and clear the render state's dirty flags — both
    /// layers: `ghostty_render_state_clean` sets the global state to
    /// `DIRTY_FALSE` and clears every per-row flag, so a consumed frame
    /// needs no per-row setter.
    pub fn frame_end(&mut self) {
        let Some(handles) = self.handles.as_ref() else {
            return;
        };
        end(&mut self.frame, handles);
    }
}

/// Start a frame (`pinwin_frame_begin`): refresh the render state, capture the
/// default colours and cursor, and rewind the row iterator.
fn begin(frame: &mut FrameState, handles: &mut Handles) -> bool {
    let terminal = handles.terminal.expect("frame_begin with a live terminal");
    let render_state = handles
        .render_state
        .expect("frame_begin with a live render state");
    // SAFETY: both handles are live and belong together.
    if unsafe { ghostty_render_state_update(render_state, terminal) } != GHOSTTY_SUCCESS {
        return false;
    }

    // SAFETY: the render state is live and `colors` is a sized struct of the
    // type the selector expects.
    let mut colors = unsafe { mem::zeroed::<GhosttyRenderStateColors>() };
    colors.size = mem::size_of::<GhosttyRenderStateColors>();
    // SAFETY: `colors` is writable storage of the expected type.
    if unsafe {
        ghostty_render_state_get(
            render_state,
            GHOSTTY_RENDER_STATE_DATA_COLORS,
            ptr::from_mut(&mut colors).cast(),
        )
    } == GHOSTTY_SUCCESS
    {
        frame.background = colors.background.into();
        frame.foreground = colors.foreground.into();
    }

    let mut row_iterator = handles
        .row_iterator
        .expect("frame_begin with a row iterator");
    // SAFETY: the row iterator handle is writable storage of the type the
    // selector returns; the render state is live.
    let _ = unsafe {
        ghostty_render_state_get(
            render_state,
            GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            ptr::from_mut(&mut row_iterator).cast(),
        )
    };
    handles.row_iterator = Some(row_iterator);

    frame.cursor = Cursor::default();
    // SAFETY: `cursor` is a sized struct of the type the selector expects.
    let mut cursor = unsafe { mem::zeroed::<GhosttyRenderStateCursor>() };
    cursor.size = mem::size_of::<GhosttyRenderStateCursor>();
    // SAFETY: `cursor` is writable storage of the expected type.
    let have_cursor = unsafe {
        ghostty_render_state_get(
            render_state,
            GHOSTTY_RENDER_STATE_DATA_CURSOR,
            ptr::from_mut(&mut cursor).cast(),
        )
    } == GHOSTTY_SUCCESS;
    if have_cursor && cursor.visible && cursor.viewport_has_value {
        frame.cursor.has_value = true;
        frame.cursor.x = i32::from(cursor.viewport_x);
        frame.cursor.y = i32::from(cursor.viewport_y);
        frame.cursor.style = CursorStyle::from_raw(cursor.visual_style);
        frame.cursor.wide_tail = cursor.wide_tail;
    }

    // The dirty data the update just computed, and the kitty
    // placement presence. Both walks below leave their iterator wherever
    // they stop, so each one re-fetches it afterwards; the cell passes and
    // the image pass must start from their first entry.
    frame.row_filter = None;
    frame.dirty = FrameDirty::Clean;
    frame.dirty_rows.clear();
    frame.has_images = false;
    dirty::capture_dirty(frame, render_state, &mut row_iterator);
    handles.row_iterator = Some(row_iterator);
    dirty::capture_images(frame, handles);

    frame.flags.open = true;
    frame.flags.has_pending = false;
    frame.last_emitted_cp = 0;
    frame.in_row = false;
    frame.flags.images_started = false;
    frame.placeholders.clear();
    frame.cell_y = -1;
    true
}

/// Rewind the row iterator for the glyph pass (`pinwin_frame_rewind`).
fn rewind(frame: &mut FrameState, handles: &mut Handles) {
    if !frame.flags.open {
        return;
    }
    let render_state = handles
        .render_state
        .expect("frame_rewind with a live render state");
    let mut row_iterator = handles
        .row_iterator
        .expect("frame_rewind with a row iterator");
    // SAFETY: the row iterator handle is writable storage of the type the
    // selector returns; the render state is live.
    let _ = unsafe {
        ghostty_render_state_get(
            render_state,
            GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            ptr::from_mut(&mut row_iterator).cast(),
        )
    };
    handles.row_iterator = Some(row_iterator);
    frame.flags.has_pending = false;
    frame.last_emitted_cp = 0;
    frame.in_row = false;
    frame.placeholders.clear();
    frame.cell_y = -1;
}

/// End a frame and clean the render state (`pinwin_frame_end`).
fn end(frame: &mut FrameState, handles: &Handles) {
    if !frame.flags.open {
        return;
    }
    frame.flags.open = false;
    let render_state = handles
        .render_state
        .expect("frame_end with a live render state");
    // SAFETY: the render state is live.
    let _ = unsafe { ghostty_render_state_clean(render_state) };
}

/// Advance to the next raw cell, before grapheme joining (`nextRawCell`).
/// `None` at the end of the grid.
fn next_raw_cell(frame: &mut FrameState, handles: &mut Handles) -> Option<Cell> {
    loop {
        if frame.in_row {
            let cells = handles.row_cells.expect("next_raw_cell with row cells");
            // SAFETY: the row cells handle was filled by the row iterator for
            // the current row.
            if unsafe { ghostty_render_state_row_cells_next(cells) } {
                let cell = fill_cell(frame, cells);
                frame.cell_x += 1;
                return Some(cell);
            }
            frame.in_row = false;
        }

        let row_iterator = handles
            .row_iterator
            .expect("next_raw_cell with a row iterator");
        // SAFETY: the row iterator was filled from the live render state.
        if !unsafe { ghostty_render_state_row_iterator_next(row_iterator) } {
            return None;
        }

        // The whole grid is walked every frame (design D5): the draw callback
        // repaints the panel from scratch, so a row that is not visited would
        // be left blank.
        let mut viewport_y: i32 = 0;
        // SAFETY: `viewport_y` is writable storage of the expected type and
        // the row iterator is live.
        if unsafe {
            ghostty_render_state_row_get(
                row_iterator,
                GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y,
                ptr::from_mut(&mut viewport_y).cast(),
            )
        } != GHOSTTY_SUCCESS
        {
            continue;
        }
        // The partial repaint's row filter: skip the rows the
        // painter is not repainting this frame. The filter persists across
        // `frame_rewind`, so every pass of the frame walks the same rows.
        if !dirty::row_visible(frame, viewport_y) {
            continue;
        }
        frame.cell_y = viewport_y;

        let mut row_cells = handles.row_cells.expect("next_raw_cell with row cells");
        // SAFETY: the row cells handle is writable storage of the type the
        // selector returns and the row iterator is live.
        if unsafe {
            ghostty_render_state_row_get(
                row_iterator,
                GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                ptr::from_mut(&mut row_cells).cast(),
            )
        } != GHOSTTY_SUCCESS
        {
            continue;
        }
        handles.row_cells = Some(row_cells);
        frame.in_row = true;
        frame.cell_x = 0;
    }
}

/// The cells, with a one-cell lookahead so that a grapheme cluster split over
/// several cells is drawn as a single glyph at the first of them
/// (`pinwin_cell_next`).
fn cell_next(frame: &mut FrameState, handles: &mut Handles) -> Option<Cell> {
    if !frame.flags.open {
        return None;
    }
    loop {
        let Some(mut next) = next_raw_cell(frame, handles) else {
            if frame.flags.has_pending {
                frame.flags.has_pending = false;
                let mut out = Cell::default();
                emit_pending(frame, &mut out, &Cell::default());
                return Some(out);
            }
            return None;
        };

        if !frame.flags.has_pending {
            frame.pending_cell = next;
            frame.flags.has_pending = true;
            continue;
        }
        if next.len == 0 {
            // A blank or wide-glyph spacer cell cannot join a cluster, but it
            // must not flush the pending cell either: a wide emoji is followed
            // by one of these before the modifier that belongs to it. Cells
            // are painted by position, so emitting it first is harmless.
            return Some(next);
        }
        if graphemes::merge_grapheme(&mut frame.pending_cell, &mut next) {
            continue;
        }
        let mut out = Cell::default();
        emit_pending(frame, &mut out, &next);
        frame.pending_cell = next;
        return Some(out);
    }
}

/// Give the pending cell its constraint width (it needs the cell to its right)
/// and hand it to the caller (`emitPending`).
fn emit_pending(frame: &mut FrameState, out: &mut Cell, next: &Cell) {
    let same_row = next.len > 0 && next.y == frame.pending_cell.y;
    let next_cp = if same_row {
        graphemes::first_codepoint(&next.text[..next.len])
    } else {
        0
    };
    let prev_cp = if frame.pending_cell.x == 0 {
        0
    } else {
        frame.last_emitted_cp
    };

    frame.pending_cell.cw =
        graphemes::constraint_width(&frame.pending_cell, prev_cp, next_cp, !same_row).cast_signed();
    *out = frame.pending_cell;
    frame.last_emitted_cp =
        graphemes::first_codepoint(&frame.pending_cell.text[..frame.pending_cell.len]);
}

/// Read the current cell's graphemes, width, style and colours (`fillCell`).
fn fill_cell(frame: &mut FrameState, cells: GhosttyRenderStateRowCells) -> Cell {
    let mut cell = Cell {
        x: frame.cell_x,
        y: frame.cell_y,
        ..Cell::default()
    };
    let is_placeholder = fill_text(cells, &mut cell);
    cell.wide = fill_width(cells);
    let style = fill_style(cells);
    cell.flags = style_flags(&style);
    fill_colors(frame, cells, &style, is_placeholder, &mut cell);
    cell
}

/// Fill `cell`'s text from the grapheme buffer (`fillCell` text half). True
/// when the cell is a kitty placeholder, which carries no glyph.
#[must_use]
fn fill_text(cells: GhosttyRenderStateRowCells, cell: &mut Cell) -> bool {
    let mut is_placeholder = false;
    let mut grapheme_len: u32 = 0;
    // SAFETY: `grapheme_len` is writable storage of the expected type and the
    // row cells handle is live.
    unsafe {
        ghostty_render_state_row_cells_get(
            cells,
            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
            ptr::from_mut(&mut grapheme_len).cast(),
        );
    };
    if grapheme_len > 0 {
        let mut codepoints = [0u32; 8];
        let count = (grapheme_len as usize).min(codepoints.len());
        // SAFETY: `codepoints` is writable storage for up to 8 u32, which the
        // selector documents as the grapheme buffer.
        unsafe {
            ghostty_render_state_row_cells_get(
                cells,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                codepoints.as_mut_ptr().cast(),
            );
        };
        is_placeholder = codepoints[0] == PLACEHOLDER;
        if !is_placeholder {
            let mut len = 0usize;
            for &codepoint in &codepoints[..count] {
                let Some(ch) = char::from_u32(codepoint) else {
                    continue;
                };
                let mut buf = [0u8; 4];
                let written = ch.encode_utf8(&mut buf).len();
                if len + written > CELL_TEXT_CAP {
                    break;
                }
                cell.text[len..len + written].copy_from_slice(&buf[..written]);
                len += written;
            }
            cell.len = len;
        }
    }
    is_placeholder
}

/// The cell's width: a wide glyph owns two columns and the tail cell after
/// it must not be drawn (`GHOSTTY_CELL_WIDE_SPACER_TAIL`).
#[must_use]
fn fill_width(cells: GhosttyRenderStateRowCells) -> Wide {
    let mut raw = 0u64;
    let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
    // SAFETY: `raw` is writable storage of the packed-cell type and the row
    // cells handle is live.
    if unsafe {
        ghostty_render_state_row_cells_get(
            cells,
            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
            ptr::from_mut(&mut raw).cast(),
        )
    } == GHOSTTY_SUCCESS
    {
        // SAFETY: `wide` is writable storage of the expected type.
        unsafe {
            ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, ptr::from_mut(&mut wide).cast());
        }
    }
    Wide::from_raw(wide)
}

/// The cell's raw style: a sized struct of the type the selector expects.
#[must_use]
fn fill_style(cells: GhosttyRenderStateRowCells) -> GhosttyStyle {
    // SAFETY: `style` is a sized struct of the type the selector expects and
    // the row cells handle is live.
    let mut style = unsafe { mem::zeroed::<GhosttyStyle>() };
    style.size = mem::size_of::<GhosttyStyle>();
    // SAFETY: `style` is writable storage of the expected type.
    unsafe {
        ghostty_render_state_row_cells_get(
            cells,
            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
            ptr::from_mut(&mut style).cast(),
        );
    };
    style
}

/// The cell's style flags derived from a raw [`GhosttyStyle`].
#[must_use]
fn style_flags(style: &GhosttyStyle) -> StyleFlags {
    let mut flags = StyleFlags::default();
    if style.bold {
        flags |= StyleFlags::BOLD;
    }
    if style.italic {
        flags |= StyleFlags::ITALIC;
    }
    if style.inverse {
        flags |= StyleFlags::INVERSE;
    }
    if style.faint {
        flags |= StyleFlags::FAINT;
    }
    if style.invisible {
        flags |= StyleFlags::INVISIBLE;
    }
    if style.strikethrough {
        flags |= StyleFlags::STRIKETHROUGH;
    }
    if style.underline != 0 {
        flags |= StyleFlags::UNDERLINE;
    }
    flags
}

/// The cell's colours: the style's image id for placeholders, else the
/// resolved foreground and background over the frame defaults (`fillCell`
/// colour half).
fn fill_colors(
    frame: &mut FrameState,
    cells: GhosttyRenderStateRowCells,
    style: &GhosttyStyle,
    is_placeholder: bool,
    cell: &mut Cell,
) {
    // The placeholder's image id is the cell's foreground colour, and the
    // resolved-colour getter has no value for placeholder cells, so take it
    // from the style itself.
    let style_fg_rgb = (style.fg_color.tag == GHOSTTY_STYLE_COLOR_RGB).then(|| {
        // SAFETY: the tag says the active union arm is `rgb`.
        let rgb = unsafe { style.fg_color.value.rgb };
        u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b)
    });

    let mut fg = GhosttyColorRgb {
        r: frame.foreground.r,
        g: frame.foreground.g,
        b: frame.foreground.b,
    };
    let mut bg = GhosttyColorRgb {
        r: frame.background.r,
        g: frame.background.g,
        b: frame.background.b,
    };
    // SAFETY: each out pointer is writable storage of the expected type and
    // the row cells handle is live.
    let mut foreground_present = unsafe {
        ghostty_render_state_row_cells_get(
            cells,
            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
            ptr::from_mut(&mut fg).cast(),
        )
    } == GHOSTTY_SUCCESS;
    // SAFETY: `bg` is writable storage of the expected type and the row
    // cells handle is live.
    let mut background_present = unsafe {
        ghostty_render_state_row_cells_get(
            cells,
            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
            ptr::from_mut(&mut bg).cast(),
        )
    } == GHOSTTY_SUCCESS;

    if style.inverse {
        mem::swap(&mut fg, &mut bg);
        mem::swap(&mut foreground_present, &mut background_present);
        // Both sides are "explicit" under inverse: one of them is the default.
        foreground_present = true;
        background_present = true;
    }

    if is_placeholder {
        // No glyph: the image is drawn at this cell instead, and the cell's
        // foreground colour is the image id.
        if let Some(image_id) = style_fg_rgb {
            frame
                .placeholders
                .note(image_id, frame.cell_y, frame.cell_x);
        }
    } else if foreground_present {
        cell.has_fg = true;
        cell.fg = fg.into();
    }
    if background_present {
        cell.has_bg = true;
        cell.bg = bg.into();
    }
}

/// Feed the frame protocol a real terminal: text, SGR truecolor and bold, a
/// wide character and a cursor move, then walk the frame.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::{DecodedPng, PngDecoder, PtySink};

    /// A sink with nowhere to write.
    struct NullSink;

    impl PtySink for NullSink {
        fn write_pty(&mut self, _data: &[u8]) {}
    }

    /// Rejects every image; these tests do not exercise PNG decoding.
    struct NoDecoder;

    impl PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
            None
        }
    }

    fn terminal() -> Terminal {
        let mut terminal = Terminal::new(crate::guard::Poisoned::new(), NullSink, NoDecoder, || {});
        assert!(terminal.push_size(40, 24, 8, 16));
        terminal
    }

    #[test]
    fn frame_walks_text_colors_wide_and_cursor() {
        let mut terminal = terminal();
        terminal.push_pty_data(b"\x1b[38;2;10;20;30mhi\x1b[0m \x1b[1mB\x1b[0m \xe4\xb8\xad");
        terminal.push_pty_data(b"\x1b[2;3H"); // row 2, column 3 (1-based)

        assert!(terminal.frame_begin());
        let cells = collect_cells(&mut terminal);
        let cursor = terminal.cursor().expect("a visible cursor");
        terminal.frame_end();

        let at = |x: i32, y: i32| {
            cells
                .iter()
                .find(|cell| cell.x == x && cell.y == y)
                .copied()
                .expect("cell exists")
        };

        let h = at(0, 0);
        assert_eq!(h.text_str(), "h");
        assert!(h.has_fg, "the truecolor foreground is explicit");
        assert_eq!(
            h.fg,
            Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );

        let bold = at(3, 0);
        assert_eq!(bold.text_str(), "B");
        assert!(bold.flags.contains(StyleFlags::BOLD));

        let wide = at(5, 0);
        assert_eq!(wide.text_str(), "\u{4E2D}");
        assert_eq!(wide.wide, Wide::WIDE, "the glyph owns two columns");
        let tail = at(6, 0);
        assert_eq!(tail.wide, Wide::SPACER_TAIL, "the tail must be skipped");
        assert_eq!(tail.len, 0, "the tail carries no glyph");

        assert_eq!((cursor.x, cursor.y), (2, 1));
        assert!(cursor.has_value);
    }

    #[test]
    fn frame_begin_needs_a_terminal_and_end_cleans_it() {
        let mut terminal = Terminal::new(crate::guard::Poisoned::new(), NullSink, NoDecoder, || {});
        assert!(!terminal.frame_begin(), "no terminal yet");
        assert!(terminal.cell_next().is_none());
        assert!(terminal.image_next().is_none());

        assert!(terminal.push_size(40, 24, 8, 16));
        assert!(terminal.frame_begin());
        terminal.frame_end();
        // A second frame is independent and starts clean.
        assert!(terminal.frame_begin());
        terminal.frame_end();
    }

    #[test]
    fn colors_are_stable_across_frames() {
        let mut terminal = terminal();
        assert!(terminal.frame_begin());
        let colors = terminal.colors();
        terminal.frame_end();
        assert_eq!(colors, terminal.colors());
    }

    fn collect_cells(terminal: &mut Terminal) -> Vec<Cell> {
        let mut cells = Vec::new();
        while let Some(cell) = terminal.cell_next() {
            cells.push(cell);
        }
        cells
    }
}
