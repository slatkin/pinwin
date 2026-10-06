//! The sprite layer of the grid painter (row 4.3): the frame's block
//! elements ([`sprite::block`]), braille patterns ([`sprite::braille`]) and
//! polygons ([`sprite::poly`] — the corner triangles and the powerline
//! separators, solid and hollow) drawn on the canvas in the cell's
//! foreground colour — or the theme foreground when the cell names none —
//! exactly as [`crate::render::sprites::draw_sprite`] draws them on the
//! GTK path. The cursor shapes plug in as a further dispatch arm in the
//! next row; until then their code points are not sprites and fall to the
//! text pass.
//!
//! The per-cell skips are the GTK cell pass's: a wide glyph's spacer tail
//! is never rendered, and a cell with no glyph or the INVISIBLE flag draws
//! no sprite. [`cell_sprite`] is the pure per-cell decision — skips,
//! routing and colour — so the dispatch stays testable without a frame
//! walk; [`paint`] only walks and fills.
//!
//! The layer sits after the backgrounds and before the bands: GTK draws
//! each cell's decorations after its glyph, so the bands layer must come
//! after this one (and after the text pass later rows add).
//!
//! GTK-free (`replace-gtk-with-wayland` D10); private to the painter, whose
//! frame pass calls it between the backgrounds and the bands.

use super::canvas::{Canvas, CanvasColor};
use super::geom::{DeviceRect, FrameInput, PainterMetrics, device_f32};
use crate::term::Terminal;
use crate::term::cells::{StyleFlags, Wide, first_codepoint};

mod block;
mod braille;
mod poly;

/// The drawing primitives one sprite cell carries (row 4.3): unantialiased
/// rectangles for the blocks and the braille dots, an antialiased filled
/// polygon for the solid corner and powerline triangles, and an antialiased
/// closed-outline stroke with miter joins for the hollow powerline outline —
/// design decision 5's one line sprite, which the GTK path strokes as one
/// closed triangle so cairo's joins fill the corners. All coordinates are
/// device pixels; a `None` from [`cell_sprite`] still means the text pass
/// owns the cell.
#[derive(Debug)]
pub(crate) enum Primitive {
    /// Axis-aligned rectangles to fill exactly with the sprite colour.
    Rects(Vec<DeviceRect>),
    /// A closed polygon to fill with the sprite colour, its vertices in
    /// draw order.
    FillPolygon(Vec<(f64, f64)>),
    /// A closed polygon's outline to stroke with the sprite colour at the
    /// given device width, its vertices in draw order (the path closes
    /// back to the first vertex).
    StrokePolygon(Vec<(f64, f64)>, f64),
}

/// Walk the open frame's cells and draw the sprites, shifted by `offset`
/// device pixels along x. The frame must be open
/// ([`Terminal::frame_begin`]); the caller rewinds the frame afterwards.
pub(super) fn paint(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    terminal: &mut Terminal,
    offset: i32,
) {
    while let Some(cell) = terminal.cell_next() {
        let Some((color, primitive)) = cell_sprite(metrics, frame, &cell) else {
            continue;
        };
        match primitive {
            Primitive::Rects(rects) => {
                for rect in rects {
                    canvas.fill_rect(rect.x() + offset, rect.y(), rect.w(), rect.h(), color);
                }
            }
            Primitive::FillPolygon(points) => {
                let points: Vec<(f32, f32)> = points
                    .iter()
                    .map(|(x, y)| (shift(*x, offset), device_f32(*y)))
                    .collect();
                canvas.fill_polygon(&points, color);
            }
            Primitive::StrokePolygon(points, width) => {
                let points: Vec<(f32, f32)> = points
                    .iter()
                    .map(|(x, y)| (shift(*x, offset), device_f32(*y)))
                    .collect();
                canvas.stroke_polygon(&points, device_f32(width), color);
            }
        }
    }
}

/// Shift one device x coordinate by the tween's device-pixel offset.
fn shift(x: f64, offset: i32) -> f32 {
    device_f32(x + f64::from(offset))
}

/// The sprite one cell draws: its colour and drawing primitives, or `None`
/// when the cell is not drawn as a sprite — the skipped cells, and every
/// code point no dispatch arm owns yet (the text pass draws those). Pure,
/// so the tests build cells by hand and no frame walk is needed.
fn cell_sprite(
    metrics: &PainterMetrics,
    frame: &FrameInput,
    cell: &crate::term::cells::Cell,
) -> Option<(CanvasColor, Primitive)> {
    if cell.wide == Wide::SpacerTail {
        return None; // never rendered
    }
    if cell.len == 0 || cell.flags.contains(StyleFlags::INVISIBLE) {
        return None;
    }
    let cp = first_codepoint(cell.text_bytes());
    // The sprite dispatch: one arm per family a row ports. Row 4.3 owns
    // the blocks, the braille patterns and the polygons.
    let rects = block::rects(cp, metrics, cell).or_else(|| braille::rects(cp, metrics, cell));
    let primitive = match rects {
        Some(rects) => Primitive::Rects(rects),
        None => poly::primitive(cp, metrics, cell)?,
    };
    let color = if cell.has_fg {
        CanvasColor::from_theme(cell.fg)
    } else {
        frame.foreground()
    };
    Some((color, primitive))
}

#[cfg(test)]
mod tests;
