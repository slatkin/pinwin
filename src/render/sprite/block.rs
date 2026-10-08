//! The block-element sprites of the canvas painter: the full,
//! half and eighth blocks and the quadrants of U+2580–U+259F drawn as
//! unantialiased rectangles, with Ghostty's
//! `draw/block.zig` integer arithmetic on the cell's snapped device box, so
//! a bar of one fraction has the same thickness whichever edge it anchors
//! to and two blocks that share an edge share that device edge.
//!
//! The shades (U+2591–U+2593) are not sprites: this pass leaves them to
//! the font — the text pass draws them.
//!
//! GTK-free like the rest of the painter (`replace-gtk-with-wayland` D10).

use super::super::geom::{PainterMetrics, device_px};
use crate::term::cells::{Cell, Wide};

/// Quadrant fill bits for U+2596–U+259F, ported from `draw_sprite`'s table:
/// bit0 upper left, bit1 upper right, bit2 lower left, bit3 lower right.
const QUADRANTS: [u8; 10] = [0x4, 0x8, 0x1, 0xD, 0x9, 0x7, 0xB, 0x2, 0x6, 0xE];

/// The device rectangles of the block element `cp` in `cell`, or `None`
/// when `cp` is not a block sprite (the shades and every code point outside
/// the block range — the caller's dispatch sends those on). Pure geometry,
/// no drawing.
pub(super) fn rects(cp: u32, metrics: &PainterMetrics, cell: &Cell) -> Option<Vec<DeviceRect>> {
    if !(0x2580..=0x259F).contains(&cp) {
        return None;
    }
    if (0x2591..=0x2593).contains(&cp) {
        return None; // the shades stay text
    }
    let (x0, y0, x1, y1) = cell_box(metrics, cell);
    let dw = x1 - x0;
    let dh = y1 - y0;
    // `blockShade`'s bar: `round(size * fraction)` device pixels, and at
    // least one so a thin bar never vanishes (as `snap_rect` guarantees).
    let bar = |size: f64, fraction: f64| (fraction * size).round().max(1.0).min(size);

    // Device edges (left, top, right, bottom), as `block_rects` computes
    // them.
    let edges: Vec<(f64, f64, f64, f64)> = match cp {
        // Upper half, upper 1/8.
        0x2580 | 0x2594 => {
            let fh = if cp == 0x2580 { 0.5 } else { 0.125 };
            vec![(x0, y0, x1, y0 + bar(dh, fh))]
        }
        // Lower 1/8..8/8 (8/8 is the whole box).
        0x2581..=0x2588 => {
            let fh = f64::from(cp - 0x2580) / 8.0;
            vec![(x0, y1 - bar(dh, fh), x1, y1)]
        }
        // Left 7/8..1/8.
        0x2589..=0x258F => vec![(x0, y0, x0 + bar(dw, f64::from(0x2590 - cp) / 8.0), y1)],
        // Right half, right 1/8.
        0x2590 | 0x2595 => {
            let fw = if cp == 0x2590 { 0.5 } else { 0.125 };
            vec![(x1 - bar(dw, fw), y0, x1, y1)]
        }
        // Quadrants: `fill` with the half line, the first half's max edge
        // and the second half's min edge.
        _ => {
            let index = usize::try_from(cp - 0x2596).expect("the range check bounds the index");
            let bits = QUADRANTS[index];
            let (x_max, x_min) = (x0 + fraction_max(dw, 0.5), x0 + fraction_min(dw, 0.5));
            let (y_max, y_min) = (y0 + fraction_max(dh, 0.5), y0 + fraction_min(dh, 0.5));
            let mut edges = Vec::with_capacity(4);
            if bits & 0x1 != 0 {
                edges.push((x0, y0, x_max, y_max));
            }
            if bits & 0x2 != 0 {
                edges.push((x_min, y0, x1, y_max));
            }
            if bits & 0x4 != 0 {
                edges.push((x0, y_min, x_max, y1));
            }
            if bits & 0x8 != 0 {
                edges.push((x_min, y_min, x1, y1));
            }
            edges
        }
    };
    Some(
        edges
            .into_iter()
            .map(|(ax, ay, bx, by)| {
                DeviceRect::new(
                    device_px(ax),
                    device_px(ay),
                    device_px(bx - ax),
                    device_px(by - ay),
                )
                .expect("the block edges are ordered and non-negative")
            })
            .collect(),
    )
}

use super::super::geom::DeviceRect;

