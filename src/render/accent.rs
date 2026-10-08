//! The focus accent layer of the grid painter: the stroke around
//! the whole window in the configured accent colour and width, drawn only
//! while the panel holds keyboard focus. Layer surfaces get no compositor
//! focus ring (niri draws one only around layout windows), so the focused
//! panel marks itself.
//!
//! The geometry is that stroke's, in raw surface (device) coordinates: a
//! rectangle inset by half the stroke width, so the stroke stays inside
//! the window. The stroke of that rectangle is the four axis-aligned bands
//! its line covers — outer edges on the window's edges, inner edges one
//! stroke in — filled here as four rectangles, which tiles the same
//! region. Unlike the grid layers, the accent is not snapped and not
//! translated by the tween's draw offset: a stroke the window edges pin
//! cannot shift.
//!
//! GTK-free (`replace-gtk-with-wayland` D10); private to the painter, whose
//! frame pass calls it last, on top of every grid layer.

use super::canvas::Canvas;
use super::geom::{DeviceRect, FrameAccent, FrameInput, PainterMetrics, device_f32};

/// Draw the focus accent for `frame`, if the frame is focused and an accent
/// is configured. `metrics` supplies the output scale the accent's logical
/// stroke width is converted with.
pub(super) fn paint(canvas: &mut Canvas, frame: &FrameInput, metrics: &PainterMetrics) {
    let Some(accent) = frame.accent() else {
        return;
    };
    if !frame.focused() {
        return;
    }
    let Some(bands) = bands(frame, metrics, accent) else {
        return;
    };
    for (x, y, w, h) in bands {
        canvas.fill_rect_f32(x, y, w, h, accent.color());
    }
}

/// The four accent bands of [`paint`], as `(x, y, w, h)` device f32
/// rectangles — the one place the band geometry is written, so the full
/// and the partial draw cannot drift. The caller has checked that the
/// frame carries an accent and passes it in.
fn bands(
    frame: &FrameInput,
    metrics: &PainterMetrics,
    accent: FrameAccent,
) -> Option<[(f32, f32, f32, f32); 4]> {
    let (device_w, device_h) = frame.device_size();
    let (device_w, device_h) = to_f32_pair(device_w, device_h)?;
    let stroke = f32::from(accent.width().get()) * device_f32(metrics.scale());
    Some([
        (0.0, 0.0, device_w, stroke),
        (0.0, device_h - stroke, device_w, stroke),
        (0.0, 0.0, stroke, device_h),
        (device_w - stroke, 0.0, stroke, device_h),
    ])
}

/// Draw the part of the focus accent that falls inside `rows`:
/// the partial repaint cleared those device rectangles to the theme
/// background, which erased the accent where it crosses them, so the bands
/// are redrawn — clipped to each row rectangle, exactly the pixels a full
/// repaint would put there. The clipping edges are the rows' snapped whole
/// device pixels; a band's own fractional edge that falls inside a row is
/// preserved, so the anti-aliased edge pixels come out identical. Nothing
/// draws when the frame is unfocused or carries no accent, like [`paint`].
pub(super) fn paint_rows(
    canvas: &mut Canvas,
    frame: &FrameInput,
    metrics: &PainterMetrics,
    rows: &[DeviceRect],
) {
    if !frame.focused() {
        return;
    }
    let Some(accent) = frame.accent() else {
        return;
    };
    let color = accent.color();
    let Some(bands) = bands(frame, metrics, accent) else {
        return;
    };
    for row in rows {
        for (x, y, w, h) in bands {
            // The intersection of the row rectangle and the band: empty in
            // one axis draws nothing, like every canvas fill.
            let left = x.max(device_f32(f64::from(row.x())));
            let top = y.max(device_f32(f64::from(row.y())));
            let right = (x + w).min(device_f32(f64::from(row.x() + row.w())));
            let bottom = (y + h).min(device_f32(f64::from(row.y() + row.h())));
            if right > left && bottom > top {
                canvas.fill_rect_f32(left, top, right - left, bottom - top, color);
            }
        }
    }
}

/// The canvas' device extents as `f32`. `None` past the `f32` range — a
/// canvas that large cannot exist, so the accent simply does not draw.
fn to_f32_pair(width: u32, height: u32) -> Option<(f32, f32)> {
    let width = device_f32(f64::from(width));
    let height = device_f32(f64::from(height));
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((width, height))
}
