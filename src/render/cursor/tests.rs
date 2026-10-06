use super::super::canvas::Canvas;
use super::super::geom::{DeviceRect, FrameInput, PainterMetrics, device_px};
use super::super::painter::paint_frame;
use super::{CursorShape, cursor_shape, glyph_redraw};
use crate::fontconfig::ThemeColours;
use crate::guard::Poisoned;
use crate::term::cells::{Cursor, CursorStyle};
use crate::term::{PngDecoder, PtySink, Terminal};

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

/// The test theme, distinct from every colour the tests draw — including
/// the pinned vt's own default colours, which the cursor draws.
const THEME: ThemeColours = ThemeColours {
    background: [10, 20, 30],
    foreground: [200, 150, 100],
};

/// Frame input for a 64×64 logical frame at `scale`, with focus and
/// accent on demand.
fn frame(scale: f64, focused: bool, accent: Option<crate::layout::Accent>) -> FrameInput {
    let device = u32::try_from(device_px(f64::from(64) * scale)).expect("frame fits u32");
    FrameInput::new(64, 64, device, device, 0.0, focused, THEME, accent)
}

/// Metrics for the 8×16 pitch at `scale`.
fn metrics(scale: f64) -> PainterMetrics {
    PainterMetrics::new(8.0, 16.0, 12.0, scale).expect("test metrics are valid")
}

/// A canvas of `frame`'s device size with the frame painted into it.
fn painted(terminal: &mut Terminal, scale: f64) -> Canvas {
    let frame = frame(scale, false, None);
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

/// The terminal's default foreground, the colour every cursor shape
/// draws in — read off the terminal's opened frame, the same read
/// `paint` does (the frame colours are captured at `frame_begin`).
fn cursor_bytes(terminal: &mut Terminal) -> [u8; 4] {
    assert!(terminal.frame_begin(), "frame began");
    let default = terminal.colors().foreground;
    terminal.frame_end();
    bytes([default.r, default.g, default.b])
}

/// The scales every seam test runs at (design decision 10).
const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 1.8];

/// A cursor at `(x, y)` with `style`.
fn cursor(x: i32, y: i32, style: CursorStyle) -> Cursor {
    Cursor {
        has_value: true,
        x,
        y,
        style,
        wide_tail: false,
    }
}

/// A device coordinate as the canvas' unsigned pixel index.
fn px(value: i32) -> u32 {
    u32::try_from(value).expect("the coordinate fits u32")
}

/// Assert every pixel of the device rectangle is `expected`.
fn rect_is(canvas: &Canvas, rect: &DeviceRect, expected: [u8; 4], label: &str) {
    for y in rect.y()..rect.y() + rect.h() {
        for x in rect.x()..rect.x() + rect.w() {
            assert_eq!(
                canvas.pixel(px(x), px(y)),
                Some(expected),
                "{label} at ({x}, {y})"
            );
        }
    }
}

// The pure geometry: the device rectangles below are computed by hand
// from `cursor_shape`'s formulas — the logical rectangle snapped edge by
// edge, `round(edge * scale)` per device edge, on the 8×16 test pitch.

/// The bar, underline and block cursor rectangles equal the GTK snapped
/// rectangles, computed by hand from the formulas, at every scale the
/// painter tests, for a cursor away from the origin (cell (2, 1)).
#[test]
fn fill_shapes_match_the_hand_computed_rectangles() {
    for scale in SCALES {
        let m = metrics(scale);
        // Cell (2, 1)'s snapped device edges on the 8×16 pitch:
        // x [16·s, 24·s], y [16·s, 32·s], rounded per edge.
        let (x0, x1) = (device_px(16.0 * scale), device_px(24.0 * scale));
        let (y0, y1) = (device_px(16.0 * scale), device_px(32.0 * scale));
        let at = |cursor: Cursor| cursor_shape(&cursor, &m);

        // Bar: two logical pixels wide, the cell tall. At 1.5 that is 3
        // device pixels wide.
        let bar_w = device_px(18.0 * scale) - x0;
        assert_eq!(
            at(cursor(2, 1, CursorStyle::Bar)),
            CursorShape::Fill(DeviceRect::new(x0, y0, bar_w, y1 - y0).expect("positive")),
            "bar at {scale}"
        );

        // Underline: two logical pixels tall at the cell's bottom.
        let u_y0 = device_px(30.0 * scale);
        assert_eq!(
            at(cursor(2, 1, CursorStyle::Underline)),
            CursorShape::Fill(DeviceRect::new(x0, u_y0, x1 - x0, y1 - u_y0).expect("positive")),
            "underline at {scale}"
        );

        // Block: the whole cell.
        assert_eq!(
            at(cursor(2, 1, CursorStyle::Block)),
            CursorShape::Fill(DeviceRect::new(x0, y0, x1 - x0, y1 - y0).expect("positive")),
            "block at {scale}"
        );
    }
}

