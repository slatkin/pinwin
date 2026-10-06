//! The surfaces' pure core: the layout-validation gate and the publish
//! verdict, plus the held-gap decisions in [`gap`] — the parts of the former
//! GTK layer-shell surfaces (`src/glue.c`) that the panel thread applies
//! against its own Wayland surfaces (replace-gtk-with-wayland rows 3.1 and
//! 3.5). The GTK window wiring itself went with the GTK path.
//!
//! Layout types come from [`crate::layout`]; a monitor with degenerate metrics
//! maps to the invalid-layout verdict (port-to-rust D6), as `glue.c`
//! `PINWIN_GEOM_ERR_METRICS` did.

use crate::layout::Layout;

pub(crate) mod gap;
mod publish;

pub use publish::PublishOutcome;

/// Validate a layout against live metrics (`pinwin_layout_validate` plus the
/// degenerate-metrics guard): non-positive cell or output metrics are the one
/// former `PINWIN_GEOM_ERR_METRICS` case the layout types cannot absorb (D6),
/// and any verdict maps to the invalid-layout outcome.
pub(crate) fn metrics_valid(
    layout: Layout,
    cell_w: i32,
    cell_h: i32,
    output_w: i32,
    output_h: i32,
) -> bool {
    match (
        crate::layout::CellSize::new(cell_w, cell_h),
        crate::layout::OutputSize::new(output_w, output_h),
    ) {
        (Some(cell), Some(output)) => layout.validate(cell, output).is_ok(),
        // Degenerate monitor metrics: never valid (D6).
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Side;
    use std::num::NonZeroU16;

    fn layout(side: Side, cols: u16, top: i32, bottom: i32, left: i32, right: i32) -> Layout {
        Layout::new(
            side,
            NonZeroU16::new(cols).expect("test column count is non-zero"),
            top,
            bottom,
            left,
            right,
        )
    }

    #[test]
    fn metrics_validation_rejects_degenerate_and_bad_geometry() {
        let good = layout(Side::Left, 60, 0, 0, 0, 12);
        assert!(metrics_valid(good, 8, 16, 1920, 1080));
        // Degenerate monitor metrics are never valid (D6).
        assert!(!metrics_valid(good, 8, 16, 0, 1080));
        assert!(!metrics_valid(good, 8, 16, 1920, 0));
        assert!(!metrics_valid(good, 0, 16, 1920, 1080));
        assert!(!metrics_valid(good, 8, 0, 1920, 1080));
        // Negative metrics: same story.
        assert!(!metrics_valid(good, -8, 16, 1920, 1080));
        // Reachable layout verdicts reject.
        assert!(!metrics_valid(
            layout(Side::Left, 600, 0, 0, 0, 0),
            1,
            16,
            600,
            1080
        ));
        assert!(!metrics_valid(
            layout(Side::Left, 1, 1000, 1000, 0, 0),
            1,
            16,
            1920,
            1080
        ));
        assert!(!metrics_valid(
            layout(Side::Left, 1, 0, 0, -100, -100),
            1,
            16,
            1920,
            1080
        ));
        assert!(!metrics_valid(
            layout(Side::Left, 65535, 0, 0, 0, 0),
            65536,
            16,
            1920,
            1080
        ));
    }
}
