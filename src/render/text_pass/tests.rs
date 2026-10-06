//! Display-free pixel tests for the text pass (replace-gtk-with-wayland
//! D10): one test per scenario of the spec requirement "Cell text on the
//! device pixel lattice", plus the colour, layer-order, cursor-redraw and
//! draw-offset rules the old GTK walk pins. Real system fonts, real
//! shaping, no display. The font-dependent tests skip with a printed
//! message only when the family is not installed; a broken `FontBook` or a
//! failed lookup is an `expect`, so a regression cannot hide behind "not
//! installed".
//!
//! The frames run at the font's own cell pitch — the production pairing,
//! where [`CellMetrics`] and [`PainterMetrics`] describe the same grid and
//! a glyph's ink fits inside its cell.

use super::super::canvas::Canvas;
use super::super::geom::{DeviceRect, FrameInput, PainterMetrics, device_px};
use super::super::painter::paint_frame;
use super::super::sprite;
use super::{CursorText, TextPass, text_owns};
use crate::fontconfig::{FontConfig, ThemeColours};
use crate::guard::Poisoned;
use crate::render::font::FontBook;
use crate::render::text_pass::test_support::TestPass;
use crate::render::text_pass::test_support::{FAMILY, SIZE, from_faces};
use crate::term::cells::{CELL_TEXT_CAP, Cell, StyleFlags, Wide};
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

/// The test theme, distinct from every colour the tests draw.
const THEME: ThemeColours = ThemeColours {
    background: [10, 20, 30],
    foreground: [200, 150, 100],
};

/// Hides the frame's cursor (DECTCEM) for the tests that are not about the
/// cursor.
const HIDE_CURSOR: &[u8] = b"\x1b[?25l";

/// The test rig: the shared text pass plus the 8-column, 4-row frame at
/// the font's own cell pitch. A `cell_h` the test names explicitly (the
/// 19-logical-pixel scenario) replaces the font's row height.
struct Rig {
    test: TestPass,
    cell_w: i32,
    cell_h: i32,
    ascent: f64,
}

impl Rig {
    /// The rig over the test family, or `None` (printed) when the machine
    /// lacks the family.
    fn new() -> Option<Self> {
        let test = super::super::text_pass::test_support::text_pass()?;
        let (cell_w, cell_h) = (test.cell_metrics.cell_w(), test.cell_metrics.cell_h());
        Some(Rig {
            ascent: f64::from(test.cell_metrics.ascent()),
            test,
            cell_w,
            cell_h,
        })
    }

    /// A terminal at the font's pitch, or at an explicit row height.
    fn terminal(&self, cell_h: i32) -> Terminal {
        let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
        assert!(terminal.push_size(8, 4, self.cell_w, cell_h));
        terminal
    }

    /// The frame's metrics at `scale`, with the font's truncated ascent.
    fn metrics(&self, scale: f64, cell_h: i32) -> PainterMetrics {
        PainterMetrics::new(
            f64::from(self.cell_w),
            f64::from(cell_h),
            self.ascent,
            scale,
        )
        .expect("test metrics are valid")
    }

    /// Frame input for the 8-column, 4-row frame at `scale`.
    fn frame(&self, scale: f64, cell_h: i32, draw_offset: f64) -> FrameInput {
        let logical_w = f64::from(8 * self.cell_w);
        let logical_h = f64::from(4 * cell_h);
        let device_w = u32::try_from(device_px(logical_w * scale)).expect("frame fits u32");
        let device_h = u32::try_from(device_px(logical_h * scale)).expect("frame fits u32");
        FrameInput::new(
            u32::try_from(device_px(logical_w)).expect("logical fits u32"),
            u32::try_from(device_px(logical_h)).expect("logical fits u32"),
            device_w,
            device_h,
            draw_offset,
            false,
            THEME,
            None,
        )
    }

    /// A canvas of the frame's device size with the frame painted into it.
    fn painted(
        &self,
        terminal: &mut Terminal,
        scale: f64,
        cell_h: i32,
        draw_offset: f64,
    ) -> Canvas {
        let frame = self.frame(scale, cell_h, draw_offset);
        let (w, h) = frame.device_size();
        let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
        paint_frame(
            &mut canvas,
            &self.metrics(scale, cell_h),
            &frame,
            terminal,
            &mut self.pass(),
        );
        canvas
    }

