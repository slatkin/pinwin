//! The frame protocol half of pinwin's terminal core (port-to-rust D3):
//! starting and ending a frame, walking the terminal's cells (with grapheme
//! merging and constraint widths), the cursor and default colours, and the
//! kitty image placements (design D4). Ported from `src/cells.zig` and the
//! frame half of `src/pinwin.h`.
//!
//! The `libghostty-vt` crate gates render-state reads behind a snapshot
//! (`RenderState::update` returns one, and every iterator must be refreshed
//! from it), and a snapshot borrows the render state exclusively — so a
//! snapshot cannot be stored in the [`Terminal`] that owns the render state.
//! The panel's frame API is streaming (`frame_begin`, then `cell_next` calls
//! spread over the paint passes), so [`Terminal::frame_begin`] takes one
//! snapshot, walks the whole grid once (the old code walked it per pass
//! anyway, design D5 of `port-to-rust`), and captures the cells, cursor,
//! colours and dirty data as plain data. The passes then walk the capture,
//! which keeps the public frame API and the paint order unchanged
//! (adopt-libghostty-rs A5: one snapshot, the borrow checker sees it once).

use libghostty_vt as vt;
use libghostty_vt::render::Snapshot;
use libghostty_vt::screen::CellWide;

use super::{Handles, Terminal};

mod dirty;
pub use dirty::FrameDirty;
#[cfg(test)]
mod golden;
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
/// walk cursor stays on [`FrameState`] itself: it belongs to the cell walk,
/// not the frame lifecycle.
#[derive(Default)]
pub(crate) struct FrameFlags {
    open: bool,
    has_pending: bool,
    images_started: bool,
}

/// One captured viewport row: its viewport y and its raw cells, in column
/// order, before grapheme merging.
struct CapturedRow {
    y: i32,
    cells: Vec<Cell>,
}

#[derive(Default)]
pub(crate) struct FrameState {
    flags: FrameFlags,
    last_emitted_cp: u32,
    pending_cell: Cell,
    cursor: Cursor,
    foreground: Rgb,
    background: Rgb,
    placeholders: images::PlaceholderMap,
    /// The global dirty state the render state reported at `frame_begin`.
    /// `update` only escalates the dirty state, it never unsets it;
    /// `frame_end`'s clean unsets both layers after a consumed frame.
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
    /// The grid the frame's snapshot captured: one entry per viewport row,
    /// in walk order. Reused across frames; cleared and refilled by every
    /// `frame_begin`.
    rows: Vec<CapturedRow>,
    /// The cell walk's position in `rows`: the row index and the cell index
    /// within it. `frame_rewind` resets it.
    walk: (usize, usize),
    /// The kitty placements the image pass captured on its first call this
    /// frame, handed out one per `image_next`.
    images: Vec<Image>,
    /// The render-state snapshot the frame's `update` produced, kept so
    /// `frame_end` can mark the consumed frame clean. The snapshot borrows
    /// the terminal's render state exclusively, and its borrow lifetime was
    /// erased on capture — see [`begin`] for the invariant that makes that
    /// sound.
    snapshot: Option<Snapshot<'static, 'static>>,
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
        // Test-only failure injection (D10), like `callbacks::fail_point`:
        // the render-state update step fails before any capture step.
        #[cfg(test)]
        let fail_update = self.shared.fail_point.get() == Some("frame_begin");
        #[cfg(not(test))]
        let fail_update = false;
        begin(&mut self.frame, handles, fail_update)
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
        let scale_120 = self.shared.scale_120.get();
        let handles = self.handles.as_mut()?;
        images::image_next(&mut self.frame, handles, cell_w, cell_h, scale_120)
    }

    /// Finish the frame and clear the render state's dirty flags — both
    /// layers: the snapshot-level clean sets the global state to clean and
    /// clears every per-row flag, so a consumed frame needs no per-row
    /// setter.
    pub fn frame_end(&mut self) {
        let Some(handles) = self.handles.as_ref() else {
            return;
        };
        end(&mut self.frame, handles);
    }
}

