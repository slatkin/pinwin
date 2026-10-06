use super::super::geom::device_px;
use super::super::painter::paint_frame;
use super::*;
use crate::fontconfig::ThemeColours;
use crate::guard::Poisoned;
use crate::term::cells::{CELL_TEXT_CAP, Cell};
use crate::term::{PngDecoder, PtySink};

/// A sink with nowhere to write (the tests never write to a pty).
struct NullSink;
impl PtySink for NullSink {
    fn write_pty(&mut self, _data: &[u8]) {}
}

/// Decodes nothing; these tests place no kitty images.
struct NoDecoder;
impl PngDecoder for NoDecoder {
    fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
        None
    }
}

/// An 8-column, 4-row terminal at the 8×16 cell pitch the other render
/// tests use: a 64×64 logical frame.
fn terminal() -> Terminal {
    let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
    assert!(terminal.push_size(8, 4, 8, 16));
    terminal
}

/// The test theme, distinct from every colour the tests draw.
const THEME: ThemeColours = ThemeColours {
    background: [10, 20, 30],
    foreground: [200, 150, 100],
};

/// Hides the frame's cursor (DECTCEM). The fresh terminal reports a
/// visible block cursor — the GTK path draws it too — so the tests below
/// that scan what the cell layers drew hide it first; the cursor layer
/// has its own tests.
const HIDE_CURSOR: &[u8] = b"\x1b[?25l";

/// Frame input for a 64×64 logical frame at `scale`.
fn frame(scale: f64) -> FrameInput {
    let device = u32::try_from(device_px(f64::from(64) * scale)).expect("frame fits u32");
    FrameInput::new(64, 64, device, device, 0.0, false, THEME, None)
}

/// Metrics for the 8×16 pitch at `scale`.
fn metrics(scale: f64) -> PainterMetrics {
    PainterMetrics::new(8.0, 16.0, 12.0, scale).expect("test metrics are valid")
}

/// A canvas of `frame`'s device size with the frame painted into it.
fn painted(terminal: &mut Terminal, scale: f64) -> Canvas {
    let frame = frame(scale);
    let (w, h) = frame.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    paint_frame(&mut canvas, &metrics(scale), &frame, terminal);
    canvas
}

/// A colour's bytes in the canvas' memory order: blue, green, red, alpha.
fn bytes(rgb: [u8; 3]) -> [u8; 4] {
    [rgb[2], rgb[1], rgb[0], 255]
}

fn theme_bytes() -> [u8; 4] {
    bytes(THEME.background)
}

fn fg_bytes() -> [u8; 4] {
    bytes(THEME.foreground)
}

/// Encode one code point as UTF-8 pty bytes.
fn utf8(cp: u32) -> Vec<u8> {
    char::from_u32(cp)
        .expect("a scalar")
        .to_string()
        .into_bytes()
}

/// A narrow cell at column `x` holding `cp`'s character.
fn cell_at(x: i32, cp: u32) -> Cell {
    let text = char::from_u32(cp).unwrap_or(' ').to_string();
    let len = text.len().min(CELL_TEXT_CAP - 1);
    let mut bytes = [0u8; CELL_TEXT_CAP];
    bytes[..len].copy_from_slice(&text.as_bytes()[..len]);
    Cell {
        x,
        len,
        text: bytes,
        ..Cell::default()
    }
}

/// The walked cells of a terminal's frame, for asserting what the
/// terminal reports before asserting what the painter drew.
fn cells(terminal: &mut Terminal) -> Vec<Cell> {
    assert!(terminal.frame_begin(), "frame began");
    let mut cells = Vec::new();
    while let Some(cell) = terminal.cell_next() {
        cells.push(cell);
    }
    terminal.frame_end();
    cells
}

/// A device coordinate as the canvas' unsigned pixel index.
fn px(value: i32) -> u32 {
    u32::try_from(value).expect("the coordinate fits u32")
}

/// Assert the pixel at `(x, y)` is `expected`.
fn pixel_is(canvas: &Canvas, x: i32, y: i32, expected: [u8; 4], label: &str) {
    assert_eq!(
        canvas.pixel(px(x), px(y)),
        Some(expected),
        "{label} at ({x}, {y})"
    );
}

