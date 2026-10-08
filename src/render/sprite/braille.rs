//! The braille sprites of the canvas painter: U+2800–U+28FF drawn
//! as a 2×4 grid of unantialiased square dots — leftover pixels go to
//! spacing and margins before the dots grow, so the
//! pattern fills the cell like Ghostty's.
//!
//! The geometry runs on the cell's integer logical pitch and each dot
//! snaps to the device pixel grid, so a lit dot lands on the same device
//! pixels every draw.
//!
//! GTK-free like the rest of the painter (`replace-gtk-with-wayland` D10).

use super::super::geom::{DeviceRect, PainterMetrics, logical_px};
use crate::term::cells::Cell;

/// Braille dot columns and rows for the eight pattern bits, ported from
/// `draw_sprite`'s `dot_col`/`dot_row` tables: bit i lights the dot at
/// (`DOT_COL[i]`, `DOT_ROW[i]`).
const DOT_COL: [usize; 8] = [0, 0, 0, 1, 1, 1, 0, 1];
const DOT_ROW: [usize; 8] = [0, 1, 2, 0, 1, 2, 3, 3];

/// The braille dot geometry for one cell: the dot size and the left/top
/// offset of each of the 2 dot columns and 4 dot rows, in logical pixels.
/// Leftover pixels go to spacing and margins before the dots grow, so the
/// pattern fills the cell like Ghostty's.
fn geometry(cell_w: i32, cell_h: i32) -> (i32, [i32; 2], [i32; 4]) {
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

/// The device rectangles of the braille pattern `cp` lights in `cell`, one
/// per lit dot, or `None` when `cp` is outside the braille range. Pure
/// geometry, no drawing; an all-blank pattern (U+2800) lights no dots and
/// still is a sprite — it draws nothing and keeps the text pass off the
/// cell.
pub(super) fn rects(cp: u32, metrics: &PainterMetrics, cell: &Cell) -> Option<Vec<DeviceRect>> {
    if !(0x2800..=0x28FF).contains(&cp) {
        return None;
    }
    let pattern = cp & 0xFF;
    let (dot, dx, dy) = geometry(logical_px(metrics.cell_w()), logical_px(metrics.cell_h()));
    let mut rects = Vec::new();
    for (i, &col) in DOT_COL.iter().enumerate() {
        if pattern & (1u32 << i) != 0 {
            rects.push(metrics.cell_sub_rect(
                cell.x,
                cell.y,
                f64::from(dx[col]),
                f64::from(dy[DOT_ROW[i]]),
                f64::from(dot),
                f64::from(dot),
            ));
        }
    }
    Some(rects)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 8×16 pitch the render tests use.
    fn metrics(scale: f64) -> PainterMetrics {
        PainterMetrics::new(8.0, 16.0, 12.0, scale).expect("test metrics are valid")
    }

    fn cell(x: i32, y: i32) -> Cell {
        Cell {
            x,
            y,
            ..Cell::default()
        }
    }

    /// The dot geometry pins the 8×16 pitch numbers:
    /// 2 px dots on the margins-and-spacing grid.
    #[test]
    fn the_dot_geometry_matches_the_panel_path() {
        assert_eq!(geometry(8, 16), (2, [1, 5], [1, 5, 9, 13]));
    }

    /// A cell the size of the pitch always fits at least one dot, and the
    /// dot grid stays inside the cell.
    #[test]
    fn the_dot_grid_stays_inside_the_cell() {
        for (cell_w, cell_h) in [(8, 16), (9, 20), (10, 22), (7, 13), (4, 8)] {
            let (dot, dx, dy) = geometry(cell_w, cell_h);
            assert!(dot >= 1, "{cell_w}x{cell_h} fits a dot");
            assert!(dx[1] + dot <= cell_w, "dx {dx:?} dot {dot}");
            assert!(dy[3] + dot <= cell_h, "dy {dy:?} dot {dot}");
        }
    }

    /// Code points outside the braille range are not braille sprites.
    #[test]
    fn outsiders_are_not_braille_sprites() {
        let m = metrics(1.0);
        let cell = cell(0, 0);
        for cp in [0x27FF, 0x2900, u32::from('A'), 0] {
            assert!(rects(cp, &m, &cell).is_none(), "0x{cp:04X} is not braille");
        }
        assert!(rects(0x2800, &m, &cell).is_some(), "the blank is a sprite");
    }

    /// The standard braille bit layout, independent of [`DOT_COL`] and
    /// [`DOT_ROW`]: bit i lights the dot at this (column, row) — bits 1–6
    /// fill the upper 2×3 in reading order, bits 7–8 sit below.
    const BIT_DOT: [(usize, usize); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];

    /// Every one of the 256 patterns lights exactly the dots its code
    /// point names, at the geometry the bit layout gives them.
    #[test]
    fn every_pattern_lights_exactly_its_dots() {
        let m = metrics(1.0);
        let cell = cell(0, 0);
        for cp in 0x2800..=0x28FF {
            let pattern = cp & 0xFF;
            let lit: Vec<DeviceRect> = rects(cp, &m, &cell).expect("inside the braille range");
            let expected: Vec<usize> = (0..8).filter(|i| pattern & (1u32 << i) != 0).collect();
            assert_eq!(lit.len(), expected.len(), "0x{cp:04X} lights {expected:?}");
            for (rect, i) in lit.iter().zip(expected) {
                let (col, row) = BIT_DOT[i];
                // The dot at (col, row) sits where the geometry puts it.
                let (dot, dx, dy) = geometry(8, 16);
                let want = m.cell_sub_rect(
                    0,
                    0,
                    f64::from(dx[col]),
                    f64::from(dy[row]),
                    f64::from(dot),
                    f64::from(dot),
                );
                assert_eq!(
                    (rect.x(), rect.y(), rect.w(), rect.h()),
                    (want.x(), want.y(), want.w(), want.h()),
                    "0x{cp:04X} dot {i}"
                );
            }
        }
    }

    /// A lit dot snaps to whole device pixels and carries the full dot
    /// size at 1.5: a 2 px dot is 3 device pixels on each side.
    #[test]
    fn a_dot_snaps_to_whole_device_pixels() {
        let m = metrics(1.5);
        let cell = cell(0, 0);
        let rects = rects(0x2801, &m, &cell).expect("dot 1");
        assert_eq!(rects.len(), 1);
        let rect = rects[0];
        assert_eq!((rect.x(), rect.y(), rect.w(), rect.h()), (2, 2, 3, 3));
        // The dot sits inside the cell's device box, which is 12×24 here.
        let box_rect = m.cell_rect(0, 0);
        assert!(rect.x() + rect.w() <= box_rect.x() + box_rect.w());
        assert!(rect.y() + rect.h() <= box_rect.y() + box_rect.h());
    }

    /// The geometry function is total: no pitch of whole device pixels
    /// makes it produce a negative offset or an out-of-range dot.
    #[test]
    fn the_geometry_stays_total_for_small_pitches() {
        for cell_w in 1..24 {
            for cell_h in 1..40 {
                let (dot, dx, dy) = geometry(cell_w, cell_h);
                assert!(dot >= 0 && dx[0] >= 0 && dy[0] >= 0);
                assert!(dx[1] >= dx[0] && dy[1] >= dy[0]);
                assert!(dy[3] >= dy[2] && dy[2] >= dy[1]);
            }
        }
    }

    /// `device_px` stays the only f64-to-i32 seam here: the geometry takes
    /// the pitch through `logical_px`, which rounds like `device_px` does.
    #[test]
    fn logical_px_rounds_like_device_px() {
        assert_eq!(logical_px(8.0), 8);
        assert_eq!(logical_px(7.999_999_9), 8, "dust rounds off");
        assert_eq!(logical_px(f64::NAN), 0);
    }
}
