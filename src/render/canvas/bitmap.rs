//! The canvas' bitmap blits (replace-gtk-with-wayland D5): the coverage
//! mask draw for the grayscale glyph bitmaps and the cached image pixmap
//! for the decoded PNGs and colour glyphs. Both enter the canvas
//! premultiplied and channel-swapped — [`image_pixmap`] performs the
//! one-time swap and premultiplication at cache time — and blend
//! source-over with the same rounding tiny-skia's fills use, so every path
//! into the canvas produces the same bytes for the same colour. GTK-free:
//! the tests run without a display (`replace-gtk-with-wayland` D10).

use tiny_skia::Pixmap;

use super::{Canvas, CanvasColor, mul_round_u8};

/// An 8-bit coverage mask (the grayscale glyph bitmap, row major from the
/// top left): built once per cached glyph and drawn with
/// [`Canvas::draw_mask`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanvasMask {
    width: usize,
    height: usize,
    coverage: Vec<u8>,
}

impl CanvasMask {
    /// A mask of `width` by `height` pixels from `coverage`; `None` when a
    /// dimension is zero or the byte count does not match the size.
    #[must_use]
    pub fn new(width: u32, height: u32, coverage: &[u8]) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let len = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        if coverage.len() != len {
            return None;
        }
        Some(CanvasMask {
            width: usize::try_from(width).ok()?,
            height: usize::try_from(height).ok()?,
            coverage: coverage.to_vec(),
        })
    }

    /// The mask's width, in pixels.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// The mask's height, in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }
}

impl Canvas {
    /// Draw a coverage mask tinted with a cached colour at an integer
    /// device position, source-over blended (the grayscale glyph bitmap).
    /// The mask's top left corner lands on the position; a mask reaching
    /// past the canvas is clipped to it, and a mask wholly outside it draws
    /// nothing. The blend never panics.
    pub fn draw_mask(&mut self, mask: &CanvasMask, x: i32, y: i32, color: CanvasColor) {
        // The mask's rectangle clipped to the canvas, in canvas coordinates.
        // The clip runs in i64 because the origin is signed and the sizes
        // are not.
        let (canvas_w, canvas_h) = (
            i64::from(self.pixmap.width()),
            i64::from(self.pixmap.height()),
        );
        let Ok(mask_w) = i64::try_from(mask.width()) else {
            return;
        };
        let Ok(mask_h) = i64::try_from(mask.height()) else {
            return;
        };
        let (x, y) = (i64::from(x), i64::from(y));
        let dx0 = x.max(0);
        let dy0 = y.max(0);
        let dx1 = (x + mask_w).min(canvas_w);
        let dy1 = (y + mask_h).min(canvas_h);
        if dx1 <= dx0 || dy1 <= dy0 {
            return;
        }
        // The matching source window in mask coordinates. Every bound below
        // is a non-negative index now, so the usize conversions hold.
        let (Ok(sx0), Ok(sy0), Ok(sx1), Ok(sy1), Ok(cx0), Ok(cy0)) = (
            usize::try_from(dx0 - x),
            usize::try_from(dy0 - y),
            usize::try_from(dx1 - x),
            usize::try_from(dy1 - y),
            usize::try_from(dx0),
            usize::try_from(dy0),
        ) else {
            return;
        };

        let data = self.pixmap.data_mut();
        for sy in sy0..sy1 {
            let dy = cy0 + (sy - sy0);
            for sx in sx0..sx1 {
                let coverage = mask.coverage[sy * mask.width() + sx];
                if coverage != 0 {
                    let dx = cx0 + (sx - sx0);
                    let dst = (dy * self.width + dx) * 4;
                    blend_over(&mut data[dst..dst + 4], color.premultiplied(coverage));
                }
            }
        }
    }
}

/// Build a cached image [`Pixmap`] from straight (non-premultiplied) RGBA
/// bytes (D5): the one-time red/blue swap plus premultiplication, so
/// decoded PNGs and colour glyphs enter the canvas through one place and no
/// per-frame conversion ever runs. `None` when the byte count does not
/// match `width * height * 4` or a dimension is zero.
#[must_use]
pub fn image_pixmap(rgba: &[u8], width: u32, height: u32) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;
    let expected = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)?;
    if rgba.len() != expected {
        return None;
    }
    let bytes = pixmap.data_mut();
    let (src_px, dst_px) = (rgba.as_chunks::<4>().0, bytes.as_chunks_mut::<4>().0);
    for (src, dst) in src_px.iter().zip(dst_px) {
        let [r, g, b, a] = *src;
        // The pixmap stores tiny-skia's red first, so the swapped colour's
        // bytes are the original's blue, green, red, alpha, premultiplied.
        dst[0] = mul_round_u8(b, a);
        dst[1] = mul_round_u8(g, a);
        dst[2] = mul_round_u8(r, a);
        dst[3] = a;
    }
    Some(pixmap)
}

