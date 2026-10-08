//! The canvas' resampling primitive: one source rectangle of a
//! premultiplied [`Pixmap`] scaled into a new [`Pixmap`] of an exact
//! destination size, by area averaging ("pixel mixing") in premultiplied
//! space.
//!
//! The filter choice: each destination pixel is the coverage-weighted
//! average of the source pixels its window overlaps, the window being the
//! source rectangle's own span mapped onto that destination pixel. A
//! downscale therefore averages whole boxes — the right filter for a large
//! poster reduction, where plain bilinear aliases — and an upscale
//! degenerates to bilinear interpolation, because the window shrinks below
//! one source pixel and the overlapping neighbours' fractional weights are
//! the bilinear weights. Averaging premultiplied channels is the correct
//! area average for alpha; the result stays premultiplied.
//!
//! The edges cannot clip: a window never reaches past the source
//! rectangle, so the first and last destination columns and rows carry the
//! full weight of the source's first and last columns and rows — at an
//! upscale they carry it alone and read the source edge colours exactly,
//! which is what keeps "Images are not clipped" true at the fractional
//! scales. GTK-free: the tests run without a display
//! (`replace-gtk-with-wayland` D10).

use tiny_skia::Pixmap;

/// Resample the source rectangle `(sx, sy, sw, sh)` of `src` —
/// premultiplied, channel order irrelevant — into a new [`Pixmap`] of
/// exactly `dst_w` by `dst_h` pixels. `None` when a source extent is not
/// positive, a destination dimension is zero, the rectangle reaches past
/// `src` (it is clipped to the image first, and an empty intersection
/// draws nothing), or the destination is too large to allocate.
///
/// The scale is free: `dst_w`/`dst_h` need not be an integer multiple of
/// `sw`/`sh`, and the mapping is the exact one the image pass draws with —
/// source pixel `sx + i` covers destination span
/// `[i * dst_w / sw, (i + 1) * dst_w / sw)`, so the whole source rectangle
/// maps onto the whole destination with nothing cut off at either edge.
#[must_use]
pub(crate) fn resample(
    src: &Pixmap,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    dst_w: u32,
    dst_h: u32,
) -> Option<Pixmap> {
    if sw <= 0 || sh <= 0 || dst_w == 0 || dst_h == 0 {
        return None;
    }
    // Intersect the source rectangle with the image; a placement the
    // terminal resolved past the image edge draws only the part that
    // exists, and an empty intersection draws nothing.
    let img_w = i32::try_from(src.width()).ok()?;
    let img_h = i32::try_from(src.height()).ok()?;
    let x0 = sx.clamp(0, img_w);
    let y0 = sy.clamp(0, img_h);
    let x1 = sx.checked_add(sw)?.clamp(0, img_w).max(x0);
    let y1 = sy.checked_add(sh)?.clamp(0, img_h).max(y0);
    if x1 == x0 || y1 == y0 {
        return None;
    }

    let mut out = Pixmap::new(dst_w, dst_h)?;
    let src_data = src.data();
    let src_stride = usize::try_from(src.width()).ok()?.checked_mul(4)?;
    let dst_data = out.data_mut();
    let dst_stride = usize::try_from(dst_w).ok()?.checked_mul(4)?;

    let src_w = f64::from(x1 - x0);
    let src_h = f64::from(y1 - y0);
    let dst_cols = f64::from(dst_w);
    let dst_rows = f64::from(dst_h);
    let base_x = f64::from(x0);
    let base_y = f64::from(y0);

    for dy in 0..dst_h {
        // This destination row's source window in image pixels.
        let wy0 = base_y + src_h * f64::from(dy) / dst_rows;
        let wy1 = base_y + src_h * f64::from(dy + 1) / dst_rows;
        let row_start = usize::try_from(dy).ok()?.checked_mul(dst_stride)?;
        for dx in 0..dst_w {
            let wx0 = base_x + src_w * f64::from(dx) / dst_cols;
            let wx1 = base_x + src_w * f64::from(dx + 1) / dst_cols;
            let pixel = average_window(src_data, src_stride, wx0, wx1, wy0, wy1);
            let dst = row_start + usize::try_from(dx).ok()? * 4;
            dst_data[dst..dst + 4].copy_from_slice(&pixel);
        }
    }
    Some(out)
}

