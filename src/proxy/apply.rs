//! The live apply of a layout change (replace-terminal-with-app-host row 6.6,
//! spec "Apply layout without restarting the app", "Validate a change before
//! applying it"): `set-edge`, `set-size`, `resize`, `set-reserve` and
//! `set-layer` change a running panel's settings and re-send to the
//! compositor what changed, on the layer surface the panel window already
//! docks with. The app keeps its process and its window.
//!
//! The pure part is [`plan_change`]: it builds the settings the change would
//! leave the panel with, resolves the geometry they describe against the
//! panel's output, validates it and decides what the compositor has to hear.
//! A refusal is the error kind the client gets and nothing is applied, so the
//! panel's settings, window and reservation stay unchanged (spec "Validate a
//! change before applying it"). The applied part is thin: it stores the
//! settings, re-sends the layer surface's size, anchors and margins, and
//! publishes the held gap to the reserve surface (D9). A successful apply
//! publishes `Event::GeometryChanged` for the panel (spec "Event stream").
//!
//! Two rules come from the spec:
//!
//! - the app gets a new window size only when the panel's width or length
//!   changes ([`needs_new_app_size`]): a pure edge move with the same size
//!   sends no new size, and a size change sends exactly one;
//! - the layer of a live layer surface is fixed at its creation, so
//!   `set-layer` re-docks the window on a new layer surface ([`dock::redock`],
//!   D11) instead of re-sending state on the old one — the same re-dock a
//!   `set-output` and a `closed` surface use, so the app still keeps its
//!   window.
//!
//! Gutters have no command of their own (the design keeps them to the options
//! and the config file), so they reach the apply path only inside the
//! settings a change carries: an edge move re-reads them into the margins of
//! the new edge, and every plan validates the geometry they take part in.
//!
//! The commit that applies the re-sent state is the app's own (D7): the proxy
//! sends no commit of its own on the app's surface until row 8.1's commit
//! coordinator, exactly as the crop's steady state already does.

use std::rc::Rc;

use super::dock;
use super::mapping::anchor_of;
use super::routing::Routing;
use crate::cli::Amount;
use crate::config::PanelSettings;
use crate::daemon::events::Bus;
use crate::daemon::panel::{PanelReply, PanelRequest};
use crate::geometry::{Geometry, HeldGap, OutputSize, Size};
use crate::ipc::{ErrorKind, ErrorReply};
use crate::name::PanelName;

/// What a validated change does to the panel (row 6.6). Built only by
/// [`plan_change`] (`port-to-rust` D6).
#[derive(Debug)]
struct Plan {
    /// The settings the panel runs at after the change: what a status reply
    /// reports and what the next dock builds from.
    settings: PanelSettings,
    /// The geometry those settings resolve to on the panel's output, already
    /// validated (D9).
    geometry: Geometry,
    /// The layer surface's size to re-send, when the app's window changes
    /// size (spec "Apply layout without restarting the app"): `None` for a
    /// pure edge move, which sends no new size.
    size: Option<(u32, u32)>,
    /// Whether the geometry changes at all: a change that resolves to the
    /// geometry the panel already runs at re-sends nothing.
    layout_changed: bool,
    /// Whether the change needs a new layer surface (see the module comment).
    redock: bool,
}

/// The answer to one layout command (row 6.6, spec "Panel commands"): the
/// validated plan applied to the live panel, or the refusal the client gets.
/// `None` for a request this row does not apply, which the caller answers
/// without effect.
pub(super) fn answer(
    routing: &Rc<Routing>,
    request: &PanelRequest,
    panel: &PanelName,
    bus: &Bus,
) -> Option<PanelReply> {
    let plan = match plan_change(&routing.settings(), request, routing.known_output()) {
        Ok(Some(plan)) => plan,
        Ok(None) => return None,
        Err(error) => return Some(PanelReply::Refused(error)),
    };
    apply_plan(routing, plan, panel, bus);
    Some(PanelReply::Done)
}

