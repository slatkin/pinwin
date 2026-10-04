//! The sprite pass (port-to-rust D3): block elements, braille patterns and
//! powerline separators drawn as graphics instead of font glyphs, because the
//! font's glyphs for them do not line up between cells and leave seams across
//! a row. Ghostty draws the same ranges from its own sprites
//! (`src/font/sprite/draw/{block,braille,powerline}.zig` in the pinned
//! commit); the geometry is a port of those sprites and of `draw_sprite` and
//! `fill_rect` in `src/render.c` (design D5 there).

use crate::term::cells::{Cell, Wide};

use super::metrics::CellMetrics;

/// Unantialiased, like the cell backgrounds: at a fractional output scale the
/// edges of adjacent same-colour blocks fall between device pixels, and two
/// antialiased partial-coverage edges composite to a faint seam instead of a
/// solid fill. Without antialiasing they snap to device pixels and tile
/// exactly (`fill_rect`).
fn fill_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
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
        // The C decrements its leftovers here too; they are dead afterwards.
        #[allow(unused_assignments)]
        {
            x_left -= 2;
            y_left -= 4;
        }
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

/// Draw the sprite for `cp` in `cell` with the already-set source colour.
/// Returns true when `cp` was drawn (`draw_sprite`).
pub(crate) fn draw_sprite(
    cr: &cairo::Context,
    cell: &Cell,
    cp: u32,
    cell_metrics: &CellMetrics,
) -> bool {
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
        match cp {
            0x25E2 => {
                cr.move_to(x, y + ch);
                cr.line_to(x + w, y + ch);
                cr.line_to(x + w, y);
            }
            0x25E3 => {
                cr.move_to(x, y);
                cr.line_to(x, y + ch);
                cr.line_to(x + w, y + ch);
            }
            0x25E4 => {
                cr.move_to(x, y);
                cr.line_to(x, y + ch);
                cr.line_to(x + w, y);
            }
            0x25E5 => {
                cr.move_to(x, y);
                cr.line_to(x + w, y + ch);
                cr.line_to(x + w, y);
            }
            _ => {}
        }
        cr.close_path();
        let _ = cr.fill();
        return true;
    }

    if (0x2580..=0x259F).contains(&cp) {
        match cp {
            // Upper half.
            0x2580 => {
                fill_rect(cr, x, y, w, ch / 2.0);
                return true;
            }
            // Lower 1/8..8/8.
            0x2581..=0x2588 => {
                let h = ch * f64::from(cp - 0x2580) / 8.0;
                fill_rect(cr, x, y + ch - h, w, h);
                return true;
            }
            // Left 7/8..1/8.
            0x2589..=0x258F => {
                fill_rect(cr, x, y, w * f64::from(0x2590 - cp) / 8.0, ch);
                return true;
            }
            // Right half.
            0x2590 => {
                fill_rect(cr, x + w / 2.0, y, w / 2.0, ch);
                return true;
            }
            // Shades: the font has them.
            0x2591..=0x2593 => return false,
            // Upper 1/8: whole pixels, 3 px at our 19 px cell height.
            0x2594 => {
                fill_rect(cr, x, y, w, f64::from((cell_metrics.cell_h + 7) / 8));
                return true;
            }
            // Right 1/8.
            0x2595 => {
                fill_rect(cr, x + w * 7.0 / 8.0, y, w / 8.0, ch);
                return true;
            }
            // Quadrants.
            _ => {
                let q = QUADRANTS[(cp - 0x2596) as usize];
                if q & 0x1 != 0 {
                    fill_rect(cr, x, y, w / 2.0, ch / 2.0);
                }
                if q & 0x2 != 0 {
                    fill_rect(cr, x + w / 2.0, y, w / 2.0, ch / 2.0);
                }
                if q & 0x4 != 0 {
                    fill_rect(cr, x, y + ch / 2.0, w / 2.0, ch / 2.0);
                }
                if q & 0x8 != 0 {
                    fill_rect(cr, x + w / 2.0, y + ch / 2.0, w / 2.0, ch / 2.0);
                }
                return true;
            }
        }
    }

    if (0x2800..=0x28FF).contains(&cp) {
        // Braille: a 2x4 dot grid.
        let pattern = cp & 0xFF;
        let (dot, dx, dy) = braille_geometry(cell_metrics.cell_w, cell_metrics.cell_h);
        for (i, &col) in DOT_COL.iter().enumerate() {
            if pattern & (1u32 << i) != 0 {
                fill_rect(
                    cr,
                    x + f64::from(dx[col]),
                    y + f64::from(dy[DOT_ROW[i]]),
                    f64::from(dot),
                    f64::from(dot),
                );
            }
        }
        return true;
    }

    if cp == 0xE0B0 || cp == 0xE0B1 || cp == 0xE0B2 || cp == 0xE0B3 {
        // Powerline separators: solid right/left triangle, or its outline.
        let solid = cp == 0xE0B0 || cp == 0xE0B2;
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
        cr.move_to(ax, y);
        cr.line_to(bx, y + ch / 2.0);
        cr.line_to(ax, y + ch);
        if solid {
            cr.close_path();
            let _ = cr.fill();
        } else {
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The filled (non-zero-alpha) pixel count of a sprite drawn at 0,0 on a
    /// fresh surface, using a white source.
    fn filled_pixels(cp: u32) -> usize {
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            assert!(draw_sprite(&cr, &cell(0, 0), cp, &metrics()));
        }
        surface.flush();
        let data = surface.data().unwrap();
        data.as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count()
    }

    #[test]
    fn upper_half_block_fills_the_top_half() {
        assert_eq!(filled_pixels(0x2580), 8 * 8);
    }

    #[test]
    fn lower_full_block_fills_the_whole_cell() {
        assert_eq!(filled_pixels(0x2588), 8 * 16);
    }

    #[test]
    fn left_one_eighth_fills_one_column() {
        // 0x258F is left 1/8: one of the eight columns.
        assert_eq!(filled_pixels(0x258F), 16);
    }

    #[test]
    fn right_one_eighth_fills_one_column() {
        assert_eq!(filled_pixels(0x2595), 16);
    }

    #[test]
    fn upper_one_eighth_fills_whole_pixel_rows() {
        // (16 + 7) / 8 = 2 whole pixel rows at a 16px cell height.
        assert_eq!(filled_pixels(0x2594), 8 * 2);
    }

    #[test]
    fn quadrants_fill_their_named_quarters() {
        // 0x2596 is the lower-left quadrant (bit2).
        assert_eq!(filled_pixels(0x2596), 4 * 8);
        // 0x2599 is lower-left + upper-left (bits 0x5? table says 0x9:
        // upper right + lower left ... the table's own layout).
        assert_eq!(filled_pixels(0x259F), 4 * 8 * 3);
    }

    #[test]
    fn shades_are_left_to_the_font() {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        assert!(!draw_sprite(&cr, &cell(0, 0), 0x2591, &metrics()));
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
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            // All eight dots set.
            assert!(draw_sprite(&cr, &cell(0, 0), 0x28FF, &metrics()));
        }
        surface.flush();
        let data = surface.data().unwrap();
        let filled: usize = data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count();
        assert_eq!(filled, 8 * dot_count(8, 16));
    }

    fn dot_count(cell_w: i32, cell_h: i32) -> usize {
        let (dot, _, _) = braille_geometry(cell_w, cell_h);
        (dot * dot) as usize
    }

    #[test]
    fn powerline_right_triangle_fills_half_the_cell() {
        // 0xE0B0: a right-pointing solid triangle filling the cell.
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            assert!(draw_sprite(&cr, &cell(0, 0), 0xE0B0, &metrics()));
        }
        surface.flush();
        let data = surface.data().unwrap();
        let filled: usize = data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count();
        // A solid triangle through the full cell: roughly the half-box,
        // plus or minus the antialiased edge pixels.
        assert!(filled > 8 * 16 / 4, "filled {filled}");
        assert!(filled < 8 * 8 + 16, "filled {filled}");
    }
}