/// The snapped device box of `cell`: one column wide, or two for a wide
/// glyph's head. The edges snap individually, so neighbouring cells' boxes share
/// their device edges and blocks cannot leave a seam.
fn cell_box(metrics: &PainterMetrics, cell: &Cell) -> (f64, f64, f64, f64) {
    let width = if cell.wide == Wide::Wide {
        2.0 * metrics.cell_w()
    } else {
        metrics.cell_w()
    };
    let rect = metrics.cell_sub_rect(cell.x, cell.y, 0.0, 0.0, width, metrics.cell_h());
    (
        f64::from(rect.x()),
        f64::from(rect.y()),
        f64::from(rect.x() + rect.w()),
        f64::from(rect.y() + rect.h()),
    )
}

/// Ghostty's `Fraction.min` (`draw/common.zig`) on a size in whole device
/// pixels: the min edge of a section, taken as the complement of the max
/// edge so that rounding evens out. Ported from ghostty's `draw/common.zig`.
fn fraction_min(size: f64, fraction: f64) -> f64 {
    size - ((1.0 - fraction) * size).round()
}

/// Ghostty's `Fraction.max` on a size in whole device pixels.
fn fraction_max(size: f64, fraction: f64) -> f64 {
    (fraction * size).round()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::cells::CELL_TEXT_CAP;

    /// The 8×16 pitch the render tests use.
    fn metrics(scale: f64) -> PainterMetrics {
        PainterMetrics::new(8.0, 16.0, 12.0, scale).expect("test metrics are valid")
    }

    /// A narrow cell at `(x, y)` holding `cp`.
    fn cell_at(x: i32, y: i32, cp: u32) -> Cell {
        let text = char::from_u32(cp).expect("a block code point").to_string();
        let len = text.len().min(CELL_TEXT_CAP - 1);
        let mut bytes = [0u8; CELL_TEXT_CAP];
        bytes[..len].copy_from_slice(&text.as_bytes()[..len]);
        Cell {
            x,
            y,
            len,
            text: bytes,
            ..Cell::default()
        }
    }

    /// A wide head cell at `(x, y)` holding `cp`.
    fn wide_cell_at(x: i32, y: i32, cp: u32) -> Cell {
        Cell {
            wide: Wide::Wide,
            ..cell_at(x, y, cp)
        }
    }

    /// The one rectangle `cp` draws in its cell, as device edges.
    fn one_rect(cp: u32, m: &PainterMetrics, cell: &Cell) -> DeviceRect {
        let rects = rects(cp, m, cell).expect("the code point is a block sprite");
        assert_eq!(rects.len(), 1, "0x{cp:04X} draws one rectangle");
        rects[0]
    }

    /// The shades and the code points outside the block range are not
    /// block sprites.
    #[test]
    fn shades_and_outsiders_are_not_block_sprites() {
        let m = metrics(1.0);
        let cell = cell_at(0, 0, 0x2588);
        for cp in [
            0x257F,
            0x2591,
            0x2592,
            0x2593,
            0x25A0,
            0x2800,
            u32::from('A'),
            0,
        ] {
            assert!(rects(cp, &m, &cell).is_none(), "0x{cp:04X} is not a block");
        }
        for cp in [0x2580, 0x2588, 0x258F, 0x2590, 0x2595, 0x2596, 0x259F] {
            assert!(rects(cp, &m, &cell).is_some(), "0x{cp:04X} is a block");
        }
    }

    /// At scale 1 the bars land on the obvious halves and eighths of the
    /// 8×16 cell.
    #[test]
    fn at_scale_1_the_bars_land_on_the_fractions() {
        let m = metrics(1.0);
        let cell = cell_at(0, 0, 0x2580);
        assert_eq!(
            one_rect(0x2580, &m, &cell),
            DeviceRect::new(0, 0, 8, 8).expect("valid")
        );
        assert_eq!(
            one_rect(0x2584, &m, &cell),
            DeviceRect::new(0, 8, 8, 8).expect("valid")
        );
        assert_eq!(
            one_rect(0x2588, &m, &cell),
            DeviceRect::new(0, 0, 8, 16).expect("valid")
        );
        // Lower 1/8: round(16/8) = 2 rows at the bottom.
        assert_eq!(
            one_rect(0x2581, &m, &cell),
            DeviceRect::new(0, 14, 8, 2).expect("valid")
        );
        // Upper 1/8: the same 2 rows at the top.
        assert_eq!(
            one_rect(0x2594, &m, &cell),
            DeviceRect::new(0, 0, 8, 2).expect("valid")
        );
        // Left 1/8 and right 1/8: one of the eight columns each.
        assert_eq!(
            one_rect(0x258F, &m, &cell),
            DeviceRect::new(0, 0, 1, 16).expect("valid")
        );
        assert_eq!(
            one_rect(0x2595, &m, &cell),
            DeviceRect::new(7, 0, 1, 16).expect("valid")
        );
        // Left and right half.
        assert_eq!(
            one_rect(0x258C, &m, &cell),
            DeviceRect::new(0, 0, 4, 16).expect("valid")
        );
        assert_eq!(
            one_rect(0x2590, &m, &cell),
            DeviceRect::new(4, 0, 4, 16).expect("valid")
        );
    }

    /// A bar of one fraction has the same device thickness whichever edge
    /// it is anchored to, at the fractional scales the painter tests: the
    /// lower and upper eighth agree, and the left and right eighth agree.
    #[test]
    fn a_bar_has_the_same_thickness_at_either_anchor() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(scale);
            for col in 0..4 {
                let cell = cell_at(col, 0, 0x2581);
                let lower = one_rect(0x2581, &m, &cell);
                let upper = one_rect(0x2594, &m, &cell);
                assert_eq!(
                    lower.h(),
                    upper.h(),
                    "the lower and upper eighth match at scale {scale} col {col}"
                );
                let left = one_rect(0x258F, &m, &cell);
                let right = one_rect(0x2595, &m, &cell);
                assert_eq!(
                    left.w(),
                    right.w(),
                    "the left and right eighth match at scale {scale} col {col}"
                );
                // The lower bar sits on the cell's bottom edge, the upper on
                // its top edge.
                let cell_rect = m.cell_rect(col, 0);
                assert_eq!(lower.y() + lower.h(), cell_rect.y() + cell_rect.h());
                assert_eq!(upper.y(), cell_rect.y());
            }
        }
    }

    /// A wide head cell's block spans both of its columns; the box edges
    /// are the snapped device edges of the two-cell span.
    #[test]
    fn a_wide_cells_block_spans_both_columns() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(scale);
            let wide = wide_cell_at(0, 0, 0x2588);
            let rect = one_rect(0x2588, &m, &wide);
            let span = m.cell_sub_rect(0, 0, 0.0, 0.0, 16.0, 16.0);
            assert_eq!((rect.x(), rect.w()), (span.x(), span.w()));
            assert_eq!((rect.y(), rect.h()), (span.y(), span.h()));
            // A narrow cell in the same spot stays one column wide.
            let narrow = one_rect(0x2588, &m, &cell_at(0, 0, 0x2588));
            assert!(narrow.w() < rect.w(), "the wide block is wider");
        }
    }

    /// The quadrant table fills exactly the named quarters: at scale 1 the
    /// four single-quadrant code points each fill one 4×8 quarter, and the
    /// three-quarter code point fills three.
    #[test]
    fn quadrants_fill_their_named_quarters_at_scale_1() {
        let m = metrics(1.0);
        let cell = cell_at(0, 0, 0x2596);
        // 0x2596 is the lower-left quadrant.
        assert_eq!(
            one_rect(0x2596, &m, &cell),
            DeviceRect::new(0, 8, 4, 8).expect("valid")
        );
        // 0x259F fills three quarters (upper right, lower left, lower
        // right): three rectangles.
        let quads = rects(0x259F, &m, &cell).expect("a quadrant");
        assert_eq!(quads.len(), 3);
        let area: i32 = quads.iter().map(|r| r.w() * r.h()).sum();
        assert_eq!(area, 3 * 4 * 8);
        // The two-quadrant code point fills its two quarters.
        let quads = rects(0x259E, &m, &cell).expect("a quadrant");
        assert_eq!(quads.len(), 2, "0x259E is lower left + lower right");
        let area: i32 = quads.iter().map(|r| r.w() * r.h()).sum();
        assert_eq!(area, 2 * 4 * 8);
        // The four single-quadrant code points each cover one quarter.
        for (cp, (x, y)) in [
            (0x2598, (0, 0)), // upper left
            (0x259D, (4, 0)), // upper right
            (0x2596, (0, 8)), // lower left
            (0x2597, (4, 8)), // lower right
        ] {
            let rect = one_rect(cp, &m, &cell);
            assert_eq!(
                (rect.x(), rect.y(), rect.w(), rect.h()),
                (x, y, 4, 8),
                "0x{cp:04X}"
            );
        }
    }

    /// Every quadrant code point's rectangles stay inside the cell box and
    /// tile the quarters they name, at every scale the painter tests.
    #[test]
    fn quadrant_rects_stay_inside_the_cell_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(scale);
            let cell = cell_at(0, 0, 0x2596);
            let box_rect = m.cell_rect(0, 0);
            for cp in 0x2596..=0x259F {
                for rect in rects(cp, &m, &cell).expect("a quadrant") {
                    assert!(rect.x() >= box_rect.x() && rect.y() >= box_rect.y());
                    assert!(
                        rect.x() + rect.w() <= box_rect.x() + box_rect.w()
                            && rect.y() + rect.h() <= box_rect.y() + box_rect.h(),
                        "0x{cp:04X} stays inside at scale {scale}"
                    );
                    assert!(rect.w() > 0 && rect.h() > 0);
                }
            }
        }
    }
}
