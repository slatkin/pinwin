//! The decoration layer of the grid painter (row 4.3): the underline and
//! strikethrough bands a cell's style flags ask for, drawn in the cell's
//! foreground colour — or the theme foreground when the cell has none —
//! with the same rule the GTK painters' band rectangles used:
//! a one-logical-pixel band across the cell's width, the underline hanging
//! from the font ascent plus one pixel, the strikethrough centred on the
//! cell's mid-line.
//!
//! The cells skipped are the ones the GTK cell pass skips: a wide glyph's
//! spacer tail is never drawn, and a cell with no glyph or the INVISIBLE
//! flag carries no decorations either. A wide glyph's band therefore spans
//! its head cell's width only, as on the GTK path.
//!
//! The frame's underline flag is a boolean — libghostty's underline style
//! enum (single, double, curly, dotted, dashed) is collapsed to it in the
//! frame protocol — so the band is the single style the GTK path drew. A
//! style switch plugs in where [`paint`] picks the band geometry, once the
//! frame protocol carries the style.
//!
//! The frame's tween draw offset translates the bands with the grid, like
//! every grid layer.
//!
//! GTK-free (`replace-gtk-with-wayland` D10); private to the painter, whose
//! frame pass calls the bands after the backgrounds and before the text and
//! sprite passes later rows add.

use super::canvas::{Canvas, CanvasColor};
use super::geom::{FrameInput, PainterMetrics};
use crate::term::Terminal;
use crate::term::cells::{StyleFlags, Wide};

/// Walk the open frame's cells and fill the decoration bands, shifted by
/// `offset` device pixels along x. The frame must be open
/// ([`Terminal::frame_begin`]); the caller rewinds the frame afterwards.
pub(super) fn paint(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    terminal: &mut Terminal,
    offset: i32,
) {
    while let Some(cell) = terminal.cell_next() {
        if cell.wide == Wide::SpacerTail {
            continue; // do not render
        }
        if cell.len == 0 || cell.flags.contains(StyleFlags::INVISIBLE) {
            continue;
        }
        let color = if cell.has_fg {
            CanvasColor::from_theme(cell.fg)
        } else {
            frame.foreground()
        };
        // The band geometry picker: one rule per style the frame protocol
        // carries today.
        if cell.flags.contains(StyleFlags::UNDERLINE) {
            fill_band(
                canvas,
                metrics,
                &cell,
                offset,
                metrics.ascent() + 1.0,
                color,
            );
        }
        if cell.flags.contains(StyleFlags::STRIKETHROUGH) {
            fill_band(
                canvas,
                metrics,
                &cell,
                offset,
                metrics.cell_h() / 2.0 - 0.5,
                color,
            );
        }
    }
}

/// Fill the one-logical-pixel band across `cell` whose top edge sits `top`
/// logical pixels below the cell's top, shifted by `offset` device pixels
/// along x.
fn fill_band(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    cell: &crate::term::cells::Cell,
    offset: i32,
    top: f64,
    color: CanvasColor,
) {
    let rect = metrics.cell_sub_rect(cell.x, cell.y, 0.0, top, metrics.cell_w(), 1.0);
    canvas.fill_rect(rect.x() + offset, rect.y(), rect.w(), rect.h(), color);
}
