//! The node emitter's sprite pass (gsk-render-nodes task 2.2): the node form
//! of [`super::sprites::draw_sprite`]. Blocks, quadrants and braille dots are
//! rectangles, so they go out as colour nodes; the corner and powerline
//! triangles need a path, and `gtk4` is on `v4_8` — no `gsk::Path` yet — so
//! they are drawn with cairo into one `append_cairo` node per cell (still a
//! retained node; the triangles are a tiny fraction of the grid). The
//! geometry itself is [`super::sprites::sprite_shape`]'s, shared with the
//! cairo painter so the two cannot drift.

use gtk4::gdk;
use gtk4::graphene;
use gtk4::prelude::SnapshotExt as _;

use super::metrics::CellMetrics;
use super::sprites::{self, SpriteShape};
use crate::term::cells::{Cell, Wide};

/// Emit the sprite for `cp` in `cell` into `snapshot` with `colour` as the
/// source, the node form of [`sprites::draw_sprite`](super::sprites::draw_sprite).
/// Reports whether `cp` was drawn; `false` leaves the cell to the text pass,
/// exactly as `draw_sprite`'s `false` does on the cairo side.
pub(crate) fn emit_cell_sprite(
    snapshot: &gtk4::Snapshot,
    cell: &Cell,
    cp: u32,
    cell_metrics: &CellMetrics,
    colour: &gdk::RGBA,
) -> bool {
    match sprites::sprite_shape(cell, cp, cell_metrics) {
        SpriteShape::None => false,
        SpriteShape::Rects(rects) => {
            for (x, y, w, h) in rects {
                snapshot.append_color(
                    colour,
                    &graphene::Rect::new(x as f32, y as f32, w as f32, h as f32),
                );
            }
            true
        }
        SpriteShape::FillTriangle(points) => {
            triangle_node(snapshot, cell, cell_metrics, colour, points, false);
            true
        }
        SpriteShape::StrokeTriangle(points) => {
            triangle_node(snapshot, cell, cell_metrics, colour, points, true);
            true
        }
    }
}

/// A corner or powerline triangle as a per-cell cairo node: the same
/// `move_to`/`line_to`/fill-or-stroke sequence [`sprites::draw_sprite`]
/// (super::sprites::draw_sprite) runs, recorded into the snapshot. The
/// node's bounds are the cell plus [`PAD`] on every side — the bounds clip,
/// and the hollow powerline's 2 px stroke spills up to 1 px past the cell,
/// which the cairo painter draws unclipped.
fn triangle_node(
    snapshot: &gtk4::Snapshot,
    cell: &Cell,
    cell_metrics: &CellMetrics,
    colour: &gdk::RGBA,
    [ax, ay, bx, by, cx, cy]: [f64; 6],
    stroke: bool,
) {
    const PAD: f64 = 2.0;
    let cell_w = f64::from(cell_metrics.cell_w) * if cell.wide == Wide::Wide { 2.0 } else { 1.0 };
    let bounds = graphene::Rect::new(
        (f64::from(cell.x) * f64::from(cell_metrics.cell_w) - PAD) as f32,
        (f64::from(cell.y) * f64::from(cell_metrics.cell_h) - PAD) as f32,
        (cell_w + 2.0 * PAD) as f32,
        (f64::from(cell_metrics.cell_h) + 2.0 * PAD) as f32,
    );
    let cr = snapshot.append_cairo(&bounds);
    cr.set_source_rgba(
        f64::from(colour.red()),
        f64::from(colour.green()),
        f64::from(colour.blue()),
        f64::from(colour.alpha()),
    );
    cr.move_to(ax, ay);
    cr.line_to(bx, by);
    cr.line_to(cx, cy);
    if stroke {
        cr.set_line_width(2.0);
        let _ = cr.stroke();
    } else {
        cr.close_path();
        let _ = cr.fill();
    }
    drop(cr);
}

#[cfg(test)]
mod tests {
    use crate::render::metrics::CellMetrics;
    use crate::render::parity::{self, cairo_frame, node_frame, terminal_with};
    use crate::render::tests::state;

