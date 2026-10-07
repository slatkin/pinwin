use super::super::{Inner, Startup};
use super::*;
use crate::guard::Poisoned;
use crate::layout::Keyboard;
use crate::panel::handshake::Handshake;
use crate::surfaces::gap::start_held_gap;
use std::num::NonZeroU16;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

/// A startup for the tests; the thread does not touch the pty fd until
/// the surfaces push a grid, so a placeholder fd is fine here.
fn startup() -> Startup {
    Startup::new(
        -1,
        layout(Side::Left, 40, 0, 0, 0, 0),
        Keyboard::OnDemand,
        None,
    )
}

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

/// The startup cell metrics the tests carry, the way a start command
/// would (D3).
fn cell() -> CellSize {
    CellSize::new(9, 16).expect("test cell size is non-zero")
}

fn output(width: i32, height: i32) -> OutputSize {
    OutputSize::new(width, height).expect("test output size is non-zero")
}

/// A live handle state like a started panel's, for the thread-side
/// tests.
fn live_inner() -> Arc<Inner> {
    Arc::new(Inner {
        poisoned: Poisoned::new(),
        live: AtomicBool::new(true),
    })
}

fn headless_state() -> PanelState {
    let (tx, _rx) = mpsc::channel();
    PanelState::headless(
        Handshake::new(tx),
        Poisoned::new(),
        live_inner(),
        startup(),
        cell(),
    )
}

/// An invalid layout is rejected with the applied state untouched: the
/// verdict is `InvalidLayout`, no grid is pushed and the held gap stays
/// at the startup one — a rejected covering apply stages nothing
/// (overlay-expand D4).
#[test]
fn an_invalid_layout_is_rejected_and_stages_nothing() {
    let mut state = headless_state();
    // A covering 120-column layout is 1080px wide at a 9px cell, past
    // the 600px output: rejected.
    let too_wide = layout(Side::Left, 120, 0, 0, 0, 0).covering();
    let mut pushed: Vec<Grid> = Vec::new();
    let outcome = state.apply_against(output(600, 1080), too_wide, &mut |grid| pushed.push(grid));
    assert_eq!(outcome, Err(PublishOutcome::InvalidLayout));
    assert!(pushed.is_empty(), "a rejected apply pushes no grid");
    assert_eq!(
        state.held,
        start_held_gap(startup().layout(), cell().width().get()),
        "the held gap is untouched"
    );
}

/// A valid pushing apply moves the reserve zone and the panel size to
/// the target layout's own strip and width, moves the held gap with
/// them, and pushes the widened grid through the sizing path.
#[test]
fn a_valid_pushing_apply_moves_the_zone_and_the_size() {
    let mut state = headless_state();
    // A configure delivered the startup height, so the rows are live.
    state.sizing.configure(1080, &mut |_| {});

    let target = layout(Side::Left, 120, 0, 0, 8, 12);
    let mut pushed: Vec<Grid> = Vec::new();
    let geometry = state
        .apply_against(output(1920, 1080), target, &mut |grid| pushed.push(grid))
        .expect("the layout fits the output");

    assert_eq!(geometry.panel_side, Side::Left);
    assert_eq!(geometry.panel_margins, (0, 0, 0, 8), "the edge gutter");
    assert_eq!(geometry.panel_width, Some(1080), "the final width");
    assert_eq!(geometry.reserve_side, Side::Left);
    assert_eq!(
        geometry.reserve_zone,
        8 + 1080 + 12,
        "the reserve holds the target's own strip"
    );
    assert_eq!(pushed.len(), 1, "the widened grid is pushed");
    assert_eq!(pushed[0].cols(), 120, "the columns come from the layout");
    assert_eq!(pushed[0].rows(), 1080 / 16, "the rows stay from the height");
    assert_eq!(
        state.held.zone(),
        1100,
        "a pushing apply resets the held strip to its own"
    );
}

