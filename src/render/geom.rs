//! The geometry seam between the snapped logical grid and the device-pixel
//! canvas (replace-gtk-with-wayland D5, D11). The snap rules in
//! `super::snap` work in logical pixels and land every edge on a device
//! pixel; this module turns the snapped rectangles into the integer device
//! rectangles the canvas primitives take, and carries the per-frame input
//! the painter needs: the cell metrics, the frame geometry and the theme
//! colours as cached [`crate::render::canvas::CanvasColor`]s.
//!
//! Every f64-to-integer conversion of the renderer goes through
//! [`crate::render::geom::device_px`], the module's one cast seam: tiny-skia takes `i32`/`f32`
//! geometry, the snapped edges are `f64`, and the `NumCast` conversion
//! keeps the cast lints quiet without a lint suppression. An `f32` reaches
//! the canvas' fractional primitives through `f32::from` of a
//! [`crate::render::geom::device_px`] result, or through this module when a
//! later painter row
//! needs fractional device coordinates.
//!
//! GTK-free like the snap rules (`replace-gtk-with-wayland` D10): the
//! tests run without a display. The module is `pub` because its only
//! consumer, the grid painter, arrives in rows 4.3 to 4.7 (`pub(crate)`
//! entries with no caller are dead code under `-D warnings`, and no lint
//! suppression is permitted).

use std::num::NonZeroU16;

use super::canvas::CanvasColor;
use super::snap::OutputScale;
use crate::fontconfig::ThemeColours;
use crate::layout::Accent;
use crate::term::cells::Rgb;

/// Convert one snapped device value to the integer device pixels the
/// canvas primitives take — the renderer's single f64-to-i32 seam (D5).
/// The input is a device-space value (a snapped edge times the scale), so
/// the conversion rounds to the nearest pixel: a snapped edge is a whole
/// number up to floating-point dust, and the rounding removes the dust. A
/// value that is not a number becomes 0, and one outside the `i32` range
/// clamps to the nearest end — a canvas that large cannot exist, so the
/// clamped value never reaches a draw.
#[must_use]
pub fn device_px(value: f64) -> i32 {
    let rounded = if value.is_nan() { 0.0 } else { value.round() };
    let clamped = rounded.clamp(-2_147_483_648.0, 2_147_483_647.0);
    // The clamp keeps the value inside the i32 range, so the conversion
    // cannot fail. Qualified on purpose: the NumCast trait in scope would
    // make every `f64::from` call ambiguous.
    num_traits::cast(clamped).unwrap_or(0)
}

/// Convert one device-space value to the `f32` the canvas' fractional
/// primitives take — the f64-to-f32 half of the cast seam (D5). The input
/// keeps its fractional part (the focus accent's inset stroke is not
/// snapped), so this only narrows the type: a value that is not a number
/// becomes 0, and one outside the `f32` range clamps to the nearest end —
/// a canvas that large cannot exist, so the clamped value never reaches a
/// draw.
#[must_use]
pub fn device_f32(value: f64) -> f32 {
    let clamped = if value.is_finite() { value } else { 0.0 };
    // The finite value fits an f32's magnitude range at canvas sizes, so
    // the conversion cannot fail. Qualified like `device_px`.
    num_traits::cast(clamped).unwrap_or(0.0)
}

/// Convert one logical value to the integer form the sprite geometry
/// computes on — the logical half of the cast seam (D5). The GTK path's
/// cell pitch is already an integer (`CellMetrics::cell_w` is `i32`); the
/// painter carries it as `f64`, and the braille dot grid needs that integer
/// back. Rounds like [`crate::render::geom::device_px`] does, for the same
/// dust and degeneracy
/// reasons.
#[must_use]
pub(crate) fn logical_px(value: f64) -> i32 {
    device_px(value)
}