/// Assert every pixel of `area` is the sprite colour inside any of
/// `rects` and the theme background outside them.
fn area_matches_rects(canvas: &Canvas, area: DeviceRect, rects: &[DeviceRect], label: &str) {
    for y in area.y()..area.y() + area.h() {
        for x in area.x()..area.x() + area.w() {
            let covered = rects
                .iter()
                .any(|r| x >= r.x() && x < r.x() + r.w() && y >= r.y() && y < r.y() + r.h());
            let expected = if covered { fg_bytes() } else { theme_bytes() };
            pixel_is(canvas, x, y, expected, label);
        }
    }
}

/// The dispatch's routing: the block, braille and polygon code points
/// are sprites (the blank U+2800 too — it draws nothing), the shades
/// and text are not.
#[test]
fn the_dispatch_routes_the_sprite_ranges() {
    let m = metrics(1.0);
    let frame = frame(1.0);
    let owns = |cp: u32| cell_sprite(&m, &frame, &cell_at(0, cp)).is_some();
    for cp in 0x2580..=0x2590 {
        assert!(owns(cp), "0x{cp:04X} is a block sprite");
    }
    for cp in [0x2594, 0x2595, 0x2596, 0x259E, 0x259F] {
        assert!(owns(cp), "0x{cp:04X} is a block sprite");
    }
    for cp in 0x2800..=0x28FF {
        assert!(owns(cp), "0x{cp:04X} is a braille sprite");
    }
    for cp in [
        0x25E2, 0x25E3, 0x25E4, 0x25E5, 0xE0B0, 0xE0B1, 0xE0B2, 0xE0B3,
    ] {
        assert!(owns(cp), "0x{cp:04X} is a polygon sprite");
    }
    // Not owned: the shades (the font has them) and text.
    for cp in [0x2591, 0x2592, 0x2593, u32::from('A'), 0x2630] {
        assert!(!owns(cp), "0x{cp:04X} is not a sprite");
    }
}

/// The skipped cells draw no sprite: a wide glyph's spacer tail, a
/// cell with no glyph, and an INVISIBLE cell.
#[test]
fn the_skipped_cells_draw_no_sprite() {
    let m = metrics(1.0);
    let frame = frame(1.0);
    let tail = Cell {
        wide: Wide::SpacerTail,
        ..cell_at(4, 0x2588)
    };
    assert!(
        cell_sprite(&m, &frame, &tail).is_none(),
        "the tail is skipped"
    );

    let empty = Cell {
        x: 3,
        ..Cell::default()
    };
    assert!(
        cell_sprite(&m, &frame, &empty).is_none(),
        "no glyph, no sprite"
    );

    let invisible = Cell {
        flags: StyleFlags::INVISIBLE,
        ..cell_at(3, 0x2588)
    };
    assert!(
        cell_sprite(&m, &frame, &invisible).is_none(),
        "an INVISIBLE cell draws no sprite"
    );

    // The same skips hold for the polygon sprites.
    let poly_tail = Cell {
        wide: Wide::SpacerTail,
        ..cell_at(4, 0xE0B0)
    };
    assert!(
        cell_sprite(&m, &frame, &poly_tail).is_none(),
        "a polygon tail is skipped"
    );
    let invisible_poly = Cell {
        flags: StyleFlags::INVISIBLE,
        ..cell_at(3, 0xE0B0)
    };
    assert!(
        cell_sprite(&m, &frame, &invisible_poly).is_none(),
        "an INVISIBLE polygon cell draws no sprite"
    );
}

/// A region of full-block glyphs is one uniform colour — the cell
/// foreground, the theme foreground when the cell names none — with no
/// seams between the cells, at every scale the painter tests.
#[test]
fn a_row_of_full_blocks_is_one_colour() {
    for scale in [1.0, 1.25, 1.5, 1.8] {
        let mut terminal = terminal();
        terminal.push_pty_data(HIDE_CURSOR);
        let blocks: Vec<u8> = (0..8).flat_map(|_| utf8(0x2588)).collect();
        terminal.push_pty_data(&blocks);
        let canvas = painted(&mut terminal, scale);
        let m = metrics(scale);
        let row = DeviceRect::new(
            0,
            m.cell_rect(0, 0).y(),
            i32::try_from(canvas.size().0).expect("fits"),
            m.cell_rect(0, 0).h(),
        )
        .expect("the row fits the canvas");
        for y in row.y()..row.y() + row.h() {
            for x in 0..canvas.size().0 {
                assert_eq!(
                    canvas.pixel(x, px(y)),
                    Some(fg_bytes()),
                    "the full-block row is uniform theme foreground at ({x}, {y}) scale {scale}"
                );
            }
        }
        // The row below is the theme background: the sprites did not
        // bleed.
        pixel_is(
            &canvas,
            0,
            m.cell_rect(0, 1).y(),
            theme_bytes(),
            "the row below stays theme",
        );
    }
}

