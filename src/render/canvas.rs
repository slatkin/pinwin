//! The canvas the grid painter draws a frame into (replace-gtk-with-wayland
//! D5, D11): a tiny-skia [`Pixmap`] at device size whose bytes read in the
//! `ARGB8888` memory order blue, green, red, alpha, ready to copy straight
//! into a `wl_shm` buffer.
//!
//! The channel swap that makes the byte orders meet happens exactly once,
//! at colour-cache time: a theme colour, a decoded image or a colour glyph
//! enters the cache as a [`CanvasColor`] with its red and blue channels
//! already exchanged, and no per-frame conversion pass ever runs (D5).
//!
//! Premultiplied alpha is what both tiny-skia's pixmap and `ARGB8888` use.
//! The cache stores the colour still non-premultiplied, and tiny-skia's
//! raster pipeline premultiplies at fill time; premultiplication scales
//! each channel by the alpha, so it commutes with the one-time red/blue
//! swap — the pixmap bytes are the premultiplied colour in the swapped
//! order, which is exactly the `ARGB8888` memory order. The pixel test
//! below pins the exact bytes.
//!
//! GTK-free like the snap rules (`replace-gtk-with-wayland` D10): the tests
//! below run without a display. The module is `pub` because its only
//! consumer, the grid painter, arrives in rows 4.3 to 4.7 (`pub(crate)`
//! entries with no caller are dead code under `-D warnings`, and no lint
//! suppression is permitted).

use tiny_skia::{Paint, Pixmap, Shader, Transform};

use crate::term::cells::Rgb;

/// A colour cached for painting into the [`Canvas`] (D5): the red and blue
/// channels are already swapped, so the pixmap bytes read in the `ARGB8888`
/// memory order blue, green, red, alpha. The value stays non-premultiplied;
/// tiny-skia's raster pipeline premultiplies at fill time, which keeps the
/// premultiplied bytes exact (see the module comment).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasColor {
    color: tiny_skia::Color,
}

impl CanvasColor {
    /// Cache one 8-bit RGBA colour (D5): the one-time red/blue swap. Later
    /// images and colour glyphs enter the cache through this constructor
    /// too, so the swap never runs per frame.
    #[must_use]
    pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        CanvasColor {
            color: tiny_skia::Color::from_rgba8(b, g, r, a),
        }
    }

    /// Cache an opaque theme colour (`D5`): the theme carries fully opaque
    /// background, foreground and accent colours.
    #[must_use]
    pub fn from_theme(color: Rgb) -> Self {
        CanvasColor::from_rgba(color.r, color.g, color.b, 255)
    }
}

/// The drawing surface one frame paints into (D5): a tiny-skia pixmap at
/// device size, filled with cached colours and read back as `ARGB8888`
/// bytes for the shared-memory buffer.
#[derive(Debug)]
pub struct Canvas {
    pixmap: Pixmap,
}

impl Canvas {
    /// Create a canvas at a device size. `None` when a dimension is zero or
    /// the size does not fit tiny-skia — a size no buffer could hold.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let pixmap = Pixmap::new(width, height)?;
        Some(Canvas { pixmap })
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

    /// Fill one device-pixel rectangle with a cached colour. The edges are
    /// pixel-aligned, so the fill is unantialiased and writes the colour's
    /// bytes exactly. A rectangle with no size draws nothing; a rectangle
    /// reaching past the canvas is clipped to it.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: CanvasColor) {
        if w == 0 || h == 0 {
            return;
        }
        // The rectangle is built through the integer path: the coordinates
        // are device pixels, and the conversion refuses what does not fit.
        let (Ok(x_i), Ok(y_i)) = (i32::try_from(x), i32::try_from(y)) else {
            return;
        };
        let Some(int_rect) = tiny_skia::IntRect::from_xywh(x_i, y_i, w, h) else {
            return;
        };
        let rect = int_rect.to_rect();
        let paint = Paint {
            shader: Shader::SolidColor(color.color),
            // Pixel-aligned device rectangles need no antialiasing, and the
            // default linear colour space passes the bytes through untransformed:
            // no gamma conversion runs between the cached colour and the pixmap.
            anti_alias: false,
            ..Paint::default()
        };
        self.pixmap
            .fill_rect(rect, &paint, Transform::identity(), None);
    }

    /// The canvas bytes in the `ARGB8888` memory order blue, green, red,
    /// alpha, for the copy into the `wl_shm` buffer (D5).
    #[must_use]
    pub fn data(&self) -> &[u8] {
        self.pixmap.data()
    }
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
}