/// A rectangle in device pixels (D5): the integer form the canvas
/// primitives take. The fields are private; [`PainterMetrics`] is the
/// constructor, so a `DeviceRect` always carries non-negative extents
/// (port-to-rust D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl DeviceRect {
    /// A rectangle from its parts; `None` when an extent is negative. An
    /// empty rectangle (a zero extent) is valid — it draws nothing.
    #[must_use]
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Option<Self> {
        if w < 0 || h < 0 {
            return None;
        }
        Some(DeviceRect { x, y, w, h })
    }

    /// The left edge, in device pixels.
    #[must_use]
    pub fn x(&self) -> i32 {
        self.x
    }

    /// The top edge, in device pixels.
    #[must_use]
    pub fn y(&self) -> i32 {
        self.y
    }

    /// The width, in device pixels.
    #[must_use]
    pub fn w(&self) -> i32 {
        self.w
    }

    /// The height, in device pixels.
    #[must_use]
    pub fn h(&self) -> i32 {
        self.h
    }

    /// Whether the rectangle draws nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// The cell metrics the grid painter draws with — the painter's form of
/// [`crate::render::cell_metrics::CellMetrics`]: the cell pitch in logical
/// pixels and the output
/// scale the frame snaps at. The fields are private (port-to-rust D6); the
/// constructor and the rectangle helpers carry the invariants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PainterMetrics {
    cell_w: f64,
    cell_h: f64,
    ascent: f64,
    scale: OutputScale,
}

impl PainterMetrics {
    /// Metrics for a cell pitch and a font ascent in logical pixels at
    /// `scale`. `None` when a pitch is not finite and positive — a cell no
    /// terminal could have. A non-finite ascent falls back to 0, which only
    /// moves the underline band to the cell's top. A `scale` that is not
    /// positive and finite falls back to 1, like `OutputScale::new` does.
    #[must_use]
    pub fn new(cell_w: f64, cell_h: f64, ascent: f64, scale: f64) -> Option<Self> {
        if !cell_w.is_finite() || !cell_h.is_finite() || cell_w <= 0.0 || cell_h <= 0.0 {
            return None;
        }
        Some(PainterMetrics {
            cell_w,
            cell_h,
            ascent: if ascent.is_finite() { ascent } else { 0.0 },
            scale: OutputScale::new(scale),
        })
    }

    /// The horizontal cell pitch, in logical pixels.
    #[must_use]
    pub fn cell_w(&self) -> f64 {
        self.cell_w
    }

    /// The vertical cell pitch, in logical pixels.
    #[must_use]
    pub fn cell_h(&self) -> f64 {
        self.cell_h
    }

    /// The font ascent, in logical pixels: the anchor the underline band
    /// hangs from.
    #[must_use]
    pub fn ascent(&self) -> f64 {
        self.ascent
    }

