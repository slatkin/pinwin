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

/// One sprite's geometry, consumed by the cairo painter ([`draw_sprite`])
/// (gsk-render-nodes task 2.2's shared geometry, kept for the cairo painter
/// alone since replace-gtk-with-wayland D11). Blocks, quadrants and braille
/// dots are integer-ish rectangles; the corner and powerline triangles need
/// a cairo path.
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
fn snap_rects(
    scale: OutputScale,
    mut rects: Vec<(f64, f64, f64, f64)>,
) -> Vec<(f64, f64, f64, f64)> {
    for rect in &mut rects {
        let (x, y, w, h) = *rect;
        *rect = scale.snap_rect(x, y, w, h);
    }
    rects
}

/// Snap each triangle vertex coordinate (`snap-grid-edges` D5): the
/// straight sides of a corner triangle lie on the cell boundary, and an
/// unsnapped side blends with the neighbour. Each vertex moves by at most
/// half a device pixel.
fn snap_vertices(scale: OutputScale, points: [f64; 6]) -> [f64; 6] {
    points.map(|v| scale.snap_edge(v))
}

/// Ghostty's `Fraction.min` (`draw/common.zig`) on a size in whole device
/// pixels: the min edge of a section, taken as the complement of the max
/// edge so that rounding evens out.
fn fraction_min(size: f64, fraction: f64) -> f64 {
    size - ((1.0 - fraction) * size).round()
}

/// Ghostty's `Fraction.max` on a size in whole device pixels.
fn fraction_max(size: f64, fraction: f64) -> f64 {
    (fraction * size).round()
}

/// The rectangles of a block element or quadrant (0x2580..=0x259F, less the
/// shades) in `cell` = (x, y, w, h) logical pixels, as Ghostty's
/// `draw/block.zig` draws them: its integer arithmetic runs on the cell's
/// snapped device box, so every bar is a whole number of device pixels and
/// a bar of the same fraction has the same thickness whichever edge it is
/// anchored to. The result is converted back to logical pixels.
fn block_rects(
    cp: u32,
    (left, top, width, height): (f64, f64, f64, f64),
    scale: OutputScale,
) -> Vec<(f64, f64, f64, f64)> {
    let factor = scale.get();
    let (x0, y0) = ((left * factor).round(), (top * factor).round());
    let (x1, y1) = (
        ((left + width) * factor).round(),
        ((top + height) * factor).round(),
    );
    let (dw, dh) = (x1 - x0, y1 - y0);
    // `blockShade`'s bar: `round(size * fraction)` device pixels, and at
    // least one so a thin bar never vanishes (as `snap_rect` guaranteed).
    let bar = |size: f64, fraction: f64| fraction_max(size, fraction).max(1.0).min(size);

    // Device edges (left, top, right, bottom).
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
            let bits = QUADRANTS[(cp - 0x2596) as usize];
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
    edges
        .into_iter()
        .map(|(ax, ay, bx, by)| {
            (
                ax / factor,
                ay / factor,
                (bx - ax) / factor,
                (by - ay) / factor,
            )
        })
        .collect()
}

/// The geometry [`draw_sprite`] paints `cp` with, or [`SpriteShape::None`]
/// when `cp` is left to the text pass. Pure geometry — no drawing — so the
/// tests can pin the numbers. Rectangles and triangle vertices are snapped
/// to the device pixel grid (`snap-grid-edges` D1, D4, D5).
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
        // Shades: the font has them.
        if (0x2591..=0x2593).contains(&cp) {
            return SpriteShape::None;
        }
        return SpriteShape::Rects(block_rects(cp, (x, y, w, ch), cell_metrics.scale));
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
/// Returns true when `cp` was drawn (`draw_sprite`).
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

/// The underline band `cell`'s UNDERLINE flag asks for, as the rectangle
/// [`crate::render::DrawState::render_grid`] fills, snapped to the device
/// pixel grid (`snap-grid-edges` D1, D4). `None` when the flag is unset; the
/// split (instead of a shared list) keeps the per-cell pass allocation-free.
pub(crate) fn underline_rect(
    cell: &Cell,
    cell_metrics: &CellMetrics,
) -> Option<(f64, f64, f64, f64)> {
    if !cell
        .flags
        .contains(crate::term::cells::StyleFlags::UNDERLINE)
    {
        return None;
    }
    Some(underline_strikethrough_rect(
        cell,
        cell_metrics,
        f64::from(cell_metrics.ascent) + 1.0,
    ))
}