/// A right-half block and a left-half block in neighbouring cells meet
/// on the shared snapped edge with no seam: every device pixel from the
/// right half's left edge to the left half's right edge is foreground.
#[test]
fn half_blocks_meet_without_a_seam() {
    for scale in [1.25, 1.8] {
        let mut terminal = terminal();
        let cells_bytes: Vec<u8> = [0x2590u32, 0x258C]
            .into_iter()
            .chain(std::iter::repeat_n(0x2588, 6))
            .flat_map(utf8)
            .collect();
        terminal.push_pty_data(&cells_bytes);
        let canvas = painted(&mut terminal, scale);
        let m = metrics(scale);
        let boundary = m.cell_rect(1, 0).x();
        let right = block::rects(0x2590, &m, &cell_at(0, 0x2590))
            .expect("a block sprite")
            .remove(0);
        let left = block::rects(0x258C, &m, &cell_at(1, 0x258C))
            .expect("a block sprite")
            .remove(0);
        // The right half ends on the boundary, the left half starts on
        // it, and together they cover the boundary without a gap.
        assert_eq!(right.x() + right.w(), boundary, "right half");
        assert_eq!(left.x(), boundary, "left half");
        let seam_row = right.y() + 1;
        for x in right.x()..left.x() + left.w() {
            pixel_is(&canvas, x, seam_row, fg_bytes(), "the halves meet");
        }
        // Just outside the halves the cell shows the theme background:
        // the blocks really are halves.
        pixel_is(
            &canvas,
            right.x() - 1,
            seam_row,
            theme_bytes(),
            "left of the right half",
        );
        pixel_is(
            &canvas,
            left.x() + left.w(),
            seam_row,
            theme_bytes(),
            "right of the left half",
        );
    }
}

/// Every block code point paints exactly the device rectangles its
/// geometry names — nothing inside them missing, nothing outside them
/// painted — across the whole block range at 1.5, shades included
/// (they stay unpainted: the text pass owns them).
#[test]
fn every_block_code_point_paints_its_geometry() {
    let scale = 1.5;
    let m = metrics(scale);
    let mut terminal = terminal();
    terminal.push_pty_data(HIDE_CURSOR);
    let data: Vec<u8> = (0x2580..=0x259F).flat_map(utf8).collect();
    terminal.push_pty_data(&data);
    let walked = cells(&mut terminal);
    assert_eq!(walked.len(), 32, "one cell per code point");
    let canvas = painted(&mut terminal, scale);
    // The walk wraps at the frame's 8 columns, so each walked cell
    // carries its own position; the assertions follow it.
    for cell in &walked {
        let cp = first_codepoint(cell.text_bytes());
        let cell_rect = m.cell_rect(cell.x, cell.y);
        let expected = if (0x2591..=0x2593).contains(&cp) {
            Vec::new() // the shades stay text; nothing paints them yet
        } else {
            block::rects(cp, &m, cell).unwrap_or_default()
        };
        area_matches_rects(
            &canvas,
            cell_rect,
            &expected,
            &format!("0x{cp:04X} at scale {scale}"),
        );
    }
}

/// The all-dots braille pattern fills exactly its eight dot squares
/// and nothing else, and a single-dot pattern lights only its dot, at
/// 1.5 — the device positions the geometry and the snap rule give.
#[test]
fn braille_paints_exactly_its_dots() {
    let scale = 1.5;
    let m = metrics(scale);
    let origin = Cell::default();
    for (cp, lit) in [(0x28FF, 8), (0x2801, 1), (0x2800, 0)] {
        let mut terminal = terminal();
        terminal.push_pty_data(&utf8(cp));
        let canvas = painted(&mut terminal, scale);
        let cell_rect = m.cell_rect(0, 0);
        let dots = braille::rects(cp, &m, &origin).expect("inside the braille range");
        assert_eq!(dots.len(), lit, "0x{cp:04X} lights {lit} dots");
        area_matches_rects(&canvas, cell_rect, &dots, "0x{cp:04X}");
    }
}