    /// The scale the frame snaps at, in device pixels per logical pixel.
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.scale.get()
    }

    /// The snapped device rectangle of the cell at `(col, row)` (D5): the
    /// cell's edges snap individually, so two cells that share an edge
    /// compute the same snapped value and never leave a seam.
    #[must_use]
    pub fn cell_rect(&self, col: i32, row: i32) -> DeviceRect {
        self.snap_device(
            f64::from(col) * self.cell_w,
            f64::from(row) * self.cell_h,
            self.cell_w,
            self.cell_h,
        )
    }

    /// The snapped device rectangle of `len` cells in one row, starting at
    /// `(col, row)`: one rectangle for the whole run, so the cells inside
    /// it cannot develop seams. A run with no cells gives an empty
    /// rectangle.
    #[must_use]
    pub fn run_rect(&self, col: i32, row: i32, len: i32) -> DeviceRect {
        if len <= 0 {
            return DeviceRect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
        }
        self.snap_device(
            f64::from(col) * self.cell_w,
            f64::from(row) * self.cell_h,
            f64::from(len) * self.cell_w,
            self.cell_h,
        )
    }

    /// The snapped device rectangle of a part of the cell at `(col, row)`:
    /// `dx`/`dy` offset and `w`/`h` size in logical pixels relative to the
    /// cell's top left corner. The half-block cases pass fractions of the
    /// pitch (`dx = cell_w / 2`), the bar cursor passes an absolute width
    /// (`w = 2.0`); both snap like every other rectangle, so a half cell
    /// and its other half share the snapped middle edge. A part with no
    /// size, or a negative size, gives an empty rectangle at its snapped
    /// origin.
    #[must_use]
    pub fn cell_sub_rect(
        &self,
        col: i32,
        row: i32,
        dx: f64,
        dy: f64,
        w: f64,
        h: f64,
    ) -> DeviceRect {
        self.snap_device(
            f64::from(col) * self.cell_w + dx,
            f64::from(row) * self.cell_h + dy,
            w,
            h,
        )
    }

    /// The snapped device rectangle of an arbitrary logical rectangle —
    /// the kitty image placements' destination rectangles, which the
    /// terminal reports in logical pixels rather than cell units. The
    /// edges snap on their own (`OutputScale::snap_rect`: the right
    /// edge is `snap(x + w)`, never `snap(x) + snap(w)`), so the
    /// rectangle keeps its last device pixel and a positive size never
    /// collapses below one device pixel. A negative or zero size gives an
    /// empty rectangle at its snapped origin.
    #[must_use]
    pub fn logical_rect(&self, x: f64, y: f64, w: f64, h: f64) -> DeviceRect {
        self.snap_device(x, y, w, h)
    }

    /// Snap a logical rectangle and scale it to device pixels. Each device
    /// edge is `device_px` of the snapped edge times the scale, so two
    /// rectangles that share a snapped logical edge share the device edge
    /// too. A rectangle whose snapped extent came out inverted (a negative
    /// logical size) normalizes to empty at its snapped origin.
    fn snap_device(&self, x: f64, y: f64, w: f64, h: f64) -> DeviceRect {
        let (lx, ly, lw, lh) = self.scale.snap_rect(x, y, w, h);
        let scale = self.scale.get();
        let x0 = device_px(lx * scale);
        let y0 = device_px(ly * scale);
        let x1 = device_px((lx + lw) * scale).max(x0);
        let y1 = device_px((ly + lh) * scale).max(y0);
        DeviceRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

/// The focus accent as the painter draws it: the accent colour cached once
/// (D5) and the stroke width in pixels — the same meaning the cairo
/// painter's `draw_focus_accent` gives them today, a stroke around the
/// whole window inset by half the width so it stays inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameAccent {
    color: CanvasColor,
    width: NonZeroU16,
}

impl FrameAccent {
    /// The cached accent colour.
    #[must_use]
    pub fn color(&self) -> CanvasColor {
        self.color
    }

    /// The stroke width, in pixels.
    #[must_use]
    pub fn width(&self) -> NonZeroU16 {
        self.width
    }
}

/// One frame's input to the grid painter: the geometry, the focus state
/// and the theme colours as cached [`CanvasColor`]s (D5 — the colours swap
/// their channels once, here at construction). Built per frame from the
/// configure and the draw state.
#[derive(Clone, Copy, Debug)]
pub struct FrameInput {
    logical_width: u32,
    logical_height: u32,
    device_width: u32,
    device_height: u32,
    draw_offset: f64,
    focused: bool,
    background: CanvasColor,
    foreground: CanvasColor,
    accent: Option<FrameAccent>,
}

impl FrameInput {
    /// A frame input for the frame's sizes, the tween's draw offset, the
    /// focus state, the Ghostty theme colours and the startup accent — the
    /// inputs the frame draw takes.
    ///
    /// `draw_offset` is the tween's snapped docked-edge offset in logical
    /// pixels: the grid draws shifted by it along x, the focus accent does
    /// not.
    #[must_use]
    pub fn new(
        logical_width: u32,
        logical_height: u32,
        device_width: u32,
        device_height: u32,
        draw_offset: f64,
        focused: bool,
        theme: ThemeColours,
        accent: Option<Accent>,
    ) -> Self {
        let background = CanvasColor::from_theme(Rgb {
            r: theme.background[0],
            g: theme.background[1],
            b: theme.background[2],
        });
        let foreground = CanvasColor::from_theme(Rgb {
            r: theme.foreground[0],
            g: theme.foreground[1],
            b: theme.foreground[2],
        });
        let accent = accent.map(|accent| FrameAccent {
            color: CanvasColor::from_theme(Rgb {
                r: accent.rgb()[0],
                g: accent.rgb()[1],
                b: accent.rgb()[2],
            }),
            width: accent.width(),
        });
        FrameInput {
            logical_width,
            logical_height,
            device_width,
            device_height,
            draw_offset,
            focused,
            background,
            foreground,
            accent,
        }
    }

