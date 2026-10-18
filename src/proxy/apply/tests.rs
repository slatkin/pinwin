use std::rc::Rc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use super::{Plan, app_size, needs_new_app_size, plan_change, resize_size};
use crate::cli::Amount;
use crate::config::{Layer, PanelSettings};
use crate::daemon::events::{Bus, Event};
use crate::daemon::panel::PanelReply;
use crate::geometry::{Edge, Gutters, OutputSize, ReserveMode, Size};
use crate::ipc::{ErrorKind, Request};
use crate::name::PanelName;
use crate::proxy::routing::Routing;

/// An output of `width` by `height` logical pixels.
fn output(width: u32, height: u32) -> OutputSize {
    OutputSize::new(width, height).expect("a test output has two axes")
}

/// The built-in settings: a pushing panel, 25% of the output, on the
/// left, with no gutters (spec "Built-in defaults").
fn settings() -> PanelSettings {
    PanelSettings::built_in()
}

/// A routing with one tracked output of 1920x1080, so the percent settings
/// resolve and the live apply validates against it (D9).
fn routing() -> Rc<Routing> {
    let routing = Routing::new(settings());
    routing.globals().install_test_output(1, output(1920, 1080));
    routing
}

/// A pixel size the test knows is valid.
fn pixels(px: u32) -> Size {
    Size::pixels(px).expect("a positive pixel size")
}

/// A percent the test knows is valid.
fn percent(value: u32) -> Size {
    Size::percent(value).expect("a percent from 1 to 100")
}

/// The panel name the layout requests carry; the plan ignores it.
fn notes() -> PanelName {
    PanelName::parse("notes").expect("a panel name")
}

/// A growing and a shrinking amount of `size`.
fn grow(size: Size) -> Amount {
    Amount::grow(size)
}

fn shrink(size: Size) -> Amount {
    Amount::shrink(size)
}

/// The plan of one request, unwrapped.
fn planned(current: &PanelSettings, request: &Request, output: OutputSize) -> Plan {
    plan_change(current, request, output)
        .expect("the change fits the output")
        .expect("the request is a layout command")
}

/// The kind of a refused request.
fn refused(current: &PanelSettings, request: &Request, output: OutputSize) -> ErrorKind {
    plan_change(current, request, output)
        .expect_err("the change is refused")
        .kind()
}

/// A pure edge move with the same size (spec "Change the edge"): the
/// panel docks at the other side and the app gets no new size.
#[test]
fn a_pure_edge_move_sends_no_new_size() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetEdge {
            panel: notes(),
            edge: Edge::Right,
        },
        output(1920, 1080),
    );
    assert_eq!(plan.settings.edge(), Edge::Right);
    assert_eq!(plan.size, None, "the same size across the other side");
    assert!(plan.layout_changed, "the anchors and margins move");
    assert!(!plan.redock, "the window keeps its layer surface");
}

/// An edge move to a perpendicular edge changes the app's size, because
/// the size resolves against the other axis and the length runs along the
/// other one: exactly one new size.
#[test]
fn an_edge_move_across_the_axes_sends_one_new_size() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetEdge {
            panel: notes(),
            edge: Edge::Top,
        },
        output(1920, 1080),
    );
    assert_eq!(plan.settings.edge(), Edge::Top);
    assert_eq!(plan.size, Some((0, 270)), "25% of the 1080 pixel height");
}

/// A `set-size` with `ms` above zero animates (D8): the plan carries the
/// start length and the duration instead of re-sending the target size
/// at once.
#[test]
fn a_size_change_with_a_duration_animates_from_the_current_width() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetSize {
            panel: notes(),
            size: pixels(800),
            ms: Some(200),
        },
        output(1920, 1080),
    );
    assert_eq!(plan.from_across, 480, "25% of the 1920 pixel output");
    assert_eq!(plan.animate, Some(200));
    assert_eq!(plan.size, Some((800, 0)), "the size the tween targets");
}

/// A `resize` with `ms` animates too (D8), from the panel's current
/// width to the resized one.
#[test]
fn a_resize_with_a_duration_animates() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::Resize {
            panel: notes(),
            by: grow(pixels(120)),
            ms: Some(150),
        },
        output(1920, 1080),
    );
    assert_eq!(plan.geometry.across(), 600);
    assert_eq!(plan.from_across, 480);
    assert_eq!(plan.animate, Some(150));
}

/// A duration of zero snaps (D8), and the plan marks the size command as
/// a snap so it still interrupts a running tween (spec "Snap on zero
/// duration", "Snap interrupts").
#[test]
fn a_zero_duration_snaps_and_interrupts() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetSize {
            panel: notes(),
            size: pixels(800),
            ms: Some(0),
        },
        output(1920, 1080),
    );
    assert_eq!(plan.animate, None, "a zero duration snaps");
    assert!(plan.snaps, "the snap wins over a running tween");
}

