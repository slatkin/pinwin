//! The grid painter (row 4.3, `replace-gtk-with-wayland` D5): one frame of
//! the terminal drawn into a [`Canvas`] at device size — the CPU painter
//! the panel thread will present through its `wl_shm` buffers once row 8.1
//! switches `Panel` over. The GTK painters (`DrawState::draw`, the GSK
//! emitter) stay the production path until then; this module gives the same
//! pixels for the same cells.
//!
//! A frame draws in layers, each one pass over the open frame's cells in
//! its own file, called in order:
//!
//! 1. [`bg`] — the cell backgrounds, merged into runs of equal colour.
//! 2. [`bands`] — the underline and strikethrough decorations.
//! 3. later rows plug the text and sprite pass in here, then the cursor
//!    pass and the kitty image pass, in [`crate::render::DrawState::
//!    render_grid`]'s order.
//!
//! The focus accent ([`accent`]) draws last, on top, in raw surface
//! coordinates — the one layer the tween's draw offset does not translate,
//! as `DrawState::draw` draws it after the translated grid.
//!
//! The layers are stateless passes over one frame, so they are free
//! functions; the stateful caches later rows bring (the glyph and sprite
//! caches) will live beside the frame call, not reshape it.
//!
//! GTK-free (`replace-gtk-with-wayland` D10): `paint_frame` and the pixel
//! tests below run without a display.

use super::canvas::Canvas;
use super::geom::{FrameInput, PainterMetrics, device_px};
use super::{accent, bands, bg};
use crate::term::Terminal;