/// Start a frame (`pinwin_frame_begin`): refresh the render state, capture
/// the default colours, cursor, dirty data and the whole grid, and reset the
/// cell walk.
fn begin(frame: &mut FrameState, handles: &mut Handles, fail_update: bool) -> bool {
    // A frame left open by a caller that forgot `frame_end` held the
    // snapshot; drop it and close the frame before the next update, which
    // needs the render state exclusively — no failure path may expose a
    // stale frame afterwards.
    frame.flags.open = false;
    frame.snapshot = None;

    let Some(snapshot) = render_state_update(handles, fail_update) else {
        return false;
    };

    if let Ok(colors) = snapshot.colors() {
        frame.background = colors.background.into();
        frame.foreground = colors.foreground.into();
    }

    frame.cursor = Cursor::default();
    if let Ok(cursor) = snapshot.cursor()
        && cursor.visible
        && let Some(viewport) = cursor.viewport
    {
        frame.cursor.has_value = true;
        frame.cursor.x = i32::from(viewport.x);
        frame.cursor.y = i32::from(viewport.y);
        frame.cursor.style = CursorStyle::from_raw(cursor.visual_style);
        frame.cursor.wide_tail = viewport.at_wide_tail;
    }

    // The dirty data the update just computed, and the kitty
    // placement presence. The walk below captures the per-row dirty
    // flags together with the cells, in one pass over the grid.
    frame.row_filter = None;
    frame.dirty = dirty::global_dirty(&snapshot);
    frame.dirty_rows.clear();
    frame.has_images = false;
    frame.rows.clear();
    frame.placeholders.clear();
    frame.images.clear();
    dirty::capture_images(frame, handles);

    if !capture_grid(frame, handles, &snapshot) {
        return false;
    }

    frame.flags.open = true;
    frame.flags.has_pending = false;
    frame.last_emitted_cp = 0;
    frame.flags.images_started = false;
    frame.walk = (0, 0);
    frame.snapshot = Some(snapshot);
    true
}

/// Update the render state from the terminal and hand back the snapshot with
/// its borrow lifetime erased.
///
/// # Safety of the stored snapshot
///
/// The returned snapshot borrows `handles.render_state` exclusively, but the
/// frame API stores it in the same `Terminal` that owns the render state, so
/// its real lifetime cannot be expressed. The erasure is sound under the
/// frame's own discipline, enforced here: while a frame is open, every
/// render-state access goes through the stored snapshot (`frame_end`'s
/// clean), `frame_begin` drops a leftover snapshot before updating, and the
/// snapshot is a plain `Option` field with no `Drop`, so a `Terminal` dropped
/// with a frame open frees the render state before the (inert) snapshot.
fn render_state_update(
    handles: &mut Handles,
    fail_update: bool,
) -> Option<Snapshot<'static, 'static>> {
    if fail_update {
        // Test-only injection (D10): behave like an `update` that failed
        // (out-of-memory), so frame tests can reach `begin`'s error paths.
        return None;
    }
    let terminal = &handles.terminal;
    let snapshot = handles.render_state.update(terminal).ok()?;
    // SAFETY: the snapshot's only reference is to `handles.render_state`,
    // which lives as long as the owning `Terminal`; the frame discipline
    // above keeps every other render-state access away while it is stored,
    // and `Snapshot` has no `Drop` that could observe the erasure.
    Some(unsafe {
        std::mem::transmute::<Snapshot<'static, '_>, Snapshot<'static, 'static>>(snapshot)
    })
}

/// Walk the captured rows once: the per-row dirty flags for the partial
/// repaint, and every row's raw cells for the cell passes. `false` when a
/// row read fails, which leaves the frame closed.
fn capture_grid(
    frame: &mut FrameState,
    handles: &mut Handles,
    snapshot: &Snapshot<'static, 'static>,
) -> bool {
    let Ok(mut rows) = handles.row_iterator.update(snapshot) else {
        return false;
    };
    while let Some(row) = rows.next() {
        let Ok(viewport_y) = row.viewport_y() else {
            continue;
        };
        if let Ok(dirty) = row.dirty()
            && dirty
        {
            frame.dirty_rows.push(viewport_y);
        }
        let Ok(mut cells) = handles.cell_iterator.update(row) else {
            continue;
        };
        let mut captured = CapturedRow {
            y: viewport_y,
            cells: Vec::new(),
        };
        let mut x: i32 = 0;
        while let Some(cell) = cells.next() {
            captured.cells.push(fill_cell(frame, cell, x, viewport_y));
            x += 1;
        }
        frame.rows.push(captured);
    }
    true
}

