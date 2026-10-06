//! The canvas the grid painter draws a frame into (replace-gtk-with-wayland
//! D5, D11): a tiny-skia [`Pixmap`] at device size whose bytes read in the
//! `ARGB8888` memory order blue, green, red, alpha, ready to copy straight
//! into a `wl_shm` buffer.
//!
//! The channel swap that makes the byte orders meet happens exactly once,
//! at colour-cache time: a theme colour enters as a [`CanvasColor`], a
//! decoded image or a colour glyph through [`image_pixmap`], and both
//! already carry their red and blue exchanged — no per-frame conversion
//! pass ever runs (D5).
//!
//! Premultiplied alpha is what both tiny-skia's pixmap and `ARGB8888` use.
//! [`CanvasColor`] stores the colour still non-premultiplied, and
//! tiny-skia's raster pipeline premultiplies at fill time; premultiplication
//! scales each channel by the alpha, so it commutes with the one-time
//! red/blue swap — the pixmap bytes are the premultiplied colour in the
//! swapped order, which is exactly the `ARGB8888` memory order. The pixel
//! tests below pin the exact bytes.
//!
//! The primitive set is the full set the grid painter needs (rows 4.3 to
//! 4.7): an exact and a fractional rectangle fill, a polygon fill for the
//! powerline triangles, a straight-line stroke for the one-line sprite, an
//! image blit and a coverage-mask blit for the glyphs, and a pixel read for
//! the tests. The module is frozen for those rows.
//!
//! GTK-free like the snap rules (`replace-gtk-with-wayland` D10): the tests
//! below run without a display. The module is `pub` because its only
//! consumer, the grid painter, arrives in rows 4.3 to 4.7 (`pub(crate)`
//! entries with no caller are dead code under `-D warnings`, and no lint
//! suppression is permitted).

use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Shader, Stroke, Transform};

use crate::term::cells::Rgb;

mod bitmap;

pub use bitmap::{CanvasMask, image_pixmap};

/// A colour cached for painting into the [`Canvas`] (D5): the channels are
/// stored in the pixmap's memory order — blue, green, red, alpha of the
/// colour as cached — which is the one-time red/blue swap. The value stays
/// non-premultiplied; tiny-skia's raster pipeline premultiplies at fill
/// time, and [`Canvas::draw_mask`] premultiplies with the same rounding
/// tiny-skia uses, so every path into the canvas produces the same bytes
/// for the same colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasColor {
    /// The colour's channels in the pixmap's memory order: blue, green,
    /// red, alpha of the colour as cached.
    channels: [u8; 4],
}

impl CanvasColor {
    /// Cache one 8-bit RGBA colour (D5): the one-time red/blue swap. Later
    /// images and colour glyphs enter the cache through this constructor
    /// too, so the swap never runs per frame.
    #[must_use]
    pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        CanvasColor {
            channels: [b, g, r, a],
        }
    }

    /// Cache an opaque theme colour (`D5`): the theme carries fully opaque
    /// background, foreground and accent colours.
    #[must_use]
    pub fn from_theme(color: Rgb) -> Self {
        CanvasColor::from_rgba(color.r, color.g, color.b, 255)
    }

    /// The tiny-skia colour the pixmap stores: the pixmap's memory order is
    /// tiny-skia's red, green, blue, alpha, so the slots go through as they
    /// are — slot 0, the original colour's blue, becomes tiny-skia's red,
    /// which is what puts the original blue first in the pixmap bytes.
    fn color(self) -> tiny_skia::Color {
        let [b, g, r, a] = self.channels;
        tiny_skia::Color::from_rgba8(b, g, r, a)
    }

    /// The premultiplied source pixel for source-over blending at
    /// `coverage`/255, in the pixmap's channel order: the colour
    /// premultiplied by its own alpha, then scaled by the coverage, each
    /// step with tiny-skia's rounding.
    fn premultiplied(self, coverage: u8) -> [u8; 4] {
        let [b, g, r, a] = self.channels;
        let scaled = |c: u8| mul_round_u8(mul_round_u8(c, a), coverage);
        [scaled(b), scaled(g), scaled(r), mul_round_u8(a, coverage)]
    }
}