/// A size change without `ms` animates over the panel's `tween_ms` (D8,
/// spec "Snap when animations are disabled" leaves the zero case to
/// [`a_zero_duration_snaps_and_interrupts`]).
#[test]
fn an_absent_ms_uses_the_panel_tween_ms() {
    let current = settings();
    assert_eq!(current.tween_ms(), 200, "the built-in duration");
    let plan = planned(
        &current,
        &Request::SetSize {
            panel: notes(),
            size: pixels(800),
            ms: None,
        },
        output(1920, 1080),
    );
    assert_eq!(plan.animate, Some(200));
    assert!(!plan.snaps, "an animated change is not a snap");
}

/// A long duration is clamped to one second (D8, spec "Duration
/// clamp").
#[test]
fn a_long_duration_is_clamped() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetSize {
            panel: notes(),
            size: pixels(800),
            ms: Some(60_000),
        },
        output(1920, 1080),
    );
    assert_eq!(plan.animate, Some(1000));
}

/// A size change (spec "Change the size"): one new window size of the
/// requested width.
#[test]
fn a_size_change_sends_one_new_size() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetSize {
            panel: notes(),
            size: pixels(800),
            ms: None,
        },
        output(1920, 1080),
    );
    assert_eq!(plan.settings.size(), pixels(800));
    assert_eq!(plan.size, Some((800, 0)));
}

/// A size whose resolution does not change sends no new size (spec
/// "Apply layout without restarting the app"): 25% and the 480 pixels it
/// resolves to are the same panel.
#[test]
fn a_size_that_resolves_the_same_sends_no_new_size() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetSize {
            panel: notes(),
            size: pixels(480),
            ms: None,
        },
        output(1920, 1080),
    );
    assert_eq!(plan.size, None);
    assert!(!plan.layout_changed, "nothing has to be re-sent");
}

/// A reserve mode change never resizes the app: the strip moves, the
/// panel does not.
#[test]
fn a_reserve_change_sends_no_new_size() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetReserve {
            panel: notes(),
            reserve: ReserveMode::Cover,
        },
        output(1920, 1080),
    );
    assert_eq!(plan.size, None);
    assert!(plan.layout_changed, "the strip the reserve holds moves");
}

/// A layer change re-docks the window, because a live layer surface
/// carries its layer from its creation (D11): no size of its own, the
/// re-dock sends the geometry.
#[test]
fn a_layer_change_re_docks_the_window() {
    let current = settings();
    let plan = planned(
        &current,
        &Request::SetLayer {
            panel: notes(),
            layer: Layer::Bottom,
        },
        output(1920, 1080),
    );
    assert!(plan.redock);
    assert_eq!(plan.size, None, "the re-dock sends the size itself");
}

/// A percent amount moves a percent size in percent (spec "Size
/// values"): the size keeps following the output.
#[test]
fn a_percent_resize_keeps_the_percent_form() {
    let current = settings();
    let size = resize_size(&current, grow(percent(10)), output(1920, 1080));
    assert_eq!(size, Some(percent(35)));
    let size = resize_size(&current, shrink(percent(5)), output(1920, 1080));
    assert_eq!(size, Some(percent(20)));
}

/// A pixel amount moves a pixel size in pixels (spec "Relative resize"):
/// 640 pixels less 40 is 600.
#[test]
fn a_pixel_resize_moves_pixels() {
    let current = PanelSettings {
        size: pixels(640),
        ..settings()
    };
    assert_eq!(
        resize_size(&current, shrink(pixels(40)), output(1920, 1080)),
        Some(pixels(600))
    );
    assert_eq!(
        resize_size(&current, grow(pixels(40)), output(1920, 1080)),
        Some(pixels(680))
    );
}

/// A mixed resize resolves both sides against the output (spec "Size
/// values"): a percent of the output's width and a pixel step add up in
/// pixels.
#[test]
fn a_mixed_resize_resolves_against_the_output() {
    let current = settings();
    assert_eq!(
        resize_size(&current, grow(pixels(40)), output(1920, 1080)),
        Some(pixels(520)),
        "25% of 1920 is 480, plus 40"
    );
    let sized = PanelSettings {
        size: pixels(640),
        ..settings()
    };
    assert_eq!(
        resize_size(&sized, grow(percent(10)), output(1920, 1080)),
        Some(pixels(832)),
        "10% of 1920 is 192, added to 640 pixels"
    );
}

