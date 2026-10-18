//! The `set-gutters` tests of `app-env-reserve-show-existing` row 5.2 (spec
//! "Set gutters", "Gutter request fields", design D7): the plan that merges
//! the gutters a request names, the refusal that leaves everything alone,
//! the hidden panel that stores them for its next show, the app-size rule
//! and the no-change that publishes nothing.
//!
//! The plan is display-free, so these tests run without a compositor
//! (`port-to-rust` D10).

use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use super::super::{geometry_of, needs_new_app_size};
use super::{notes, output, planned, routing, settings};
use crate::daemon::events::{Bus, Event};
use crate::daemon::panel::PanelReply;
use crate::geometry::{Geometry, Gutters, HeldGap, Margins};
use crate::ipc::{ErrorKind, Request};

/// `set-gutters` changes the gutters a request names and keeps the rest as
/// one plan with the new margins, and it never animates, even when the
/// panel has a `tween_ms` (spec "Set the side gutters", "Gutter change
/// snaps", D7).
#[test]
fn a_gutter_change_is_one_plan_with_the_new_margins() {
    let current = settings();
    assert_eq!(current.tween_ms(), 200, "the built-in duration");
    let plan = planned(
        &current,
        &Request::SetGutters {
            panel: notes(),
            top: Some(8),
            bottom: Some(8),
            left: None,
            right: None,
        },
        output(1920, 1080),
    );
    assert_eq!(plan.settings.gutters(), Gutters::new(0, 0, 8, 8));
    assert_eq!(plan.geometry.margins(), Margins::new(8, 0, 8, 0));
    assert!(plan.layout_changed, "the margins move");
    assert_eq!(
        plan.animate, None,
        "a gutter change has no duration of its own"
    );
    assert!(!plan.snaps, "a gutter change is not a size command");
}

/// A pushing left panel whose far gutter leaves no output width for other
/// windows is refused with `invalid_layout`, and the settings and the gap
/// stay (spec "Refused gutter change", "Validate a change before applying
/// it").
#[test]
fn a_refused_gutter_change_leaves_the_settings_and_the_gap_alone() {
    let bus = Bus::new();
    let events = bus.subscribe();
    let routing = routing();
    let current = settings();
    routing.set_settings(current.clone());
    let gap = HeldGap::start(geometry_of(&current, output(1920, 1080)));
    routing.set_held_gap(gap);
    let panel = notes();
    let reply = super::super::answer(
        &routing,
        &Request::SetGutters {
            panel: notes(),
            top: None,
            bottom: None,
            left: None,
            right: Some(1500),
        },
        &panel,
        &bus,
    );
    let Some(PanelReply::Refused(error)) = reply else {
        panic!("the gutter change is refused: {reply:?}");
    };
    assert_eq!(error.kind(), ErrorKind::InvalidLayout);
    assert_eq!(routing.settings(), current, "the gutters stay");
    assert_eq!(routing.held_gap(), Some(gap), "the reservation stays");
    assert_eq!(
        events.next(Duration::from_millis(50)),
        Err(RecvTimeoutError::Timeout),
        "a refusal sends no event"
    );
}

/// A hidden panel stores the new gutters without showing anything, and the
/// geometry its next show sends already carries them (spec "Hidden panel
/// stores the gutters", "Change while hidden").
#[test]
fn a_hidden_panel_stores_the_gutters_for_its_next_show() {
    let bus = Bus::new();
    let events = bus.subscribe();
    let routing = routing();
    routing.hidden.set_hidden(true);
    let panel = notes();
    let reply = super::super::answer(
        &routing,
        &Request::SetGutters {
            panel: notes(),
            top: Some(8),
            bottom: Some(8),
            left: None,
            right: None,
        },
        &panel,
        &bus,
    );
    assert_eq!(reply, Some(PanelReply::Done));
    assert_eq!(routing.settings().gutters(), Gutters::new(0, 0, 8, 8));
    assert!(routing.hidden.is_hidden(), "nothing shows");
    assert_eq!(
        routing.geometry().map(Geometry::margins),
        Some(Margins::new(8, 0, 8, 0)),
        "the next show sends the new margins"
    );
    assert_eq!(
        events.next(Duration::from_millis(100)),
        Ok(Event::GeometryChanged { panel }.line())
    );
}

/// A near gutter moves the panel without changing its length, while a side
/// gutter shortens it, so only the side change sends the app a new size
/// (spec "App size follows the length", "Change the gutters").
#[test]
fn a_side_gutter_sends_one_new_size_and_a_near_gutter_none() {
    let current = settings();
    let output = output(1920, 1080);
    let side = planned(
        &current,
        &Request::SetGutters {
            panel: notes(),
            top: Some(8),
            bottom: Some(8),
            left: None,
            right: None,
        },
        output,
    );
    assert_eq!(
        side.size,
        Some((480, 0)),
        "the side gutters re-send the panel's size"
    );
    assert!(
        needs_new_app_size(geometry_of(&current, output), side.geometry, output),
        "the app gets one new window height"
    );

    let near = planned(
        &side.settings,
        &Request::SetGutters {
            panel: notes(),
            top: None,
            bottom: None,
            left: Some(12),
            right: None,
        },
        output,
    );
    assert_eq!(
        near.size, None,
        "a near gutter moves the panel, not its length"
    );
    assert!(near.layout_changed, "the left margin moves");
}

/// A gutter request that names the gutters the panel already has is a
/// no-change and publishes no event (D7).
#[test]
fn a_gutter_change_that_names_the_current_gutters_publishes_nothing() {
    let bus = Bus::new();
    let events = bus.subscribe();
    let routing = routing();
    let panel = notes();
    let reply = super::super::answer(
        &routing,
        &Request::SetGutters {
            panel: notes(),
            top: None,
            bottom: None,
            left: Some(0),
            right: None,
        },
        &panel,
        &bus,
    );
    assert_eq!(reply, Some(PanelReply::Done));
    assert_eq!(
        events.next(Duration::from_millis(50)),
        Err(RecvTimeoutError::Timeout),
        "a no-change sends no event"
    );
}