/// The drawing surface one frame paints into (D5): a tiny-skia pixmap at
/// device size, filled with cached colours and read back as `ARGB8888`
/// bytes for the shared-memory buffer.
#[derive(Debug)]
pub struct Canvas {
    pixmap: Pixmap,
    width: usize,
}

impl Canvas {
    /// Create a canvas at a device size. `None` when a dimension is zero or
    /// the size does not fit tiny-skia — a size no buffer could hold.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let pixmap = Pixmap::new(width, height)?;
        Some(Canvas {
            width: usize::try_from(width).ok()?,
            pixmap,
        })
    }

    /// The canvas size, in device pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.pixmap.width(), self.pixmap.height())
    }

    /// Clear the canvas to fully transparent premultiplied zero: every byte
    /// reads 0, which is both the transparent and the premultiplied zero of
    /// `ARGB8888`.
    pub fn clear(&mut self) {
        self.pixmap.fill(tiny_skia::Color::TRANSPARENT);
    }

    /// Fill one rectangle in whole device pixels with a cached colour. The
    /// edges are pixel-aligned, so the fill is unantialiased and writes the
    /// colour's bytes exactly. A rectangle with no size draws nothing; a
    /// rectangle reaching past the canvas is clipped to it.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: CanvasColor) {
        if w <= 0 || h <= 0 {
            return;
        }
        // The extents are positive, so the unsigned conversions hold.
        let (Ok(w), Ok(h)) = (u32::try_from(w), u32::try_from(h)) else {
            return;
        };
        let Some(int_rect) = tiny_skia::IntRect::from_xywh(x, y, w, h) else {
            return;
        };
        let paint = paint(color, false);
        self.pixmap
            .fill_rect(int_rect.to_rect(), &paint, Transform::identity(), None);
    }

    /// Fill a rectangle given in device pixels as floats. The painter snaps
    /// the edges with `OutputScale` first, so a rectangle with whole-number
    /// edges fills exactly — the same bytes [`Canvas::fill_rect`] writes —
    /// and one with fractional edges is anti-aliased. A rectangle with no
    /// size or a non-finite edge draws nothing; one reaching past the
    /// canvas is clipped to it.
    pub fn fill_rect_f32(&mut self, x: f32, y: f32, w: f32, h: f32, color: CanvasColor) {
        if w <= 0.0 || h <= 0.0 || ![x, y, w, h].iter().all(|v| v.is_finite()) {
            return;
        }
        let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) else {
            return;
        };
        let paint = paint(color, true);
        self.pixmap
            .fill_rect(rect, &paint, Transform::identity(), None);
    }

    /// Fill a closed polygon from a list of points, anti-aliased (the
    /// corner and powerline triangles). Fewer than three points or a
    /// non-finite coordinate draws nothing; a polygon reaching past the
    /// canvas is clipped to it.
    pub fn fill_polygon(&mut self, points: &[(f32, f32)], color: CanvasColor) {
        if points.len() < 3 || !points.iter().all(|(x, y)| x.is_finite() && y.is_finite()) {
            return;
        }
        let mut left = points.iter().copied();
        let Some((x0, y0)) = left.next() else {
            return;
        };
        let mut builder = PathBuilder::new();
        builder.move_to(x0, y0);
        for (x, y) in left {
            builder.line_to(x, y);
        }
        builder.close();
        let Some(path) = builder.finish() else {
            return;
        };
        let paint = paint(color, true);
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    /// Stroke a straight line with a width in device pixels, anti-aliased
    /// (the one-line sprite and the hollow powerline outline, butt caps
    /// like the cairo painter's default). A non-positive width, a
    /// zero-length line or a non-finite coordinate draws nothing; the
    /// stroke is clipped to the canvas.
    pub fn stroke_line(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        color: CanvasColor,
    ) {
        if width <= 0.0
            || !width.is_finite()
            || ![x0, y0, x1, y1].iter().all(|v| v.is_finite())
            || ((x1 - x0).abs() < f32::EPSILON && (y1 - y0).abs() < f32::EPSILON)
        {
            return;
        }
        let mut builder = PathBuilder::new();
        builder.move_to(x0, y0);
        builder.line_to(x1, y1);
        let Some(path) = builder.finish() else {
            return;
        };
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        let paint = paint(color, true);
        self.pixmap
            .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    /// Draw a cached image at an integer device position, source-over
    /// blended (kitty images and colour glyphs). The image is premultiplied
    /// and already channel-swapped — it comes from [`image_pixmap`] — so
    /// the blit copies its bytes into the canvas' order unchanged. An image
    /// reaching past the canvas is clipped to it; the call never panics.
    pub fn draw_image(&mut self, image: &Pixmap, x: i32, y: i32) {
        self.pixmap.draw_pixmap(
            x,
            y,
            image.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    /// Read one whole pixel back, in the canvas' memory order blue, green,
    /// red, alpha — the form the tests compare. `None` outside the canvas.
    /// The bounds check here is the real one: tiny-skia's own `pixel` only
    /// checks the flat index, so an in-range flat index past the row would
    /// answer a pixel from another row.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.pixmap.width() || y >= self.pixmap.height() {
            return None;
        }
        let pixel = self.pixmap.pixel(x, y)?;
        Some([pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()])
    }

    /// The canvas bytes in the `ARGB8888` memory order blue, green, red,
    /// alpha, for the copy into the `wl_shm` buffer (D5).
    #[must_use]
    pub fn data(&self) -> &[u8] {
        self.pixmap.data()
    }
}
/// The paint for one cached colour: a solid shader over the default linear
/// colour space, so no gamma conversion runs between the cached colour and
/// the pixmap and the bytes pass through untransformed.
fn paint(color: CanvasColor, anti_alias: bool) -> Paint<'static> {
    Paint {
        shader: Shader::SolidColor(color.color()),
        anti_alias,
        ..Paint::default()
    }
}