/// The plan of one layout command (row 6.6): the settings the change would
/// leave the panel with, the geometry they resolve to on `output` and what
/// the compositor has to hear. `None` for a request row 6.6 does not apply.
///
/// Every command is validated against the panel's output before anything is
/// applied, whatever it changes: a panel the output cannot hold is refused
/// with `invalid_layout`, and a value outside the size grammar with
/// `invalid_value` (spec "Panel commands", "Validate a change before
/// applying it").
fn plan_change(
    current: &PanelSettings,
    request: &PanelRequest,
    output: OutputSize,
) -> Result<Option<Plan>, ErrorReply> {
    let next = match request {
        PanelRequest::SetEdge(edge) => current.with_edge(*edge),
        PanelRequest::SetSize { size, .. } => current.with_size(*size),
        PanelRequest::Resize { by, .. } => {
            current.with_size(resize_size(current, *by, output).ok_or_else(resize_refused)?)
        }
        PanelRequest::SetReserve(reserve) => current.with_reserve(*reserve),
        PanelRequest::SetLayer(layer) => current.with_layer(*layer),
        _ => return Ok(None),
    };
    let redock = matches!(request, PanelRequest::SetLayer(_));
    let geometry = geometry_of(&next, output);
    geometry
        .validate(output)
        .map_err(|error| ErrorReply::new(ErrorKind::InvalidLayout, error.to_string()))?;
    let current_geometry = geometry_of(current, output);
    // A re-dock sends the whole geometry, so it carries no size of its own.
    let needs_size = !redock && needs_new_app_size(current_geometry, geometry, output);
    Ok(Some(Plan {
        layout_changed: current_geometry != geometry,
        settings: next,
        geometry,
        size: needs_size.then(|| geometry.size()),
        redock,
    }))
}

/// The geometry `settings` describe on `output` (D9): the edge, the size
/// across that edge — a percent resolved against the output — the gutters and
/// the reserve mode.
fn geometry_of(settings: &PanelSettings, output: OutputSize) -> Geometry {
    Geometry::new(
        settings.edge(),
        settings.size().resolve(settings.edge(), output),
        settings.gutters(),
        settings.reserve(),
    )
}

/// The size a `resize` asks for (spec "Size values", "Relative resize"): a
/// percent amount moves a percent size in percent, so a size that follows the
/// output keeps following it, and every other combination moves the pixels
/// the two sides resolve to on `output`. `None` when the result is outside
/// the size grammar, which is the `invalid_value` the client gets.
fn resize_size(current: &PanelSettings, by: Amount, output: OutputSize) -> Option<Size> {
    let edge = current.edge();
    let magnitude = by.size();
    match (current.size(), magnitude) {
        (Size::Percent(held), Size::Percent(step)) => {
            let shifted = if by.shrinks() {
                held.get().checked_sub(step.get())
            } else {
                held.get().checked_add(step.get())
            }?;
            Size::percent(shifted)
        }
        (held, step) => {
            let base = held.resolve(edge, output);
            let moved = step.resolve(edge, output);
            let shifted = if by.shrinks() {
                base.checked_sub(moved)
            } else {
                base.checked_add(moved)
            }?;
            Size::pixels(shifted)
        }
    }
}

/// The refusal of a `resize` whose result is outside the size grammar (spec
/// "Size values"): a panel is at least one pixel wide, or one to a hundred
/// percent of its output.
fn resize_refused() -> ErrorReply {
    ErrorReply::new(
        ErrorKind::InvalidValue,
        "resize: the change would leave the panel's size outside the size grammar \
         (at least one pixel, or 1 to 100 percent)",
    )
}

/// The window size the panel's geometry configures its app with on `output`
/// (row 6.6): the size across the edge on the edge's own axis, and the
/// output's length less the two side gutters on the other — which is what a
/// layer surface anchored to its edge plus both ends is given. A negative
/// length, which validation refuses before any apply, clamps to zero here:
/// the comparison only has to tell a change from a no-change.
fn app_size(geometry: Geometry, output: OutputSize) -> (u32, u32) {
    let edge = geometry.edge();
    let gutters = geometry.gutters();
    let length = u32::try_from(
        i64::from(output.length(edge))
            - i64::from(gutters.along_start(edge))
            - i64::from(gutters.along_end(edge)),
    )
    .unwrap_or_default();
    if edge.across_is_width() {
        (geometry.across(), length)
    } else {
        (length, geometry.across())
    }
}

