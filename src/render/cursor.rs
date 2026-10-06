//! The cursor layer of the grid painter (row 4.3): the frame's cursor
//! drawn after the cell layers and before the focus accent, exactly as
//! `DrawState::draw_cursor` draws it on the GTK path — the shape in the
//! terminal's default foreground, the hollow block as four
//! one-logical-pixel bands each snapped on its own so they tile the ring
//! with no gap and no overlap.
//!
//! The draw conditions are the GTK path's, all folded into
//! [`Terminal::cursor`]: the frame protocol reports the cursor only while
//! the frame is open and the cursor is visible with a viewport position —
//! a hidden cursor, one blinking off, or one outside the viewport draws
//! nothing. An unfocused panel draws the style the protocol reports
//! (ghostty's own hollow style); the painter does not re-derive it.
//!
//! The colour rule differs from the cell layers': a cell falls back to
//! the theme foreground when it names none, but the cursor is always the
//! terminal's default foreground (`Terminal::colors().foreground`), the
//! same unconditional colour `draw_cursor` sets.
//!
//! Seam for the text unit (row 4.4) — the block cursor's glyph redraw.
//! Under a `Block` cursor the GTK path redraws the glyph under the cursor
//! in the terminal's default background, on top of the block; the cell
//! pass collected that glyph's text into `render_grid`'s `cursor_text`
//! buffer while it walked. This layer draws the shape only. When the text
//! pass arrives it must hand this layer (a) the text bytes of the cell at
//! the cursor's position, collected during its own cell walk, and (b) a
//! way to draw one cell's text, and this layer then calls it after the
//! block fill, in `Terminal::colors().background`, skipped exactly when
//! [`glyph_redraw`] is false — no redraw on a wide tail or with no text.
//!
//! GTK-free (`replace-gtk-with-wayland` D10): [`cursor_shape`],
//! [`glyph_redraw`] and the pixel tests below run without a display.

use super::canvas::{Canvas, CanvasColor};
use super::geom::{DeviceRect, PainterMetrics};
use crate::term::Terminal;
use crate::term::cells::{Cursor, CursorStyle};

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
/// grid — the device form of [`crate::render::node_cursor::cursor_shape`]'s
/// logical rectangles, the same formulas the GTK painters share. A cursor
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
/// default background — the GTK path's condition (`draw_cursor` and the
/// node emitter's, identical). The text unit calls this with the cell
/// text it collected at the cursor's position.
#[must_use]
pub fn glyph_redraw(cursor: &Cursor, text: &[u8]) -> bool {
    cursor.style == CursorStyle::Block && !text.is_empty() && !cursor.wide_tail
}

/// The hollow block cursor's outline as the four one-logical-pixel bands
/// the GTK path fills (top, bottom, left, right), each snapped on its own
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
/// along x. The frame must be open; the cursor layer walks no cells, it
/// reads the frame's captured cursor.
pub(super) fn paint(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    terminal: &Terminal,
    offset: i32,
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
    // Seam (row 4.4, text pass): here the block cursor's glyph redraw goes
    // — see the module comment. The shape fills above the text pass's
    // glyph, the redraw above the block.
}

/// Fill one snapped device rectangle, shifted by `offset` device pixels
/// along x.
fn fill(canvas: &mut Canvas, rect: DeviceRect, offset: i32, color: CanvasColor) {
    canvas.fill_rect(rect.x() + offset, rect.y(), rect.w(), rect.h(), color);
}

#[cfg(test)]
mod tests;
