//! The sprite pass (port-to-rust D3): block elements, braille patterns and
//! powerline separators drawn as graphics instead of font glyphs, because the
//! font's glyphs for them do not line up between cells and leave seams across
//! a row. Ghostty draws the same ranges from its own sprites
//! (`src/font/sprite/draw/{block,braille,powerline}.zig` in the pinned
//! commit); the geometry is a port of those sprites and of `draw_sprite` and
//! `fill_rect` in `src/render.c` (design D5 there).

use crate::term::cells::{Cell, Wide};

use super::metrics::CellMetrics;
use super::snap::OutputScale;

/// Unantialiased, like the cell backgrounds: at a fractional output scale the
/// edges of adjacent same-colour blocks fall between device pixels, and two
/// antialiased partial-coverage edges composite to a faint seam instead of a
/// solid fill. Without antialiasing they snap to device pixels and tile
/// exactly (`fill_rect`). Shared with the cairo painter's decoration and
/// cursor fills, which fill the same snapped bands instead of stroking them
/// (`snap-grid-edges` D6).
pub(crate) fn fill_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let saved = cr.antialias();
    cr.set_antialias(cairo::Antialias::None);
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();
    cr.set_antialias(saved);
}

/// The braille dot geometry for one cell: the dot size and the left/top
/// offset of each of the 2x4 dot columns and rows. Leftover pixels go to
/// spacing and margins before the dots grow, so the pattern fills the cell
/// like Ghostty's. Factored out of `draw_sprite` so tests can pin it.
fn braille_geometry(cell_w: i32, cell_h: i32) -> (i32, [i32; 2], [i32; 4]) {
    let mut dot = (cell_w / 4).min(cell_h / 8);
    let mut x_spacing = cell_w / 4;
    let mut y_spacing = cell_h / 8;
    let mut x_margin = x_spacing / 2;
    let mut y_margin = y_spacing / 2;
    let mut x_left = cell_w - 2 * x_margin - x_spacing - 2 * dot;
    let mut y_left = cell_h - 2 * y_margin - 3 * y_spacing - 4 * dot;

    if x_left >= 2 && y_left >= 4 && dot == 0 {
        dot += 1;
        x_left -= 2;
        y_left -= 4;
    }
    if x_left >= 2 && x_margin == 0 {
        x_margin = 1;
        x_left -= 2;
    }
    if y_left >= 2 && y_margin == 0 {
        y_margin = 1;
        y_left -= 2;
    }
    if x_left >= 1 {
        x_spacing += 1;
        x_left -= 1;
    }
    if y_left >= 3 {
        y_spacing += 1;
        y_left -= 3;
    }
    if x_left >= 2 {
        x_margin += 1;
        x_left -= 2;
    }
    if y_left >= 2 {
        y_margin += 1;
        y_left -= 2;
    }
    if x_left >= 2 && y_left >= 4 {
        dot += 1;
    }

    let dx = [x_margin, x_margin + dot + x_spacing];
    let dy = [
        y_margin,
        y_margin + dot + y_spacing,
        y_margin + 2 * (dot + y_spacing),
        y_margin + 3 * (dot + y_spacing),
    ];
    (dot, dx, dy)
}

/// Braille dot columns and rows for the eight pattern bits, ported from
/// `draw_sprite`'s `dot_col`/`dot_row` tables.
const DOT_COL: [usize; 8] = [0, 0, 0, 1, 1, 1, 0, 1];
const DOT_ROW: [usize; 8] = [0, 1, 2, 0, 1, 2, 3, 3];

/// Quadrant fill bits for 0x2596..=0x259F: bit0 upper left, bit1 upper right,
/// bit2 lower left, bit3 lower right.
const QUADRANTS: [u8; 10] = [0x4, 0x8, 0x1, 0xD, 0x9, 0x7, 0xB, 0x2, 0x6, 0xE];