/// A sprite takes the cell's explicit foreground colour.
#[test]
fn a_sprite_takes_the_cells_foreground() {
    let scale = 1.0;
    let mut terminal = terminal();
    // SGR 4-bit red foreground, then a full block.
    let mut data = b"\x1b[31m".to_vec();
    data.extend(utf8(0x2588));
    data.extend(b"\x1b[0m.");
    terminal.push_pty_data(&data);
    let walked = cells(&mut terminal);
    assert!(walked[0].has_fg, "the cell carries the SGR colour");
    let fg = bytes([walked[0].fg.r, walked[0].fg.g, walked[0].fg.b]);
    let canvas = painted(&mut terminal, scale);
    let m = metrics(scale);
    rect_is_full_cell(&canvas, &m, 0, 0, fg, "the block is the cell's red");
    // The plain cell after it stays theme.
    pixel_is(
        &canvas,
        m.cell_rect(1, 0).x(),
        m.cell_rect(1, 0).y(),
        theme_bytes(),
        "the plain cell stays theme",
    );
}

/// Assert every pixel of the cell at `(col, row)` is `expected`.
fn rect_is_full_cell(
    canvas: &Canvas,
    m: &PainterMetrics,
    col: i32,
    row: i32,
    expected: [u8; 4],
    label: &str,
) {
    rect_is(canvas, &m.cell_rect(col, row), expected, label);
}

/// Assert every pixel of a device rectangle is `expected`.
fn rect_is(canvas: &Canvas, rect: &DeviceRect, expected: [u8; 4], label: &str) {
    for y in rect.y()..rect.y() + rect.h() {
        for x in rect.x()..rect.x() + rect.w() {
            pixel_is(canvas, x, y, expected, label);
        }
    }
}

/// An INVISIBLE cell draws no sprite. The pinned vt does not map SGR 8
/// (conceal) through the frame protocol — probed — so the flag reaches
/// this test through a hand-built cell; [`paint`] skips exactly what
/// this decision rejects, and the skipped-cells test above pins the
/// other two skips the same way.
#[test]
fn an_invisible_sprite_cell_draws_nothing() {
    let m = metrics(1.0);
    let frame = frame(1.0);
    let invisible = Cell {
        flags: StyleFlags::INVISIBLE,
        ..cell_at(0, 0x2588)
    };
    assert!(cell_sprite(&m, &frame, &invisible).is_none());
}

/// A solid powerline separator fills its interior with the cell
/// foreground and leaves the corners past its apex unpainted, at
/// scale 1 where the 8×16 cell's device box is exact. U+E0B0 points
/// right: its base is the cell's left edge, its apex the middle of
/// the right edge.
#[test]
fn a_solid_powerline_fills_its_interior_and_leaves_the_far_corners() {
    let scale = 1.0;
    let mut terminal = terminal();
    terminal.push_pty_data(&utf8(0xE0B0));
    let canvas = painted(&mut terminal, scale);
    pixel_is(&canvas, 0, 8, fg_bytes(), "the base midpoint");
    pixel_is(&canvas, 2, 8, fg_bytes(), "the interior");
    pixel_is(&canvas, 0, 12, fg_bytes(), "the lower base");
    // Past the hypotenuse the corners on the apex's side stay
    // background.
    pixel_is(&canvas, 7, 0, theme_bytes(), "the top far corner");
    pixel_is(&canvas, 7, 15, theme_bytes(), "the bottom far corner");
    pixel_is(&canvas, 6, 1, theme_bytes(), "inside the far corner");
}

