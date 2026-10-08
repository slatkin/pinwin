//! The cursor layer of the grid painter: the frame's cursor
//! drawn after the cell layers and before the focus accent — the shape in
//! the terminal's default foreground, the hollow block as four
//! one-logical-pixel bands each snapped on its own so they tile the ring
//! with no gap and no overlap.
//!
//! The draw conditions live in
//! [`Terminal::cursor`]: the frame protocol reports the cursor only while
//! the frame is open and the cursor is visible with a viewport position —
//! a hidden cursor, one blinking off, or one outside the viewport draws
//! nothing. An unfocused panel draws the style the protocol reports
//! (ghostty's own hollow style); the painter does not re-derive it.
//!
//! The colour rule differs from the cell layers': a cell falls back to
//! the theme foreground when it names none, but the cursor is always the
//! terminal's default foreground (`Terminal::colors().foreground`),
//! unconditionally.
//!
//! The block cursor's glyph redraw: after the block fill, `paint` redraws
//! the glyph under the cursor in the terminal's default background through
//! the text pass's [`TextPass::draw_cell`], so a cell the sprite pass
//! owns (a block character under the cursor) is redrawn with its font
//! glyph there too. The redraw inputs come
//! from the text pass's walk: the collected [`CursorText`] (the cell bytes
//! at the cursor's position) and the pass itself, skipped exactly when
//! [`glyph_redraw`] is false — not a Block cursor, no text, or a wide tail.
//!
//! GTK-free (`replace-gtk-with-wayland` D10): [`cursor_shape`],
//! [`glyph_redraw`] and the pixel tests below run without a display.

use super::canvas::{Canvas, CanvasColor};
use super::geom::{DeviceRect, PainterMetrics};
use super::text_pass::{CursorText, TextPass};
use crate::term::Terminal;
use crate::term::cells::{CELL_TEXT_CAP, Cell, Cursor, CursorStyle};

/// The cursor's shape as the painter draws it: one rectangle to fill, or
/// the hollow block's four bands (top, bottom, left, right), all snapped
/// device rectangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    /// One rectangle to fill (bar, underline, block).
    Fill(DeviceRect),
    /// The hollow block's outline as four bands that tile the ring.
    Hollow([DeviceRect; 4]),
}

/// The geometry `paint` fills for `cursor`, snapped to the device pixel
/// grid. A cursor
/// covers its own cell only: a wide glyph's head cell carries the cursor,
/// the tail keeps the cell layers' pixels.
#[must_use]
pub fn cursor_shape(cursor: &Cursor, metrics: &PainterMetrics) -> CursorShape {
    let (cw, ch) = (metrics.cell_w(), metrics.cell_h());
    match cursor.style {
        // Two logical pixels wide, the full cell tall.
        CursorStyle::Bar => {
            CursorShape::Fill(metrics.cell_sub_rect(cursor.x, cursor.y, 0.0, 0.0, 2.0, ch))
        }
        // Two logical pixels tall, at the cell's bottom.
        CursorStyle::Underline => {
            CursorShape::Fill(metrics.cell_sub_rect(cursor.x, cursor.y, 0.0, ch - 2.0, cw, 2.0))
        }
        CursorStyle::BlockHollow => CursorShape::Hollow(hollow_bands(metrics, cursor.x, cursor.y)),
        // The full cell.
        CursorStyle::Block => {
            CursorShape::Fill(metrics.cell_sub_rect(cursor.x, cursor.y, 0.0, 0.0, cw, ch))
        }
    }
}

/// Whether the block cursor redraws the glyph under it in the terminal's
/// default background: a Block cursor with collected text and no wide
/// tail. The text unit calls this with the cell
/// text it collected at the cursor's position.
#[must_use]
pub fn glyph_redraw(cursor: &Cursor, text: &[u8]) -> bool {
    cursor.style == CursorStyle::Block && !text.is_empty() && !cursor.wide_tail
}

/// The hollow block cursor's outline as the four one-logical-pixel bands
/// (top, bottom, left, right), each snapped on its own
/// through the geom helpers. Bands that share an edge pass the same
/// logical edge to the snap — the vertical bands' ends are the horizontal
/// bands' sides — so they tile the ring without a gap or an overlap.
fn hollow_bands(metrics: &PainterMetrics, x: i32, y: i32) -> [DeviceRect; 4] {
    let (cw, ch) = (metrics.cell_w(), metrics.cell_h());
    [
        metrics.cell_sub_rect(x, y, 0.0, 0.0, cw, 1.0),
        metrics.cell_sub_rect(x, y, 0.0, ch - 1.0, cw, 1.0),
        metrics.cell_sub_rect(x, y, 0.0, 1.0, 1.0, ch - 2.0),
        metrics.cell_sub_rect(x, y, cw - 1.0, 1.0, 1.0, ch - 2.0),
    ]
}

/// Fill the open frame's cursor shape, shifted by `offset` device pixels
/// along x, and redraw the glyph under a block cursor in the terminal's
/// default background through the text pass. The frame must be open; the
/// cursor layer walks no cells, it reads the frame's captured cursor and
/// the text the text pass collected at the cursor's position.
pub(super) fn paint(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    terminal: &Terminal,
    offset: i32,
    cursor_text: &CursorText,
    text: &mut TextPass,
) {
    let Some(cursor) = terminal.cursor() else {
        return;
    };
    // The terminal's default foreground, unconditionally — no theme
    // fallback, unlike the cell layers (see the module comment).
    let color = CanvasColor::from_theme(terminal.colors().foreground);
    match cursor_shape(&cursor, metrics) {
        CursorShape::Fill(rect) => fill(canvas, rect, offset, color),
        CursorShape::Hollow(bands) => {
            for rect in bands {
                fill(canvas, rect, offset, color);
            }
        }
    }
    // The block cursor's glyph redraw, above the block fill: the synthetic
    // cell carries the cursor's bytes and the terminal's default background,
    // drawn through the text path whenever [`glyph_redraw`] holds, so a
    // sprite-owned cell's block character is redrawn with its font glyph
    // here too.
    let bytes = cursor_text.as_bytes();
    if glyph_redraw(&cursor, bytes) {
        let mut cell_text = [0; CELL_TEXT_CAP];
        cell_text[..bytes.len()].copy_from_slice(bytes);
        let cell = Cell {
            x: cursor.x,
            y: cursor.y,
            text: cell_text,
            len: bytes.len(),
            ..Cell::default()
        };
        let background = CanvasColor::from_theme(terminal.colors().background);
        text.draw_cell(canvas, metrics, &cell, background, offset);
    }
}

/// Fill one snapped device rectangle, shifted by `offset` device pixels
/// along x.
fn fill(canvas: &mut Canvas, rect: DeviceRect, offset: i32, color: CanvasColor) {
    canvas.fill_rect(rect.x() + offset, rect.y(), rect.w(), rect.h(), color);
}

#[cfg(test)]
mod tests;
