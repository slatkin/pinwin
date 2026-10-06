//! The tween's wide draw and its copies (replace-gtk-with-wayland
//! row 6.2, design decision 7): the grid drawn once into a wide
//! [`Canvas`] through the existing painter, the one upload of the cached
//! canvas into its `wl_shm` buffer, and the no-viewporter fallback that
//! copies each frame's crop into a fresh buffer — a memory copy, no
//! glyph work. Pure and display-free (`port-to-rust` D10); the
//! planner's decisions live in [`super`].

use crate::fontconfig::ThemeColours;
use crate::layout::Accent;
use crate::render::canvas::Canvas;
use crate::render::geom::{FrameInput, PainterMetrics};
use crate::render::image_pass::ImagePass;
use crate::render::painter::paint_frame;
use crate::render::text_pass::TextPass;
use crate::term::Terminal;

use super::{CropFrame, WideDraw};

/// Draw the live grid once into the wide canvas (D7, row 6.2): the theme
/// background fills the whole canvas and the grid sits against the docked
/// edge, shifted by the draw offset [`TweenCrop::wide_draw`] decided. The
/// existing painter draws every layer — nothing about it changes.
///
/// `metrics` must carry the same scale the crop geometry was decided at; it
/// is the painter's own scale input, and the offset's round trip through it
/// is what the offset test pins. Returns `None` when the wide device size
/// is no canvas — a zero or oversized dimension, which the frame plan
/// refuses before this runs.
#[must_use]
pub fn draw_wide(
    draw: WideDraw,
    metrics: &PainterMetrics,
    focused: bool,
    theme: ThemeColours,
    accent: Option<Accent>,
    terminal: &mut Terminal,
    text: &mut TextPass,
    images: &mut ImagePass,
) -> Option<Canvas> {
    let (device_w, device_h) = draw.device();
    let mut canvas = Canvas::new(device_w, device_h)?;
    let (logical_w, logical_h) = draw.logical();
    let frame = FrameInput::new(
        logical_w,
        logical_h,
        device_w,
        device_h,
        draw.draw_offset(),
        focused,
        theme,
        accent,
    );
    paint_frame(&mut canvas, metrics, &frame, terminal, text, images);
    Some(canvas)
}

/// Why a crop copy refused. The callers degrade the same way for every
/// variant — the frame is skipped, the next one retries — so the variants
/// only carry the comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropCopyError {
    /// The wide canvas, the crop rectangle and the destination disagree
    /// about their shapes: a canvas that is not the wide buffer, a crop
    /// outside it, or a destination that is not exactly the crop.
    Shape,
}

/// Copy the wide canvas into `dest` — the pool buffer's bytes — at the
/// tween's start (D7): the one full upload of the cached wide canvas. A
/// destination that is not exactly the canvas' size is refused rather than
/// panicking, the way a raw `copy_from_slice` would.
///
/// # Errors
/// [`CropCopyError::Shape`] when `dest` does not match the canvas.
pub fn upload_wide(canvas: &Canvas, dest: &mut [u8]) -> Result<(), CropCopyError> {
    if dest.len() != canvas.data().len() {
        return Err(CropCopyError::Shape);
    }
    dest.copy_from_slice(canvas.data());
    Ok(())
}