/// The right-pointing and left-pointing solid separators are mirror
/// images: every pixel's coverage class — the theme background, the
/// exact foreground, or a blend strictly between — mirrors to the
/// opposite cell. tiny-skia's antialiasing carries a small
/// slope-sign bias on a 45° edge (a diagonal that bisects a pixel
/// covers 62% of it one way and 38% the other), so two mirrored
/// blended pixels blend by slightly different amounts; the class is
/// what the shape pins.
#[test]
fn the_two_solid_separators_are_mirror_images() {
    let scale = 1.0;
    let mut terminal = terminal();
    let mut data = utf8(0xE0B0);
    data.extend(utf8(0xE0B2));
    terminal.push_pty_data(&data);
    let canvas = painted(&mut terminal, scale);
    let class = |pixel: [u8; 4]| -> u8 {
        if pixel == theme_bytes() {
            0
        } else if pixel == fg_bytes() {
            2
        } else {
            1
        }
    };
    for y in 0..16u32 {
        for x in 0..8u32 {
            let left = class(canvas.pixel(x, y).expect("inside the canvas"));
            let right = class(canvas.pixel(8 + (7 - x), y).expect("inside the canvas"));
            assert_eq!(left, right, "the mirror class of ({x}, {y})");
        }
    }
    // All three classes really occur: the filled interior, a blended
    // diagonal pixel and the empty far corner.
    assert_eq!(class(canvas.pixel(2, 8).expect("inside")), 2, "filled");
    assert_eq!(class(canvas.pixel(0, 0).expect("inside")), 1, "blended");
    assert_eq!(class(canvas.pixel(7, 0).expect("inside")), 0, "empty");
}

/// Each corner triangle fills the half of the cell its corner names:
/// the interior at its right angle is foreground, the opposite
/// corner background, and the painted coverage sums to half the cell.
#[test]
fn each_corner_triangle_covers_its_half_of_the_cell() {
    let scale = 1.0;
    for (cp, full, empty) in [
        (0x25E2, (6, 14), (0, 0)), // lower right
        (0x25E3, (1, 14), (7, 0)), // lower left
        (0x25E4, (1, 1), (7, 15)), // upper left
        (0x25E5, (6, 1), (0, 15)), // upper right
    ] {
        let mut terminal = terminal();
        terminal.push_pty_data(&utf8(cp));
        let canvas = painted(&mut terminal, scale);
        pixel_is(&canvas, full.0, full.1, fg_bytes(), "the interior corner");
        pixel_is(&canvas, empty.0, empty.1, theme_bytes(), "the far corner");
        // The antialiased coverage, read off the blue channel's blend
        // between the theme background and the foreground, sums to
        // the triangle's area — half the 8×16 cell — within
        // rasterization dust.
        let (theme_b, fg_b) = (f64::from(theme_bytes()[0]), f64::from(fg_bytes()[0]));
        let coverage: f64 = (0..16u32)
            .flat_map(|y| (0..8u32).map(move |x| (x, y)))
            .map(|(x, y)| {
                let blue = f64::from(canvas.pixel(x, y).expect("inside")[0]);
                (blue - theme_b) / (fg_b - theme_b)
            })
            .sum();
        assert!(
            (coverage - 64.0).abs() <= 2.0,
            "0x{cp:04X} covers {coverage} of 64 square pixels"
        );
    }
}