/// A covering apply holds the strip: the reserve keeps the last pushing
/// layout's side and zone (overlay-expand D2) while the panel takes the
/// covering layout's width, and the held gap is untouched.
#[test]
fn a_covering_apply_holds_the_strip() {
    let mut state = headless_state();
    // A pushing start holds its own strip: 40 columns at 9px.
    assert_eq!(state.held.zone(), 360);
    // A configure delivered the startup height, so the rows are live.
    state.sizing.configure(1080, &mut |_| {});

    let covering = layout(Side::Left, 120, 0, 0, 0, 0).covering();
    let mut pushed: Vec<Grid> = Vec::new();
    let geometry = state
        .apply_against(output(1920, 1080), covering, &mut |grid| pushed.push(grid))
        .expect("the covering layout fits the output");

    assert_eq!(geometry.panel_side, Side::Left);
    assert_eq!(geometry.panel_width, Some(1080), "the covering width");
    assert_eq!(geometry.reserve_side, Side::Left, "the held side");
    assert_eq!(
        geometry.reserve_zone, 360,
        "the strip stays where the last pushing layout put it"
    );
    assert_eq!(state.held.zone(), 360, "the held gap is untouched");
    assert_eq!(pushed.len(), 1, "the grid still follows the columns");
}

/// A side switch re-anchors both surfaces: the panel to the new side,
/// the reserve with it at the held width — the shuffle belongs to
/// whoever moved the panel (overlay-expand D2).
#[test]
fn a_side_switch_re_anchors_both_surfaces() {
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});

    let switched = layout(Side::Right, 120, 0, 0, 0, 0).covering();
    let geometry = state
        .apply_against(output(1920, 1080), switched, &mut |_| {})
        .expect("the layout fits the output");

    assert_eq!(geometry.panel_side, Side::Right, "the panel re-anchors");
    assert_eq!(geometry.panel_margins, (0, 0, 0, 0));
    assert_eq!(
        geometry.reserve_side,
        Side::Right,
        "the gap follows the panel to the new side"
    );
    assert_eq!(geometry.reserve_zone, 360, "at the last pushing width");

    // A pushing switch also moves the strip to the new side's own.
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});
    let geometry = state
        .apply_against(
            output(1920, 1080),
            layout(Side::Right, 40, 0, 0, 0, 0),
            &mut |_| {},
        )
        .expect("the layout fits the output");
    assert_eq!(geometry.reserve_side, Side::Right);
    assert_eq!(geometry.reserve_zone, 360, "the new side's own strip");
}

/// A repeat apply of the same layout derives the same grid and pushes
/// nothing more: the pty size has one source (D3) and a repeated apply
/// is not one of them.
#[test]
fn a_repeat_apply_of_the_same_layout_pushes_no_extra_grid() {
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});
    let target = layout(Side::Left, 120, 0, 0, 0, 0);

    let mut pushed: Vec<Grid> = Vec::new();
    let first = state
        .apply_against(output(1920, 1080), target, &mut |grid| pushed.push(grid))
        .expect("the layout fits the output");
    let second = state
        .apply_against(output(1920, 1080), target, &mut |grid| pushed.push(grid))
        .expect("the layout fits the output");

    assert_eq!(pushed.len(), 1, "the repeat apply pushes nothing");
    assert_eq!(first.panel_width, second.panel_width);
    assert_eq!(first.reserve_zone, second.reserve_zone);
}

/// An apply before the first configure records the columns without a
/// push: there is no height to derive rows from, and the next configure
/// derives with the applied columns (the sizing path's rule).
#[test]
fn an_apply_before_the_first_configure_records_the_columns() {
    let mut state = headless_state();
    let mut pushed: Vec<Grid> = Vec::new();
    let geometry = state
        .apply_against(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 0),
            &mut |grid| pushed.push(grid),
        )
        .expect("the layout fits the output");
    assert!(pushed.is_empty(), "no height, no push");
    assert_eq!(geometry.panel_width, Some(1080));

    state.sizing.configure(1080, &mut |grid| pushed.push(grid));
    assert_eq!(pushed.len(), 1, "the next configure derives");
    assert_eq!(pushed[0].cols(), 120, "with the applied columns");
}

