//! The shared test helper for the text pass (replace-gtk-with-wayland D10):
//! one place builds a [`TextPass`] over the real test family, so every
//! `paint_frame` pixel test — the painter's, the sprite's, the cursor's and
//! the text pass's own — draws with the same font, plus the frame rig and
//! the pixel helpers the text pass's two test files share. The
//! font-dependent tests skip with a printed message only when the family
//! is not installed; a broken `FontBook` or a failed lookup is an `expect`,
//! so a regression cannot hide behind "not installed".

use crate::fontconfig::{FontConfig, ThemeColours};
use crate::guard::Poisoned;
use crate::render::canvas::Canvas;
use crate::render::cell_metrics::CellMetrics;
use crate::render::font::{FamilyFaces, FontBook};
use crate::render::geom::{DeviceRect, FrameInput, PainterMetrics, device_px};
use crate::render::glyph::GlyphCache;
use crate::render::image_pass::ImagePass;
use crate::render::painter::paint_frame;
use crate::render::shape::TextShaper;
use crate::term::cells::{CELL_TEXT_CAP, Cell};
use crate::term::{PngDecoder, PtySink, Terminal};

use super::TextPass;

/// The family the tests pin; CI installs it (design D10). The same family
/// the shape, glyph and nerd tests pin.
pub(crate) const FAMILY: &str = "JetBrainsMono Nerd Font";

/// The size in points the tests run at: the Ghostty default 11.
pub(crate) const SIZE: f64 = 11.0;

/// A [`TextPass`] plus the cell metrics it was built with, so a test can
/// reason about the font's own numbers (the nerd scenario, the logical
/// sizes scenario).
pub(crate) struct TestPass {
    /// The pass the frame paints with.
    pub pass: TextPass,
    /// The cell metrics measured from the family's regular face at
    /// [`SIZE`].
    pub cell_metrics: CellMetrics,
}

/// A text pass over the test family's faces, or `None` (printed) when the
/// machine lacks the family.
pub(crate) fn text_pass() -> Option<TestPass> {
    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(FAMILY) {
        println!("skipped: {FAMILY} is not installed");
        return None;
    }
    let config = FontConfig {
        family: Some(FAMILY.to_owned()),
        size: SIZE,
    };
    let faces = book.family_faces(&config).expect("the family's faces load");
    Some(from_faces(&faces, book))
}

/// A text pass over faces a test built itself — the emoji scenario builds
/// the same family and lets the shaper's fallback find the emoji face.
pub(crate) fn from_faces(faces: &FamilyFaces, book: FontBook) -> TestPass {
    let cell_metrics = crate::render::cell_metrics::measure(faces.regular(), SIZE)
        .expect("the cell metrics compute");
    let shaper = TextShaper::new(faces.clone(), book);
    TestPass {
        pass: TextPass::new(shaper, GlyphCache::new(), cell_metrics, SIZE),
        cell_metrics,
    }
}

/// The test theme, distinct from every colour the tests draw.
pub(crate) const THEME: ThemeColours = ThemeColours {
    background: [10, 20, 30],
    foreground: [200, 150, 100],
};

/// Hides the frame's cursor (DECTCEM) for the tests that are not about the
/// cursor.
pub(crate) const HIDE_CURSOR: &[u8] = b"\x1b[?25l";

/// The test rig: the shared text pass plus the 8-column, 4-row frame at
/// the font's own cell pitch — the production pairing, where
/// [`CellMetrics`] and [`PainterMetrics`] describe the same grid and a
/// glyph's ink fits inside its cell. A `cell_h` a test names explicitly
/// (the 19-logical-pixel scenario) replaces the font's row height.
pub(crate) struct Rig {
    pub test: TestPass,
    pub cell_w: i32,
    pub cell_h: i32,
    pub ascent: f64,
}

impl Rig {
    /// The rig over the test family, or `None` (printed) when the machine
    /// lacks the family.
    pub(crate) fn new() -> Option<Self> {
        let test = text_pass()?;
        let (cell_w, cell_h) = (test.cell_metrics.cell_w(), test.cell_metrics.cell_h());
        Some(Rig {
            ascent: f64::from(test.cell_metrics.ascent()),
            test,
            cell_w,
            cell_h,
        })
    }

    /// A terminal at the font's pitch, or at an explicit row height.
    pub(crate) fn terminal(&self, cell_h: i32) -> Terminal {
        let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
        assert!(terminal.push_size(8, 4, self.cell_w, cell_h));
        terminal
    }

    /// The frame's metrics at `scale`, with the font's truncated ascent.
    pub(crate) fn metrics(&self, scale: f64, cell_h: i32) -> PainterMetrics {
        PainterMetrics::new(
            f64::from(self.cell_w),
            f64::from(cell_h),
            self.ascent,
            scale,
        )
        .expect("test metrics are valid")
    }

