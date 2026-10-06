//! The sprite layer of the grid painter (row 4.3): the frame's block
//! elements ([`sprite::block`]) and braille patterns ([`sprite::braille`])
//! drawn as unantialiased rectangles on the canvas, in the cell's
//! foreground colour — or the theme foreground when the cell names none —
//! exactly as [`crate::render::sprites::draw_sprite`] draws them on the
//! GTK path. Powerline separators, the one-line sprite, the corner
//! triangles and the cursor shapes plug in as further dispatch arms in
//! later rows; until then their code points are not sprites and fall to
//! the text pass.
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
use super::geom::{DeviceRect, FrameInput, PainterMetrics};
use crate::term::Terminal;
use crate::term::cells::{StyleFlags, Wide, first_codepoint};

mod block;
mod braille;

/// Walk the open frame's cells and fill the sprites, shifted by `offset`
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
        let Some((color, rects)) = cell_sprite(metrics, frame, &cell) else {
            continue;
        };
        for rect in rects {
            canvas.fill_rect(rect.x() + offset, rect.y(), rect.w(), rect.h(), color);
        }
    }
}

/// The sprite one cell draws: its colour and device rectangles, or `None`
/// when the cell is not drawn as a sprite — the skipped cells, and every
/// code point no dispatch arm owns yet (the text pass draws those). Pure,
/// so the tests build cells by hand and no frame walk is needed.
fn cell_sprite(
    metrics: &PainterMetrics,
    frame: &FrameInput,
    cell: &crate::term::cells::Cell,
) -> Option<(CanvasColor, Vec<DeviceRect>)> {
    if cell.wide == Wide::SpacerTail {
        return None; // never rendered
    }
    if cell.len == 0 || cell.flags.contains(StyleFlags::INVISIBLE) {
        return None;
    }
    let cp = first_codepoint(cell.text_bytes());
    // The sprite dispatch: one arm per family a row ports. Row 4.3 owns
    // the blocks and the braille patterns.
    let rects = block::rects(cp, metrics, cell).or_else(|| braille::rects(cp, metrics, cell))?;
    let color = if cell.has_fg {
        CanvasColor::from_theme(cell.fg)
    } else {
        frame.foreground()
    };
    Some((color, rects))
}

#[cfg(test)]
mod tests {
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

    /// The dispatch's routing: the block and braille code points are
    /// sprites (the blank U+2800 too — it draws nothing), the shades, the
    /// corner triangles, powerline and text are not.
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
        // Not owned: the shades (the font has them), the corner triangles
        // and powerline (later rows), and text.
        for cp in [
            0x2591,
            0x2592,
            0x2593,
            0x25E2,
            0x25E5,
            0xE0B0,
            0xE0B3,
            u32::from('A'),
            0x2630,
        ] {
            assert!(!owns(cp), "0x{cp:04X} is not a sprite yet");
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
    }

    /// A region of full-block glyphs is one uniform colour — the cell
    /// foreground, the theme foreground when the cell names none — with no
    /// seams between the cells, at every scale the painter tests.
    #[test]
    fn a_row_of_full_blocks_is_one_colour() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let mut terminal = terminal();
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
}