/// The strikethrough band `cell`'s STRIKETHROUGH flag asks for, centred on
/// the cell's mid-line, snapped like the underline. `None` when the flag is
/// unset.
pub(crate) fn strikethrough_rect(
    cell: &Cell,
    cell_metrics: &CellMetrics,
) -> Option<(f64, f64, f64, f64)> {
    if !cell
        .flags
        .contains(crate::term::cells::StyleFlags::STRIKETHROUGH)
    {
        return None;
    }
    Some(underline_strikethrough_rect(
        cell,
        cell_metrics,
        f64::from(cell_metrics.cell_h) / 2.0 - 0.5,
    ))
}

/// The 1 px band across `cell` whose top edge sits `top` px below the
/// cell's top, spanning the cell's width, snapped to the device pixel grid
/// (`snap-grid-edges` D1, D4).
fn underline_strikethrough_rect(
    cell: &Cell,
    cell_metrics: &CellMetrics,
    top: f64,
) -> (f64, f64, f64, f64) {
    cell_metrics.scale.snap_rect(
        f64::from(cell.x) * f64::from(cell_metrics.cell_w),
        f64::from(cell.y) * f64::from(cell_metrics.cell_h) + top,
        f64::from(cell_metrics.cell_w),
        1.0,
    )
}

/// The cursor's shape as the cairo painter draws it
/// ([`crate::render::DrawState::draw_cursor`]). `Fill` is one rectangle to
/// fill; `Hollow` is the hollow block cursor's outline as the four 1 px
/// bands its former 1 px stroke covered (`snap-grid-edges` D6).
pub(crate) enum CursorShape {
    Fill((f64, f64, f64, f64)),
    Hollow([(f64, f64, f64, f64); 4]),
}

/// The geometry [`crate::render::DrawState::draw_cursor`] paints `cursor`
/// with, snapped to the device pixel grid (`snap-grid-edges` D1, D4).
pub(crate) fn cursor_shape(
    cursor: &crate::term::cells::Cursor,
    cell_metrics: &CellMetrics,
) -> CursorShape {
    let scale = cell_metrics.scale;
    let x = f64::from(cursor.x) * f64::from(cell_metrics.cell_w);
    let y = f64::from(cursor.y) * f64::from(cell_metrics.cell_h);
    let cw = f64::from(cell_metrics.cell_w);
    let ch = f64::from(cell_metrics.cell_h);
    match cursor.style {
        crate::term::cells::CursorStyle::Bar => CursorShape::Fill(scale.snap_rect(x, y, 2.0, ch)),
        crate::term::cells::CursorStyle::Underline => {
            CursorShape::Fill(scale.snap_rect(x, y + ch - 2.0, cw, 2.0))
        }
        crate::term::cells::CursorStyle::BlockHollow => {
            CursorShape::Hollow(hollow_bands(scale, x, y, cw, ch))
        }
        crate::term::cells::CursorStyle::Block => CursorShape::Fill(scale.snap_rect(x, y, cw, ch)),
    }
}