    /// The pass, handed to `paint_frame` per call (a fresh pass per frame
    /// keeps the tests independent; the caches rebuild in microseconds).
    fn pass(&self) -> TextPass {
        TextPass::new(
            crate::render::shape::TextShaper::new(Self::faces(), Self::book()),
            crate::render::glyph::GlyphCache::new(),
            self.test.cell_metrics,
            SIZE,
        )
    }

    /// The family's faces, for `pass`.
    fn faces() -> crate::render::font::FamilyFaces {
        let config = FontConfig {
            family: Some(FAMILY.to_owned()),
            size: SIZE,
        };
        Self::book()
            .family_faces(&config)
            .expect("the family's faces load")
    }

    /// A fresh font book (no state of the rig's is used).
    fn book() -> FontBook {
        FontBook::new().expect("the font book opens")
    }
}

/// A colour's bytes in the canvas' memory order: blue, green, red, alpha.
fn bytes(rgb: [u8; 3]) -> [u8; 4] {
    [rgb[2], rgb[1], rgb[0], 255]
}

fn theme_bytes() -> [u8; 4] {
    bytes(THEME.background)
}

fn theme_fg_bytes() -> [u8; 4] {
    bytes(THEME.foreground)
}

/// The terminal's default colour, read off the opened frame — the same
/// read the cursor layer's redraw uses.
fn default_color(terminal: &mut Terminal, background: bool) -> [u8; 4] {
    assert!(terminal.frame_begin(), "frame began");
    let colors = terminal.colors();
    terminal.frame_end();
    let rgb = if background {
        colors.background
    } else {
        colors.foreground
    };
    bytes([rgb.r, rgb.g, rgb.b])
}

/// The frame's walked cells, for asserting what the terminal reports
/// before asserting what the painter drew.
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

/// The canvas pixels of the `w`×`h` block at `rect`'s top-left corner —
/// the crop the cross-cell comparisons use when the snapped cell sizes
/// differ by a device pixel.
fn block(canvas: &Canvas, rect: DeviceRect, w: i32, h: i32) -> Vec<[u8; 4]> {
    let mut pixels = Vec::new();
    for y in rect.y()..rect.y() + h {
        for x in rect.x()..rect.x() + w {
            pixels.push(
                canvas
                    .pixel(px(x), px(y))
                    .expect("the block is inside the canvas"),
            );
        }
    }
    pixels
}

/// The narrowest common size of `rects`.
fn common_size(rects: &[DeviceRect]) -> (i32, i32) {
    (
        rects.iter().map(DeviceRect::w).min().expect("non-empty"),
        rects.iter().map(DeviceRect::h).min().expect("non-empty"),
    )
}

/// UTF-8 bytes of one code point.
fn utf8(cp: u32) -> Vec<u8> {
    char::from_u32(cp)
        .expect("a scalar")
        .to_string()
        .into_bytes()
}

/// A narrow cell holding `cp`, hand-built for the pure decisions.
fn cell_at(x: i32, cp: u32) -> Cell {
    let text = utf8(cp);
    let len = text.len().min(CELL_TEXT_CAP - 1);
    let mut cell_text = [0; CELL_TEXT_CAP];
    cell_text[..len].copy_from_slice(&text[..len]);
    Cell {
        x,
        text: cell_text,
        len,
        ..Cell::default()
    }
}

/// Whether any pixel of `rect` reads exactly `expected`.
fn contains_pixel(canvas: &Canvas, rect: DeviceRect, expected: [u8; 4]) -> bool {
    (rect.y()..rect.y() + rect.h())
        .flat_map(move |y| (rect.x()..rect.x() + rect.w()).map(move |x| (x, y)))
        .any(|(x, y)| canvas.pixel(px(x), px(y)) == Some(expected))
}

// ---- The scenarios of "Cell text on the device pixel lattice" ----

