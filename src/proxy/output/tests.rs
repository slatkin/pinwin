//! The output tracking's display-free tests (D9, D11): the mode-and-scale
//! fallback, the logical size's precedence, and the output a panel's first
//! dock resolves its percent size against.

use std::rc::Rc;

use super::{Globals, OUTPUT_VERSION, OutputTracker, effective_output_size, logical_from_mode};
use crate::geometry::OutputSize;
use crate::proxy::dock::{SetOutputDecision, set_output_decision};
use wl_proxy::baseline::Baseline;
use wl_proxy::protocols::wayland::wl_output::WlOutput;
use wl_proxy::state::State;

/// The fallback divides the current mode by the integer scale (D9):
/// a 2x-scaled 4K output is 1920x1080 logical.
#[test]
fn mode_and_scale_fallback_divides_by_the_scale() {
    assert_eq!(
        logical_from_mode(Some((3840, 2160)), 2),
        OutputSize::new(1920, 1080)
    );
    assert_eq!(
        logical_from_mode(Some((1920, 1080)), 1),
        OutputSize::new(1920, 1080)
    );
}

/// A missing mode, a non-positive scale and a division that collapses to
/// zero give no fallback size — `OutputSize` never carries a zero axis.
#[test]
fn the_fallback_refuses_unknown_or_degenerate_inputs() {
    assert_eq!(logical_from_mode(None, 2), None);
    assert_eq!(logical_from_mode(Some((3840, 2160)), 0), None);
    assert_eq!(logical_from_mode(Some((0, 0)), 1), None);
    assert_eq!(logical_from_mode(Some((1, 1)), 2), None);
}

/// The `zxdg_output_v1` logical size wins over the mode-and-scale
/// fallback whenever it has been announced (D9).
#[test]
fn the_logical_size_wins_over_the_fallback() {
    let logical = OutputSize::new(2560, 1440);
    assert_eq!(
        effective_output_size(logical, Some((3840, 2160)), 2),
        logical
    );
    assert_eq!(
        effective_output_size(None, Some((3840, 2160)), 2),
        OutputSize::new(1920, 1080)
    );
    assert_eq!(effective_output_size(None, None, 2), None);
}

/// A proxy-bound output with its compositor name pre-announced, for the
/// removal tests: the object is minted in a client-side state and never
/// sent anywhere, so the tests run without a compositor (`port-to-rust`
/// D10).
fn tracked_output(state: &Rc<State>, global: u32, name: Option<&str>) -> Rc<OutputTracker> {
    let wl_output = state.create_object::<WlOutput>(OUTPUT_VERSION);
    let tracker = Rc::new(OutputTracker::new(global, Rc::clone(&wl_output)));
    if let Some(name) = name {
        *tracker.output_name.borrow_mut() = Some(name.to_owned());
    }
    tracker
}

/// A tracked output that already announced its logical size, for the first
/// dock's output selection.
fn sized_output(
    state: &Rc<State>,
    global: u32,
    name: Option<&str>,
    width: u32,
    height: u32,
) -> Rc<OutputTracker> {
    let tracker = tracked_output(state, global, name);
    tracker.logical.set(OutputSize::new(width, height));
    tracker
}

/// A client-side state the removal tests mint their objects in: no
/// server is attached, so nothing is sent and no compositor is needed.
fn object_state() -> Rc<State> {
    State::builder(Baseline::V6)
        .without_server()
        .build()
        .expect("a client-side state needs no compositor")
}

/// A sole tracked output supplies the size a percent panel's first dock
/// needs (D9): with one output the compositor can only choose that one, so
/// no output `enter` is required before the first map.
#[test]
fn a_sole_output_supplies_the_first_dock_size() {
    let state = object_state();
    let globals = Globals::new();
    globals.add_output(10, sized_output(&state, 10, Some("DP-1"), 2560, 1440));
    assert_eq!(
        globals.sole_output_size(),
        OutputSize::new(2560, 1440),
        "the only output's size is decidable"
    );
    assert_eq!(
        globals.first_known_output_size(),
        OutputSize::new(2560, 1440)
    );
}