/// The hollow separator strokes its closed outline at 2 logical
/// pixels of width — 2·scale device pixels, cairo's user-space rule —
/// and leaves the interior empty. Probed across the base edge, away
/// from the corners, at every scale the painter tests: the canvas is
/// opaque, so a partial column reads as a blend between the theme
/// background and the foreground, and the blend pins the width.
#[test]
fn the_hollow_separator_strokes_at_two_logical_pixels() {
    for (scale, coverage) in [(1.0, 0.0f64), (1.25, 0.25), (1.5, 0.5), (1.8, 0.8)] {
        let mut terminal = terminal();
        let mut data = utf8(u32::from(' '));
        data.extend(utf8(0xE0B1));
        terminal.push_pty_data(&data);
        let canvas = painted(&mut terminal, scale);
        let m = metrics(scale);
        let base = m.cell_rect(1, 0).x();
        let mid = device_px(8.0 * scale);
        let at = |dx: i32| {
            canvas
                .pixel(
                    u32::try_from(base + dx).expect("fits"),
                    u32::try_from(mid).expect("fits"),
                )
                .expect("inside the canvas")
        };
        // The stroke is centred on the base edge: both columns it
        // covers fully carry the exact foreground bytes.
        assert_eq!(at(-1), fg_bytes(), "the inner column at {scale}");
        assert_eq!(at(0), fg_bytes(), "the base column at {scale}");
        // The columns past the width's edge blend by the coverage the
        // width leaves: none at scale 1 (width 2), then 1/4, 1/2 and
        // 4/5 at 1.25, 1.5 and 1.8.
        let blend = |pixel: [u8; 4], want: f64, label: &str| {
            for (channel, (fg, theme)) in fg_bytes()
                .into_iter()
                .zip(theme_bytes())
                .take(3)
                .enumerate()
            {
                let expected = want * f64::from(fg) + (1.0 - want) * f64::from(theme);
                // The tolerance covers tiny-skia's sub-pixel edge
                // quantization: at 1.8 the width's edge at +1.8 lands
                // on its scanline grid near +1.75, so the blend reads
                // about 0.75 coverage instead of 0.8. The neighbouring
                // widths (0, 1/4, 1/2) stay distinct.
                assert!(
                    (f64::from(pixel[channel]) - expected).abs() <= 16.0,
                    "{label} channel {channel} at scale {scale}: {} vs {expected}",
                    pixel[channel]
                );
            }
        };
        for dx in [-2, 1] {
            blend(at(dx), coverage, "the edge column");
        }
        // The interior stays empty: probed a few columns right of the
        // base edge on the mid row, clear of the apex join, where the
        // two diagonal sides are far.
        assert_eq!(at(3), theme_bytes(), "the interior is empty at {scale}");
        assert_eq!(
            at(4),
            theme_bytes(),
            "the deep interior is empty at {scale}"
        );
    }
}

/// A solid separator followed by a full block leaves no seam at 1.5:
/// across the separator's base-to-apex row every pixel carries some
/// foreground, so no background column opens between the two cells,
/// and the block continues at full strength on the shared edge.
#[test]
fn a_solid_separator_and_a_full_block_leave_no_seam_at_1_5() {
    let scale = 1.5;
    let mut terminal = terminal();
    let mut data = utf8(0xE0B0);
    data.extend(utf8(0x2588));
    terminal.push_pty_data(&data);
    let canvas = painted(&mut terminal, scale);
    let m = metrics(scale);
    let cell0 = m.cell_rect(0, 0);
    let cell1 = m.cell_rect(1, 0);
    let mid = device_px(8.0 * scale);
    for x in cell0.x()..cell1.x() {
        let pixel = canvas
            .pixel(
                u32::try_from(x).expect("fits"),
                u32::try_from(mid).expect("fits"),
            )
            .expect("inside the canvas");
        assert_ne!(pixel, theme_bytes(), "no background seam at x {x}");
    }
    // The apex pixel itself is a blend — the triangle's tip — and the
    // block next to it is exact.
    let apex = canvas
        .pixel(
            u32::try_from(cell1.x() - 1).expect("fits"),
            u32::try_from(mid).expect("fits"),
        )
        .expect("inside the canvas");
    assert_ne!(apex, theme_bytes(), "the apex pixel is painted");
    assert_ne!(apex, fg_bytes(), "the apex pixel blends");
    pixel_is(&canvas, cell1.x(), mid, fg_bytes(), "the block continues");
    // Just off the apex row the tip is thin, as the shape asks.
    pixel_is(
        &canvas,
        cell1.x() - 1,
        mid - 8,
        theme_bytes(),
        "above the apex the tip is thin",
    );
}

/// A polygon sprite takes the cell's foreground colour, falling back
/// to the theme foreground when the cell names none. The pinned vt
/// walks a styled private-use glyph's cell last, so the test finds
/// the separator cell by its code point.
#[test]
fn a_polygon_sprite_takes_the_cells_foreground() {
    let scale = 1.0;
    let mut explicit = terminal();
    let mut data = b"\x1b[31m".to_vec();
    data.extend(utf8(0xE0B0));
    explicit.push_pty_data(&data);
    let walked = cells(&mut explicit);
    let separator = walked
        .iter()
        .find(|cell| first_codepoint(cell.text_bytes()) == 0xE0B0)
        .expect("the separator cell");
    assert!(separator.has_fg, "the cell carries the SGR colour");
    let fg = bytes([separator.fg.r, separator.fg.g, separator.fg.b]);
    let canvas = painted(&mut explicit, scale);
    let m = metrics(scale);
    let cell_rect = m.cell_rect(separator.x, separator.y);
    pixel_is(
        &canvas,
        cell_rect.x() + 2,
        cell_rect.y() + 8,
        fg,
        "the separator is the cell's red",
    );

    // No colour named: the theme foreground.
    let mut fallback = terminal();
    fallback.push_pty_data(&utf8(0xE0B0));
    let canvas = painted(&mut fallback, scale);
    pixel_is(
        &canvas,
        2,
        8,
        fg_bytes(),
        "the separator is the theme foreground",
    );
}