/// The coverage-weighted average of the source pixels the window
/// `[wx0, wx1) × [wy0, wy1)` overlaps: premultiplied channels summed with
/// their overlap weights, divided by the window's area. The window is
/// clipped to whole non-negative source coordinates by its pixel loop, so
/// the weights only ever cover pixels that exist.
fn average_window(src: &[u8], stride: usize, wx0: f64, wx1: f64, wy0: f64, wy1: f64) -> [u8; 4] {
    // The overlapping source columns and rows, as integer ranges. `floor`
    // of the window start and `ceil` of the end bound the touched pixels.
    let ix0 = num_traits::cast::<f64, i64>(wx0.floor().max(0.0)).unwrap_or(0);
    let ix1 = num_traits::cast::<f64, i64>(wx1.ceil().max(0.0)).unwrap_or(0);
    let iy0 = num_traits::cast::<f64, i64>(wy0.floor().max(0.0)).unwrap_or(0);
    let iy1 = num_traits::cast::<f64, i64>(wy1.ceil().max(0.0)).unwrap_or(0);

    let mut sum = [0f64; 4];
    for iy in iy0..iy1 {
        let fy = num_traits::cast::<i64, f64>(iy).unwrap_or(0.0);
        let overlap_y = overlap(fy, fy + 1.0, wy0, wy1);
        if overlap_y <= 0.0 {
            continue;
        }
        let Some(src_row) = usize::try_from(iy)
            .ok()
            .and_then(|row| row.checked_mul(stride))
        else {
            continue;
        };
        for ix in ix0..ix1 {
            let fx = num_traits::cast::<i64, f64>(ix).unwrap_or(0.0);
            let overlap_x = overlap(fx, fx + 1.0, wx0, wx1);
            if overlap_x <= 0.0 {
                continue;
            }
            let base = usize::try_from(ix)
                .ok()
                .and_then(|col| src_row.checked_add(col.checked_mul(4)?));
            let Some(base) = base else { continue };
            let weight = overlap_x * overlap_y;
            for (channel, byte) in sum.iter_mut().zip(&src[base..base + 4]) {
                *channel += weight * f64::from(*byte);
            }
        }
    }

    // The window's total coverage is its area; the average divides by it.
    // A window always covers at least one pixel (the caller's rectangle is
    // non-empty), so the area is positive.
    let area = (wx1 - wx0) * (wy1 - wy0);
    let mut out = [0u8; 4];
    if area > 0.0 {
        for (out_byte, channel) in out.iter_mut().zip(sum) {
            let scaled = channel / area + 0.5;
            // The weighted average of bytes stays within [0, 255]; the
            // rounding dust cannot leave that range.
            *out_byte =
                u8::try_from(num_traits::cast::<f64, i64>(scaled).unwrap_or(0)).unwrap_or(u8::MAX);
        }
    }
    out
}

