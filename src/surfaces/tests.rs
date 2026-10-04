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
fn only_a_column_change_animates() {
    let applied = layout(Side::Left, 40, 0, 0, 0, 12);
    // Columns change, everything else matches, animations on: animate.
    assert!(should_animate(
        200,
        true,
        &applied,
        &layout(Side::Left, 120, 0, 0, 0, 12)
    ));
    // Zero duration snaps.
    assert!(!should_animate(
        0,
        true,
        &applied,
        &layout(Side::Left, 120, 0, 0, 0, 12)
    ));
    // Animations disabled snap.
    assert!(!should_animate(
        200,
        false,
        &applied,
        &layout(Side::Left, 120, 0, 0, 0, 12)
    ));
    // The side moves the reservation: snap.
    assert!(!should_animate(
        200,
        true,
        &applied,
        &layout(Side::Right, 120, 0, 0, 0, 12)
    ));
    // The left or right gutter moves the reservation: snap.
    assert!(!should_animate(
        200,
        true,
        &applied,
        &layout(Side::Left, 120, 0, 0, 4, 12)
    ));
    assert!(!should_animate(
        200,
        true,
        &applied,
        &layout(Side::Left, 120, 0, 0, 0, 20)
    ));
    // The top and bottom gutters do not: animate.
    assert!(should_animate(
        200,
        true,
        &applied,
        &layout(Side::Left, 120, 8, 8, 0, 12)
    ));
    // The same columns keep an already-heading tween going.
    assert!(should_animate(200, true, &applied, &applied));
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

/// The deferred grid resize waits out a still-running tween, then applies
/// exactly once and returns `Break` — an idle that kept looping after the
/// tween ended (a latched tween flag, or a `Continue` past the end) would
/// spin hot and keep the pty drain throttled.
#[test]
fn the_deferred_grid_idle_waits_out_the_tween_then_applies_once() {
    let applies = Cell::new(0);
    let apply = || {
        applies.set(applies.get() + 1);
        true
    };
    // A tween still running: keep waiting, no grid resize.
    assert_eq!(
        deferred_grid_step(false, true, apply),
        glib::ControlFlow::Continue
    );
    assert_eq!(applies.get(), 0);
    // The tween ended: one apply, then the idle is gone.
    assert_eq!(
        deferred_grid_step(false, false, apply),
        glib::ControlFlow::Break
    );
    assert_eq!(applies.get(), 1);
    // A closed panel never applies.
    assert_eq!(
        deferred_grid_step(true, false, apply),
        glib::ControlFlow::Break
    );
    assert_eq!(applies.get(), 1);
}