/// The production apply without a bound session is `NotLive` (D10): the
/// headless core has no resolved output to validate against, the same
/// lifecycle verdict the GTK side's missing monitor gives.
#[test]
fn an_apply_without_a_session_is_not_live() {
    let mut state = headless_state();
    assert_eq!(
        state.apply(startup().layout(), 0),
        PublishOutcome::NotLive,
        "no session, no publish"
    );
    assert!(!state.done, "an apply does not end the thread");
}

/// The animated apply's staging defers the grid push (row 6.2): the
/// staging records the target columns without deriving or pushing, and
/// the finish pushes once through the sizing path — the new columns,
/// the rows of the latest configure height.
#[test]
fn an_animated_apply_defers_the_push_and_the_finish_pushes_once() {
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});
    let mut pushed: Vec<Grid> = Vec::new();

    let staged = state
        .stage_animated(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            200,
            &mut |grid| pushed.push(grid),
        )
        .expect("the layout fits the output");
    assert!(pushed.is_empty(), "a staged animated apply pushes nothing");
    assert_eq!(staged.from_px, 360, "the applied width at rest");
    assert_eq!(staged.target_px, 1080, "the staged target width");
    assert_eq!(staged.crop.wide_px(), 1080, "the wide width is the larger");
    assert_eq!(staged.grid_px, 360, "the live grid is the applied one");
    assert!(staged.gap_tweening, "a pushing expand tweens the gap");
    assert_eq!(staged.gap_zone, 360, "the strip the gap rests at");
    assert_eq!(staged.duration_ms, 200);

    // The finish pushes once, through the sizing path.
    state.tween_finished(1080, &mut |grid| pushed.push(grid));
    assert_eq!(pushed.len(), 1, "exactly one push");
    assert_eq!(pushed[0].cols(), 120, "the tweened columns");
    assert_eq!(pushed[0].rows(), 1080 / 16, "the recorded height's rows");
}

/// A snap apply during a running tween pushes at once (row 6.2): the
/// stop relay drops the wide cache, cancels the driver and lifts the
/// sizing defer, so the deferred grid lands in the same apply.
#[test]
fn a_snap_apply_during_a_tween_pushes_at_once() {
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});
    let mut pushed: Vec<Grid> = Vec::new();
    let _ = state.stage_animated(
        output(1920, 1080),
        layout(Side::Left, 120, 0, 0, 0, 12),
        200,
        &mut |grid| pushed.push(grid),
    );
    state.tween.begin(360, 1080, 200, Instant::now(), None);
    assert!(
        state.tween.is_active(),
        "the test tweens like the apply would"
    );

    // A zero-duration apply of the same layout: the relay lifts the
    // defer and the deferred grid pushes at once.
    let geometry = state
        .apply_snap_against(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            &mut |grid| pushed.push(grid),
        )
        .expect("the layout fits the output");
    assert_eq!(pushed.len(), 1, "the snap pushes at once");
    assert_eq!(pushed[0].cols(), 120);
    assert_eq!(geometry.panel_width, Some(1080));
    assert!(!state.tween.is_active(), "the snap cancelled the tween");
    assert!(state.tween_draw.is_none(), "the wide cache dropped");
}

/// A snap apply whose staging the output rejects leaves a running tween
/// alone — the relay runs after the staged mutation, the GTK publish's
/// reject-before-mutate order (overlay-expand D4) — and pushes nothing.
#[test]
fn a_rejected_snap_apply_leaves_the_tween_running() {
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});
    let mut pushed: Vec<Grid> = Vec::new();
    let _ = state.stage_animated(
        output(1920, 1080),
        layout(Side::Left, 120, 0, 0, 0, 12),
        200,
        &mut |grid| pushed.push(grid),
    );
    state.tween.begin(360, 1080, 200, Instant::now(), None);

    // A covering 400-column layout is 3600px wide, past the output.
    let too_wide = layout(Side::Left, 400, 0, 0, 0, 12).covering();
    assert_eq!(
        state.apply_snap_against(output(1920, 1080), too_wide, &mut |grid| pushed.push(grid)),
        Err(PublishOutcome::InvalidLayout)
    );
    assert!(state.tween.is_active(), "the tween keeps running");
    assert!(pushed.is_empty(), "a rejected apply pushes nothing");
}