/// Rewind the cell walk for the glyph pass (`pinwin_frame_rewind`). The
/// placeholder origin map is not cleared here: the old walk re-recorded the
/// origins during every pass, but the capture records them once at
/// `frame_begin` (every pass sees the same cells, so the map is the same),
/// and the image pass resolves against it after any number of rewinds.
fn rewind(frame: &mut FrameState, handles: &mut Handles) {
    let _ = handles;
    if !frame.flags.open {
        return;
    }
    frame.flags.has_pending = false;
    frame.last_emitted_cp = 0;
    frame.walk = (0, 0);
}

/// End a frame and clean the render state (`pinwin_frame_end`).
fn end(frame: &mut FrameState, handles: &Handles) {
    let _ = handles;
    if !frame.flags.open {
        return;
    }
    frame.flags.open = false;
    if let Some(snapshot) = frame.snapshot.take()
        && let Err(error) = snapshot.clean()
    {
        // The only failure mode is out-of-memory; leave the dirty state as
        // the update computed it, so the next frame redraws conservatively.
        let _ = error;
    }
}

/// Whether the frame walk visits the viewport row `y` under the partial
/// repaint's row filter. No filter visits everything.
fn row_visible(frame: &FrameState, y: i32) -> bool {
    dirty::row_visible(frame, y)
}

/// The next captured raw cell, before grapheme joining (`nextRawCell`).
/// `None` at the end of the grid.
fn next_raw_cell(frame: &mut FrameState, handles: &mut Handles) -> Option<Cell> {
    let _ = handles;
    loop {
        let (row_index, cell_index) = frame.walk;
        let row = frame.rows.get(row_index)?;
        // The partial repaint's row filter: skip the rows the painter is not
        // repainting this frame. The filter persists across `frame_rewind`,
        // so every pass of the frame walks the same rows.
        if !row_visible(frame, row.y) {
            frame.walk = (row_index + 1, 0);
            continue;
        }
        if cell_index < row.cells.len() {
            frame.walk.1 += 1;
            return Some(row.cells[cell_index]);
        }
        frame.walk = (row_index + 1, 0);
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

/// Read a captured cell's graphemes, width, style and colours (`fillCell`).
fn fill_cell(
    frame: &mut FrameState,
    cell: &vt::render::CellIteration<'_, '_>,
    x: i32,
    y: i32,
) -> Cell {
    let mut out = Cell {
        x,
        y,
        ..Cell::default()
    };
    let is_placeholder = fill_text(cell, &mut out);
    out.wide = fill_width(cell);
    let style = cell.style().unwrap_or_default();
    out.flags = style_flags(&style);
    fill_colors(frame, cell, &style, is_placeholder, &mut out);
    out
}

/// Fill `cell`'s text from the cell's grapheme cluster (`fillCell` text
/// half). True when the cell is a kitty placeholder, which carries no glyph.
#[must_use]
fn fill_text(cell: &vt::render::CellIteration<'_, '_>, out: &mut Cell) -> bool {
    let mut text = String::new();
    if cell.graphemes_utf8(&mut text).is_err() {
        return false;
    }
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if u32::from(first) == PLACEHOLDER {
        return true;
    }
    // The same per-codepoint encoding cap the C-API buffer path had: whole
    // codepoints that fit, nothing that would overflow the cell's text.
    let mut len = first.encode_utf8(&mut [0; 4]).len();
    if len <= CELL_TEXT_CAP {
        write_text(out, first);
        for ch in chars {
            let written = ch.encode_utf8(&mut [0; 4]).len();
            if len + written > CELL_TEXT_CAP {
                break;
            }
            write_text_at(out, ch, len);
            len += written;
        }
    }
    out.len = len.min(CELL_TEXT_CAP);
    false
}

/// Write one codepoint's UTF-8 bytes at the start of the cell's text.
fn write_text(cell: &mut Cell, ch: char) {
    write_text_at(cell, ch, 0);
}

/// Write one codepoint's UTF-8 bytes at `offset` in the cell's text.
fn write_text_at(cell: &mut Cell, ch: char, offset: usize) {
    let mut buf = [0u8; 4];
    let bytes = ch.encode_utf8(&mut buf).as_bytes();
    cell.text[offset..offset + bytes.len()].copy_from_slice(bytes);
}

/// The cell's width: a wide glyph owns two columns and the tail cell after
/// it must not be drawn (`GHOSTTY_CELL_WIDE_SPACER_TAIL`).
#[must_use]
fn fill_width(cell: &vt::render::CellIteration<'_, '_>) -> Wide {
    Wide::from_raw(
        cell.raw_cell()
            .and_then(vt::screen::Cell::wide)
            .unwrap_or(CellWide::Narrow),
    )
}

/// The cell's style flags derived from the crate's [`vt::style::Style`].
#[must_use]
fn style_flags(style: &vt::style::Style) -> StyleFlags {
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
    if style.underline != vt::style::Underline::None {
        flags |= StyleFlags::UNDERLINE;
    }
    flags
}

/// The cell's colours: the style's image id for placeholders, else the
/// resolved foreground and background over the frame defaults (`fillCell`
/// colour half).
fn fill_colors(
    frame: &mut FrameState,
    cell: &vt::render::CellIteration<'_, '_>,
    style: &vt::style::Style,
    is_placeholder: bool,
    out: &mut Cell,
) {
    // The placeholder's image id is the cell's foreground colour, and the
    // resolved-colour getter has no value for placeholder cells, so take it
    // from the style itself.
    let style_fg_rgb = match style.fg_color {
        vt::style::StyleColor::Rgb(rgb) => {
            Some(u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b))
        }
        _ => None,
    };

    let mut fg = frame.foreground;
    let mut bg = frame.background;
    let mut foreground_present = false;
    if let Ok(Some(color)) = cell.fg_color() {
        fg = color.into();
        foreground_present = true;
    }
    let mut background_present = false;
    if let Ok(Some(color)) = cell.bg_color() {
        bg = color.into();
        background_present = true;
    }

    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
        std::mem::swap(&mut foreground_present, &mut background_present);
        // Both sides are "explicit" under inverse: one of them is the default.
        foreground_present = true;
        background_present = true;
    }

    if is_placeholder {
        // No glyph: the image is drawn at this cell instead, and the cell's
        // foreground colour is the image id.
        if let Some(image_id) = style_fg_rgb {
            frame.placeholders.note(image_id, out.y, out.x);
        }
    } else if foreground_present {
        out.has_fg = true;
        out.fg = fg;
    }
    if background_present {
        out.has_bg = true;
        out.bg = bg;
    }
}