/// Multiply two 8-bit channels and round to the nearest `u8`, with
/// tiny-skia's own premultiply rounding (`premultiply_u8` in its
/// `color.rs`). The product of two `u8`s cannot round past 255, so the
/// fallback never fires.
fn mul_round_u8(a: u8, b: u8) -> u8 {
    let prod = u32::from(a) * u32::from(b) + 128;
    u8::try_from((prod + (prod >> 8)) >> 8).unwrap_or(u8::MAX)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A filled theme colour reads back in the `ARGB8888` memory order blue,
    /// green, red, alpha (D5): the red and blue channels come out swapped,
    /// the green and alpha channels unchanged, and the bytes are exactly
    /// the ones the compositor expects.
    #[test]
    fn a_filled_theme_colour_reads_back_as_blue_green_red_alpha() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.fill_rect(
            0,
            0,
            4,
            4,
            CanvasColor::from_theme(Rgb {
                r: 0x10,
                g: 0x20,
                b: 0x30,
            }),
        );
        assert_eq!(canvas.data()[0], 0x30, "the blue channel comes first");
        assert_eq!(canvas.data()[1], 0x20, "the green channel comes second");
        assert_eq!(
            canvas.data()[2],
            0x10,
            "the swapped red channel comes third"
        );
        assert_eq!(
            canvas.data()[3],
            0xff,
            "an opaque theme colour stays opaque"
        );
    }

    /// A colour with partial alpha reads back premultiplied, in the same
    /// blue, green, red, alpha order: tiny-skia premultiplies at fill time,
    /// and the one-time swap commutes with the per-channel premultiplication.
    #[test]
    fn a_partial_alpha_fill_reads_back_premultiplied() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.fill_rect(0, 0, 4, 4, CanvasColor::from_rgba(100, 150, 200, 150));
        // Premultiplied: 100*150/255 -> 59, 150*150/255 -> 88,
        // 200*150/255 -> 118, alpha unchanged at 150. Swapped: blue first.
        assert_eq!(canvas.data()[0], 118, "the premultiplied blue comes first");
        assert_eq!(canvas.data()[1], 88, "the premultiplied green comes second");
        assert_eq!(
            canvas.data()[2],
            59,
            "the premultiplied swapped red comes third"
        );
        assert_eq!(canvas.data()[3], 150, "the alpha itself is unchanged");
    }

    /// A fill covers exactly its rectangle: pixels outside stay at the
    /// transparent zero the clear left, and the clipped part of an
    /// oversized rectangle stops at the canvas edge.
    #[test]
    fn a_fill_covers_exactly_its_rectangle_and_clips_at_the_edge() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.clear();
        canvas.fill_rect(
            1,
            1,
            2,
            2,
            CanvasColor::from_theme(Rgb {
                r: 0xff,
                g: 0,
                b: 0,
            }),
        );
        // Row and column 0 and the last row and column stay transparent.
        for offset in [0, 4, 8, 12, 48, 52, 56, 60, 16, 32, 28, 44] {
            assert_eq!(
                canvas.data()[offset],
                0,
                "the border byte {offset} stays transparent"
            );
        }
        // The filled cell at (1, 1) reads blue, green, red, alpha: pixel
        // (1, 1) sits at byte offset (1 * 4 + 1) * 4.
        assert_eq!(&canvas.data()[20..24], &[0x00, 0x00, 0xff, 0xff]);

        // An oversized rectangle clips at the canvas edge.
        canvas.clear();
        canvas.fill_rect(
            2,
            2,
            8,
            8,
            CanvasColor::from_theme(Rgb {
                r: 0,
                g: 0,
                b: 0x44,
            }),
        );
        assert_eq!(&canvas.data()[40..44], &[0x44, 0x00, 0x00, 0xff]);
        assert_eq!(
            &canvas.data()[0..4],
            &[0, 0, 0, 0],
            "outside the clip stays clear"
        );
    }

    /// A fill whose rectangle starts left of or above the canvas clips to
    /// the visible part, and a fill wholly outside draws nothing.
    #[test]
    fn a_fill_starting_outside_the_canvas_clips_without_panic() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.fill_rect(
            -2,
            -2,
            4,
            4,
            CanvasColor::from_theme(Rgb {
                r: 0,
                g: 0x66,
                b: 0,
            }),
        );
        // Only pixel (1, 1) of the rectangle lands inside: the visible part
        // runs over pixels (0, 0) and (1, 1).
        assert_eq!(canvas.pixel(0, 0), Some([0x00, 0x66, 0x00, 0xff]));
        assert_eq!(canvas.pixel(1, 1), Some([0x00, 0x66, 0x00, 0xff]));
        assert_eq!(
            canvas.pixel(2, 2),
            Some([0, 0, 0, 0]),
            "clipped at the edge"
        );

        canvas.clear();
        canvas.fill_rect(
            8,
            8,
            4,
            4,
            CanvasColor::from_theme(Rgb { r: 9, g: 9, b: 9 }),
        );
        canvas.fill_rect(
            -8,
            -8,
            4,
            4,
            CanvasColor::from_theme(Rgb { r: 9, g: 9, b: 9 }),
        );
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "a wholly outside fill draws nothing"
        );
    }

    /// A cleared canvas reads all zero: the transparent and premultiplied
    /// zero of `ARGB8888`, also after a fill.
    #[test]
    fn a_clear_makes_every_byte_zero() {
        let mut canvas = Canvas::new(2, 2).expect("test canvas size is valid");
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "a fresh canvas is clear"
        );
        canvas.fill_rect(
            0,
            0,
            2,
            2,
            CanvasColor::from_theme(Rgb { r: 1, g: 2, b: 3 }),
        );
        canvas.clear();
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "a cleared canvas is zero"
        );
    }

    /// Degenerate inputs draw nothing and stay safe: a zero-sized fill is a
    /// no-op, and a zero-sized canvas cannot be created.
    #[test]
    fn degenerate_sizes_are_refused_or_no_ops() {
        assert!(Canvas::new(0, 4).is_none());
        assert!(Canvas::new(4, 0).is_none());
        let mut canvas = Canvas::new(2, 2).expect("test canvas size is valid");
        canvas.fill_rect(
            0,
            0,
            0,
            2,
            CanvasColor::from_theme(Rgb { r: 9, g: 9, b: 9 }),
        );
        canvas.fill_rect(
            0,
            0,
            2,
            0,
            CanvasColor::from_theme(Rgb { r: 9, g: 9, b: 9 }),
        );
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "a zero-sized fill draws nothing"
        );
    }

    /// The fractional fill on whole-number edges is pixel exact: the same
    /// bytes the exact fill writes, which is what the snapped rectangles
    /// the painter passes rely on.
    #[test]
    fn a_fractional_fill_with_whole_edges_is_exact() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.fill_rect_f32(
            1.0,
            1.0,
            2.0,
            2.0,
            CanvasColor::from_theme(Rgb {
                r: 0x10,
                g: 0x20,
                b: 0x30,
            }),
        );
        assert_eq!(canvas.pixel(1, 1), Some([0x30, 0x20, 0x10, 0xff]));
        assert_eq!(canvas.pixel(2, 2), Some([0x30, 0x20, 0x10, 0xff]));
        assert_eq!(
            canvas.pixel(0, 0),
            Some([0, 0, 0, 0]),
            "outside stays clear"
        );
        assert_eq!(
            canvas.pixel(3, 3),
            Some([0, 0, 0, 0]),
            "outside stays clear"
        );
    }

    /// The fractional fill on fractional edges anti-aliases: the half-covered
    /// column carries roughly half the colour's premultiplied bytes over the
    /// transparent backdrop, the fully covered column the exact bytes, and
    /// the untouched column nothing.
    #[test]
    fn a_fractional_fill_with_fractional_edges_anti_aliases() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        // The rectangle covers half of column 0, all of column 1 and none
        // of column 2.
        canvas.fill_rect_f32(
            0.5,
            0.0,
            1.5,
            4.0,
            CanvasColor::from_theme(Rgb {
                r: 0,
                g: 0,
                b: 0xff,
            }),
        );
        let half = canvas.pixel(0, 2).expect("inside the canvas");
        assert_eq!(half[3], 128, "the half-covered edge pixel is half covered");
        assert!(
            half[0] > 0 && half[0] < 0xff,
            "the half-covered pixel blends, not swaps"
        );
        assert_eq!(
            canvas.pixel(1, 2),
            Some([0xff, 0, 0, 0xff]),
            "the covered column is exact"
        );
        assert_eq!(
            canvas.pixel(2, 2),
            Some([0, 0, 0, 0]),
            "the untouched column stays clear"
        );
    }

    /// A fractional fill with no size or a non-finite edge draws nothing,
    /// and one reaching past the canvas clips without panic.
    #[test]
    fn degenerate_fractional_fills_draw_nothing() {
        let mut canvas = Canvas::new(2, 2).expect("test canvas size is valid");
        let red = CanvasColor::from_theme(Rgb {
            r: 0xff,
            g: 0,
            b: 0,
        });
        canvas.fill_rect_f32(0.0, 0.0, 0.0, 2.0, red);
        canvas.fill_rect_f32(0.0, 0.0, 2.0, 0.0, red);
        canvas.fill_rect_f32(f32::NAN, 0.0, 2.0, 2.0, red);
        canvas.fill_rect_f32(0.0, f32::INFINITY, 2.0, 2.0, red);
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "degenerate fractional fills draw nothing"
        );
        canvas.fill_rect_f32(-1.0, -1.0, 8.0, 8.0, red);
        assert_eq!(
            canvas.pixel(0, 0),
            Some([0, 0, 0xff, 0xff]),
            "clipped at the edge"
        );
    }

    /// An opaque polygon covers its interior pixels and nothing outside:
    /// the filled right triangle over a 4x4 canvas.
    #[test]
    fn an_opaque_polygon_covers_its_interior() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.fill_polygon(
            &[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)],
            CanvasColor::from_theme(Rgb {
                r: 0,
                g: 0xff,
                b: 0,
            }),
        );
        // The triangle's interior and its axis-aligned legs are fully
        // covered; the far corner is untouched.
        assert_eq!(canvas.pixel(0, 0), Some([0, 0xff, 0, 0xff]));
        assert_eq!(canvas.pixel(2, 0), Some([0, 0xff, 0, 0xff]));
        assert_eq!(canvas.pixel(0, 2), Some([0, 0xff, 0, 0xff]));
        assert_eq!(canvas.pixel(1, 1), Some([0, 0xff, 0, 0xff]));
        assert_eq!(
            canvas.pixel(3, 3),
            Some([0, 0, 0, 0]),
            "outside the hypotenuse"
        );
        // Pixel (3, 0) sits on the hypotenuse: an anti-aliased edge pixel,
        // neither full nor empty.
        let edge = canvas.pixel(3, 0).expect("inside the canvas");
        assert!(
            edge[3] > 0 && edge[3] < 255,
            "the hypotenuse edge pixel blends"
        );
    }

    /// A polygon clipped at every canvas edge draws its visible part
    /// without panic, and fewer than three points or a non-finite
    /// coordinate draws nothing.
    #[test]
    fn degenerate_polygons_draw_nothing_or_clip() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        let red = CanvasColor::from_theme(Rgb {
            r: 0xff,
            g: 0,
            b: 0,
        });
        canvas.fill_polygon(&[(0.0, 0.0), (4.0, 0.0)], red);
        canvas.fill_polygon(&[], red);
        canvas.fill_polygon(&[(f32::NAN, 0.0), (4.0, 0.0), (0.0, 4.0)], red);
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "degenerate polygons draw nothing"
        );
        // A triangle wholly outside stays invisible; one reaching past the
        // edge clips to it.
        canvas.fill_polygon(&[(8.0, 8.0), (12.0, 8.0), (8.0, 12.0)], red);
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "wholly outside"
        );
        // A triangle reaching past the top left corner clips to it: its
        // interior covers the canvas' first pixel.
        canvas.fill_polygon(&[(-2.0, -2.0), (6.0, -2.0), (-2.0, 6.0)], red);
        assert_eq!(
            canvas.pixel(0, 0),
            Some([0, 0, 0xff, 0xff]),
            "clipped at the edge"
        );
    }

    /// A stroke has the width asked: a horizontal line of width 2 centred
    /// on y = 2 covers exactly the two device rows 1 and 2, butt-capped at
    /// the line's ends.
    #[test]
    fn a_stroke_has_the_width_asked() {
        let mut canvas = Canvas::new(6, 6).expect("test canvas size is valid");
        canvas.stroke_line(
            0.0,
            2.0,
            4.0,
            2.0,
            2.0,
            CanvasColor::from_theme(Rgb {
                r: 0,
                g: 0,
                b: 0xff,
            }),
        );
        for y in [1u32, 2] {
            for x in 0..4u32 {
                assert_eq!(
                    canvas.pixel(x, y),
                    Some([0xff, 0, 0, 0xff]),
                    "the stroke covers row {y} at x {x}"
                );
            }
        }
        // Outside the stroke's width and beyond its butt end: untouched.
        assert_eq!(canvas.pixel(0, 0), Some([0, 0, 0, 0]), "above the stroke");
        assert_eq!(canvas.pixel(0, 3), Some([0, 0, 0, 0]), "below the stroke");
        assert_eq!(canvas.pixel(4, 2), Some([0, 0, 0, 0]), "past the butt end");
    }

    /// A stroke with no width, a zero-length line or a non-finite
    /// coordinate draws nothing, and a stroke reaching past the canvas
    /// clips without panic.
    #[test]
    fn degenerate_strokes_draw_nothing_or_clip() {
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        let red = CanvasColor::from_theme(Rgb {
            r: 0xff,
            g: 0,
            b: 0,
        });
        canvas.stroke_line(0.0, 2.0, 4.0, 2.0, 0.0, red);
        canvas.stroke_line(1.0, 2.0, 1.0, 2.0, 2.0, red);
        canvas.stroke_line(f32::NAN, 0.0, 4.0, 0.0, 2.0, red);
        canvas.stroke_line(0.0, 0.0, 4.0, 4.0, f32::INFINITY, red);
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "degenerate strokes draw nothing"
        );
        canvas.stroke_line(-2.0, 1.0, 8.0, 1.0, 2.0, red);
        assert_eq!(
            canvas.pixel(0, 1),
            Some([0, 0, 0xff, 0xff]),
            "clipped at the edge"
        );
    }

    /// A pixel read answers `None` outside the canvas and the memory-order
    /// bytes inside it.
    #[test]
    fn a_pixel_read_answers_the_memory_order_or_none() {
        let mut canvas = Canvas::new(2, 2).expect("test canvas size is valid");
        assert!(canvas.pixel(2, 0).is_none());
        assert!(canvas.pixel(0, 2).is_none());
        canvas.fill_rect(
            0,
            0,
            1,
            1,
            CanvasColor::from_theme(Rgb { r: 1, g: 2, b: 3 }),
        );
        assert_eq!(canvas.pixel(0, 0), Some([3, 2, 1, 255]));
    }
}
