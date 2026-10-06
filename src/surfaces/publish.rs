//! The publish verdict and the animate decision: the pure half of
//! [`super::Surfaces::publish`]. Split out so `src/panel/handshake.rs` and
//! the panel thread (`src/panel/wayland_side/`) can use them without
//! depending on the GTK-bound surfaces module (row 8.1).

use crate::layout::Layout;

/// The outcome of publishing a layout (`glue_publish_layout`'s return codes).
/// The `panel` row (4.1) maps these onto `PinwinError`: `NotLive` is
/// `NotRunning`, `InvalidLayout` is `InvalidLayout` and `Terminal` is
/// `Internal` (`pinwin_api.c`'s `apply_on_gtk_thread`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    /// The layout is applied and the grid follows (`PINWIN_GEOM_OK`).
    Applied,
    /// The panel has no live metrics yet, or is already torn down
    /// (`GLUE_NOT_LIVE`).
    NotLive,
    /// The layout the live monitor refuses (`PINWIN_GEOM_ERR_METRICS`).
    InvalidLayout,
    /// The layout published but the terminal grid could not be allocated; the
    /// previous grid stays (`GLUE_ERR_TERMINAL`).
    Terminal,
}

/// Whether a layout apply animates (`glue_publish_layout`'s decision): a
/// layout differing only in its column count and/or push/cover choice animates
/// — the side and the left and right gutters move the reservation's side, so a
/// layout touching them snaps. A covering-only change animates so a pushing
/// retarget at the same width can ease its gap back (overlay-expand D3). The
/// duration clamp to 1000 ms happens upstream (`pinwin_api.c`), as in the C.
/// The library reads no desktop animation setting: the host's duration is the
/// only control.
pub(crate) fn should_animate(duration_ms: u32, applied: &Layout, requested: &Layout) -> bool {
    duration_ms > 0
        && requested.side() == applied.side()
        && requested.left() == applied.left()
        && requested.right() == applied.right()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use super::*;
    use crate::layout::Side;

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

    /// A layout differing only in its column count and/or push/cover choice
    /// animates (overlay-expand D3); anything touching the side or the left
    /// and right gutters snaps, as does a zero duration. The library reads no
    /// desktop animation setting: the host's duration is the only control.
    #[test]
    fn only_a_column_or_coverage_change_animates() {
        let applied = layout(Side::Left, 40, 0, 0, 0, 12);
        // Columns change, everything else matches: animate.
        assert!(should_animate(
            200,
            &applied,
            &layout(Side::Left, 120, 0, 0, 0, 12)
        ));
        // Zero duration snaps.
        assert!(!should_animate(
            0,
            &applied,
            &layout(Side::Left, 120, 0, 0, 0, 12)
        ));
        // The side moves the reservation: snap.
        assert!(!should_animate(
            200,
            &applied,
            &layout(Side::Right, 120, 0, 0, 0, 12)
        ));
        // The left or right gutter moves the reservation: snap.
        assert!(!should_animate(
            200,
            &applied,
            &layout(Side::Left, 120, 0, 0, 4, 12)
        ));
        assert!(!should_animate(
            200,
            &applied,
            &layout(Side::Left, 120, 0, 0, 0, 20)
        ));
        // The top and bottom gutters do not: animate.
        assert!(should_animate(
            200,
            &applied,
            &layout(Side::Left, 120, 8, 8, 0, 12)
        ));
        // A coverage-only change at the same width animates, so a pushing
        // retarget can ease its gap back (overlay-expand D3).
        assert!(should_animate(
            200,
            &applied,
            &layout(Side::Left, 40, 0, 0, 0, 12).covering()
        ));
        // Coverage and columns together: animate.
        assert!(should_animate(
            200,
            &applied,
            &layout(Side::Left, 120, 0, 0, 0, 12).covering()
        ));
        // A side switch is never a covering move, and it still snaps.
        assert!(!should_animate(
            200,
            &applied,
            &layout(Side::Right, 40, 0, 0, 0, 12).covering()
        ));
        // The same columns keep an already-heading tween going.
        assert!(should_animate(200, &applied, &applied));
    }
}