/// Identical letters render alike at a fractional scale: a row of the same
/// letter at 1.8 draws the same device pixels out of every cell's snapped
/// top-left corner.
#[test]
fn identical_letters_render_alike_at_a_fractional_scale() {
    let Some(rig) = Rig::new() else { return };
    let scale = 1.8;
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"AAAAAAAA");
    let canvas = rig.painted(&mut terminal, scale, rig.cell_h, 0.0);
    let m = rig.metrics(scale, rig.cell_h);
    let rects: Vec<DeviceRect> = (0..8).map(|col| m.cell_rect(col, 0)).collect();
    let (w, h) = common_size(&rects);
    let first = block(&canvas, rects[0], w, h);
    assert!(
        first.iter().any(|pixel| *pixel != theme_bytes()),
        "the glyph has ink, so the comparison is not vacuous"
    );
    for (col, rect) in rects.iter().enumerate().skip(1) {
        assert_eq!(
            block(&canvas, *rect, w, h),
            first,
            "column {col} renders column 0's pixels at {scale}"
        );
    }
}

/// Cell height that is not a whole device pixel count: 19 logical pixels
/// at 1.8 — the glyph sits at the same device pixel offset from the top of
/// its cell in every row.
#[test]
fn the_glyph_sits_at_the_same_offset_in_every_row_of_a_19_pixel_cell() {
    let Some(rig) = Rig::new() else { return };
    let scale = 1.8;
    let cell_h = 19;
    let mut terminal = rig.terminal(cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    let canvas = rig.painted(&mut terminal, scale, cell_h, 0.0);
    let m = rig.metrics(scale, cell_h);
    let rects: Vec<DeviceRect> = (0..4).map(|row| m.cell_rect(0, row)).collect();
    let (w, h) = common_size(&rects);
    let first = block(&canvas, rects[0], w, h);
    assert!(
        first.iter().any(|pixel| *pixel != theme_bytes()),
        "the glyph has ink, so the comparison is not vacuous"
    );
    for (row, rect) in rects.iter().enumerate().skip(1) {
        assert_eq!(
            block(&canvas, *rect, w, h),
            first,
            "row {row} renders row 0's pixels at {scale}"
        );
    }
}

/// Constrained glyph: the nerd-font icon the constraints scale and centre
/// sits at the same device pixel offset inside each cell at 1.5, at
/// different columns.
#[test]
fn the_constrained_glyph_sits_alike_in_every_column_at_1_5() {
    let Some(rig) = Rig::new() else { return };
    let scale = 1.5;
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    let icon: Vec<u8> = (0..8).flat_map(|_| utf8(0xEA61)).collect();
    terminal.push_pty_data(&icon);
    let walked = cells(&mut terminal);
    let icon_cell = walked
        .iter()
        .find(|cell| cell.len > 0)
        .expect("an icon cell");
    assert_eq!(icon_cell.len, 3, "the icon is one cell's cluster");
    let canvas = rig.painted(&mut terminal, scale, rig.cell_h, 0.0);
    let m = rig.metrics(scale, rig.cell_h);
    let rects: Vec<DeviceRect> = (0..8).map(|col| m.cell_rect(col, 0)).collect();
    let (w, h) = common_size(&rects);
    let first = block(&canvas, rects[0], w, h);
    assert!(
        first.iter().any(|pixel| *pixel != theme_bytes()),
        "the constrained icon has ink, so the comparison is not vacuous"
    );
    for (col, rect) in rects.iter().enumerate().skip(1) {
        assert_eq!(
            block(&canvas, *rect, w, h),
            first,
            "column {col} renders column 0's constrained pixels at {scale}"
        );
    }
}

/// Text sizes stay logical: the cell size from `cell_metrics` and the
/// `PainterMetrics` logical cell at 1.8 equal their values at 1. The reply
/// to `CSI 16 t` is pinned by the row 4.6 cell-metrics test; this pins the
/// painter's inputs.
#[test]
fn text_sizes_stay_logical() {
    let Some(rig) = Rig::new() else { return };
    let (cell_w, cell_h) = rig.test.cell_metrics.cell_size();
    for scale in [1.0, 1.8] {
        let m = rig
            .test
            .cell_metrics
            .painter_metrics(scale)
            .expect("the metrics are valid");
        assert_eq!(m.cell_w(), cell_w, "the logical cell width at {scale}");
        assert_eq!(m.cell_h(), cell_h, "the logical cell height at {scale}");
        assert_eq!(
            m.ascent(),
            f64::from(rig.test.cell_metrics.ascent()),
            "the logical ascent at {scale}"
        );
    }
}

// ---- Plausibility and colour ----

/// At scale 1 the output is plausible: a letter has ink inside its cell
/// and the pixels outside the glyph stay the background.
#[test]
fn a_letter_has_ink_inside_its_cell_at_scale_1() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"M");
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    let cell = m.cell_rect(0, 0);
    let ink = (cell.y()..cell.y() + cell.h())
        .flat_map(move |y| (cell.x()..cell.x() + cell.w()).map(move |x| (x, y)))
        .filter(|(x, y)| canvas.pixel(px(*x), px(*y)).expect("inside") != theme_bytes())
        .count();
    assert!(ink > 0, "the letter drew ink inside its cell");
    assert!(
        ink < usize::try_from(cell.w() * cell.h()).expect("fits"),
        "the glyph does not fill the cell"
    );
    // The pixels outside the glyph — the cell's top corners, far from any
    // stem — stay the background.
    for (x, y) in [
        (cell.x() + 1, cell.y() + 1),
        (cell.x() + cell.w() - 2, cell.y() + 1),
    ] {
        assert_eq!(
            canvas.pixel(px(x), px(y)),
            Some(theme_bytes()),
            "the corner at ({x}, {y}) stays background"
        );
    }
    // The next cell, which holds no glyph, stays bare background.
    let next = m.cell_rect(1, 0);
    assert_eq!(
        canvas.pixel(px(next.x()), px(next.y())),
        Some(theme_bytes()),
        "the empty cell stays background"
    );
}