    /// Frame input for the 8-column, 4-row frame at `scale`.
    pub(crate) fn frame(&self, scale: f64, cell_h: i32, draw_offset: f64) -> FrameInput {
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
    pub(crate) fn painted(
        &self,
        terminal: &mut Terminal,
        scale: f64,
        cell_h: i32,
        draw_offset: f64,
    ) -> Canvas {
        let frame = self.frame(scale, cell_h, draw_offset);
        let (w, h) = frame.device_size();
        let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
        let mut images = ImagePass::new();
        paint_frame(
            &mut canvas,
            &self.metrics(scale, cell_h),
            &frame,
            terminal,
            &mut self.pass(),
            &mut images,
        );
        canvas
    }

    /// The pass, handed to `paint_frame` per call (a fresh pass per frame
    /// keeps the tests independent; the caches rebuild in microseconds).
    pub(crate) fn pass(&self) -> TextPass {
        TextPass::new(
            TextShaper::new(Self::faces(), Self::book()),
            GlyphCache::new(),
            self.test.cell_metrics,
            SIZE,
        )
    }

    /// The family's faces, for `pass`.
    pub(crate) fn faces() -> FamilyFaces {
        let config = FontConfig {
            family: Some(FAMILY.to_owned()),
            size: SIZE,
        };
        Self::book()
            .family_faces(&config)
            .expect("the family's faces load")
    }

    /// A fresh font book (no state of the rig's is used).
    pub(crate) fn book() -> FontBook {
        FontBook::new().expect("the font book opens")
    }
}

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

/// A canvas of `frame`'s device size with the frame painted into it —
/// the shared form of the per-pass `painted` helpers, so each pass's
/// tests build only its own pass and this one place carries the frame's
/// kitty image pass.
pub(crate) fn painted_frame(
    terminal: &mut Terminal,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    text: &mut TextPass,
) -> Canvas {
    let (w, h) = frame.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    let mut images = ImagePass::new();
    paint_frame(&mut canvas, metrics, frame, terminal, text, &mut images);
    canvas
}

/// A colour's bytes in the canvas' memory order: blue, green, red, alpha.
pub(crate) fn bytes(rgb: [u8; 3]) -> [u8; 4] {
    [rgb[2], rgb[1], rgb[0], 255]
}

/// The theme background's canvas bytes.
pub(crate) fn theme_bytes() -> [u8; 4] {
    bytes(THEME.background)
}

/// The theme foreground's canvas bytes.
pub(crate) fn theme_fg_bytes() -> [u8; 4] {
    bytes(THEME.foreground)
}

/// The terminal's default colour, read off the opened frame — the same
/// read the cursor layer's redraw uses.
pub(crate) fn default_color(terminal: &mut Terminal, background: bool) -> [u8; 4] {
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
pub(crate) fn cells(terminal: &mut Terminal) -> Vec<Cell> {
    assert!(terminal.frame_begin(), "frame began");
    let mut cells = Vec::new();
    while let Some(cell) = terminal.cell_next() {
        cells.push(cell);
    }
    terminal.frame_end();
    cells
}

/// A device coordinate as the canvas' unsigned pixel index.
pub(crate) fn px(value: i32) -> u32 {
    u32::try_from(value).expect("the coordinate fits u32")
}

/// The canvas pixels of the `w`×`h` block at `rect`'s top-left corner —
/// the crop the cross-cell comparisons use when the snapped cell sizes
/// differ by a device pixel.
pub(crate) fn block(canvas: &Canvas, rect: DeviceRect, w: i32, h: i32) -> Vec<[u8; 4]> {
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
pub(crate) fn common_size(rects: &[DeviceRect]) -> (i32, i32) {
    (
        rects.iter().map(DeviceRect::w).min().expect("non-empty"),
        rects.iter().map(DeviceRect::h).min().expect("non-empty"),
    )
}

/// UTF-8 bytes of one code point.
pub(crate) fn utf8(cp: u32) -> Vec<u8> {
    char::from_u32(cp)
        .expect("a scalar")
        .to_string()
        .into_bytes()
}

/// A narrow cell holding `cp`, hand-built for the pure decisions.
pub(crate) fn cell_at(x: i32, cp: u32) -> Cell {
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
pub(crate) fn contains_pixel(canvas: &Canvas, rect: DeviceRect, expected: [u8; 4]) -> bool {
    (rect.y()..rect.y() + rect.h())
        .flat_map(move |y| (rect.x()..rect.x() + rect.w()).map(move |x| (x, y)))
        .any(|(x, y)| canvas.pixel(px(x), px(y)) == Some(expected))
}