/// A rejected animated staging restores the defer a running tween
/// holds: the tween keeps deferring its pushes, the way the GTK
/// publish's rejected apply leaves the tween's resize deferred.
#[test]
fn a_rejected_animated_staging_keeps_the_running_tweens_defer() {
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});
    let mut pushed: Vec<Grid> = Vec::new();
    let _ = state.stage_animated(
        output(1920, 1080),
        layout(Side::Left, 120, 0, 0, 0, 12),
        200,
        &mut |grid| pushed.push(grid),
    );
    state.tween.begin(360, 1080, 200, Instant::now(), None);

    // A covering 400-column layout is rejected; the staging restores
    // the defer the running tween holds.
    let too_wide = layout(Side::Left, 400, 0, 0, 0, 12).covering();
    assert!(
        state
            .stage_animated(output(1920, 1080), too_wide, 200, &mut |grid| pushed
                .push(grid))
            .is_none(),
        "the rejected staging stages nothing"
    );
    assert!(state.tween.is_active());
    assert!(pushed.is_empty(), "the defer still holds the push");

    // And a mid-tween configure pushes nothing either, while a height
    // lands for the finish to derive from.
    state.sizing.configure(1040, &mut |grid| pushed.push(grid));
    assert!(
        pushed.is_empty(),
        "the defer still holds the configure's push"
    );
    state.tween_finished(1080, &mut |grid| pushed.push(grid));
    assert_eq!(pushed.len(), 1, "the finish pushes once, at the new height");
    assert_eq!(pushed[0].rows(), 1040 / 16);
}

/// A snap apply with no tween running pushes exactly once: the
/// staging's own push lands and the relay's is the sizing's no-op.
#[test]
fn a_snap_apply_without_a_tween_pushes_once() {
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});
    let mut pushed: Vec<Grid> = Vec::new();
    let geometry = state
        .apply_snap_against(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            &mut |grid| pushed.push(grid),
        )
        .expect("the layout fits the output");
    assert_eq!(pushed.len(), 1);
    assert_eq!(geometry.panel_width, Some(1080));
}

/// A repeat animated apply of the same layout during a running tween
/// leaves the tween running and the deferred push pending — the GTK
/// publish's "a tween already heading for these columns keeps going": the
/// repeat animates nothing, so it takes the quiet path and the stop relay
/// does not run. The deferred push lands at the tween's finish.
#[test]
fn a_repeat_animated_apply_leaves_the_running_tween_alone() {
    let mut state = headless_state();
    state.sizing.configure(1080, &mut |_| {});
    let mut pushed: Vec<Grid> = Vec::new();
    let _ = state.stage_animated(
        output(1920, 1080),
        layout(Side::Left, 120, 0, 0, 0, 12),
        200,
        &mut |grid| pushed.push(grid),
    );
    state.tween.begin(360, 1080, 200, Instant::now(), None);

    // The repeat animates nothing: the animate rule accepts the layout and
    // the width's drivers are unchanged, so the apply routes it to the
    // quiet path — staged, no relay.
    let same = layout(Side::Left, 120, 0, 0, 0, 12);
    assert!(tween::should_animate(200, &state.applied, &same));
    assert!(!PanelState::animates_width(&state.applied, &same));
    assert_eq!(
        state.apply_quiet(output(1920, 1080), same),
        PublishOutcome::Applied
    );
    assert!(state.tween.is_active(), "the tween keeps easing");
    assert!(pushed.is_empty(), "the deferred push waits for the finish");

    state.tween_finished(1080, &mut |grid| pushed.push(grid));
    assert_eq!(pushed.len(), 1, "the finish pushes once");
}