/// Whether the app's window changes size when `next` replaces `current` on
/// the same output (spec "Apply layout without restarting the app"): only a
/// change of the panel's width or length does, so a pure edge move with the
/// same size sends no new size.
fn needs_new_app_size(current: Geometry, next: Geometry, output: OutputSize) -> bool {
    app_size(current, output) != app_size(next, output)
}

/// Applies a validated plan to the live panel (row 6.6): the settings are
/// stored first, so a status reply already reports them, and then the
/// compositor hears what changed. A `set-layer` re-docks the window; every
/// other change re-sends state on the layer surface the window docks with.
fn apply_plan(routing: &Rc<Routing>, plan: Plan, panel: &PanelName, bus: &Bus) {
    routing.set_settings(plan.settings);
    if plan.redock {
        // D11: the panel stays where it is pinned, in the layer it now asks
        // for; the re-dock sends the whole geometry and re-creates the strip.
        dock::redock(routing, routing.pinned_output().map(|(_, global)| global));
        bus.publish_geometry_changed(panel);
        return;
    }
    if !plan.layout_changed {
        return;
    }
    routing.set_last_resolved_across(Some(plan.geometry.across()));
    if let Some(layer_surface) = routing.panel_layer_surface() {
        if let Some((width, height)) = plan.size {
            layer_surface.send_set_size(width, height);
        }
        let margins = plan.geometry.margins();
        layer_surface.send_set_margin(
            margins.top(),
            margins.right(),
            margins.bottom(),
            margins.left(),
        );
        layer_surface.send_set_anchor(anchor_of(plan.geometry.anchors()));
    }
    publish_gap(routing, &plan.geometry);
    bus.publish_geometry_changed(panel);
}

/// The re-apply the panel's output state triggers (D9): a percent size
/// resolves against the fresh output size, the layer surface gets the fresh
/// size and the held gap and the reserve follow. A geometry the real output
/// cannot hold keeps the size the panel already runs at: this path reports
/// the refusal in the log, where a change command answers its client (row
/// 6.6).
pub(super) fn reapply(routing: &Routing) {
    let Some(output) = routing.globals().panel_output_size() else {
        return;
    };
    let settings = routing.settings();
    let across = settings.size().resolve(settings.edge(), output);
    if routing.last_resolved_across() == Some(across) {
        return;
    }
    let geometry = geometry_of(&settings, output);
    if let Err(error) = geometry.validate(output) {
        eprintln!("pinwin: the panel geometry does not fit its output: {error}");
        return;
    }
    routing.set_last_resolved_across(Some(across));
    if let Some(layer_surface) = routing.panel_layer_surface() {
        let (width, height) = geometry.size();
        layer_surface.send_set_size(width, height);
    }
    publish_gap(routing, &geometry);
}