/// One source-over blend of the premultiplied `src` pixel over the
/// premultiplied `dst` bytes, in the canvas' blue, green, red, alpha order:
/// `out = src + dst * (1 - src.alpha)`, each surviving channel scaled with
/// tiny-skia's rounding. The sum cannot round past 255, so the saturating
/// add never saturates.
fn blend_over(dst: &mut [u8], src: [u8; 4]) {
    let keep = 255 - src[3];
    for (d, s) in dst.iter_mut().zip(src) {
        *d = s.saturating_add(mul_round_u8(*d, keep));
    }
}

#[cfg(test)]
mod tests {
    use super::{Canvas, CanvasColor, CanvasMask, image_pixmap};
    use crate::term::cells::Rgb;

    /// A cached image keeps its bytes through the cache: straight RGBA in,
    /// swapped and premultiplied in the pixmap, and the blit lands those
    /// bytes at the position asked.
    #[test]
    fn an_image_blit_lands_the_cached_swapped_bytes() {
        let image =
            image_pixmap(&[10, 20, 30, 255, 40, 50, 60, 128], 2, 1).expect("image size is valid");
        // Swapped and premultiplied, memory order blue, green, red, alpha:
        // pixel 0 reads (blue 30, green 20, red 10); pixel 1 at alpha 128
        // premultiplies to blue 30, green 25, red 20.
        assert_eq!(image.data(), &[30, 20, 10, 255, 30, 25, 20, 128]);

        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.draw_image(&image, 1, 2);
        assert_eq!(canvas.pixel(1, 2), Some([30, 20, 10, 255]));
        assert_eq!(canvas.pixel(2, 2), Some([30, 25, 20, 128]));
        assert_eq!(canvas.pixel(0, 2), Some([0, 0, 0, 0]), "left of the image");
        assert_eq!(canvas.pixel(3, 2), Some([0, 0, 0, 0]), "right of the image");
    }

    /// An image blit blends source-over: a half-alpha image over an opaque
    /// background keeps half of each side, with the same rounding the mask
    /// blend and the fills use.
    #[test]
    fn an_image_blit_blends_source_over() {
        // A half-alpha, half-intensity grey: straight (128, 128, 128, 128)
        // premultiplies to 64 per channel (128*128+128 rounds to 64).
        let image = image_pixmap(&[128, 128, 128, 128], 1, 1).expect("image size is valid");
        let mut canvas = Canvas::new(2, 1).expect("test canvas size is valid");
        // Background: opaque white in the canvas' order (blue first).
        canvas.fill_rect(0, 0, 2, 1, CanvasColor::from_rgba(255, 255, 255, 255));
        canvas.draw_image(&image, 1, 0);
        // src = 64 per channel at alpha 128; keep = 127.
        // out = 64 + 255*127/255 rounded = 64 + 127 = 191.
        assert_eq!(canvas.pixel(1, 0), Some([191, 191, 191, 255]));
        assert_eq!(canvas.pixel(0, 0), Some([255, 255, 255, 255]), "untouched");
    }