/// Draw one frame of `terminal` into `canvas`: the theme background, the
/// grid layers in order, and the focus accent on top.
///
/// `metrics` carries the cell pitch, the font ascent and the output scale
/// the frame snaps at; `frame` carries the sizes, the tween's draw offset,
/// the focus state and the theme colours. The canvas must already be the
/// frame's device size ([`FrameInput::device_size`]).
///
/// A terminal with no live frame (none created yet, or a failed refresh)
/// draws the theme background and the accent only — the degraded draw
/// `DrawState::draw_inner` keeps to.
pub fn paint_frame(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    terminal: &mut Terminal,
) {
    // The theme background is the whole surface: the grid draws over it,
    // and cells without an explicit background show it.
    let (device_w, device_h) = frame.device_size();
    if let (Ok(device_w), Ok(device_h)) = (i32::try_from(device_w), i32::try_from(device_h)) {
        canvas.fill_rect(0, 0, device_w, device_h, frame.background());
    }

    // The tween's draw offset translates the grid layers, not the accent.
    // The offset is snapped to the device pixel grid by the tween, so the
    // product is a whole number of device pixels up to floating-point dust.
    let offset = device_px(frame.draw_offset() * metrics.scale());

    if !terminal.frame_begin() {
        accent::paint(canvas, frame, metrics);
        return;
    }

    bg::paint(canvas, metrics, frame, terminal, offset);
    terminal.frame_rewind();
    bands::paint(canvas, metrics, frame, terminal, offset);
    terminal.frame_rewind();
    // Row 4.4 and later plug the text and sprite pass in here; the cursor
    // pass and the image pass follow it (`render_grid`'s order).
    terminal.frame_end();

    accent::paint(canvas, frame, metrics);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fontconfig::ThemeColours;
    use crate::guard::Poisoned;
    use crate::layout::Accent;
    use crate::term::cells::{Cell, Wide};
    use std::num::NonZeroU16;

    /// A sink with nowhere to write (the tests never write to a pty).
    struct NullSink;
    impl crate::term::PtySink for NullSink {
        fn write_pty(&mut self, _data: &[u8]) {}
    }

    /// Decodes nothing; these tests place no kitty images.
    struct NoDecoder;
    impl crate::term::PngDecoder for NoDecoder {
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

    /// A terminal without a grid: `frame_begin` fails, the degraded draw.
    fn uninitialized_terminal() -> Terminal {
        Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {})
    }

    /// The test theme, distinct from every colour the tests draw.
    const THEME: ThemeColours = ThemeColours {
        background: [10, 20, 30],
        foreground: [200, 150, 100],
    };

    /// The font ascent the band positions hang from.
    const ASCENT: f64 = 12.0;

    /// Frame input for a 64×64 logical frame at `scale`.
    fn frame(focused: bool, accent: Option<Accent>, draw_offset: f64, scale: f64) -> FrameInput {
        let device = device_px(f64::from(64) * scale);
        let device = u32::try_from(device).expect("frame fits u32");
        FrameInput::new(64, 64, device, device, draw_offset, focused, THEME, accent)
    }

    /// Metrics for the 8×16 pitch at `scale`.
    fn metrics(scale: f64) -> PainterMetrics {
        PainterMetrics::new(8.0, 16.0, ASCENT, scale).expect("test metrics are valid")
    }

    /// A canvas of `frame`'s device size with the frame painted into it.
    fn painted(terminal: &mut Terminal, frame: &FrameInput, scale: f64) -> Canvas {
        let (w, h) = frame.device_size();
        let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
        paint_frame(&mut canvas, &metrics(scale), frame, terminal);
        canvas
    }

    /// A colour's bytes in the canvas' memory order: blue, green, red, alpha.
    fn bytes(rgb: [u8; 3]) -> [u8; 4] {
        [rgb[2], rgb[1], rgb[0], 255]
    }

    /// The theme background's canvas bytes.
    fn theme_bytes() -> [u8; 4] {
        bytes(THEME.background)
    }

    /// Collect the frame's cells once, for the tests that assert what the
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

    /// Assert every pixel of a cell's device rectangle is `expected`.
    fn cell_is(
        canvas: &Canvas,
        m: &PainterMetrics,
        col: i32,
        row: i32,
        expected: [u8; 4],
        label: &str,
    ) {
        let cell = m.cell_rect(col, row);
        for y in cell.y()..cell.y() + cell.h() {
            for x in cell.x()..cell.x() + cell.w() {
                assert_eq!(
                    canvas.pixel(
                        u32::try_from(x).expect("x fits"),
                        u32::try_from(y).expect("y fits")
                    ),
                    Some(expected),
                    "{label} at ({x}, {y})"
                );
            }
        }
    }

    /// The scales every seam test runs at (design decision 10).
    const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 1.8];

    /// A region of one background colour is the same bytes everywhere, at
    /// every scale: no seams between the cells and runs the backgrounds
    /// merge into.
    #[test]
    fn a_region_of_one_background_has_no_seams() {
        for scale in SCALES {
            let mut terminal = terminal();
            terminal.push_pty_data(b"\x1b[41m........\x1b[0m");
            // The pinned vt's palette gives the SGR colour its own RGB, so
            // the expectation is the cell's own background, not a hard-coded
            // one.
            let walked = cells(&mut terminal);
            let bg = bytes([walked[0].bg.r, walked[0].bg.g, walked[0].bg.b]);
            let frame = frame(false, None, 0.0, scale);
            let canvas = painted(&mut terminal, &frame, scale);
            let row = metrics(scale).run_rect(0, 0, 8);
            for y in row.y()..row.y() + row.h() {
                for x in 0..canvas.size().0 {
                    assert_eq!(
                        canvas.pixel(x, u32::try_from(y).expect("y fits u32")),
                        Some(bg),
                        "every pixel of the row is the cell's background at {scale}"
                    );
                }
            }
            // The row below is the theme background: the fill did not bleed.
            let below = metrics(scale).cell_rect(0, 1);
            assert_eq!(
                canvas.pixel(0, u32::try_from(below.y()).expect("fits")),
                Some(theme_bytes()),
                "the row below stays theme at {scale}"
            );
        }
    }

    /// Two runs of different colour meet on one snapped edge: no gap and no
    /// overlap, at every scale.
    #[test]
    fn two_runs_meet_on_one_snapped_edge() {
        for scale in SCALES {
            let mut terminal = terminal();
            terminal.push_pty_data(b"\x1b[41m....\x1b[42m....\x1b[0m");
            let walked = cells(&mut terminal);
            let first = bytes([walked[0].bg.r, walked[0].bg.g, walked[0].bg.b]);
            let second = bytes([walked[4].bg.r, walked[4].bg.g, walked[4].bg.b]);
            assert_ne!(first, second, "the two runs have different colours");

            let frame = frame(false, None, 0.0, scale);
            let canvas = painted(&mut terminal, &frame, scale);
            let boundary = metrics(scale).cell_rect(4, 0).x();
            let row = metrics(scale).run_rect(0, 0, 8);
            for y in row.y()..row.y() + row.h() {
                let y = u32::try_from(y).expect("y fits u32");
                assert_eq!(
                    canvas.pixel(u32::try_from(boundary - 1).expect("fits"), y),
                    Some(first),
                    "the last pixel of the first run sits left of the edge at {scale}"
                );
                assert_eq!(
                    canvas.pixel(u32::try_from(boundary).expect("fits"), y),
                    Some(second),
                    "the first pixel of the second run sits on the edge at {scale}"
                );
            }
            // The whole row is one run's colour or the other's: nothing
            // blended in between.
            for x in 0..canvas.size().0 {
                let pixel = canvas
                    .pixel(x, u32::try_from(row.y()).expect("y fits"))
                    .expect("in bounds");
                assert!(
                    pixel == first || pixel == second,
                    "pixel {x} of the row is one run's colour at {scale}"
                );
            }
        }
    }

    /// A wide glyph occupies two columns — the head cell and its spacer
    /// tail — and, with no style of its own, both show the theme
    /// background the painter filled.
    #[test]
    fn wide_cells_fill_two_columns() {
        for scale in SCALES {
            let mut terminal = terminal();
            terminal.push_pty_data("\u{6f22}".as_bytes());
            // The frame walk emits the tail before its head (the one-cell
            // lookahead), so the tests find them by their width.
            let walked = cells(&mut terminal);
            let head = walked
                .iter()
                .find(|cell| cell.wide == Wide::WIDE)
                .expect("the wide head");
            let tail = walked
                .iter()
                .find(|cell| cell.wide == Wide::SPACER_TAIL)
                .expect("the spacer tail");
            assert_eq!(head.x, 0);
            assert_eq!(tail.x, 1);
            assert!(!head.has_bg && !tail.has_bg, "no explicit background");

            let frame = frame(false, None, 0.0, scale);
            let canvas = painted(&mut terminal, &frame, scale);
            let m = metrics(scale);
            for col in [0, 1] {
                cell_is(
                    &canvas,
                    &m,
                    col,
                    0,
                    theme_bytes(),
                    "an unstyled wide cell shows the theme",
                );
            }
        }
    }

    /// A styled wide glyph's background covers both of its columns: the
    /// head cell and its spacer tail carry the same explicit colour, and
    /// the run fills them as two adjacent rectangles that share the snapped
    /// edge.
    #[test]
    fn a_styled_wide_glyph_fills_both_columns() {
        for scale in SCALES {
            let mut terminal = terminal();
            terminal.push_pty_data(b"\x1b[44m\xe6\xbc\xa2\x1b[0m");
            let walked = cells(&mut terminal);
            let head = walked
                .iter()
                .find(|cell| cell.wide == Wide::WIDE)
                .expect("the wide head");
            let tail = walked
                .iter()
                .find(|cell| cell.wide == Wide::SPACER_TAIL)
                .expect("the spacer tail");
            assert!(head.has_bg && tail.has_bg, "both cells carry the style");
            assert_eq!(head.bg, tail.bg, "the tail keeps the head's colour");

            let bg = bytes([head.bg.r, head.bg.g, head.bg.b]);
            let frame = frame(false, None, 0.0, scale);
            let canvas = painted(&mut terminal, &frame, scale);
            let m = metrics(scale);
            for col in [0, 1] {
                cell_is(&canvas, &m, col, 0, bg, "the wide glyph's background");
            }
            // Column 2, past the wide glyph, stays theme.
            let next = m.cell_rect(2, 0);
            assert_eq!(
                canvas.pixel(
                    u32::try_from(next.x()).expect("fits"),
                    u32::try_from(next.y()).expect("fits")
                ),
                Some(theme_bytes())
            );
        }
    }

    /// An inverse cell's background is the terminal's default foreground,
    /// and a plain cell shows the theme background — the theme fallback.
    #[test]
    fn inverse_and_theme_fallback_colours() {
        for scale in SCALES {
            let mut terminal = terminal();
            terminal.push_pty_data(b"\x1b[7m..\x1b[0m......");
            let frame = frame(false, None, 0.0, scale);
            let canvas = painted(&mut terminal, &frame, scale);
            let default = terminal.colors().foreground;
            let default_fg = bytes([default.r, default.g, default.b]);
            let m = metrics(scale);
            for col in [0, 1] {
                cell_is(
                    &canvas,
                    &m,
                    col,
                    0,
                    default_fg,
                    "the inverse cell carries the default foreground",
                );
            }
            let plain = m.cell_rect(4, 1);
            assert_eq!(
                canvas.pixel(
                    u32::try_from(plain.x()).expect("fits"),
                    u32::try_from(plain.y()).expect("fits")
                ),
                Some(theme_bytes()),
                "the plain cell shows the theme background at {scale}"
            );
        }
    }

    /// The underline and strikethrough bands have the thickness and
    /// position the snap rule gives them at 1.5 (device pixels, snapped):
    /// the underline hangs from the ascent plus one logical pixel, the
    /// strikethrough sits on the cell's mid-line, both in the cell's
    /// foreground colour — the theme foreground, when the cell names none.
    #[test]
    fn the_bands_have_their_thickness_and_position_at_1_5() {
        let scale = 1.5;
        let mut terminal = terminal();
        // Underline only, strikethrough only, both, and a plain cell.
        terminal.push_pty_data(b"\x1b[4mU\x1b[9mS\x1b[4;9mB\x1b[0m.\x1b[0m");
        let frame = frame(false, None, 0.0, scale);
        let canvas = painted(&mut terminal, &frame, scale);
        let fg = bytes(THEME.foreground);

        // The snapped device band rows, computed from the rule itself: a
        // logical edge snaps to `round(edge * scale)` device pixels.
        let band_rows = |top: f64| -> (i32, i32) {
            let y0 = device_px(top * scale);
            let y1 = device_px((top + 1.0) * scale);
            (y0, y1.max(y0 + 1))
        };
        let underline = band_rows(0.0 * 16.0 + ASCENT + 1.0);
        let strike = band_rows(16.0 / 2.0 - 0.5);

        let band_is = |canvas: &Canvas, x: i32, rows: (i32, i32), color: [u8; 4], label: &str| {
            for y in rows.0..rows.1 {
                assert_eq!(
                    canvas.pixel(
                        u32::try_from(x).expect("x fits"),
                        u32::try_from(y).expect("y fits")
                    ),
                    Some(color),
                    "{label} band row {y} at column {x}"
                );
            }
            // One row above and below: the band stops there.
            for y in [rows.0 - 1, rows.1] {
                assert_eq!(
                    canvas.pixel(
                        u32::try_from(x).expect("x fits"),
                        u32::try_from(y).expect("y fits")
                    ),
                    Some(theme_bytes()),
                    "the {label} band stops at row {y}"
                );
            }
        };

        // Cells 0 and 1 carry one band each, across their whole column
        // span; the band rows sit where the rule puts them.
        for (cell, rows) in [(0, underline), (1, strike)] {
            let x0 = device_px(f64::from(cell) * 8.0 * scale);
            let x1 = device_px(f64::from(cell + 1) * 8.0 * scale);
            for x in x0..x1 {
                band_is(&canvas, x, rows, fg, "styled");
            }
        }

        // The cell with both flags carries both bands.
        let both_x = device_px(2.0 * 8.0 * scale) + 2;
        band_is(&canvas, both_x, underline, fg, "underline");
        band_is(&canvas, both_x, strike, fg, "strikethrough");

        // The plain cell carries none.
        let plain_x = device_px(3.0 * 8.0 * scale) + 2;
        for y in [underline.0, strike.0] {
            assert_eq!(
                canvas.pixel(
                    u32::try_from(plain_x).expect("fits"),
                    u32::try_from(y).expect("fits")
                ),
                Some(theme_bytes()),
                "the plain cell has no band"
            );
        }
    }

    /// A band spans its head cell's width only: the wide glyph's spacer
    /// tail is skipped, so its underline does not continue across the tail.
    #[test]
    fn a_wide_glyphs_band_stops_at_its_head_cell() {
        let scale = 1.5;
        let mut terminal = terminal();
        terminal.push_pty_data("\x1b[4m漢\x1b[0m".as_bytes());
        let frame = frame(false, None, 0.0, scale);
        let canvas = painted(&mut terminal, &frame, scale);
        let fg = bytes(THEME.foreground);
        // Head cell columns [0, 12) at 1.5; the tail is [12, 24).
        let underline_y = device_px((ASCENT + 1.0) * scale);
        for x in [0, 11] {
            assert_eq!(
                canvas.pixel(
                    u32::try_from(x).expect("fits"),
                    u32::try_from(underline_y).expect("fits")
                ),
                Some(fg),
                "the underline spans the head cell at x = {x}"
            );
        }
        for x in [12, 23] {
            assert_eq!(
                canvas.pixel(
                    u32::try_from(x).expect("fits"),
                    u32::try_from(underline_y).expect("fits")
                ),
                Some(theme_bytes()),
                "the spacer tail has no underline at x = {x}"
            );
        }
    }

    /// The focus accent draws only when focused, and fills the four bands
    /// the inset stroke covers: here at scale 1, where the bands are whole
    /// device pixels and read back exactly.
    #[test]
    fn the_accent_draws_only_when_focused() {
        let accent = Accent::new([255, 0, 0], NonZeroU16::new(2).expect("nonzero"));
        let mut terminal = terminal();
        let unfocused = painted(&mut terminal, &frame(false, Some(accent), 0.0, 1.0), 1.0);
        assert_eq!(
            unfocused.pixel(0, 0),
            Some(theme_bytes()),
            "no accent while unfocused"
        );
        assert_eq!(
            unfocused.pixel(32, 32),
            Some(theme_bytes()),
            "the centre stays theme"
        );

        let focused = painted(&mut terminal, &frame(true, Some(accent), 0.0, 1.0), 1.0);
        let accent_bytes = bytes([255, 0, 0]);
        // The stroke of width 2 covers the two outermost rows and columns.
        for (x, y) in [
            (0, 0),
            (1, 1),
            (32, 0),
            (0, 32),
            (63, 63),
            (63, 32),
            (32, 63),
        ] {
            assert_eq!(
                focused.pixel(x, y),
                Some(accent_bytes),
                "the accent covers ({x}, {y})"
            );
        }
        assert_eq!(
            focused.pixel(32, 32),
            Some(theme_bytes()),
            "the centre stays theme"
        );
        assert_eq!(
            focused.pixel(32, 2),
            Some(theme_bytes()),
            "the stroke does not reach past its width"
        );
    }

    /// No accent configured: a focused frame draws none either.
    #[test]
    fn a_focused_frame_without_an_accent_draws_none() {
        let mut terminal = terminal();
        let canvas = painted(&mut terminal, &frame(true, None, 0.0, 1.0), 1.0);
        assert_eq!(canvas.pixel(0, 0), Some(theme_bytes()));
    }

    /// The accent is not translated by the draw offset while the grid is:
    /// the border stays on the window's edges and the grid shifts.
    #[test]
    fn the_accent_is_not_translated_by_the_draw_offset() {
        let accent = Accent::new([255, 0, 0], NonZeroU16::new(2).expect("nonzero"));
        let mut terminal = terminal();
        terminal.push_pty_data(b"\x1b[41m........\x1b[0m");
        let bg = {
            let walked = cells(&mut terminal);
            bytes([walked[0].bg.r, walked[0].bg.g, walked[0].bg.b])
        };
        let frame = frame(true, Some(accent), 5.0, 1.0);
        let canvas = painted(&mut terminal, &frame, 1.0);
        let accent_bytes = bytes([255, 0, 0]);
        // The accent hugs the window's edges, unmoved. Row 32 is the third
        // row's area, clear of the accent's top and bottom bands.
        assert_eq!(canvas.pixel(0, 32), Some(accent_bytes), "left edge accent");
        assert_eq!(
            canvas.pixel(63, 32),
            Some(accent_bytes),
            "right edge accent"
        );
        // The grid shifted right by the offset's device pixels: the strip
        // before it is theme, the background starts at the shift. Row 8 is
        // inside the background row but below the accent's top band.
        assert_eq!(
            canvas.pixel(4, 8),
            Some(theme_bytes()),
            "the strip before the shift stays theme"
        );
        assert_eq!(canvas.pixel(5, 8), Some(bg), "the grid shifted");
    }

    /// A nonzero draw offset crops the grid at the canvas' edges: the
    /// shifted cells clip where they leave the surface, and nothing panics.
    #[test]
    fn a_nonzero_draw_offset_crops_the_grid() {
        for (scale, offset) in [(1.0, 5.0), (1.5, 3.0)] {
            let mut terminal = terminal();
            terminal.push_pty_data(b"\x1b[41m........\x1b[0m");
            let bg = {
                let walked = cells(&mut terminal);
                bytes([walked[0].bg.r, walked[0].bg.g, walked[0].bg.b])
            };
            let frame = frame(false, None, offset, scale);
            let canvas = painted(&mut terminal, &frame, scale);
            let shift = device_px(offset * scale);
            let m = metrics(scale);
            // The assertions run inside the background row (row 0); the
            // frame has no accent, so nothing else draws there.
            let y = u32::try_from(m.cell_rect(0, 0).y() + 1).expect("fits");
            // The strip before the shift is theme background.
            for x in 0..shift {
                assert_eq!(
                    canvas.pixel(u32::try_from(x).expect("fits"), y),
                    Some(theme_bytes()),
                    "the cropped strip stays theme at scale {scale}"
                );
            }
            // The shifted row starts at the shift and runs to the canvas'
            // right edge, where the last cells clip.
            for x in shift..i32::try_from(canvas.size().0).expect("fits") {
                assert_eq!(
                    canvas.pixel(u32::try_from(x).expect("fits"), y),
                    Some(bg),
                    "the shifted row fills to the right edge at scale {scale}"
                );
            }
        }
    }

    /// A frame of an empty grid is the theme background and nothing else.
    #[test]
    fn an_empty_grid_frame_is_the_theme_background() {
        for scale in SCALES {
            let mut terminal = terminal();
            let frame = frame(false, None, 0.0, scale);
            let canvas = painted(&mut terminal, &frame, scale);
            let theme = theme_bytes();
            for y in 0..canvas.size().1 {
                for x in 0..canvas.size().0 {
                    assert_eq!(
                        canvas.pixel(x, y),
                        Some(theme),
                        "the empty frame is all theme at ({x}, {y}) scale {scale}"
                    );
                }
            }
        }
    }

    /// A terminal with no live frame still draws the theme background and
    /// the accent: the degraded draw.
    #[test]
    fn a_frame_that_cannot_open_draws_the_background_and_the_accent() {
        let accent = Accent::new([255, 0, 0], NonZeroU16::new(2).expect("nonzero"));
        let mut terminal = uninitialized_terminal();
        let frame = frame(true, Some(accent), 0.0, 1.0);
        let canvas = painted(&mut terminal, &frame, 1.0);
        assert_eq!(canvas.pixel(32, 32), Some(theme_bytes()));
        assert_eq!(canvas.pixel(0, 32), Some(bytes([255, 0, 0])));
        assert_eq!(canvas.pixel(32, 0), Some(bytes([255, 0, 0])));
    }
}