/// A bold cell and an italic cell draw different pixels than the regular
/// face — the real faces where the family has them, the synthesized ones
/// where it does not.
#[test]
fn a_bold_and_an_italic_cell_draw_unlike_the_regular() {
    let Some(rig) = Rig::new() else { return };
    let paint_style = |sgr: &[u8]| -> Vec<[u8; 4]> {
        let mut terminal = rig.terminal(rig.cell_h);
        terminal.push_pty_data(HIDE_CURSOR);
        let mut data = sgr.to_vec();
        data.extend(b"M\x1b[0m");
        terminal.push_pty_data(&data);
        let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
        let cell = rig.metrics(1.0, rig.cell_h).cell_rect(0, 0);
        block(&canvas, cell, cell.w(), cell.h())
    };
    let regular = paint_style(b"");
    let bold = paint_style(b"\x1b[1m");
    let italic = paint_style(b"\x1b[3m");
    assert!(
        regular.iter().any(|pixel| *pixel != theme_bytes()),
        "the regular cell draws"
    );
    assert_ne!(bold, regular, "the bold cell draws unlike the regular");
    assert_ne!(italic, regular, "the italic cell draws unlike the regular");
}

/// A cell's glyph takes the cell's foreground colour, and a cell without
/// one takes the theme foreground: a full-coverage pixel of the stem reads
/// the exact cached colour.
#[test]
fn a_glyph_takes_the_cell_or_theme_foreground() {
    let Some(rig) = Rig::new() else { return };

    // An explicit SGR red foreground.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"\x1b[31ml\x1b[0m");
    let walked = cells(&mut terminal);
    let cell_with_fg = walked.iter().find(|cell| cell.len > 0).expect("the cell");
    assert!(cell_with_fg.has_fg, "the cell carries the SGR colour");
    let red = bytes([cell_with_fg.fg.r, cell_with_fg.fg.g, cell_with_fg.fg.b]);
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    assert!(
        contains_pixel(&canvas, m.cell_rect(0, 0), red),
        "some stem pixel reads the cell's exact red"
    );

    // No colour named: the theme foreground.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"l");
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    assert!(
        contains_pixel(&canvas, m.cell_rect(0, 0), theme_fg_bytes()),
        "some stem pixel reads the theme foreground"
    );
}