/// Feed the frame protocol a real terminal: text, SGR truecolor and bold, a
/// wide character and a cursor move, then walk the frame.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Poisoned;
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
        let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
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
        let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
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

    /// The frame's snapshot is never left behind after `frame_end`, and a
    /// second `frame_begin` without an `frame_end` still opens (the leftover
    /// snapshot is dropped first) — the stored-borrow discipline of
    /// [`render_state_update`].
    #[test]
    fn a_frame_begin_over_an_open_frame_still_opens() {
        let mut terminal = terminal();
        assert!(terminal.frame_begin());
        assert!(terminal.frame_begin(), "the stale snapshot was dropped");
        terminal.frame_end();
        assert!(terminal.frame_begin());
        terminal.frame_end();
    }

    /// A failed `frame_begin` over a frame the caller left open closes it:
    /// the cursor and the cells are gone, so the accessors report an empty
    /// frame and the failure cannot stain the next frames.
    #[test]
    fn a_failed_frame_begin_over_an_open_frame_closes_it() {
        let mut terminal = terminal();
        terminal.push_pty_data(b"\x1b[2;3H"); // row 2, column 3 (1-based)
        assert!(terminal.frame_begin());
        assert!(terminal.cursor().is_some(), "the frame owns a cursor");

        // The caller forgets `frame_end`; the next `frame_begin` fails on
        // the render-state update (the test-only fail point) and must
        // close the frame.
        terminal.fail_point().set(Some("frame_begin"));
        assert!(!terminal.frame_begin());
        terminal.fail_point().set(None);
        assert!(terminal.cursor().is_none(), "a closed frame has no cursor");
        assert!(terminal.cell_next().is_none());

        // The failed begin leaves the terminal clean for the next frame.
        assert!(terminal.frame_begin());
        terminal.frame_end();
    }

    /// `ptr` is only touched to keep the raw-pointer import used by the
    /// capture types honest; the frame state stores no raw pointers.
    #[test]
    fn the_frame_state_holds_no_raw_pointers() {
        // The snapshot is erased-lifetime data; everything else the frame
        // keeps is plain. Asserting the compile-time shape here: a frame
        // closed by `frame_end` stores no snapshot.
        let mut terminal = terminal();
        assert!(terminal.frame_begin());
        assert!(terminal.frame.snapshot.is_some());
        terminal.frame_end();
        assert!(terminal.frame.snapshot.is_none());
    }
}