/// One sprite's geometry, shared by the cairo painter ([`draw_sprite`]) and
/// the node emitter ([`super::node_sprites`]) so the two cannot drift
/// (gsk-render-nodes task 2.2). Blocks, quadrants and braille dots are
/// integer-ish rectangles; the corner and powerline triangles need a cairo
/// path on both sides (a per-cell cairo node on the node side).
pub(crate) enum SpriteShape {
    /// Axis-aligned rectangles to fill with the sprite colour.
    Rects(Vec<(f64, f64, f64, f64)>),
    /// A closed triangle to fill with the sprite colour.
    FillTriangle([f64; 6]),
    /// An open triangle to stroke with a 2 px line.
    StrokeTriangle([f64; 6]),
    /// Not a sprite: the shades (0x2591–0x2593) and every text codepoint.
    None,
}

/// Snap each rectangle of a sprite to the device pixel grid
/// (`snap-grid-edges` D1, D4): neighbouring cells pass the same shared-edge
/// value, so both compute the same snapped edge and no seam appears.
fn snap_rects(scale: OutputScale, rects: Vec<(f64, f64, f64, f64)>) -> Vec<(f64, f64, f64, f64)> {
    rects
        .into_iter()
        .map(|(x, y, w, h)| scale.snap_rect(x, y, w, h))
        .collect()
}

/// Snap each triangle vertex coordinate (`snap-grid-edges` D5): the
/// straight sides of a corner triangle lie on the cell boundary, and an
/// unsnapped side blends with the neighbour. Each vertex moves by at most
/// half a device pixel.
fn snap_vertices(scale: OutputScale, [ax, ay, bx, by, cx, cy]: [f64; 6]) -> [f64; 6] {
    [
        scale.snap_edge(ax),
        scale.snap_edge(ay),
        scale.snap_edge(bx),
        scale.snap_edge(by),
        scale.snap_edge(cx),
        scale.snap_edge(cy),
    ]
}