/// An INVISIBLE cell draws nothing. The pinned vt does not map SGR 8
/// (conceal) through the frame protocol — probed, as the sprite tests
/// probed it — so the flag reaches this test through a hand-built cell;
/// [`text_owns`] is the pure per-cell decision [`TextPass::paint`] skips
/// by, and it rejects exactly the skips the sprite pass rejects.
#[test]
fn an_invisible_cell_is_not_owned_by_the_text_pass() {
    let Some(rig) = Rig::new() else { return };
    let m = rig.metrics(1.0, rig.cell_h);
    let frame = rig.frame(1.0, rig.cell_h, 0.0);
    let invisible = Cell {
        flags: StyleFlags::INVISIBLE,
        ..cell_at(0, u32::from(b'A'))
    };
    assert!(
        !text_owns(&m, &frame, &invisible),
        "an INVISIBLE cell draws no text"
    );
    // The other skips hold too, and a plain letter is owned.
    assert!(
        !text_owns(&m, &frame, &Cell::default()),
        "no glyph, no text"
    );
    let tail = Cell {
        wide: Wide::SpacerTail,
        ..cell_at(1, u32::from(b'A'))
    };
    assert!(!text_owns(&m, &frame, &tail), "a spacer tail draws no text");
    assert!(text_owns(&m, &frame, &cell_at(0, u32::from(b'A'))));
}

/// A wide cell draws its head and not its tail: the CJK head cell carries
/// the glyph's ink, the spacer tail — which the walk emits first — stays
/// bare background.
#[test]
fn a_wide_cell_draws_its_head_and_not_its_tail() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data("\u{6f22}".as_bytes());
    let walked = cells(&mut terminal);
    let head = walked
        .iter()
        .find(|cell| cell.wide == Wide::WIDE)
        .expect("the wide head");
    let tail = walked
        .iter()
        .find(|cell| cell.wide == Wide::SPACER_TAIL)
        .expect("the spacer tail");
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    // The walk emits the tail before its head (the one-cell lookahead).
    let tail_index = walked
        .iter()
        .position(|cell| cell.wide == Wide::SPACER_TAIL)
        .expect("the tail is walked");
    let head_index = walked
        .iter()
        .position(|cell| cell.wide == Wide::WIDE)
        .expect("the head is walked");
    assert!(tail_index < head_index, "the tail is emitted first");

    // The head carries the glyph's ink. The glyph colour here is the theme
    // foreground (no SGR colour named), so the blend fraction reads off
    // any channel: a half-covered or better pixel — CJK strokes are thin
    // enough that no pixel may read exactly full coverage.
    let head_rect = m.cell_rect(head.x, head.y);
    let half = i32::from(theme_bytes()[0]).midpoint(i32::from(theme_fg_bytes()[0]));
    let strongest = |rect: DeviceRect| {
        (rect.y()..rect.y() + rect.h())
            .flat_map(move |y| (rect.x()..rect.x() + rect.w()).map(move |x| (x, y)))
            .filter_map(|(x, y)| canvas.pixel(px(x), px(y)))
            .map(|pixel| i32::from(pixel[0]))
            .max()
            .expect("the cell is inside the canvas")
    };
    assert!(
        strongest(head_rect) >= half,
        "the head cell carries the glyph's ink"
    );

    // The tail draws nothing as itself: the pure routing on the walked
    // tail cell declines it. Its area may still carry the head glyph's
    // ink — a CJK glyph's advance spans into the tail and text is not
    // clipped to its cell, on the GTK path or here — so no pixel
    // assertion can separate the two draws.
    let frame = rig.frame(1.0, rig.cell_h, 0.0);
    assert!(
        !text_owns(&m, &frame, tail),
        "the tail is not the text pass's cell"
    );
}