/// The hollow block cursor's outline as the four 1 px bands a 1 px stroke
/// over the rectangle inset by half a pixel covered: top, bottom, left,
/// right, each snapped on its own (`snap-grid-edges` D4, D6). Bands that
/// share an edge pass the same value to the snap — the vertical bands'
/// outer edges are the horizontal bands' ends — so they tile the ring
/// without a gap or an overlap.
fn hollow_bands(scale: OutputScale, x: f64, y: f64, cw: f64, ch: f64) -> [(f64, f64, f64, f64); 4] {
    let snap = |(x, y, w, h): (f64, f64, f64, f64)| scale.snap_rect(x, y, w, h);
    [
        snap((x, y, cw, 1.0)),
        snap((x, y + ch - 1.0, cw, 1.0)),
        snap((x, y + 1.0, 1.0, ch - 2.0)),
        snap((x + cw - 1.0, y + 1.0, 1.0, ch - 2.0)),
    ]
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
    /// fresh surface with a white source, on the cairo painter.
    fn filled_pixels(cp: u32) -> usize {
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        assert!(draw_sprite(&cr, &cell(0, 0), cp, &metrics()));
        // Drop the context before the flush: a live context on the
        // surface makes `data()` fail with `NonExclusive`.
        drop(cr);
        surface.flush();
        count_filled(&mut surface)
    }

    /// Whether `cp` is painted as a sprite (the shades and text codepoints
    /// are not).
    fn paints(cp: u32) -> bool {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 8, 16).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        draw_sprite(&cr, &cell(0, 0), cp, &metrics())
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

    fn assert_filled(cp: u32, expected: usize) {
        assert_eq!(filled_pixels(cp), expected, "0x{cp:04X}");
    }

    #[test]
    fn upper_half_block_fills_the_top_half() {
        assert_filled(0x2580, 8 * 8);
    }

    #[test]
    fn lower_full_block_fills_the_whole_cell() {
        assert_filled(0x2588, 8 * 16);
    }

    #[test]
    fn left_one_eighth_fills_one_column() {
        // 0x258F is left 1/8: one of the eight columns.
        assert_filled(0x258F, 16);
    }

    #[test]
    fn right_one_eighth_fills_one_column() {
        assert_filled(0x2595, 16);
    }

    #[test]
    fn upper_one_eighth_fills_whole_pixel_rows() {
        // round(16 / 8) = 2 whole pixel rows at a 16px cell height.
        assert_filled(0x2594, 8 * 2);
    }

    /// The device top and bottom edges of the one rectangle `cp` draws in
    /// `cell`.
    fn device_edges(cell: &Cell, cp: u32, metrics: &CellMetrics) -> (f64, f64) {
        let SpriteShape::Rects(rects) = sprite_shape(cell, cp, metrics) else {
            panic!("0x{cp:04X} is a rect sprite");
        };
        assert_eq!(rects.len(), 1);
        let (_, y, _, h) = rects[0];
        let scale = metrics.scale.get();
        ((y * scale).round(), ((y + h) * scale).round())
    }

    /// U+2581 and U+2594 are the same one-eighth bar at either edge, so they
    /// are the same thickness (issue #17): `round(device_cell_h / 8)` device
    /// pixels, whatever the scale, the cell height or the row.
    #[test]
    fn lower_and_upper_one_eighth_have_the_same_device_thickness() {
        for scale in [1.0, 1.5, 1.8] {
            for cell_h in [19, 20] {
                let metrics = CellMetrics {
                    cell_w: 8,
                    cell_h,
                    scale: OutputScale::new(scale),
                    ..CellMetrics::default()
                };
                for row in 0..6 {
                    let top = (f64::from(row * cell_h) * scale).round();
                    let bottom = (f64::from((row + 1) * cell_h) * scale).round();
                    let expected = ((bottom - top) / 8.0).round();
                    let lower = device_edges(&cell(0, row), 0x2581, &metrics);
                    let upper = device_edges(&cell(0, row), 0x2594, &metrics);
                    let ctx = format!("scale {scale} cell_h {cell_h} row {row}");
                    assert_eq!(lower.1 - lower.0, expected, "lower thickness, {ctx}");
                    assert_eq!(upper.1 - upper.0, expected, "upper thickness, {ctx}");
                    assert_eq!(lower.1, bottom, "lower bar sits on the cell bottom, {ctx}");
                    assert_eq!(upper.0, top, "upper bar sits on the cell top, {ctx}");
                }
            }
        }
    }

    #[test]
    fn quadrants_fill_their_named_quarters() {
        // 0x2596 is the lower-left quadrant (bit2).
        assert_filled(0x2596, 4 * 8);
        // 0x2599 is lower-left + upper-left (bits 0x5? table says 0x9:
        // upper right + lower left ... the table's own layout).
        assert_filled(0x259F, 4 * 8 * 3);
    }

    #[test]
    fn shades_are_left_to_the_font() {
        assert!(!paints(0x2591), "no shade is a sprite");
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
        // All eight dots set.
        assert_filled(0x28FF, 8 * dot_count(8, 16));
    }

    fn dot_count(cell_w: i32, cell_h: i32) -> usize {
        let (dot, _, _) = braille_geometry(cell_w, cell_h);
        (dot.cast_unsigned() * dot.cast_unsigned()) as usize
    }

    #[test]
    fn powerline_right_triangle_fills_half_the_cell() {
        // 0xE0B0: a right-pointing solid triangle filling the cell.
        let filled = filled_pixels(0xE0B0);
        // A solid triangle through the full cell: roughly the half-box,
        // plus or minus the antialiased edge pixels.
        assert!(filled > 8 * 16 / 4, "filled {filled}");
        assert!(filled < 8 * 8 + 16, "filled {filled}");
    }

    /// The sprite routing holds across the ranges and their neighbours:
    /// blocks are sprites, the shades and text stay text (gsk-render-nodes
    /// task 2.2's sweep, kept against the cairo painter since
    /// replace-gtk-with-wayland D11).
    #[test]
    fn the_sprite_routing_sweep_covers_the_ranges() {
        let mut checked = 0usize;
        let mut sweep = |cps: std::ops::RangeInclusive<u32>| {
            for cp in cps {
                let _ = paints(cp);
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
        assert!(!paints(u32::from('A')));
        assert!(!paints(0x2630));
        assert!(!paints(0x6F22));
        assert!(!paints(0x1F600));
        assert!(paints(0x2588), "a block is a sprite");
        assert!(checked > 420, "sweep covered {checked} codepoints");
    }
}