/// The geometry [`draw_sprite`] paints `cp` with, or [`SpriteShape::None`]
/// when `cp` is left to the text pass. Pure geometry — no drawing — so both
/// painters consume the same numbers. Rectangles and triangle vertices are
/// snapped to the device pixel grid (`snap-grid-edges` D1, D4, D5).
pub(crate) fn sprite_shape(cell: &Cell, cp: u32, cell_metrics: &CellMetrics) -> SpriteShape {
    let cw = f64::from(cell_metrics.cell_w);
    let ch = f64::from(cell_metrics.cell_h);
    let x = f64::from(cell.x) * cw;
    let y = f64::from(cell.y) * ch;
    let w = if cell.wide == Wide::Wide {
        2.0 * cw
    } else {
        cw
    };

    if (0x25E2..=0x25E5).contains(&cp) {
        // Ghostty draws the four corner triangles as full-cell sprites.
        let points = match cp {
            0x25E2 => [x, y + ch, x + w, y + ch, x + w, y],
            0x25E3 => [x, y, x, y + ch, x + w, y + ch],
            0x25E4 => [x, y, x, y + ch, x + w, y],
            0x25E5 => [x, y, x + w, y + ch, x + w, y],
            _ => return SpriteShape::None,
        };
        return SpriteShape::FillTriangle(snap_vertices(cell_metrics.scale, points));
    }

    if (0x2580..=0x259F).contains(&cp) {
        let rects: Vec<(f64, f64, f64, f64)> = match cp {
            // Upper half.
            0x2580 => vec![(x, y, w, ch / 2.0)],
            // Lower 1/8..8/8.
            0x2581..=0x2588 => {
                let h = ch * f64::from(cp - 0x2580) / 8.0;
                vec![(x, y + ch - h, w, h)]
            }
            // Left 7/8..1/8.
            0x2589..=0x258F => vec![(x, y, w * f64::from(0x2590 - cp) / 8.0, ch)],
            // Right half.
            0x2590 => vec![(x + w / 2.0, y, w / 2.0, ch)],
            // Shades: the font has them.
            0x2591..=0x2593 => return SpriteShape::None,
            // Upper 1/8: whole pixels, 3 px at our 19 px cell height.
            0x2594 => vec![(x, y, w, f64::from((cell_metrics.cell_h + 7) / 8))],
            // Right 1/8.
            0x2595 => vec![(x + w * 7.0 / 8.0, y, w / 8.0, ch)],
            // Quadrants.
            _ => {
                let q = QUADRANTS[(cp - 0x2596) as usize];
                let mut rects = Vec::with_capacity(4);
                if q & 0x1 != 0 {
                    rects.push((x, y, w / 2.0, ch / 2.0));
                }
                if q & 0x2 != 0 {
                    rects.push((x + w / 2.0, y, w / 2.0, ch / 2.0));
                }
                if q & 0x4 != 0 {
                    rects.push((x, y + ch / 2.0, w / 2.0, ch / 2.0));
                }
                if q & 0x8 != 0 {
                    rects.push((x + w / 2.0, y + ch / 2.0, w / 2.0, ch / 2.0));
                }
                rects
            }
        };
        return SpriteShape::Rects(snap_rects(cell_metrics.scale, rects));
    }

    if (0x2800..=0x28FF).contains(&cp) {
        // Braille: a 2x4 dot grid.
        let pattern = cp & 0xFF;
        let (dot, dx, dy) = braille_geometry(cell_metrics.cell_w, cell_metrics.cell_h);
        let mut rects = Vec::new();
        for (i, &col) in DOT_COL.iter().enumerate() {
            if pattern & (1u32 << i) != 0 {
                rects.push((
                    x + f64::from(dx[col]),
                    y + f64::from(dy[DOT_ROW[i]]),
                    f64::from(dot),
                    f64::from(dot),
                ));
            }
        }
        return SpriteShape::Rects(snap_rects(cell_metrics.scale, rects));
    }

    if cp == 0xE0B0 || cp == 0xE0B1 || cp == 0xE0B2 || cp == 0xE0B3 {
        // Powerline separators: solid right/left triangle, or its outline.
        let ax = if cp == 0xE0B0 || cp == 0xE0B1 {
            x
        } else {
            x + w
        };
        let bx = if cp == 0xE0B0 || cp == 0xE0B1 {
            x + w
        } else {
            x
        };
        let points = [ax, y, bx, y + ch / 2.0, ax, y + ch];
        return if cp == 0xE0B0 || cp == 0xE0B2 {
            SpriteShape::FillTriangle(snap_vertices(cell_metrics.scale, points))
        } else {
            SpriteShape::StrokeTriangle(snap_vertices(cell_metrics.scale, points))
        };
    }

    SpriteShape::None
}