/// Copy the frame's crop rectangle of the cached wide canvas into `dest` —
/// a fresh pool buffer's bytes (D7, the no-viewporter fallback): a memory
/// copy, row by row, no glyph work. The canvas bytes are already in the
/// `ARGB8888` memory order, so the copy is exact.
///
/// # Errors
/// [`CropCopyError::Shape`] when the canvas is not the wide buffer the
/// frame was planned against, the crop rectangle leaves it, or `dest` is
/// not exactly the crop.
pub fn copy_crop(wide: &Canvas, frame: &CropFrame, dest: &mut [u8]) -> Result<(), CropCopyError> {
    let source = frame.source();
    let (wide_w, wide_h) = wide.size();
    let Ok(source_h) = u32::try_from(source.height()) else {
        return Err(CropCopyError::Shape);
    };
    if wide_w != frame.wide_width_dev() || wide_h != source_h {
        return Err(CropCopyError::Shape);
    }
    if source.x() < 0
        || source.width() < 0
        || i64::from(source.x()) + i64::from(source.width()) > i64::from(wide_w)
    {
        return Err(CropCopyError::Shape);
    }
    let Ok(crop_w) = u32::try_from(source.width()) else {
        return Err(CropCopyError::Shape);
    };
    let Some(row_bytes) = u64::from(crop_w).checked_mul(4) else {
        return Err(CropCopyError::Shape);
    };
    let Some(total) = row_bytes.checked_mul(u64::from(source_h)) else {
        return Err(CropCopyError::Shape);
    };
    if !u64::try_from(dest.len()).is_ok_and(|len| len == total) {
        return Err(CropCopyError::Shape);
    }
    let Ok(row) = usize::try_from(row_bytes) else {
        return Err(CropCopyError::Shape);
    };
    let Ok(row_index_max) = usize::try_from(u64::from(source_h)) else {
        return Err(CropCopyError::Shape);
    };
    let Ok(x_bytes) = usize::try_from(i64::from(source.x()) * 4) else {
        return Err(CropCopyError::Shape);
    };
    let Ok(stride) = usize::try_from(u64::from(wide_w) * 4) else {
        return Err(CropCopyError::Shape);
    };
    for row_index in 0..row_index_max {
        let Some(src_start) = row_index
            .checked_mul(stride)
            .and_then(|offset| offset.checked_add(x_bytes))
        else {
            return Err(CropCopyError::Shape);
        };
        let Some(src_end) = src_start.checked_add(row) else {
            return Err(CropCopyError::Shape);
        };
        let Some(dest_start) = row_index.checked_mul(row) else {
            return Err(CropCopyError::Shape);
        };
        let Some(dest_end) = dest_start.checked_add(row) else {
            return Err(CropCopyError::Shape);
        };
        dest[dest_start..dest_end].copy_from_slice(&wide.data()[src_start..src_end]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::layout::Side;
    use crate::panel::wayland_side::buffers::FractionalScale;
    use crate::panel::wayland_side::crop::TweenCrop;
    use crate::render::canvas::Canvas;
    use crate::render::geom::{FrameInput, device_px};
    use crate::render::image_pass::ImagePass;
    use crate::render::painter::paint_frame;
    use crate::render::text_pass::test_support::{self, Rig, THEME};

    use super::{CropCopyError, copy_crop, draw_wide, upload_wide};

    fn scale(units: u32) -> FractionalScale {
        FractionalScale::from_120ths(units)
    }

    /// A `u32` device size as `i32`, for the tests' arithmetic.
    fn dev(value: u32) -> i32 {
        i32::try_from(value).expect("test device size fits i32")
    }

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

    /// An 8-column, 4-row terminal with text in every row, at the rig's
    /// pitch.
    fn inked_terminal(rig: &Rig) -> crate::term::Terminal {
        let mut terminal =
            crate::term::Terminal::new(crate::guard::Poisoned::new(), NullSink, NoDecoder, || {});
        assert!(terminal.push_size(8, 4, rig.cell_w, rig.cell_h));
        terminal.push_pty_data(test_support::HIDE_CURSOR);
        let row: Vec<u8> = b"ABCDEFGHIJKLMNOP".to_vec();
        terminal.push_pty_data(&row);
        terminal.push_pty_data(&row);
        terminal.push_pty_data(&row);
        terminal
    }

    /// The device width the crop of `logical` device pixels is at `s`, as
    /// the planner computes it.
    fn crop_device(logical: i32, s: FractionalScale) -> i32 {
        dev(s
            .scale_dimension(u32::try_from(logical).expect("test width fits u32"))
            .expect("test width scales"))
    }

    /// The row 6.2 pixel verify: the crop of the wide buffer matches what a
    /// direct draw at the narrower width shows at the docked edge, for both
    /// sides, at scale 1.5.
    #[test]
    fn the_cropped_wide_canvas_matches_a_direct_draw_at_the_docked_edge() {
        let Some(mut rig) = Rig::new() else { return };
        let s = scale(180);
        let metrics = rig.metrics(1.5, rig.cell_h);
        let height = u32::try_from(rig.cell_h * 4).expect("test height fits u32");
        let grid_px = rig.cell_w * 8;
        let end = grid_px - 3 * rig.cell_w;
        let current = end - 13;
        assert!(current >= 1, "the test width is presentable");

        for &side in &[Side::Left, Side::Right] {
            let mut terminal = inked_terminal(&rig);
            let mut images = ImagePass::new();

            // The wide buffer: drawn once at the tween's start, for the
            // larger width (the grid's own).
            let crop = TweenCrop::new(side, grid_px, end).expect("a valid tween");
            let draw = crop.wide_draw(height, grid_px, s).expect("a drawable size");
            let wide = draw_wide(
                draw,
                &metrics,
                false,
                THEME,
                None,
                &mut terminal,
                &mut rig.test.pass,
                &mut images,
            )
            .expect("the wide canvas");

            // The frame plan at the eased width, and the direct draw at the
            // same width for comparison.
            let frame = crop.frame(current, height, s).expect("a presentable frame");
            let crop_w = crop_device(current, s);
            let direct_offset = match side {
                Side::Left => 0.0,
                Side::Right => {
                    let grid_dev = crop_device(grid_px, s);
                    f64::from(crop_w - grid_dev) / s.as_f64()
                }
            };
            assert_eq!(
                device_px(direct_offset * 1.5),
                match side {
                    Side::Left => 0,
                    Side::Right => crop_w - crop_device(grid_px, s),
                },
                "the direct draw sits on the docked edge in device pixels"
            );
            let direct_frame = FrameInput::new(
                u32::try_from(current).expect("test width fits u32"),
                height,
                u32::try_from(crop_w).expect("test width fits u32"),
                u32::try_from(frame.source().height()).expect("test height fits u32"),
                direct_offset,
                false,
                THEME,
                None,
            );
            let mut direct = Canvas::new(
                u32::try_from(crop_w).expect("test width fits u32"),
                u32::try_from(frame.source().height()).expect("test height fits u32"),
            )
            .expect("the direct canvas");
            paint_frame(
                &mut direct,
                &metrics,
                &direct_frame,
                &mut terminal,
                &mut rig.test.pass,
                &mut images,
            );

            // The crop of the wide buffer equals the direct draw, pixel for
            // pixel — with ink in it, so the comparison is not vacuous.
            let mut cropped = vec![0_u8; direct.data().len()];
            copy_crop(&wide, &frame, &mut cropped).expect("the crop copies");
            assert_eq!(cropped.as_slice(), direct.data(), "side {side:?}");
            assert!(
                direct
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| *pixel != [30, 20, 10, 255]),
                "the comparison has ink"
            );
        }
    }

    /// The no-viewporter fallback copy equals the crop, taken by hand from
    /// the wide canvas; a full-width crop is the whole canvas, and a wrong
    /// destination or canvas shape is refused.
    #[test]
    fn the_fallback_copy_equals_the_crop() {
        let s = scale(180);
        // A wide canvas of one tween's own wide size: 8 logical px at 1.5
        // is 12 device px, 12 device px tall (8 logical). Distinct
        // per-column colours, so a misplaced copy shows.
        let mut wide = Canvas::new(12, 12).expect("test canvas");
        for column in 0..12_i32 {
            let shade = u8::try_from(column * 20).expect("test shade fits u8");
            wide.fill_rect(
                column,
                0,
                1,
                12,
                crate::render::canvas::CanvasColor::from_rgba(shade, 0, 255, 255),
            );
        }
        let crop = TweenCrop::new(Side::Right, 8, 8).expect("a valid tween");
        let frame = crop.frame(5, 8, s).expect("a presentable frame");
        // 5 logical px at 1.5 is 8 device px, cropped from x = 12 - 8 = 4.
        assert_eq!(frame.source().width(), 8);
        assert_eq!(frame.source().x(), 4);
        assert_eq!(frame.source().height(), 12);
        assert_eq!(frame.wide_width_dev(), 12);

        let mut dest = vec![0_u8; 8 * 12 * 4];
        copy_crop(&wide, &frame, &mut dest).expect("the crop copies");
        for (row_index, wide_row) in wide.data().as_chunks::<48>().0.iter().enumerate() {
            assert_eq!(
                &dest[row_index * 32..(row_index + 1) * 32],
                &wide_row[16..48],
                "row {row_index}"
            );
        }

        // A full-width crop is the whole canvas.
        let full = crop.frame(8, 8, s).expect("a presentable frame");
        let mut whole = vec![0_u8; wide.data().len()];
        copy_crop(&wide, &full, &mut whole).expect("the full crop copies");
        assert_eq!(whole.as_slice(), wide.data());

        // Wrong shapes are refused, not panicked.
        assert_eq!(
            copy_crop(&wide, &frame, &mut [0_u8; 7]),
            Err(CropCopyError::Shape)
        );
        let tall = TweenCrop::new(Side::Left, 8, 8)
            .expect("a valid tween")
            .frame(5, 5, s)
            .expect("a presentable frame");
        assert_eq!(
            copy_crop(&wide, &tall, &mut vec![0_u8; 8 * 8 * 4]),
            Err(CropCopyError::Shape),
            "the canvas is not the buffer the frame was planned against"
        );

        // The start upload checks its destination the same way.
        let mut snapshot = wide.data().to_vec();
        assert_eq!(upload_wide(&wide, &mut snapshot), Ok(()));
        assert_eq!(
            upload_wide(&wide, &mut [0_u8; 3]),
            Err(CropCopyError::Shape)
        );
    }
}