/// The length of `[a0, a1)` inside `[b0, b1)`.
fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 premultiplied pixmap of the given pixel values, channel order
    /// as stored (the tests treat channels as anonymous numbers).
    fn pixmap2x2(pixels: [[u8; 4]; 4]) -> Pixmap {
        let mut pixmap = Pixmap::new(2, 2).expect("test pixmap size is valid");
        pixmap.data_mut().copy_from_slice(pixels.as_flattened());
        pixmap
    }

    /// A 4x4 pixmap, row major.
    fn pixmap4x4(pixels: [[u8; 4]; 16]) -> Pixmap {
        let mut pixmap = Pixmap::new(4, 4).expect("test pixmap size is valid");
        pixmap.data_mut().copy_from_slice(pixels.as_flattened());
        pixmap
    }

    /// Identity: a source rect resampled at its own size returns the same
    /// bytes, so the whole-image fast path and the resampler agree.
    #[test]
    fn identity_resampling_returns_the_same_bytes() {
        let src = pixmap2x2([
            [10, 20, 30, 255],
            [40, 50, 60, 255],
            [70, 80, 90, 255],
            [1, 2, 3, 255],
        ]);
        let out = resample(&src, 0, 0, 2, 2, 2, 2).expect("identity resample");
        assert_eq!(out.data(), src.data());
    }

    /// A twofold downscale averages the boxes: each destination pixel is
    /// the mean of its 2x2 source box, per channel.
    #[test]
    fn a_twofold_downscale_averages_each_box() {
        let mut src = pixmap4x4([[0u8; 4]; 16]);
        for (i, px) in src.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px[0] = u8::try_from(i).expect("fits");
            px[3] = 255;
        }
        let out = resample(&src, 0, 0, 4, 4, 2, 2).expect("downscale");
        let got = out.data();
        // Box means of (0,1 / 4,5), (2,3 / 6,7), (8,9 / 12,13), (10,11 /
        // 14,15). Each mean is x.5, and the +0.5 rounding rounds up.
        assert_eq!(got[0], 3, "mean of 0,1,4,5 rounds 2.5 up");
        assert_eq!(got[4], 5, "mean of 2,3,6,7 rounds 4.5 up");
        assert_eq!(got[8], 11, "mean of 8,9,12,13 rounds 10.5 up");
        assert_eq!(got[12], 13, "mean of 10,11,14,15 rounds 12.5 up");
        assert_eq!(got[3], 255, "alpha stays opaque");
    }

    /// A twofold upscale replicates each source pixel into its 2x2 block
    /// exactly: bilinear degenerates to nearest at exact integer factors.
    #[test]
    fn a_twofold_upscale_replicates_each_pixel() {
        let src = pixmap2x2([
            [10, 0, 0, 255],
            [0, 40, 0, 255],
            [0, 0, 70, 255],
            [0, 0, 0, 255],
        ]);
        let out = resample(&src, 0, 0, 2, 2, 4, 4).expect("upscale");
        let got = out.data();
        let mut want = Vec::new();
        for y in 0..4u32 {
            for x in 0..4u32 {
                let source = usize::try_from(y / 2 * 2 + x / 2).expect("fits") * 4;
                want.extend_from_slice(&src.data()[source..source + 4]);
            }
        }
        assert_eq!(got, want.as_slice(), "each 2x2 block is its source pixel");
    }

    /// The whole source rectangle reaches the whole destination: with a
    /// uniform border colour, the first and last destination columns and
    /// rows read the border exactly at a non-integer upscale, and nothing
    /// is cut off at the right or bottom (the "not clipped" bug class —
    /// dropping the last source column would pull the interior into the
    /// edge pixels and lower them below the border).
    #[test]
    fn the_source_edges_reach_the_destination_edges() {
        // 4x4: a uniform border of 200 around an interior of 0.
        let mut pixels = [[0u8; 4]; 16];
        for (i, px) in pixels.iter_mut().enumerate() {
            let col = i % 4;
            let row = i / 4;
            let border = col == 0 || col == 3 || row == 0 || row == 3;
            *px = [u8::from(border) * 200, 0, 0, 255];
        }
        let src = pixmap4x4(pixels);
        // Upscale 4 -> 7 (ratio 1.75, like the snapped device width at
        // scale 1.8): every edge pixel of the destination reads the border.
        let out = resample(&src, 0, 0, 4, 4, 7, 7).expect("upscale");
        let got = out.data();
        let pixel = |x: usize, y: usize| -> u8 {
            let base = (y * 7 + x) * 4;
            got[base]
        };
        for i in 0..7usize {
            assert_eq!(pixel(i, 0), 200, "the first row is the border at x = {i}");
            assert_eq!(pixel(i, 6), 200, "the last row is the border at x = {i}");
            assert_eq!(
                pixel(0, i),
                200,
                "the first column is the border at y = {i}"
            );
            assert_eq!(pixel(6, i), 200, "the last column is the border at y = {i}");
        }
        // The interior shows through in the middle.
        assert_eq!(pixel(3, 3), 0, "the interior is the interior");
    }

    /// A downscale still shows the source's edge colours in its edge
    /// pixels: the last destination column's window includes the last
    /// source column with full weight, so the edge colour survives the
    /// average instead of being cut off.
    #[test]
    fn a_downscale_keeps_the_edge_colours_in_the_edge_pixels() {
        // 4x2: last column a distinct colour, the rest another.
        let mut pixels = [[0u8; 4]; 8];
        for (i, px) in pixels.iter_mut().enumerate() {
            *px = if i % 4 == 3 {
                [200, 200, 200, 255]
            } else {
                [0, 0, 0, 255]
            };
        }
        let mut src = Pixmap::new(4, 2).expect("test pixmap size is valid");
        src.data_mut().copy_from_slice(pixels.as_flattened());
        let out = resample(&src, 0, 0, 4, 2, 2, 1).expect("downscale");
        let got = out.data();
        // The last destination column averages source columns 2 and 3
        // equally: (0 + 200) / 2 = 100 — the edge colour contributes, so
        // the column is not clipped away.
        assert_eq!(got[4], 100, "the last column blends the source edge in");
        // The first column averages columns 0 and 1: pure 0.
        assert_eq!(got[0], 0);
    }

    /// A source crop resamples exactly that crop: the rect selects the
    /// pixels, the destination size scales them.
    #[test]
    fn a_source_crop_resamples_exactly_that_crop() {
        let mut src = pixmap4x4([[0u8; 4]; 16]);
        {
            for (i, px) in src.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
                px[0] = u8::try_from(i).expect("fits");
                px[3] = 255;
            }
        }
        // Crop the bottom right 2x2 (pixels 10, 11, 14, 15), upscale 2x.
        let out = resample(&src, 2, 2, 2, 2, 4, 4).expect("crop resample");
        let got = out.data();
        // Block replication: 10 top left, 11 top right, 14 bottom left,
        // 15 bottom right — the 4x4 output's blocks sit at pixel indices
        // (0,0), (2,0), (0,2), (2,2), each ×4 bytes.
        assert_eq!(got[0], 10);
        assert_eq!(got[2 * 4], 11, "top right block");
        assert_eq!(got[2 * 4 * 4], 14, "bottom left block");
        assert_eq!(got[(3 * 4 + 3) * 4], 15, "bottom right block");
    }

    /// Alpha is averaged in premultiplied space: a half-transparent pixel
    /// averaged with a transparent one stays premultiplied half.
    #[test]
    fn alpha_averages_in_premultiplied_space() {
        // (50, 0, 0, 128) premultiplied next to (0, 0, 0, 0).
        let src = pixmap2x2([[50, 0, 0, 128], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]);
        let out = resample(&src, 0, 0, 2, 2, 1, 1).expect("average");
        // Mean premultiplied: (50+0+0+0)/4 = 12.5 rounds to 13,
        // alpha (128+0+0+0)/4 = 32. The 1x1 output is one pixel.
        assert_eq!(out.data(), [13, 0, 0, 32]);
    }

    /// Degenerate inputs draw nothing: non-positive source extents, zero
    /// destination sizes, and an empty clipped intersection.
    #[test]
    fn degenerate_inputs_return_none() {
        let src = pixmap2x2([[0; 4]; 4]);
        assert!(resample(&src, 0, 0, 0, 2, 2, 2).is_none(), "zero width");
        assert!(resample(&src, 0, 0, 2, 0, 2, 2).is_none(), "zero height");
        assert!(
            resample(&src, 0, 0, -2, 2, 2, 2).is_none(),
            "negative width"
        );
        assert!(resample(&src, 0, 0, 2, 2, 0, 2).is_none(), "zero dst width");
        assert!(
            resample(&src, 0, 0, 2, 2, 2, 0).is_none(),
            "zero dst height"
        );
        // A rect wholly past the image clips to empty.
        assert!(
            resample(&src, 5, 0, 2, 2, 2, 2).is_none(),
            "past the right edge"
        );
        assert!(
            resample(&src, 0, 5, 2, 2, 2, 2).is_none(),
            "past the bottom edge"
        );
        // A rect hanging off clips to the visible part and resamples it.
        let out = resample(&src, -1, -1, 2, 2, 2, 2).expect("the visible part resamples");
        assert_eq!(out.width(), 2);
    }
}