/// A second tracked output leaves the choice open until the panel enters
/// one: the first dock must not assume the compositor picked either, while
/// the first known size stays available as the wait's last resort.
#[test]
fn a_second_output_leaves_the_choice_open() {
    let state = object_state();
    let globals = Globals::new();
    globals.add_output(10, sized_output(&state, 10, Some("DP-1"), 2560, 1440));
    globals.add_output(11, sized_output(&state, 11, Some("DP-2"), 1920, 1080));
    assert_eq!(
        globals.sole_output_size(),
        None,
        "two outputs leave the compositor's choice open"
    );
    assert_eq!(
        globals.first_known_output_size(),
        OutputSize::new(2560, 1440),
        "the fallback takes the first output in global-name order"
    );

    globals.record_client_output(11, 99);
    assert!(globals.set_panel_output(99));
    assert_eq!(
        globals.panel_output_size(),
        OutputSize::new(1920, 1080),
        "the entered output's size wins once the panel names it"
    );
}

/// A removed output is refused like an unknown one (spec "Unknown
/// output"): once the `global_remove` has dropped its tracker, its name
/// answers neither a `set-output`'s decision nor a new panel's pin
/// (D11), while the still-tracked outputs answer as before.
#[test]
fn a_removed_output_is_refused_like_an_unknown_one() {
    let state = object_state();
    let globals = Globals::new();
    for (global, name) in [(10, Some("DP-1")), (11, Some("DP-2"))] {
        globals.add_output(global, tracked_output(&state, global, name));
    }
    assert_eq!(
        set_output_decision("DP-2", &globals.tracked_output_names()),
        SetOutputDecision::Pin(11),
        "the test starts with the output tracked, so the removal is what changes it"
    );

    globals.remove_output(11);

    assert_eq!(
        set_output_decision("DP-2", &globals.tracked_output_names()),
        SetOutputDecision::Unknown,
        "a removed output is refused like an unknown name"
    );
    assert_eq!(globals.named_output("DP-2"), None);
    assert!(
        globals.output_by_global(11).is_none(),
        "a removed output pins no layer surface"
    );
    assert_eq!(
        set_output_decision("DP-1", &globals.tracked_output_names()),
        SetOutputDecision::Pin(10),
        "the still-tracked outputs answer as before"
    );
}

/// A removed output leaves no stale panel state (D11): the panel's
/// output reverts to the compositor's choice and an app `wl_output`
/// bind of the dead global no longer claims a size the proxy tracks.
#[test]
fn a_removed_output_leaves_no_stale_panel_state() {
    let state = object_state();
    let globals = Globals::new();
    globals.add_output(11, tracked_output(&state, 11, Some("DP-2")));
    globals.record_client_output(11, 99);
    assert!(globals.set_panel_output(99));
    assert_eq!(globals.panel_output(), Some(11));

    globals.remove_output(11);

    assert_eq!(
        globals.panel_output(),
        None,
        "the panel is no longer pinned to the dead output"
    );
    assert!(
        !globals.set_panel_output(99),
        "a stale app bind names no size the proxy tracks"
    );
}

/// A removed output can leave the only remaining tracked output as the
/// size a waiting first dock uses (D9): the removal wakes the same decision
/// as a size arrival.
#[test]
fn a_removed_output_leaves_a_sole_size_behind() {
    let state = object_state();
    let globals = Globals::new();
    globals.add_output(10, sized_output(&state, 10, Some("DP-1"), 2560, 1440));
    globals.add_output(11, tracked_output(&state, 11, Some("DP-2")));
    assert_eq!(globals.sole_output_size(), None);

    globals.remove_output(11);

    assert_eq!(
        globals.sole_output_size(),
        OutputSize::new(2560, 1440),
        "the remaining sole output now supplies the first dock's size"
    );
}