/// An emoji cell draws a colour glyph, not a tinted mask: the red circle
/// U+1F534's dominant channel in the middle of the glyph is red — in the
/// canvas' blue, green, red memory order (design D5), which pins the
/// channel order the glyph path produces.
#[test]
fn an_emoji_draws_its_colour_channels() {
    let Some(rig) = emoji_rig() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(&utf8(0x1F534));
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let cell = rig.metrics(1.0, rig.cell_h).cell_rect(0, 0);
    // The reddest pixel of the cell: the largest red channel against the
    // other two. A wrong channel order (red in the blue or green slot)
    // could not produce a dominant red channel at index 2.
    let reddest = (cell.y()..cell.y() + cell.h())
        .flat_map(move |y| (cell.x()..cell.x() + cell.w()).map(move |x| (x, y)))
        .map(|(x, y)| canvas.pixel(px(x), px(y)).expect("inside"))
        .max_by_key(|pixel| i32::from(pixel[2]) - i32::from(pixel[0].max(pixel[1])))
        .expect("the cell is not empty");
    assert!(
        reddest[2] > 100,
        "the red channel dominates: {reddest:?} (canvas order blue, green, red)"
    );
    assert!(
        reddest[2] > reddest[0] && reddest[2] > reddest[1],
        "red beats the other channels: {reddest:?}"
    );
}

/// A rig whose fallback resolves the red circle, or `None` (printed) when
/// no installed font covers it.
fn emoji_rig() -> Option<Rig> {
    let mut book = FontBook::new().expect("the font book opens");
    if !book.has_family(FAMILY) {
        println!("skipped: {FAMILY} is not installed");
        return None;
    }
    if book
        .fallback_face('\u{1F534}')
        .expect("the fallback lookup runs")
        .is_none()
    {
        println!("skipped: no installed font covers U+1F534");
        return None;
    }
    let config = FontConfig {
        family: Some(FAMILY.to_owned()),
        size: SIZE,
    };
    let faces = book.family_faces(&config).expect("the family's faces load");
    let mut rig = Rig::new()?;
    rig.test = from_faces(&faces, book);
    Some(rig)
}

// ---- Layer order ----

/// The bands draw after the text: a letter's underline band reads the
/// exact band colour across its whole width — over the glyph's ink too —
/// where the same letter without the flag leaves glyph pixels in the
/// band's rows.
#[test]
fn the_underline_band_draws_over_the_glyph() {
    let Some(rig) = Rig::new() else { return };
    // The band rows of the rule, on the font's pitch at scale 1: the
    // underline hangs from the ascent plus one logical pixel.
    let m = rig.metrics(1.0, rig.cell_h);
    let band_top = device_px((rig.ascent + 1.0) * 1.0);

    // Without the flag: the glyph really has ink in the band's rows.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"g");
    let plain = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    assert!(
        (m.cell_rect(0, 0).x()..m.cell_rect(0, 0).x() + m.cell_rect(0, 0).w())
            .any(|x| plain.pixel(px(x), px(band_top)) != Some(theme_bytes())),
        "the glyph has ink at the band's row, so the overlap is real"
    );

    // With the flag: the band covers the row exactly, glyph or no glyph.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"\x1b[4mg\x1b[0m");
    let underlined = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let cell = m.cell_rect(0, 0);
    for x in cell.x()..cell.x() + cell.w() {
        assert_eq!(
            underlined.pixel(px(x), px(band_top)),
            Some(theme_fg_bytes()),
            "the band reads the exact band colour at x {x}"
        );
    }
}

/// A sprite cell is drawn by the sprite pass only: the text pass skips the
/// cells [`sprite::cell_sprite`] owns, so a one-eighth block with an
/// explicit background shows the sprite's exact rectangles and nothing
/// else — no font glyph blended on top.
#[test]
fn a_sprite_cell_draws_no_text_glyph_on_top() {
    let Some(rig) = Rig::new() else { return };
    let m = rig.metrics(1.0, rig.cell_h);
    let frame = rig.frame(1.0, rig.cell_h, 0.0);

    // The routing, pure: exactly the cells the sprite pass declines.
    for cp in [0x2588, 0x2581, 0x2801, 0xE0B0] {
        assert!(
            !text_owns(&m, &frame, &cell_at(0, cp)),
            "0x{cp:04X} is the sprite pass's cell"
        );
    }
    for cp in [u32::from(b'A'), 0x2591, 0x1F534] {
        assert!(
            text_owns(&m, &frame, &cell_at(0, cp)),
            "0x{cp:04X} is the text pass's cell"
        );
    }

    // The pixels: the bottom-eighth block's cell reads the sprite's
    // rectangle and the explicit background, nothing between.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"\x1b[31;44m\xE2\x96\x81\x1b[0m");
    let walked = cells(&mut terminal);
    let block_cell = walked
        .iter()
        .find(|cell| cell.len > 0)
        .expect("the block cell");
    let fg = bytes([block_cell.fg.r, block_cell.fg.g, block_cell.fg.b]);
    let bg = bytes([block_cell.bg.r, block_cell.bg.g, block_cell.bg.b]);
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let cell = m.cell_rect(block_cell.x, block_cell.y);
    let Some((_, super::sprite::Primitive::Rects(sprite_rects))) =
        sprite::cell_sprite(&m, &frame, block_cell)
    else {
        panic!("the one-eighth block is a rect sprite");
    };
    for y in cell.y()..cell.y() + cell.h() {
        for x in cell.x()..cell.x() + cell.w() {
            let covered = sprite_rects
                .iter()
                .any(|r| x >= r.x() && x < r.x() + r.w() && y >= r.y() && y < r.y() + r.h());
            assert_eq!(
                canvas.pixel(px(x), px(y)),
                Some(if covered { fg } else { bg }),
                "the sprite cell at ({x}, {y}) is sprite or background, never text"
            );
        }
    }
}