/// The hollow block's four bands at cell (2, 1) equal the hand-computed
/// snapped rectangles at every scale the painter tests.
#[test]
fn hollow_bands_match_the_hand_computed_rectangles() {
    for scale in SCALES {
        let m = metrics(scale);
        let x0 = device_px(16.0 * scale);
        let x1 = device_px(24.0 * scale);
        let y0 = device_px(16.0 * scale);
        let y1 = device_px(32.0 * scale);
        // The bands' shared logical edges, snapped per edge: the horizontal
        // bands span the full width, the vertical bands run between them.
        let inner_y0 = device_px(17.0 * scale);
        let outer_y1 = device_px(31.0 * scale);
        let left_w = device_px(17.0 * scale) - x0;
        let right_x0 = device_px(23.0 * scale);
        let expected = [
            DeviceRect::new(x0, y0, x1 - x0, inner_y0 - y0).expect("positive"), // top
            DeviceRect::new(x0, outer_y1, x1 - x0, y1 - outer_y1).expect("positive"), // bottom
            DeviceRect::new(x0, inner_y0, left_w, outer_y1 - inner_y0).expect("positive"), // left
            DeviceRect::new(right_x0, inner_y0, x1 - right_x0, outer_y1 - inner_y0)
                .expect("positive"), // right
        ];
        assert_eq!(
            cursor_shape(&cursor(2, 1, CursorStyle::BlockHollow), &m),
            CursorShape::Hollow(expected),
            "hollow bands at {scale}"
        );
    }
}

/// The hollow block's four bands tile the ring exactly — every ring pixel
/// covered once, the interior empty — at every scale the painter tests,
/// for cursors at the origin, mid-grid and the last cell.
#[test]
fn the_hollow_bands_tile_the_ring_exactly() {
    use std::collections::HashSet;
    for scale in SCALES {
        let m = metrics(scale);
        for (x, y) in [(0, 0), (2, 1), (7, 3)] {
            let cell = m.cell_rect(x, y);
            let CursorShape::Hollow(bands) =
                cursor_shape(&cursor(x, y, CursorStyle::BlockHollow), &m)
            else {
                unreachable!("hollow is hollow")
            };
            // Every device pixel of a rectangle, row-major.
            let rect_pixels = |rect: &DeviceRect| {
                let (x0, x1) = (rect.x(), rect.x() + rect.w());
                let (y0, y1) = (rect.y(), rect.y() + rect.h());
                (y0..y1).flat_map(move |y| (x0..x1).map(move |x| (x, y)))
            };
            // Collect the bands' pixels, rejecting any pixel twice.
            let mut ring: HashSet<(i32, i32)> = HashSet::new();
            for band in &bands {
                for pixel in rect_pixels(band) {
                    assert!(
                        ring.insert(pixel),
                        "band pixel {pixel:?} covered twice at {scale}"
                    );
                }
            }
            // The ring sits inside the cell and leaves the interior
            // rectangle — one logical pixel in from each snapped edge —
            // empty; ring plus interior is exactly the cell.
            let interior = m.cell_sub_rect(x, y, 1.0, 1.0, m.cell_w() - 2.0, m.cell_h() - 2.0);
            let mut covered = ring;
            for pixel in rect_pixels(&interior) {
                assert!(covered.insert(pixel), "the interior overlaps at {scale}");
            }
            let mut full: HashSet<(i32, i32)> = HashSet::new();
            for pixel in rect_pixels(&cell) {
                assert!(full.insert(pixel), "cell pixel {pixel:?} counted twice");
            }
            assert_eq!(
                covered, full,
                "the ring plus the interior is the whole cell at {scale}"
            );
        }
    }
}