    /// The frame's logical size, in pixels.
    #[must_use]
    pub fn logical_size(&self) -> (u32, u32) {
        (self.logical_width, self.logical_height)
    }

    /// The frame's device size — the canvas size, in device pixels.
    #[must_use]
    pub fn device_size(&self) -> (u32, u32) {
        (self.device_width, self.device_height)
    }

    /// The tween's snapped docked-edge offset, in logical pixels along x.
    #[must_use]
    pub fn draw_offset(&self) -> f64 {
        self.draw_offset
    }

    /// Whether the panel currently holds keyboard focus.
    #[must_use]
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// The cached theme background.
    #[must_use]
    pub fn background(&self) -> CanvasColor {
        self.background
    }

    /// The cached theme foreground.
    #[must_use]
    pub fn foreground(&self) -> CanvasColor {
        self.foreground
    }

    /// The cached focus accent, when one is configured.
    #[must_use]
    pub fn accent(&self) -> Option<FrameAccent> {
        self.accent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::canvas::Canvas;

    fn metrics(cell_w: f64, cell_h: f64, scale: f64) -> PainterMetrics {
        PainterMetrics::new(cell_w, cell_h, 12.0, scale).expect("test metrics are valid")
    }

    /// The cast seam rounds floating-point dust off a snapped device edge,
    /// maps a value that is not a number to 0 and clamps one outside the
    /// `i32` range to the nearest end.
    #[test]
    fn device_px_rounds_dust_and_maps_nan_to_zero() {
        assert_eq!(device_px(4.0), 4);
        assert_eq!(device_px(3.999_999_9), 4, "floating-point dust rounds off");
        assert_eq!(device_px(-3.0), -3);
        assert_eq!(device_px(f64::NAN), 0);
        assert_eq!(device_px(f64::INFINITY), i32::MAX);
        assert_eq!(device_px(f64::NEG_INFINITY), i32::MIN);
        assert_eq!(device_px(1e30), i32::MAX);
        assert_eq!(device_px(-1e30), i32::MIN);
    }

    /// A device rectangle refuses a negative extent and keeps an empty one.
    #[test]
    fn a_device_rect_refuses_negative_extents() {
        assert!(DeviceRect::new(0, 0, 4, 4).is_some());
        assert!(DeviceRect::new(0, 0, 0, 4).is_some(), "empty is valid");
        assert!(DeviceRect::new(0, 0, -1, 4).is_none());
        assert!(DeviceRect::new(0, 0, 4, -1).is_none());
        let rect = DeviceRect::new(1, 2, 3, 4).expect("valid");
        assert_eq!((rect.x(), rect.y(), rect.w(), rect.h()), (1, 2, 3, 4));
        assert!(!rect.is_empty());
        assert!(DeviceRect::new(1, 2, 3, 0).expect("valid").is_empty());
    }

    /// Cell metrics refuse a pitch no terminal could have, keep a
    /// degenerate ascent at 0, and let a degenerate scale fall back to 1
    /// like the snap rules do.
    #[test]
    fn painter_metrics_refuse_degenerate_pitches() {
        assert!(PainterMetrics::new(0.0, 20.0, 12.0, 1.0).is_none());
        assert!(PainterMetrics::new(9.0, -1.0, 12.0, 1.0).is_none());
        assert!(PainterMetrics::new(f64::NAN, 20.0, 12.0, 1.0).is_none());
        assert!(PainterMetrics::new(f64::INFINITY, 20.0, 12.0, 1.0).is_none());
        let m = metrics(9.0, 20.0, 0.0);
        assert_eq!(m.cell_w(), 9.0);
        assert_eq!(m.cell_h(), 20.0);
        assert_eq!(m.ascent(), 12.0);
        assert_eq!(m.scale(), 1.0, "the degenerate scale fell back to 1");
        assert_eq!(
            m.cell_rect(0, 0).w(),
            9,
            "the degenerate scale fell back to 1"
        );
        let unsteady = PainterMetrics::new(9.0, 20.0, f64::NAN, 1.0).expect("valid");
        assert_eq!(
            unsteady.ascent(),
            0.0,
            "a non-finite ascent falls back to 0"
        );
    }

    /// Two cells that share an edge snap the same value, so their device
    /// rectangles meet exactly with no seam, at every scale the painter
    /// tests.
    #[test]
    fn adjacent_cells_share_their_snapped_device_edge() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(9.0, 20.0, scale);
            for col in 0..6 {
                let cell = m.cell_rect(col, 0);
                let next = m.cell_rect(col + 1, 0);
                assert_eq!(
                    cell.x() + cell.w(),
                    next.x(),
                    "cells {col} and {} share the edge at scale {scale}",
                    col + 1
                );
                assert_eq!(cell.y(), next.y(), "the rows align at scale {scale}");
                assert_eq!(cell.h(), next.h(), "the heights match at scale {scale}");
            }
        }
    }

    /// A half cell and its other half share the snapped middle edge and
    /// together cover the cell exactly.
    #[test]
    fn half_cells_meet_without_a_seam_and_cover_the_cell() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(9.0, 20.0, scale);
            let cell = m.cell_rect(0, 0);
            let left = m.cell_sub_rect(0, 0, 0.0, 0.0, 4.5, 20.0);
            let right = m.cell_sub_rect(0, 0, 4.5, 0.0, 4.5, 20.0);
            assert_eq!(
                left.x() + left.w(),
                right.x(),
                "the halves share the middle edge at scale {scale}"
            );
            assert_eq!(
                right.x() + right.w(),
                cell.x() + cell.w(),
                "the halves end at the cell's right edge at scale {scale}"
            );
            assert_eq!((left.y(), left.h()), (cell.y(), cell.h()));
            assert_eq!((right.y(), right.h()), (cell.y(), cell.h()));
        }
    }

    /// The bar cursor's 2 logical pixels are 3 device pixels at 1.5, the
    /// case design decision 10 names.
    #[test]
    fn a_two_logical_pixel_bar_is_three_device_pixels_at_1_5() {
        let m = metrics(9.0, 20.0, 1.5);
        let bar = m.cell_sub_rect(2, 1, 0.0, 0.0, 2.0, 20.0);
        assert_eq!(bar.w(), 3);
        assert_eq!(bar.h(), 30, "the bar keeps the cell's full height");
        // The bar sits where the cell sits.
        let cell = m.cell_rect(2, 1);
        assert_eq!((bar.x(), bar.y()), (cell.x(), cell.y()));
    }

    /// A run of cells is one snapped rectangle that starts where its first
    /// cell starts and ends where its last cell ends, at every scale.
    #[test]
    fn a_run_of_cells_matches_the_cells_it_covers() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(9.0, 20.0, scale);
            for len in 1..5 {
                let run = m.run_rect(1, 0, len);
                let first = m.cell_rect(1, 0);
                let last = m.cell_rect(len, 0);
                assert_eq!(
                    run.x(),
                    first.x(),
                    "the run starts at cell 1 at scale {scale}"
                );
                assert_eq!(
                    run.x() + run.w(),
                    last.x() + last.w(),
                    "the run ends at cell {len} at scale {scale}"
                );
                assert_eq!((run.y(), run.h()), (first.y(), first.h()));
            }
        }
    }

    /// A run with no cells is empty, and a part with no size or a negative
    /// size is empty at its snapped origin.
    #[test]
    fn degenerate_runs_and_parts_are_empty() {
        let m = metrics(9.0, 20.0, 1.5);
        assert!(m.run_rect(0, 0, 0).is_empty());
        assert!(m.run_rect(0, 0, -2).is_empty());
        assert!(m.cell_sub_rect(0, 0, 1.0, 1.0, 0.0, 5.0).is_empty());
        assert!(m.cell_sub_rect(0, 0, 1.0, 1.0, -3.0, 5.0).is_empty());
    }

    /// An arbitrary logical rectangle — the kitty placements' form — snaps
    /// its right and bottom edges on their own, so the whole rectangle
    /// keeps its last device pixel at a fractional scale, and a positive
    /// size never collapses below one device pixel.
    #[test]
    fn a_logical_rect_keeps_its_last_device_pixel_at_fractional_scales() {
        for scale in [1.0, 1.25, 1.5, 1.8] {
            let m = metrics(9.0, 20.0, scale);
            // An 8-logical-pixel-wide rectangle at x = 32: at 1.5 its
            // device width is 12, at 1.8 it is round(39.6) - 57.6 = 14 —
            // both cover every device pixel the true fractional span
            // touches.
            let rect = m.logical_rect(32.0, 32.0, 8.0, 16.0);
            assert_eq!(rect.x(), device_px(32.0 * scale));
            assert_eq!(rect.w(), device_px(40.0 * scale) - device_px(32.0 * scale));
            assert!(rect.w() > 0 && rect.h() > 0, "positive at {scale}");
            // The exact fractional span's last pixel: the device width is
            // at least ceil(round-tripped) of the true span.
            let true_span = 8.0 * scale;
            assert!(f64::from(rect.w()) + 0.5 >= true_span, "width at {scale}");
        }
        // Zero and negative sizes give an empty rectangle at the snapped
        // origin.
        let m = metrics(9.0, 20.0, 1.5);
        let empty = m.logical_rect(4.0, 4.0, 0.0, 5.0);
        assert!(empty.is_empty());
        let negative = m.logical_rect(4.0, 4.0, -2.0, 5.0);
        assert!(negative.is_empty());
    }

    /// A frame input caches the theme colours swapped (D5): drawn into a
    /// canvas, each colour reads the exact `ARGB8888` bytes of the theme
    /// colour it came from, and the accent rides along with its width.
    #[test]
    fn a_frame_input_caches_the_theme_colours_swapped() {
        let accent = Accent::new([255, 0, 0], NonZeroU16::new(2).expect("nonzero"));
        let frame = FrameInput::new(
            64,
            32,
            96,
            48,
            3.0,
            true,
            ThemeColours {
                background: [10, 20, 30],
                foreground: [200, 150, 100],
            },
            Some(accent),
        );
        assert_eq!(frame.logical_size(), (64, 32));
        assert_eq!(frame.device_size(), (96, 48));
        assert_eq!(frame.draw_offset(), 3.0);
        assert!(frame.focused());

        let mut canvas = Canvas::new(1, 1).expect("canvas");
        canvas.fill_rect(0, 0, 1, 1, frame.background());
        assert_eq!(canvas.pixel(0, 0), Some([30, 20, 10, 255]));
        canvas.fill_rect(0, 0, 1, 1, frame.foreground());
        assert_eq!(canvas.pixel(0, 0), Some([100, 150, 200, 255]));
        let accent = frame.accent().expect("the accent is carried");
        canvas.fill_rect(0, 0, 1, 1, accent.color());
        assert_eq!(canvas.pixel(0, 0), Some([0, 0, 255, 255]));
        assert_eq!(accent.width().get(), 2);

        // No accent configured: none carried.
        let plain = FrameInput::new(64, 32, 96, 48, 0.0, false, ThemeColours::default(), None);
        assert!(!plain.focused());
        assert_eq!(plain.draw_offset(), 0.0);
        assert!(plain.accent().is_none());
    }
}