/// Draw the sprite for `cp` in `cell` with the already-set source colour.
/// Returns true when `cp` was drawn (`draw_sprite`). The geometry is
/// [`sprite_shape`]'s, shared with the node emitter.
pub(crate) fn draw_sprite(
    cr: &cairo::Context,
    cell: &Cell,
    cp: u32,
    cell_metrics: &CellMetrics,
) -> bool {
    match sprite_shape(cell, cp, cell_metrics) {
        SpriteShape::None => false,
        SpriteShape::Rects(rects) => {
            for (x, y, w, h) in rects {
                fill_rect(cr, x, y, w, h);
            }
            true
        }
        SpriteShape::FillTriangle([ax, ay, bx, by, cx, cy]) => {
            cr.move_to(ax, ay);
            cr.line_to(bx, by);
            cr.line_to(cx, cy);
            cr.close_path();
            let _ = cr.fill();
            true
        }
        SpriteShape::StrokeTriangle([ax, ay, bx, by, cx, cy]) => {
            cr.move_to(ax, ay);
            cr.line_to(bx, by);
            cr.line_to(cx, cy);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::parity;

    use gtk4::gdk;
    use gtk4::prelude::SnapshotExt as _;

    fn metrics() -> CellMetrics {
        CellMetrics {
            cell_w: 8,
            cell_h: 16,
            ..CellMetrics::default()
        }
    }

    fn cell(x: i32, y: i32) -> Cell {
        Cell {
            x,
            y,
            ..Cell::default()
        }
    }

    /// The two painters the sprite tests run over (gsk-render-nodes task
    /// 2.2): the cairo painter and the node emitter through the parity
    /// harness, so every sprite assertion holds for both.
    #[derive(Clone, Copy, Debug)]
    enum Painter {
        Cairo,
        Nodes,
    }

    const PAINTERS: [Painter; 2] = [Painter::Cairo, Painter::Nodes];

    /// The white source both painters draw the test sprites with.
    fn node_colour() -> gdk::RGBA {
        gdk::RGBA::new(1.0, 1.0, 1.0, 1.0)
    }

    /// The filled (non-zero-alpha) pixel count of a sprite drawn at 0,0 on a
    /// fresh surface with a white source, on `painter`.
    fn filled_pixels(painter: Painter, cp: u32) -> usize {
        match painter {
            Painter::Cairo => {
                let mut surface =
                    cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
                {
                    let cr = cairo::Context::new(&surface).unwrap();
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    assert!(draw_sprite(&cr, &cell(0, 0), cp, &metrics()));
                }
                surface.flush();
                count_filled(&mut surface)
            }
            Painter::Nodes => {
                let snapshot = parity::snapshot();
                assert!(
                    super::super::node_sprites::emit_cell_sprite(
                        &snapshot,
                        &cell(0, 0),
                        cp,
                        &metrics(),
                        &node_colour()
                    ),
                    "0x{cp:04X} was emitted"
                );
                let node = snapshot.to_node().expect("snapshot produced a node");
                let mut surface =
                    cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
                parity::draw_node(&node, &surface);
                count_filled(&mut surface)
            }
        }
    }

    /// Whether `cp` is painted as a sprite on `painter` (the shades and text
    /// codepoints are not).
    fn paints(painter: Painter, cp: u32) -> bool {
        match painter {
            Painter::Cairo => {
                let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
                let cr = cairo::Context::new(&surface).unwrap();
                draw_sprite(&cr, &cell(0, 0), cp, &metrics())
            }
            Painter::Nodes => {
                let snapshot = parity::snapshot();
                super::super::node_sprites::emit_cell_sprite(
                    &snapshot,
                    &cell(0, 0),
                    cp,
                    &metrics(),
                    &node_colour(),
                )
            }
        }
    }

    /// Non-zero-alpha pixel count of a finished ARGB32 surface.
    fn count_filled(surface: &mut cairo::ImageSurface) -> usize {
        surface.flush();
        surface
            .data()
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count()
    }

    fn assert_filled(painter: Painter, cp: u32, expected: usize) {
        assert_eq!(
            filled_pixels(painter, cp),
            expected,
            "{painter:?} 0x{cp:04X}"
        );
    }

    #[test]
    fn upper_half_block_fills_the_top_half() {
        for painter in PAINTERS {
            assert_filled(painter, 0x2580, 8 * 8);
        }
    }

    #[test]
    fn lower_full_block_fills_the_whole_cell() {
        for painter in PAINTERS {
            assert_filled(painter, 0x2588, 8 * 16);
        }
    }

    #[test]
    fn left_one_eighth_fills_one_column() {
        for painter in PAINTERS {
            // 0x258F is left 1/8: one of the eight columns.
            assert_filled(painter, 0x258F, 16);
        }
    }

    #[test]
    fn right_one_eighth_fills_one_column() {
        for painter in PAINTERS {
            assert_filled(painter, 0x2595, 16);
        }
    }

    #[test]
    fn upper_one_eighth_fills_whole_pixel_rows() {
        for painter in PAINTERS {
            // (16 + 7) / 8 = 2 whole pixel rows at a 16px cell height.
            assert_filled(painter, 0x2594, 8 * 2);
        }
    }

    #[test]
    fn quadrants_fill_their_named_quarters() {
        for painter in PAINTERS {
            // 0x2596 is the lower-left quadrant (bit2).
            assert_filled(painter, 0x2596, 4 * 8);
            // 0x2599 is lower-left + upper-left (bits 0x5? table says 0x9:
            // upper right + lower left ... the table's own layout).
            assert_filled(painter, 0x259F, 4 * 8 * 3);
        }
    }

    #[test]
    fn shades_are_left_to_the_font() {
        for painter in PAINTERS {
            assert!(!paints(painter, 0x2591), "{painter:?} draws no shade");
        }
    }

    #[test]
    fn braille_tiles_the_cell_without_overflow() {
        let (dot, dx, dy) = braille_geometry(8, 16);
        assert!(dot >= 1, "an 8x16 cell fits at least 1px dots");
        // The rightmost dot column and the bottom dot row stay inside.
        assert!(dx[1] + dot <= 8, "dx {dx:?} dot {dot}");
        assert!(dy[3] + dot <= 16, "dy {dy:?} dot {dot}");
    }

    #[test]
    fn braille_dot_grid_lands_on_the_eight_positions() {
        for painter in PAINTERS {
            // All eight dots set.
            assert_filled(painter, 0x28FF, 8 * dot_count(8, 16));
        }
    }

    fn dot_count(cell_w: i32, cell_h: i32) -> usize {
        let (dot, _, _) = braille_geometry(cell_w, cell_h);
        (dot * dot) as usize
    }

    #[test]
    fn powerline_right_triangle_fills_half_the_cell() {
        for painter in PAINTERS {
            // 0xE0B0: a right-pointing solid triangle filling the cell.
            let filled = filled_pixels(painter, 0xE0B0);
            // A solid triangle through the full cell: roughly the half-box,
            // plus or minus the antialiased edge pixels.
            assert!(filled > 8 * 16 / 4, "{painter:?} filled {filled}");
            assert!(filled < 8 * 8 + 16, "{painter:?} filled {filled}");
        }
    }

    /// The two painters agree everywhere the sprite ranges and their
    /// neighbours live — both route every cell through [`sprite_shape`], so
    /// a drift would show here — and known codepoints keep their routing:
    /// blocks are sprites, the shades and text stay text (gsk-render-nodes
    /// task 2.2).
    #[test]
    fn both_painters_agree_across_the_ranges() {
        let mut checked = 0usize;
        let mut sweep = |cps: std::ops::RangeInclusive<u32>| {
            for cp in cps {
                assert_eq!(
                    paints(Painter::Cairo, cp),
                    paints(Painter::Nodes, cp),
                    "0x{cp:04X}"
                );
                checked += 1;
            }
        };
        // Box drawing and neighbours: blocks, shades, quadrants, triangles.
        sweep(0x2570..=0x2600);
        // Braille and its neighbours.
        sweep(0x2800..=0x2900);
        // Powerline separators and neighbours.
        sweep(0xE0B0..=0xE0C0);
        checked += 4;
        assert!(!paints(Painter::Cairo, u32::from('A')));
        assert!(!paints(Painter::Nodes, u32::from('A')));
        assert!(!paints(Painter::Cairo, 0x2630));
        assert!(!paints(Painter::Nodes, 0x2630));
        assert!(!paints(Painter::Cairo, 0x6F22));
        assert!(!paints(Painter::Nodes, 0x6F22));
        assert!(!paints(Painter::Cairo, 0x1F600));
        assert!(!paints(Painter::Nodes, 0x1F600));
        assert!(paints(Painter::Cairo, 0x2588), "a block is a sprite");
        assert!(paints(Painter::Nodes, 0x2588), "a block is a sprite");
        assert!(checked > 420, "sweep covered {checked} codepoints");
    }
}