/// A bar cursor two logical pixels wide is three device pixels wide at
/// 1.5 (design decision 10's named case).
#[test]
fn a_bar_cursor_is_three_device_pixels_wide_at_1_5() {
    let m = metrics(1.5);
    let CursorShape::Fill(bar) = cursor_shape(&cursor(0, 0, CursorStyle::Bar), &m) else {
        unreachable!("bar is a fill")
    };
    assert_eq!(bar.w(), 3, "2 logical pixels at 1.5");
    assert_eq!(bar.h(), m.cell_rect(0, 0).h(), "the bar is the cell tall");
}

/// The block cursor redraws the glyph under it only for a block with
/// text and no wide tail — the GTK path's condition.
#[test]
fn the_glyph_redraw_condition_follows_the_gtk_rule() {
    let mut block = cursor(0, 0, CursorStyle::Block);
    assert!(glyph_redraw(&block, b"X"), "a block with text redraws");
    assert!(!glyph_redraw(&block, b""), "no text, no redraw");
    block.wide_tail = true;
    assert!(!glyph_redraw(&block, b"X"), "a wide tail suppresses it");
    block.wide_tail = false;
    block.style = CursorStyle::Bar;
    assert!(!glyph_redraw(&block, b"X"), "only a block redraws");
    block.style = CursorStyle::BlockHollow;
    assert!(!glyph_redraw(&block, b"X"), "hollow does not redraw");
}

/// A wide-tail block cursor keeps its full shape — the GTK path draws
/// the shape over the tail cell too; only the glyph redraw is
/// suppressed (pinned by `the_glyph_redraw_condition_follows_the_gtk_rule`).
#[test]
fn a_wide_tail_cursor_keeps_its_shape() {
    let m = metrics(1.5);
    let mut wide = cursor(0, 0, CursorStyle::Block);
    wide.wide_tail = true;
    assert_eq!(
        cursor_shape(&wide, &m),
        cursor_shape(&cursor(0, 0, CursorStyle::Block), &m),
        "the shape ignores the wide tail"
    );
}

/// A cursor covers its own cell only: a block cursor on a wide glyph's
/// head cell fills the head, and the tail cell keeps the cell layers'
/// pixels.
#[test]
fn a_cursor_on_a_wide_head_covers_the_head_cell_only() {
    let scale = 1.5;
    let mut terminal = terminal();
    terminal.push_pty_data("漢".as_bytes());
    // The cursor back onto the wide head cell.
    terminal.push_pty_data(b"\x1b[1;1H");
    let default_fg = cursor_bytes(&mut terminal);
    let canvas = painted(&mut terminal, scale);
    let m = metrics(scale);
    rect_is(&canvas, &m.cell_rect(0, 0), default_fg, "the head cell");
    rect_is(&canvas, &m.cell_rect(1, 0), theme_bytes(), "the tail cell");
}

/// The four DECSCUSR-reachable cursor shapes paint exactly the device
/// rectangles the pure geometry names, in the terminal's default
/// foreground; the underline sits at the cell's bottom.
#[test]
fn the_frame_cursor_paints_its_shape() {
    let cases: [(&[u8], CursorStyle); 3] = [
        (b"\x1b[2 q", CursorStyle::Block),
        (b"\x1b[4 q", CursorStyle::Underline),
        (b"\x1b[6 q", CursorStyle::Bar),
    ];
    for (sequence, style) in cases {
        for scale in SCALES {
            let mut terminal = terminal();
            terminal.push_pty_data(sequence);
            terminal.push_pty_data(b"X");
            terminal.push_pty_data(b"\x1b[1;1H");
            let default_fg = cursor_bytes(&mut terminal);
            let walked_cursor = {
                assert!(terminal.frame_begin(), "frame began");
                let walked = terminal.cursor().expect("a visible cursor");
                terminal.frame_end();
                walked
            };
            assert_eq!(walked_cursor.style, style, "{sequence:?} drives {style:?}");
            assert_ne!(
                default_fg,
                theme_bytes(),
                "the default foreground is distinct from the theme"
            );
            let canvas = painted(&mut terminal, scale);
            let m = metrics(scale);
            let shape = cursor_shape(&walked_cursor, &m);
            let CursorShape::Fill(rect) = shape else {
                unreachable!("DECSCUSR styles are fills")
            };
            rect_is(&canvas, &rect, default_fg, &format!("{style:?} at {scale}"));
            if style == CursorStyle::Underline {
                // The band above the underline stays cell background.
                let above = m.cell_sub_rect(0, 0, 0.0, 0.0, m.cell_w(), m.cell_h() - 2.0);
                rect_is(&canvas, &above, theme_bytes(), "above the underline");
            }
        }
    }
}