/// Publishes a validated geometry to the strip the reserve holds (D9): the
/// gap moves by the reserve mode's rule — a pushing geometry resets the gap
/// to its own strip, a covering one keeps the strip where the last pushing
/// setting put it — and the reserve surface carries the new zone. A panel
/// without a reserve surface (one that could not be created) keeps only the
/// bookkeeping.
fn publish_gap(routing: &Routing, geometry: &Geometry) {
    let gap = match routing.held_gap() {
        Some(gap) => gap.after_publish(*geometry),
        None => HeldGap::start(*geometry),
    };
    if let Some(reserve) = routing.reserve() {
        reserve.apply_gap(gap);
    }
    routing.set_held_gap(gap);
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    use super::{Plan, Routing, app_size, needs_new_app_size, plan_change, resize_size};
    use crate::cli::Amount;
    use crate::config::{Layer, PanelSettings};
    use crate::daemon::events::{Bus, Event};
    use crate::daemon::panel::{PanelReply, PanelRequest};
    use crate::geometry::{Edge, Gutters, OutputSize, ReserveMode, Size};
    use crate::ipc::ErrorKind;
    use crate::name::PanelName;

    /// An output of `width` by `height` logical pixels.
    fn output(width: u32, height: u32) -> OutputSize {
        OutputSize::new(width, height).expect("a test output has two axes")
    }

    /// The built-in settings: a pushing panel, 25% of the output, on the
    /// left, with no gutters (spec "Built-in defaults").
    fn settings() -> PanelSettings {
        PanelSettings::built_in()
    }

    /// A pixel size the test knows is valid.
    fn pixels(px: u32) -> Size {
        Size::pixels(px).expect("a positive pixel size")
    }

    /// A percent the test knows is valid.
    fn percent(value: u32) -> Size {
        Size::percent(value).expect("a percent from 1 to 100")
    }

    /// A growing and a shrinking amount of `size`.
    fn grow(size: Size) -> Amount {
        Amount::grow(size)
    }

    fn shrink(size: Size) -> Amount {
        Amount::shrink(size)
    }

    /// The plan of one request, unwrapped.
    fn planned(current: &PanelSettings, request: &PanelRequest, output: OutputSize) -> Plan {
        plan_change(current, request, output)
            .expect("the change fits the output")
            .expect("the request is a layout command")
    }

    /// The kind of a refused request.
    fn refused(current: &PanelSettings, request: &PanelRequest, output: OutputSize) -> ErrorKind {
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
            &PanelRequest::SetEdge(Edge::Right),
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
            &PanelRequest::SetEdge(Edge::Top),
            output(1920, 1080),
        );
        assert_eq!(plan.settings.edge(), Edge::Top);
        assert_eq!(plan.size, Some((0, 270)), "25% of the 1080 pixel height");
    }

    /// A size change (spec "Change the size"): one new window size of the
    /// requested width.
    #[test]
    fn a_size_change_sends_one_new_size() {
        let current = settings();
        let plan = planned(
            &current,
            &PanelRequest::SetSize {
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
            &PanelRequest::SetSize {
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
            &PanelRequest::SetReserve(ReserveMode::Cover),
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
            &PanelRequest::SetLayer(Layer::Bottom),
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
        let current = settings().with_size(pixels(640));
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
        let sized = settings().with_size(pixels(640));
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
                &PanelRequest::Resize {
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
        let request = PanelRequest::SetSize {
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
        let current = settings().with_gutters(Gutters::new(0, 0, 600, 600));
        let request = PanelRequest::SetSize {
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
        let current = settings()
            .with_size(pixels(640))
            .with_gutters(Gutters::new(-2000, 0, 0, 0));
        let request = PanelRequest::SetSize {
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
        let routing = Routing::new(settings());
        let panel = PanelName::parse("notes").expect("a panel name");
        let reply = super::answer(&routing, &PanelRequest::SetEdge(Edge::Right), &panel, &bus);
        assert_eq!(reply, Some(PanelReply::Done));
        assert_eq!(
            events.next(Duration::from_millis(100)),
            Ok(Event::GeometryChanged { panel })
        );
    }

    /// A refused change applies nothing and publishes nothing (spec
    /// "Validate a change before applying it").
    #[test]
    fn a_refused_apply_publishes_no_geometry_change() {
        let bus = Bus::new();
        let events = bus.subscribe();
        let routing = Routing::new(settings());
        let panel = PanelName::parse("notes").expect("a panel name");
        let reply = super::answer(
            &routing,
            &PanelRequest::SetSize {
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
        let top = super::geometry_of(&settings().with_edge(Edge::Top), output(1920, 1080));
        assert_eq!(app_size(top, output(1920, 1080)), (1920, 270));
    }

    /// The decision itself: the same geometry is no change, and any change of
    /// the width or the length is.
    #[test]
    fn the_size_decision_compares_the_configured_size() {
        let output = output(1920, 1080);
        let current = super::geometry_of(&settings(), output);
        assert!(!needs_new_app_size(current, current, output));
        let wider = super::geometry_of(&settings().with_size(pixels(800)), output);
        assert!(needs_new_app_size(current, wider, output));
        let inset =
            super::geometry_of(&settings().with_gutters(Gutters::new(0, 0, 10, 10)), output);
        assert!(
            needs_new_app_size(current, inset, output),
            "the side gutters shorten the panel's length"
        );
    }
}