    /// Blocks (full, upper half, left 1/8), quadrants, braille, the four
    /// corner triangles, all four powerline separators, and a shade left to
    /// the font — plus an underlined block, so a sprite cell carrying a
    /// decoration paints both. The cursor is hidden; there are no images.
    fn terminal_with_sprites() -> crate::term::Terminal {
        terminal_with(
            "\u{2588}\u{2580}\u{258f}\u{2596}\u{259f}\u{28ff}\u{283e} \u{25e2}\u{25e3}\u{25e4}\u{25e5} \u{e0b0}\u{e0b1}\u{e0b2}\u{e0b3} \u{2591} \x1b[4m\u{2588}\x1b[24m\u{2584}"
                .as_bytes(),
        )
    }

    /// A draw state whose cell pitch is 8x16, the geometry the sprite tests
    /// pin: every block, quadrant and braille rectangle is then integral, so
    /// a colour node fills it exactly like the cairo painter's
    /// `Antialias::None` fill. The tests below need this because a sprite's
    /// eighths and quadrants are fractional at an odd pitch (the measured
    /// test font's 9x20), where the two painters differ by the snap
    /// [`fractional_pitch_matches_within_tolerance`] states.
    fn state_with_8x16_metrics() -> crate::render::DrawState {
        let mut draw_state = state();
        draw_state.cell_metrics = CellMetrics {
            cell_w: 8,
            cell_h: 16,
            ascent: 12,
            baseline: 4,
            nerd: draw_state.cell_metrics.nerd,
            scale: draw_state.cell_metrics.scale,
        };
        draw_state
    }

    /// The full frame — backgrounds plus the cell pass with sprites routed
    /// to the node emitter — matches the cairo painter exactly at scale 1
    /// when the sprite geometry is integral (8x16 cells).
    #[test]
    fn sprites_match_the_cairo_painter_exactly_at_scale_1() {
        let _font = crate::render::font_lock::guard();
        let mut draw_state = state_with_8x16_metrics();
        let mut cairo_terminal = terminal_with_sprites();
        let mut node_terminal = terminal_with_sprites();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.0);
        let mut node_side = node_frame(&draw_state, &mut node_terminal, 65, 71, 1.0, true);
        parity::assert_exact(&mut cairo_side, &mut node_side, "sprites at scale 1");
    }

    /// At the real measured pitch (9x20 in this suite) the eighths and
    /// quadrants have fractional edges: the cairo painter snaps them with
    /// `Antialias::None`, the colour nodes blend. The design states this
    /// tolerance (gsk-render-nodes design, risks): the difference is a
    /// half-coverage edge pixel at most, bounded by half the channel
    /// distance between the sprite colour and the theme background — the
    /// same blend bound as everywhere else. The measured max on this suite
    /// is exactly that bound (the quadrant's half-covered edge pixel).
    #[test]
    fn fractional_pitch_matches_within_tolerance() {
        let _font = crate::render::font_lock::guard();
        let mut draw_state = state();
        let mut cairo_terminal = terminal_with_sprites();
        let mut node_terminal = terminal_with_sprites();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.0);
        let mut node_side = node_frame(&draw_state, &mut node_terminal, 65, 71, 1.0, true);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            parity::BLEND_TOLERANCE,
            "sprites at a fractional pitch",
        );
    }

    /// At scale 1.5 the rects' device edges can land on half-pixels: the
    /// cairo painter snaps them (`Antialias::None`), the colour nodes blend,
    /// and for the small features this geometry produces — 2 px braille
    /// dots, 1 px eighth blocks — the snapped and blended coverages of one
    /// pixel can differ by up to the full channel distance between the
    /// sprite colour and the backdrop. The stated tolerance is that full
    /// distance; the measured max on this suite is 191 (a braille dot edge).
    /// The meaningful pixel-level gate is the scale-1 exactness above; the
    /// visible result at fractional scales is checked on the renderers
    /// (gsk-render-nodes task 5.1).
    #[test]
    fn sprites_match_the_cairo_painter_within_tolerance_at_scale_1_5() {
        const SPRITE_SCALE_1_5_TOLERANCE: u8 = 255 - parity::BACKDROP[0];
        let _font = crate::render::font_lock::guard();
        let mut draw_state = state_with_8x16_metrics();
        let mut cairo_terminal = terminal_with_sprites();
        let mut node_terminal = terminal_with_sprites();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.5);
        let mut node_side = node_frame(&draw_state, &mut node_terminal, 65, 71, 1.5, true);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            SPRITE_SCALE_1_5_TOLERANCE,
            "sprites at scale 1.5",
        );
    }
}