// ---- The cursor's glyph redraw ----

/// A block cursor redraws the glyph under it in the terminal's default
/// background: a pixel inside a glyph stem that read the theme foreground
/// without the cursor reads the default background under the block.
#[test]
fn a_block_cursor_redraws_the_glyph_in_the_default_background() {
    let Some(rig) = Rig::new() else { return };
    // The same letter, once with the cursor hidden, once under a block
    // cursor parked on the cell.
    let mut hidden = rig.terminal(rig.cell_h);
    hidden.push_pty_data(b"\x1b[?25lX");
    let mut blocked = rig.terminal(rig.cell_h);
    blocked.push_pty_data(b"X\x1b[1;1H");
    let visible = {
        assert!(blocked.frame_begin(), "frame began");
        let visible = blocked.cursor().is_some();
        blocked.frame_end();
        visible
    };
    assert!(visible, "the block cursor is visible");
    let default_background = default_color(&mut blocked, true);
    let default_foreground = default_color(&mut blocked, false);

    let hidden_canvas = rig.painted(&mut hidden, 1.0, rig.cell_h, 0.0);
    let blocked_canvas = rig.painted(&mut blocked, 1.0, rig.cell_h, 0.0);
    let cell = rig.metrics(1.0, rig.cell_h).cell_rect(0, 0);
    // A full-coverage stem pixel of the hidden draw: exactly the theme
    // foreground (no SGR colour named). Under the block cursor the same
    // pixel reads exactly the default background — the redraw drew the
    // glyph in the background colour on top of the block fill.
    let stem = (cell.y()..cell.y() + cell.h())
        .flat_map(move |y| (cell.x()..cell.x() + cell.w()).map(move |x| (x, y)))
        .find(|(x, y)| hidden_canvas.pixel(px(*x), px(*y)) == Some(theme_fg_bytes()))
        .expect("the glyph has a full-coverage pixel");
    assert_eq!(
        blocked_canvas.pixel(px(stem.0), px(stem.1)),
        Some(default_background),
        "the stem pixel reads the default background under the block"
    );
    // The block fill shows where the glyph has no ink.
    let bare = (cell.y()..cell.y() + cell.h())
        .flat_map(move |y| (cell.x()..cell.x() + cell.w()).map(move |x| (x, y)))
        .find(|(x, y)| hidden_canvas.pixel(px(*x), px(*y)) == Some(theme_bytes()))
        .expect("the glyph does not fill the cell");
    assert_eq!(
        blocked_canvas.pixel(px(bare.0), px(bare.1)),
        Some(default_foreground),
        "the bare pixel reads the block's fill"
    );
}