    /// An image blit clips at every canvas edge without panic: an image
    /// hanging off each side leaves exactly its visible part.
    #[test]
    fn an_image_blit_clips_at_every_edge() {
        let image = image_pixmap(
            &[1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255],
            2,
            2,
        )
        .expect("image size is valid");
        // The image's pixels in the canvas' order: pixel (col, row) reads
        // (blue, green, red) of the straight RGBA at that position.
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.draw_image(&image, 1, 1);
        assert_eq!(
            canvas.pixel(1, 1),
            Some([3, 2, 1, 255]),
            "image pixel (0, 0)"
        );
        assert_eq!(
            canvas.pixel(2, 1),
            Some([6, 5, 4, 255]),
            "image pixel (1, 0)"
        );
        assert_eq!(
            canvas.pixel(1, 2),
            Some([9, 8, 7, 255]),
            "image pixel (0, 1)"
        );
        assert_eq!(
            canvas.pixel(2, 2),
            Some([12, 11, 10, 255]),
            "image pixel (1, 1)"
        );
        assert_eq!(canvas.pixel(0, 1), Some([0, 0, 0, 0]), "left of the image");
        // Top left corner hangs off the canvas: only the image's bottom
        // right pixel lands inside.
        canvas.clear();
        canvas.draw_image(&image, -1, -1);
        assert_eq!(
            canvas.pixel(0, 0),
            Some([12, 11, 10, 255]),
            "image pixel (1, 1)"
        );
        assert_eq!(
            canvas.pixel(1, 0),
            Some([0, 0, 0, 0]),
            "clipped at the edge"
        );
        assert_eq!(
            canvas.pixel(0, 1),
            Some([0, 0, 0, 0]),
            "clipped at the edge"
        );
        // Bottom right corner hangs off.
        canvas.clear();
        canvas.draw_image(&image, 3, 3);
        assert_eq!(
            canvas.pixel(3, 3),
            Some([3, 2, 1, 255]),
            "image pixel (0, 0)"
        );
        // Wholly outside: nothing, and no panic.
        canvas.clear();
        canvas.draw_image(&image, 8, 8);
        canvas.draw_image(&image, -8, -8);
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "wholly outside"
        );
    }

    /// A mask blit tints the coverage with the cached colour and blends
    /// source-over: full coverage writes the colour's bytes, partial
    /// coverage blends with the backdrop at the coverage fraction.
    #[test]
    fn a_mask_blit_tints_and_blends_the_coverage() {
        let mask = CanvasMask::new(2, 2, &[255, 128, 0, 64]).expect("mask size matches");
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        // Backdrop: opaque white (blue first in the canvas' order).
        canvas.fill_rect(0, 0, 4, 4, CanvasColor::from_rgba(255, 255, 255, 255));
        canvas.draw_mask(
            &mask,
            1,
            1,
            CanvasColor::from_theme(Rgb {
                r: 0xff,
                g: 0,
                b: 0,
            }),
        );
        // Full coverage over white: the red's swapped bytes (blue 0 first).
        assert_eq!(canvas.pixel(1, 1), Some([0, 0, 0xff, 0xff]));
        // Coverage 128 over white: source blue 0, green 0, red 128 at alpha
        // 128; keep = 127. Each kept channel is 255*127/255 rounded = 127,
        // so the red channel reaches 128 + 127 = 255 and the empty ones 127.
        assert_eq!(canvas.pixel(2, 1), Some([127, 127, 255, 255]));
        // Coverage 0: the backdrop untouched.
        assert_eq!(canvas.pixel(1, 2), Some([255, 255, 255, 255]));
        // Coverage 64 over white: source red 64 at alpha 64; keep = 191,
        // and 255*191/255 rounds to 191.
        assert_eq!(canvas.pixel(2, 2), Some([191, 191, 255, 255]));
    }

    /// A mask blit over a transparent backdrop writes the coverage-scaled
    /// premultiplied colour directly: no backdrop to keep.
    #[test]
    fn a_mask_blit_over_transparent_writes_the_scaled_colour() {
        let mask = CanvasMask::new(1, 1, &[128]).expect("mask size matches");
        let mut canvas = Canvas::new(1, 1).expect("test canvas size is valid");
        canvas.draw_mask(&mask, 0, 0, CanvasColor::from_rgba(100, 150, 200, 255));
        // Premultiplied at coverage 128: 100*128 -> 50, 150*128 -> 75,
        // 200*128 -> 100, alpha 128. Swapped: blue 100 first.
        assert_eq!(canvas.pixel(0, 0), Some([100, 75, 50, 128]));
    }

    /// A mask blit clips at every canvas edge without panic, and a mask
    /// wholly outside draws nothing.
    #[test]
    fn a_mask_blit_clips_at_every_edge() {
        let mask = CanvasMask::new(2, 2, &[255, 255, 255, 255]).expect("mask size matches");
        let colour = CanvasColor::from_theme(Rgb {
            r: 0,
            g: 0xff,
            b: 0,
        });
        let mut canvas = Canvas::new(4, 4).expect("test canvas size is valid");
        canvas.draw_mask(&mask, -1, -1, colour);
        assert_eq!(
            canvas.pixel(0, 0),
            Some([0, 0xff, 0, 0xff]),
            "mask pixel (1, 1)"
        );
        assert_eq!(
            canvas.pixel(1, 0),
            Some([0, 0, 0, 0]),
            "clipped at the edge"
        );
        assert_eq!(
            canvas.pixel(0, 1),
            Some([0, 0, 0, 0]),
            "clipped at the edge"
        );
        canvas.clear();
        canvas.draw_mask(&mask, 3, 3, colour);
        assert_eq!(
            canvas.pixel(3, 3),
            Some([0, 0xff, 0, 0xff]),
            "mask pixel (0, 0)"
        );
        canvas.clear();
        canvas.draw_mask(&mask, 8, 8, colour);
        canvas.draw_mask(&mask, -8, -8, colour);
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "wholly outside"
        );
    }

    /// A mask that does not match its size, or has none, is refused; a
    /// zero-coverage mask draws nothing.
    #[test]
    fn degenerate_masks_are_refused_or_draw_nothing() {
        assert!(CanvasMask::new(0, 2, &[]).is_none());
        assert!(CanvasMask::new(2, 2, &[255, 255, 255]).is_none());
        let empty = CanvasMask::new(2, 2, &[0, 0, 0, 0]).expect("mask size matches");
        let mut canvas = Canvas::new(2, 2).expect("test canvas size is valid");
        canvas.draw_mask(
            &empty,
            0,
            0,
            CanvasColor::from_theme(Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            }),
        );
        assert!(
            canvas.data().iter().all(|byte| *byte == 0),
            "a zero-coverage mask draws nothing"
        );
    }

    /// An image cache refuses bytes that do not match its size or an empty
    /// size.
    #[test]
    fn an_image_cache_refuses_degenerate_input() {
        assert!(image_pixmap(&[1, 2, 3, 4], 2, 1).is_none());
        assert!(image_pixmap(&[], 0, 1).is_none());
    }
}
