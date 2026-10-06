//! The `surfaces` module's display-free tests, split from `mod.rs` to
//! keep the module at its line budget (the split is mechanical; every item
//! stays reachable through `use super::*`).

use super::*;
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

#[test]
fn keyboard_modes_map_one_to_one() {
    assert_eq!(keyboard_mode(Keyboard::None), LayerKeyboardMode::None);
    assert_eq!(
        keyboard_mode(Keyboard::OnDemand),
        LayerKeyboardMode::OnDemand
    );
    assert_eq!(
        keyboard_mode(Keyboard::Exclusive),
        LayerKeyboardMode::Exclusive
    );
}

/// The tween stop applies the deferred grid resize synchronously: when an
/// animated apply left the resize pending, the stop's gate opens exactly
/// once, and a closed panel never applies — there is no idle and no
/// waiting-for-the-tween step any more, so the resized grid is on screen
/// with the tween's final frame instead of a frame later
/// (gsk-render-nodes design, Post-task decisions: C52).
#[test]
fn the_deferred_grid_applies_at_the_stop_itself() {
    // A pending resize on a live panel: apply.
    assert!(deferred_grid_at_stop(true, false));
    // Nothing pending (a snap apply, or a tween that was retargeted away):
    // the stop does not resize.
    assert!(!deferred_grid_at_stop(false, false));
    // A closed panel never applies.
    assert!(!deferred_grid_at_stop(true, true));
}

/// The draw shift keys against the grid that is actually on screen: while a
/// widening resize's stale content is still there (no host output yet), that
/// is the narrower pre-resize grid — the shift then keeps it glued to the
/// docked edge instead of parking it at the widget's left edge opposite the
/// docked side; once the terminal has produced output for the new width, the
/// live grid takes over (gsk-render-nodes design, Post-task decisions: C52).
#[test]
fn the_draw_shift_keys_against_the_stale_grid_until_terminal_output() {
    // Live grid 120 cols (1080px at a 9px cell), stale pre-resize content
    // (40 cols, 360px) still on screen: the shift keys against the stale
    // grid.
    assert_eq!(drawn_grid_px(1080, 360), 360);
    // No staleness: the live grid.
    assert_eq!(drawn_grid_px(1080, 0), 1080);
    // A zero live grid (no surfaces) never masks a stale width.
    assert_eq!(drawn_grid_px(0, 360), 360);
    // Neither: zero.
    assert_eq!(drawn_grid_px(0, 0), 0);
}