/// A hidden cursor draws nothing: the frame protocol reports no cursor,
/// the layer fills nothing, and the cell keeps the cell layers' pixels.
#[test]
fn a_hidden_cursor_draws_nothing() {
    let scale = 1.5;
    let mut terminal = terminal();
    terminal.push_pty_data(b"\x1b[?25lX");
    assert!(
        terminal.cursor().is_none(),
        "the hidden cursor is not reported"
    );
    let canvas = painted(&mut terminal, scale);
    let m = metrics(scale);
    rect_is(
        &canvas,
        &m.cell_rect(0, 0),
        theme_bytes(),
        "the hidden cell",
    );
}

/// The cursor draws above the backgrounds and the sprites and below the
/// focus accent: over a red cell and a red sprite the shape reads the
/// default foreground, and the accent's corner overwrites the cursor.
#[test]
fn the_cursor_draws_above_the_cells_and_below_the_accent() {
    use std::num::NonZeroU16;
    let scale = 1.0;
    let accent = crate::layout::Accent::new([0, 0, 255], NonZeroU16::new(2).expect("nonzero"));
    let m = metrics(scale);

    // Above the background: a red cell under a block cursor.
    let mut bg_terminal = terminal();
    bg_terminal.push_pty_data(b"\x1b[41m \x1b[1;1H");
    let default_fg = cursor_bytes(&mut bg_terminal);
    let unfocused = frame(scale, false, None);
    let (w, h) = unfocused.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    paint_frame(&mut canvas, &m, &unfocused, &mut bg_terminal);
    rect_is(
        &canvas,
        &m.cell_rect(0, 0),
        default_fg,
        "over the background",
    );

    // Above the sprite: a red full-block sprite under the block cursor.
    let mut sprite_terminal = terminal();
    sprite_terminal.push_pty_data(b"\x1b[31m\xE2\x96\x88\x1b[1;1H");
    let walked = {
        assert!(sprite_terminal.frame_begin(), "frame began");
        let mut walked = Vec::new();
        while let Some(cell) = sprite_terminal.cell_next() {
            walked.push(cell);
        }
        sprite_terminal.frame_end();
        walked
    };
    let sprite = bytes([walked[0].fg.r, walked[0].fg.g, walked[0].fg.b]);
    assert_ne!(
        sprite, default_fg,
        "the sprite colour differs from the cursor's"
    );
    let canvas = painted(&mut sprite_terminal, scale);
    rect_is(&canvas, &m.cell_rect(0, 0), default_fg, "over the sprite");

    // Below the accent: the focused frame's corner bands overwrite the
    // cursor's corner, the interior keeps the cursor.
    let focused = frame(scale, true, Some(accent));
    let (w, h) = focused.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    paint_frame(&mut canvas, &m, &focused, &mut sprite_terminal);
    let accent_bytes = bytes([0, 0, 255]);
    assert_eq!(
        canvas.pixel(0, 0),
        Some(accent_bytes),
        "the accent is on top"
    );
    assert_eq!(canvas.pixel(1, 1), Some(accent_bytes), "the accent's band");
    assert_eq!(
        canvas.pixel(3, 3),
        Some(default_fg),
        "the cursor shows past the accent's stroke"
    );
}
