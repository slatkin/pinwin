//! Device-pixel snapping for the grid geometry (`snap-grid-edges` D2, D4).
//! At a fractional output scale a cell edge often falls between two device
//! pixels; blending the pixels at that edge leaves a visible line
//! between cells. Every rectangle the grid pass draws therefore snaps each
//! of its edges to a whole device pixel with `round(edge * scale) / scale`.
//!
//! [`OutputScale`] is GTK-free, so its tests run without a display
//! (`snap-grid-edges` D2).

/// The output scale a frame is drawn at (`snap-grid-edges` D2): positive and
/// finite, defaulting to 1. The value is carried in
/// [`crate::render::cell_metrics::CellMetrics`] and read from the output's
/// preferred scale at each bind (`snap-grid-edges` D3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OutputScale(f64);

impl Default for OutputScale {
    fn default() -> OutputScale {
        OutputScale(1.0)
    }
}

impl OutputScale {
    /// The scale from a raw `Surface::scale()` value (`snap-grid-edges` D3):
    /// a value that is not positive and finite falls back to 1, the scale
    /// every internal test surface renders at.
    pub(crate) fn new(value: f64) -> OutputScale {
        if value.is_finite() && value > 0.0 {
            OutputScale(value)
        } else {
            OutputScale(1.0)
        }
    }

    /// The scale factor itself (device pixels per logical pixel).
    pub(crate) fn get(self) -> f64 {
        self.0
    }

    /// One device pixel in logical pixels at this scale.
    fn device_pixel(self) -> f64 {
        1.0 / self.0
    }

    /// Snap one rectangle edge to a whole device pixel (`snap-grid-edges`
    /// D4): `round(edge * scale) / scale`. `f64::round` rounds ties away
    /// from zero, which for the non-negative edges here means up.
    ///
    /// Two cells that share an edge pass the same number to this — the
    /// shared edge is an exact integer or an exact half of the cell pitch —
    /// so both get the same snapped value and no gap or overlap appears.
    pub(crate) fn snap_edge(self, edge: f64) -> f64 {
        (edge * self.0).round() / self.0
    }

    /// Snap a rectangle's four edges, each on its own (`snap-grid-edges`
    /// D4): the right edge is `snap(x + w)`, never `snap(x) + snap(w)`. A
    /// rectangle with a positive size keeps at least one device pixel in
    /// each direction — when both snapped edges land on the same pixel the
    /// far edge is pushed out by one device pixel — and a rectangle with no
    /// size stays empty.
    pub(crate) fn snap_rect(self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
        let x0 = self.snap_edge(x);
        let y0 = self.snap_edge(y);
        let mut x1 = self.snap_edge(x + w);
        let mut y1 = self.snap_edge(y + h);
        let px = self.device_pixel();
        if w > 0.0 && x1 < x0 + px {
            x1 = x0 + px;
        }
        if h > 0.0 && y1 < y0 + px {
            y1 = y0 + px;
        }
        (x0, y0, x1 - x0, y1 - y0)
    }
}

#[cfg(test)]
mod tests {
    use super::OutputScale;

    #[test]
    fn at_scale_1_a_whole_edge_stays_and_a_half_edge_rounds_up() {
        let scale = OutputScale::default();
        assert_eq!(scale.snap_edge(9.0), 9.0, "a whole edge is unchanged");
        assert_eq!(scale.snap_edge(4.5), 5.0, "a half edge rounds up");
        assert_eq!(scale.snap_edge(0.0), 0.0);
    }

    /// Two cells that share an edge pass the same number to the snap, so
    /// both compute the same snapped edge value: cell k's right edge equals
    /// cell k+1's left edge, for full cells and for half cells meeting at
    /// the pitch's half.
    #[test]
    fn adjacent_cells_share_the_same_snapped_edge_at_fractional_scales() {
        for scale in [1.5, 1.25] {
            let scale = OutputScale::new(scale);
            // A 9px cell pitch: the shared edges are exact integers (9, 18,
            // ...) and, at the pitch's half, exact halves (4.5, 13.5, ...).
            let (x0, _, w0, _) = scale.snap_rect(0.0, 0.0, 9.0, 9.0);
            let (x1, _, _, _) = scale.snap_rect(9.0, 0.0, 9.0, 9.0);
            assert_eq!(x0 + w0, x1, "full cells share their snapped edge");
            let (_, _, w_half, _) = scale.snap_rect(4.5, 0.0, 4.5, 9.0);
            let (x_half, _, _, _) = scale.snap_rect(4.5, 0.0, 4.5, 9.0);
            let (x_next, _, _, _) = scale.snap_rect(9.0, 0.0, 9.0, 9.0);
            assert_eq!(
                x_half + w_half,
                x_next,
                "half cells share their snapped edge"
            );
        }
    }

    /// A rectangle with a positive size never collapses below one device
    /// pixel; a rectangle with no size stays empty.
    #[test]
    fn a_positive_rectangle_keeps_at_least_one_device_pixel() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let scale = OutputScale::new(scale);
            let px = scale.device_pixel();
            // Both edges land on the same pixel: the far edge is pushed out.
            let (_, _, w, h) = scale.snap_rect(0.1, 0.1, 0.1, 0.1);
            assert!(w >= px && h >= px, "positive size keeps one device pixel");
            // A large rectangle keeps its snapped extent.
            let (x, y, w, h) = scale.snap_rect(3.2, 7.7, 20.4, 11.1);
            assert!(w > 0.0 && h > 0.0);
            assert_eq!(scale.snap_edge(3.2), x);
            assert_eq!(scale.snap_edge(7.7), y);
            assert_eq!(scale.snap_edge(3.2 + 20.4), x + w);
            assert_eq!(scale.snap_edge(7.7 + 11.1), y + h);
            // A rectangle with no size stays empty: both edges snap to the
            // same value, so the width and height stay 0.
            let (x, y, w, h) = scale.snap_rect(1.0, 1.0, 0.0, 0.0);
            assert_eq!((w, h), (0.0, 0.0));
            assert_eq!((x, y), (scale.snap_edge(1.0), scale.snap_edge(1.0)));
        }
    }

    /// A raw scale value that is not positive and finite falls back to 1
    /// (`snap-grid-edges` D3).
    #[test]
    fn a_degenerate_raw_scale_falls_back_to_1() {
        for raw in [0.0, -1.5, f64::NAN, f64::INFINITY] {
            assert_eq!(OutputScale::new(raw), OutputScale::default());
            assert_eq!(OutputScale::new(raw).get(), 1.0);
        }
        assert_eq!(OutputScale::new(1.5).get(), 1.5);
    }
}