/// A resize whose result is outside the size grammar is refused with
/// `invalid_value` (spec "Size values", "Invalid size"): a panel is at
/// least one pixel, and a percent stays within 1 to 100.
#[test]
fn a_resize_outside_the_size_grammar_is_refused() {
    let current = settings();
    assert_eq!(
        resize_size(&current, shrink(pixels(480)), output(1920, 1080)),
        None,
        "the panel would be zero pixels wide"
    );
    assert_eq!(
        resize_size(&current, grow(percent(90)), output(1920, 1080)),
        None,
        "the panel would be more than the whole output"
    );
    assert_eq!(
        resize_size(&current, shrink(percent(30)), output(1920, 1080)),
        None,
        "the panel would be below one percent"
    );
    assert_eq!(
        refused(
            &current,
            &Request::Resize {
                panel: notes(),
                by: shrink(pixels(480)),
                ms: None,
            },
            output(1920, 1080)
        ),
        ErrorKind::InvalidValue
    );
}

/// A size the output cannot hold is refused with `invalid_layout` and
/// nothing changes (spec "Too large for the output", "Panel commands").
#[test]
fn a_size_too_large_for_the_output_is_refused() {
    let current = settings();
    let request = Request::SetSize {
        panel: notes(),
        size: pixels(1000),
        ms: None,
    };
    assert_eq!(
        refused(&current, &request, output(1000, 1000)),
        ErrorKind::InvalidLayout,
        "a pushing panel that leaves no output space for other windows"
    );
}

/// Gutters that leave less than one pixel of visible length are refused
/// with `invalid_layout` (spec "No visible length"). The IPC has no
/// gutter command, so the test reaches the validation through the
/// settings a config merge would carry; it is the same validation the
/// first dock and every apply run.
#[test]
fn gutters_that_leave_no_visible_length_are_refused() {
    let current = PanelSettings {
        gutters: Gutters::new(0, 0, 600, 600),
        ..settings()
    };
    let request = Request::SetSize {
        panel: notes(),
        size: pixels(640),
        ms: None,
    };
    assert_eq!(
        refused(&current, &request, output(1920, 1080)),
        ErrorKind::InvalidLayout
    );
}

/// A negative reservation is refused with `invalid_layout` (spec
/// "Validate a change before applying it"), reached the same way.
#[test]
fn a_negative_reservation_is_refused() {
    let current = PanelSettings {
        size: pixels(640),
        gutters: Gutters::new(-2000, 0, 0, 0),
        ..settings()
    };
    let request = Request::SetSize {
        panel: notes(),
        size: pixels(640),
        ms: None,
    };
    assert_eq!(
        refused(&current, &request, output(1920, 1080)),
        ErrorKind::InvalidLayout
    );
}

/// A successful apply publishes the panel's geometry change (spec "Event
/// stream"): an edge move changes the geometry and names the panel.
#[test]
fn a_successful_apply_publishes_the_geometry_change() {
    let bus = Bus::new();
    let events = bus.subscribe();
    let routing = routing();
    let panel = notes();
    let reply = super::answer(
        &routing,
        &Request::SetEdge {
            panel: notes(),
            edge: Edge::Right,
        },
        &panel,
        &bus,
    );
    assert_eq!(reply, Some(PanelReply::Done));
    assert_eq!(
        events.next(Duration::from_millis(100)),
        Ok(Event::GeometryChanged { panel }.line())
    );
}

/// A refused change applies nothing and publishes nothing (spec
/// "Validate a change before applying it").
#[test]
fn a_refused_apply_publishes_no_geometry_change() {
    let bus = Bus::new();
    let events = bus.subscribe();
    let routing = routing();
    let panel = notes();
    let reply = super::answer(
        &routing,
        &Request::SetSize {
            panel: notes(),
            size: pixels(10_000),
            ms: None,
        },
        &panel,
        &bus,
    );
    assert!(matches!(reply, Some(PanelReply::Refused(_))));
    assert_eq!(
        events.next(Duration::from_millis(50)),
        Err(RecvTimeoutError::Timeout)
    );
}

/// The app size the geometry configures: the size across the edge on the
/// edge's axis and the visible length on the other, whatever the edge is.
#[test]
fn the_app_size_follows_the_edge() {
    let left = super::geometry_of(&settings(), output(1920, 1080));
    assert_eq!(app_size(left, output(1920, 1080)), (480, 1080));
    let top = super::geometry_of(
        &PanelSettings {
            edge: Edge::Top,
            ..settings()
        },
        output(1920, 1080),
    );
    assert_eq!(app_size(top, output(1920, 1080)), (1920, 270));
}

/// The decision itself: the same geometry is no change, and any change of
/// the width or the length is.
#[test]
fn the_size_decision_compares_the_configured_size() {
    let output = output(1920, 1080);
    let current = super::geometry_of(&settings(), output);
    assert!(!needs_new_app_size(current, current, output));
    let wider = super::geometry_of(
        &PanelSettings {
            size: pixels(800),
            ..settings()
        },
        output,
    );
    assert!(needs_new_app_size(current, wider, output));
    let inset = super::geometry_of(
        &PanelSettings {
            gutters: Gutters::new(0, 0, 10, 10),
            ..settings()
        },
        output,
    );
    assert!(
        needs_new_app_size(current, inset, output),
        "the side gutters shorten the panel's length"
    );
}
