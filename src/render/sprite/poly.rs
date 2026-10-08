//! The polygon sprites of the canvas painter: the four corner
//! triangles (U+25E2–U+25E5) and the four powerline separators
//! (U+E0B0–U+E0B3), with Ghostty's full-cell corner
//! triangles and the half-height powerline triangle, a wide glyph's head
//! spanning both of its columns.
//!
//! Every vertex lands on the device lattice: each
//! logical vertex coordinate snaps with `OutputScale::snap_edge`, and the
//! snapped vertex sits at `round(v · scale)`
//! device pixels — the value [`crate::render::geom::device_px`] produces
//! here.
//!
//! The two hollow separators (U+E0B1 and U+E0B3) are design decision 5's
//! "one line sprite" (`replace-gtk-with-wayland` D5): the only sprite
//! stroked instead of filled, stroked as ONE closed triangle at line
//! width 2.0 in logical user space, so the MITER joins fill the corners:
//! the apex tip pokes one miter length into the neighbouring cell and the
//! sharp 45-degree base corners keep their miter spikes. A stroke width is
//! a user-space length, so the device width is 2 logical pixels times the
//! output scale, and this pass carries that device width.
//!
//! GTK-free like the rest of the painter (`replace-gtk-with-wayland` D10).

use super::super::geom::{PainterMetrics, device_px};
use super::Primitive;
use crate::term::cells::{Cell, Wide};

/// The polygon primitive `cp` draws in `cell`, or `None` when `cp` is not
/// one of the eight polygon code points — the caller's dispatch sends the
/// rest on. Pure geometry, no drawing.
pub(super) fn primitive(cp: u32, metrics: &PainterMetrics, cell: &Cell) -> Option<Primitive> {
    let points = points(cp, metrics, cell)?;
    Some(match cp {
        // The hollow separators stroke their closed outline: one closed
        // triangle at 2 logical pixels of line width, whose
        // miter joins fill the corners.
        0xE0B1 | 0xE0B3 => Primitive::StrokePolygon(points, 2.0 * metrics.scale()),
        _ => Primitive::FillPolygon(points),
    })
}