/// The hollow separators stroke one closed triangle with MITER joins,
/// so the corners fill out to the miter the way cairo's do: the apex
/// tip is painted and pokes beside the apex into the neighbouring
/// cell, the sharp base corner keeps its miter spike on the diagonal
/// outside the triangle, and the interior stays empty. Probed at every
/// scale the painter tests; the two separators mirror each other.
#[test]
fn the_hollow_separators_miter_their_corners() {
    // At scale 1 the base corner's spike covers about five sixths of its
    // pixel, so the test pins the blend's blue channel there; at the
    // fractional scales the coverage varies with the snap, and
    // painted-or-not is what the shape pins.
    for (scale, spike_blue) in [(1.0, Some(88)), (1.25, None), (1.5, None), (1.8, None)] {
        for cp in [0xE0B1, 0xE0B3] {
            let mut terminal = terminal();
            let mut data = utf8(u32::from(' '));
            data.extend(utf8(cp));
            terminal.push_pty_data(&data);
            let canvas = painted(&mut terminal, scale);
            let m = metrics(scale);
            let cell = m.cell_rect(1, 0);
            let (x1, x2) = (cell.x(), cell.x() + cell.w());
            let ym = device_px(8.0 * scale);
            let y2 = device_px(16.0 * scale);
            // U+E0B1 points right: its apex sits on the cell's right
            // edge and its base corners on the left; U+E0B3 mirrors.
            let (apex, beside, corner, inner) = if cp == 0xE0B1 {
                // Apex on the right edge, base on the left: the tip
                // pokes right, the bottom base corner's spike leans
                // left, so the pixel it covers sits one step left of
                // and below the corner.
                ((x2, ym), (x2 + 1, ym), (x1 - 1, y2 + 1), (x1 + 4, ym))
            } else {
                // Mirrored: apex on the left edge, base on the right,
                // the spike leaning right under the corner.
                ((x1, ym), (x1 - 1, ym), (x2, y2 + 1), (x2 - 4, ym))
            };
            let label = format!("0x{cp:04X} at scale {scale}");
            // The apex's outer pixel and the pixel beside it along
            // the miter - which lies in the neighbouring cell - are
            // painted: the join fills the tip the butt caps left
            // background.
            assert_ne!(
                canvas.pixel(px(apex.0), px(apex.1)),
                Some(theme_bytes()),
                "{label}: the apex tip is painted"
            );
            assert_ne!(
                canvas.pixel(px(beside.0), px(beside.1)),
                Some(theme_bytes()),
                "{label}: the miter reaches beside the apex"
            );
            // The base corner's miter spike covers the pixel on the
            // spike's side of the corner, outside the triangle - the
            // background the butt caps left there.
            let corner_pixel = canvas.pixel(px(corner.0), px(corner.1)).expect("inside");
            match spike_blue {
                Some(want) => assert!(
                    (i32::from(corner_pixel[0]) - want).abs() <= 14,
                    "{label}: the base corner spike blends: {} vs {want}",
                    corner_pixel[0]
                ),
                None => assert_ne!(
                    canvas.pixel(px(corner.0), px(corner.1)),
                    Some(theme_bytes()),
                    "{label}: the base corner spike is painted"
                ),
            }
            // The interior stays empty.
            pixel_is(&canvas, inner.0, inner.1, theme_bytes(), &label);
            let centre = (
                if cp == 0xE0B1 {
                    (2 * x1 + x2) / 3
                } else {
                    (x1 + 2 * x2) / 3
                },
                (ym + y2) / 3,
            );
            pixel_is(&canvas, centre.0, centre.1, theme_bytes(), &label);
        }
    }
}