/// A bar cursor does not redraw the glyph: the letter's ink away from the
/// bar's columns keeps the theme foreground.
#[test]
fn a_bar_cursor_does_not_redraw_the_glyph() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(b"X\x1b[6 q\x1b[1;1H");
    let walked_cursor = {
        assert!(terminal.frame_begin(), "frame began");
        let walked = terminal.cursor().expect("a visible cursor");
        terminal.frame_end();
        walked
    };
    assert_eq!(
        walked_cursor.style,
        crate::term::cells::CursorStyle::Bar,
        "the bar style is set"
    );
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let cell = rig.metrics(1.0, rig.cell_h).cell_rect(0, 0);
    // The bar covers the first device columns; past them the glyph's
    // full-coverage pixels keep the theme foreground — no redraw.
    let kept = (cell.x() + 3..cell.x() + cell.w())
        .flat_map(move |x| (cell.y()..cell.y() + cell.h()).map(move |y| (x, y)))
        .find(|(x, y)| canvas.pixel(px(*x), px(*y)) == Some(theme_fg_bytes()))
        .expect("the glyph keeps ink right of the bar");
    assert!(
        kept.0 >= cell.x() + 3,
        "the ink at ({}, {}) was not redrawn",
        kept.0,
        kept.1
    );
}

/// A wide glyph's tail cell does not redraw: a block cursor parked on the
/// tail fills it with the default foreground and nothing else — the head's
/// glyph is not drawn over the tail's block.
#[test]
fn a_wide_tail_does_not_redraw_the_glyph() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data("\u{6f22}\x1b[1;2H".as_bytes());
    let walked_cursor = {
        assert!(terminal.frame_begin(), "frame began");
        let walked = terminal.cursor().expect("a visible cursor");
        terminal.frame_end();
        walked
    };
    assert!(walked_cursor.x == 1, "the cursor sits on the tail cell");
    assert!(walked_cursor.wide_tail, "the vt reports the tail");
    let default_fg = default_color(&mut terminal, false);
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    let tail = m.cell_rect(1, 0);
    for y in tail.y()..tail.y() + tail.h() {
        for x in tail.x()..tail.x() + tail.w() {
            assert_eq!(
                canvas.pixel(px(x), px(y)),
                Some(default_fg),
                "the tail's block keeps its fill at ({x}, {y})"
            );
        }
    }
}

/// The redraw collects the cursor cell's text through the same rule the
/// GTK walk used: the first cell with a glyph at the cursor's position —
/// the walk's return value carries it for [`super::super::cursor`] to
/// redraw.
#[test]
fn the_walk_collects_the_cursor_cells_text() {
    let Some(mut rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(b"XY\x1b[1;2H");
    assert!(terminal.frame_begin(), "frame began");
    let frame = rig.frame(1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    let (w, h) = frame.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    let collected = rig
        .test
        .pass
        .paint(&mut canvas, &m, &frame, &mut terminal, 0);
    terminal.frame_end();
    assert_eq!(collected.as_bytes(), b"Y", "the cell at the cursor");
}

/// An empty collection — no cell with a glyph at the cursor's position —
/// leaves [`CursorText::empty`], which the redraw skips.
#[test]
fn an_empty_cursor_text_carries_nothing() {
    let empty = CursorText::empty();
    assert_eq!(empty.as_bytes(), b"", "an empty collection carries nothing");
}

// ---- The tween's draw offset ----

/// A tween draw offset translates the text like the other grid layers: the
/// offset in device pixels along x moves the glyph by exactly that many
/// pixels.
#[test]
fn the_draw_offset_translates_the_text() {
    let Some(rig) = Rig::new() else { return };
    let offset = 5.0;
    let scale = 1.0;
    let mut plain = rig.terminal(rig.cell_h);
    plain.push_pty_data(HIDE_CURSOR);
    plain.push_pty_data(b"MM");
    let mut shifted = rig.terminal(rig.cell_h);
    shifted.push_pty_data(HIDE_CURSOR);
    shifted.push_pty_data(b"MM");
    let base = rig.painted(&mut plain, scale, rig.cell_h, 0.0);
    let moved = rig.painted(&mut shifted, scale, rig.cell_h, offset);
    let m = rig.metrics(scale, rig.cell_h);
    let cell = m.cell_rect(0, 0);
    let shift = device_px(offset * scale);
    for y in cell.y()..cell.y() + cell.h() {
        for x in cell.x()..cell.x() + cell.w() {
            assert_eq!(
                moved.pixel(px(x + shift), px(y)),
                base.pixel(px(x), px(y)),
                "the text moved by the offset at ({x}, {y})"
            );
        }
    }
}