/// The device-space vertices of the triangle `cp` draws in `cell`, in
/// `sprite_shape`'s draw order, or `None` when `cp` is not one of the
/// eight polygon code points. Pure geometry, no drawing.
fn points(cp: u32, metrics: &PainterMetrics, cell: &Cell) -> Option<Vec<(f64, f64)>> {
    let scale = metrics.scale();
    // Each logical vertex snaps with `snap_edge` and sits at
    // `round(v · scale)` device pixels, up to
    // floating-point dust that `device_px` rounds off (D5's cast seam).
    let device = |v: f64| f64::from(device_px(v * scale));
    let cw = metrics.cell_w();
    let ch = metrics.cell_h();
    let x = f64::from(cell.x) * cw;
    let y = f64::from(cell.y) * ch;
    let w = if cell.wide == Wide::Wide {
        2.0 * cw
    } else {
        cw
    };

    let vertices: [(f64, f64); 3] = match cp {
        // The corner triangles are full-cell sprites.
        0x25E2 => [(x, y + ch), (x + w, y + ch), (x + w, y)],
        0x25E3 => [(x, y), (x, y + ch), (x + w, y + ch)],
        0x25E4 => [(x, y), (x, y + ch), (x + w, y)],
        0x25E5 => [(x, y), (x + w, y + ch), (x + w, y)],
        // The powerline separators: solid right/left triangle, or its
        // outline.
        0xE0B0 | 0xE0B1 => [(x, y), (x + w, y + ch / 2.0), (x, y + ch)],
        0xE0B2 | 0xE0B3 => [(x + w, y), (x, y + ch / 2.0), (x + w, y + ch)],
        _ => return None,
    };
    Some(
        vertices
            .iter()
            .map(|(vx, vy)| (device(*vx), device(*vy)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight polygon code points.
    const EIGHT: [u32; 8] = [
        0x25E2, 0x25E3, 0x25E4, 0x25E5, 0xE0B0, 0xE0B1, 0xE0B2, 0xE0B3,
    ];

    /// The 8×16 pitch the render tests use.
    fn metrics(scale: f64) -> PainterMetrics {
        PainterMetrics::new(8.0, 16.0, 12.0, scale).expect("test metrics are valid")
    }

    /// A narrow cell at `(x, y)`; its text is irrelevant to the pure
    /// geometry, only its position and width are.
    fn cell(x: i32, y: i32) -> Cell {
        Cell {
            x,
            y,
            ..Cell::default()
        }
    }

    /// The logical vertices `sprite_shape` builds for `cp` in an 8×16 cell
    /// at `(cx, cy)` — the reference point tables, copied here so the canvas
    /// geometry is checked against them rather than against itself.
    fn gtk_logical_points(cp: u32, cx: i32, cy: i32, wide: bool) -> [(f64, f64); 3] {
        let x = f64::from(cx) * 8.0;
        let y = f64::from(cy) * 16.0;
        let w = if wide { 16.0 } else { 8.0 };
        match cp {
            0x25E2 => [(x, y + 16.0), (x + w, y + 16.0), (x + w, y)],
            0x25E3 => [(x, y), (x, y + 16.0), (x + w, y + 16.0)],
            0x25E4 => [(x, y), (x, y + 16.0), (x + w, y)],
            0x25E5 => [(x, y), (x + w, y + 16.0), (x + w, y)],
            0xE0B0 | 0xE0B1 => [(x, y), (x + w, y + 8.0), (x, y + 16.0)],
            _ => [(x + w, y), (x, y + 8.0), (x + w, y + 16.0)],
        }
    }

    /// A logical vertex's device coordinate: snapped with
    /// `snap_edge` and placed at `round(v · scale)` device pixels.
    fn device(v: f64, scale: f64) -> f64 {
        f64::from(device_px(v * scale))
    }

    /// At every scale the painter tests, the eight code points' vertices
    /// equal the hand-computed snapped vertices, at several cell positions
    /// and for wide heads too.
    #[test]
    fn the_vertices_equal_the_gtk_snapped_vertices() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(scale);
            for (cx, cy) in [(0, 0), (2, 1), (7, 3)] {
                for wide in [false, true] {
                    assert_cells_match(&m, cx, cy, wide, scale);
                }
            }
        }
    }

    /// Assert every polygon code point's vertices for one cell at one
    /// scale; split out of [`the_vertices_equal_the_gtk_snapped_vertices`]
    /// to keep the loop nesting flat.
    fn assert_cells_match(m: &PainterMetrics, cx: i32, cy: i32, wide: bool, scale: f64) {
        let mut cell = cell(cx, cy);
        if wide {
            cell.wide = Wide::Wide;
        }
        for cp in EIGHT {
            let got = points(cp, m, &cell).expect("a polygon code point");
            for (point, (lx, ly)) in got.iter().zip(gtk_logical_points(cp, cx, cy, wide)) {
                assert_eq!(
                    *point,
                    (device(lx, scale), device(ly, scale)),
                    "0x{cp:04X} at scale {scale}, cell ({cx}, {cy}), wide {wide}"
                );
            }
        }
    }

    /// Two pinned vertex sets, computed by hand: at scale 1 the 8×16 cell
    /// is its own device box, and at 1.8 the edges snap to the rounded
    /// device positions (14 and 29), not to the unscaled 14.4 and 28.8.
    #[test]
    fn pinned_vertices_at_two_scales() {
        let m = metrics(1.0);
        assert_eq!(
            points(0xE0B0, &m, &cell(0, 0)).expect("a separator"),
            vec![(0.0, 0.0), (8.0, 8.0), (0.0, 16.0)]
        );
        let m = metrics(1.8);
        assert_eq!(
            points(0x25E2, &m, &cell(1, 0)).expect("a corner"),
            vec![(14.0, 29.0), (29.0, 29.0), (29.0, 0.0)]
        );
    }

    /// Code points outside the eight are not polygon sprites; each of the
    /// eight is.
    #[test]
    fn outsiders_are_not_polygon_sprites() {
        let m = metrics(1.0);
        let cell = cell(0, 0);
        for cp in [
            0x25E1,
            0x25E6,
            0x2588,
            0x2800,
            0xE0AF,
            0xE0B4,
            u32::from('A'),
            0,
        ] {
            assert!(
                points(cp, &m, &cell).is_none(),
                "0x{cp:04X} is not a polygon"
            );
        }
        for cp in EIGHT {
            assert!(points(cp, &m, &cell).is_some(), "0x{cp:04X} is a polygon");
        }
    }

    /// The hollow separators stroke their closed triangle — its three
    /// vertices in draw order, the path closing back to the first vertex —
    /// at 2 logical pixels of width; the solid shapes fill instead.
    #[test]
    fn the_hollow_separators_stroke_their_closed_outline() {
        let m = metrics(1.5);
        let cell = cell(0, 0);
        for cp in [0xE0B1, 0xE0B3] {
            let Some(Primitive::StrokePolygon(vertices, width)) = primitive(cp, &m, &cell) else {
                panic!("0x{cp:04X} strokes its outline");
            };
            let points = points(cp, &m, &cell).expect("points");
            assert_eq!(vertices, points, "0x{cp:04X} strokes the triangle itself");
            assert_eq!(vertices.len(), 3, "0x{cp:04X} is a triangle");
            assert_eq!(width, 3.0, "2 logical pixels at scale 1.5");
        }
        for cp in [0x25E2, 0x25E5, 0xE0B0, 0xE0B2] {
            assert!(
                matches!(primitive(cp, &m, &cell), Some(Primitive::FillPolygon(_))),
                "0x{cp:04X} fills"
            );
        }
    }

    /// A wide head cell's polygon spans both of its columns: the apex
    /// sits on the far edge of the second column, the base edges stay on
    /// the cell's own.
    #[test]
    fn a_wide_cells_polygon_spans_both_columns() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(scale);
            let mut wide = cell(0, 0);
            wide.wide = Wide::Wide;
            let narrow = points(0xE0B0, &m, &cell(0, 0)).expect("points");
            let wide_points = points(0xE0B0, &m, &wide).expect("points");
            // The apex sits on the far edge of the two-column span.
            let span = m.cell_rect(1, 0).x() + m.cell_rect(1, 0).w();
            assert_eq!(wide_points[1].0, f64::from(span), "apex at scale {scale}");
            assert!(
                wide_points[1].0 > narrow[1].0,
                "the wide apex is farther out at scale {scale}"
            );
            // The base edges are the cell's own.
            assert_eq!(wide_points[0].0, narrow[0].0, "base at scale {scale}");
        }
    }
}
