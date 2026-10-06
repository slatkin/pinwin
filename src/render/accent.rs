//! The focus accent layer of the grid painter (row 4.3): the stroke around
//! the whole window in the configured accent colour and width, drawn only
//! while the panel holds keyboard focus. Layer surfaces get no compositor
//! focus ring (niri draws one only around layout windows), so the focused
//! panel marks itself — the same duty [`crate::render::DrawState::
//! draw_focus_accent`] performs on the GTK path.
//!
//! The geometry is that stroke's, in raw surface (device) coordinates: a
//! rectangle inset by half the stroke width, so the stroke stays inside
//! the window. The stroke of that rectangle is the four axis-aligned bands
//! its line covers — outer edges on the window's edges, inner edges one
//! stroke in — filled here as four rectangles, which tiles the same
//! region. Unlike the grid layers, the accent is not snapped and not
//! translated by the tween's draw offset (`DrawState::draw` draws it after
//! the grid, untranslated): a stroke the window edges pin cannot shift.
//!
//! GTK-free (`replace-gtk-with-wayland` D10); private to the painter, whose
//! frame pass calls it last, on top of every grid layer.

use super::canvas::Canvas;
use super::geom::{FrameInput, PainterMetrics, device_f32};

/// Draw the focus accent for `frame`, if the frame is focused and an accent
/// is configured. `metrics` supplies the output scale the accent's logical
/// stroke width is converted with.
pub(super) fn paint(canvas: &mut Canvas, frame: &FrameInput, metrics: &PainterMetrics) {
    if !frame.focused() {
        return;
    }
    let Some(accent) = frame.accent() else {
        return;
    };
    let (device_w, device_h) = frame.device_size();
    let Some((device_w, device_h)) = to_f32_pair(device_w, device_h) else {
        return;
    };
    // The logical stroke width scales to device pixels; the rectangle it
    // strokes is inset by half the stroke so the stroke stays inside.
    let stroke = f32::from(accent.width().get()) * device_f32(metrics.scale());
    let color = accent.color();
    // The stroked rectangle's four sides, outer edges first. A stroke
    // wider than the window degenerates into overlapping bands, which the
    // fills clip to the canvas.
    canvas.fill_rect_f32(0.0, 0.0, device_w, stroke, color);
    canvas.fill_rect_f32(0.0, device_h - stroke, device_w, stroke, color);
    canvas.fill_rect_f32(0.0, 0.0, stroke, device_h, color);
    canvas.fill_rect_f32(device_w - stroke, 0.0, stroke, device_h, color);
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
