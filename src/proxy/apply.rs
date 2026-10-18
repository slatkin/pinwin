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
//! - `set-layer` changes the layer of the live layer surface in place
//!   ([`dock::set_layer`], D11), so the window keeps its layer surface and
//!   stays configured and mapped (spec "Change the layer"); a
//!   `set-output` and a `closed` surface still re-dock, because the output
//!   of a live layer surface is fixed at its creation.
//!
//! Gutters have no command of their own (the design keeps them to the options
//! and the config file), so they reach the apply path only inside the
//! settings a change carries: an edge move re-reads them into the margins of
//! the new edge, and every plan validates the geometry they take part in.
//!
//! The commit that applies the re-sent state is the app's own (D7): the proxy
//! sends no separate commit on the app's surface, exactly as the crop's
//! steady state does.

use std::rc::Rc;

use super::dock;
use super::mapping::{anchor_of, layer_of};
use super::routing::Routing;
use crate::cli::Amount;
use crate::config::PanelSettings;
use crate::daemon::events::Bus;
use crate::daemon::panel::PanelReply;
use crate::geometry::{Geometry, HeldGap, OutputSize, ReserveMode, Size};
use crate::ipc::{ErrorKind, ErrorReply, Request};
use crate::name::PanelName;

/// What a validated change does to the panel (row 6.6). Built only by
/// [`plan_change`] (`port-to-rust` D6).
#[derive(Debug, PartialEq, Eq)]
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
    /// Whether the change moves the panel to another layer, which the live
    /// layer surface takes in place (see the module comment).
    layer_changed: bool,
    /// The panel's length across the edge before the change, which an
    /// animated change starts from (D8).
    from_across: u32,
    /// The reserve mode before the change (D5): only a panel that pushed
    /// before the change moves its strip with the animation.
    current_reserve: ReserveMode,
    /// The duration of an animated change that only moves the width: the
    /// request's `ms` or the panel's `tween_ms`, clamped (D8). `None` snaps.
    animate: Option<u32>,
    /// Whether this size command asks for a step change (D8): a zero
    /// duration, or a target the panel already has. It still stops a running
    /// tween at its target (spec "Snap interrupts").
    snaps: bool,
}

/// The answer to one layout command (row 6.6, spec "Panel commands"): the
/// validated plan applied to the live panel, or the refusal the client gets.
/// `None` for a request this row does not apply, which the caller answers
/// without effect.
pub(super) fn answer(
    routing: &Rc<Routing>,
    request: &Request,
    panel: &PanelName,
    bus: &Bus,
) -> Option<PanelReply> {
    if !is_layout_request(request) {
        return None;
    }
    // A layout change validates against the panel's output (D9). A panel
    // that has not learned that size yet cannot be validated, so the change
    // is refused instead of resolving against an invented output.
    let Some(output) = routing.dock_output() else {
        return Some(PanelReply::Refused(ErrorReply::new(
            ErrorKind::InvalidLayout,
            "the panel's output size is not known yet",
        )));
    };
    let plan = match plan_change(&routing.settings(), request, output) {
        Ok(Some(plan)) => plan,
        Ok(None) => return None,
        Err(error) => return Some(PanelReply::Refused(error)),
    };
    apply_plan(routing, plan, panel, bus);
    Some(PanelReply::Done)
}

/// Whether `request` is one of the layout commands this module applies (row
/// 6.6). Every one needs the panel's output size for its plan.
fn is_layout_request(request: &Request) -> bool {
    matches!(
        request,
        Request::SetEdge { .. }
            | Request::SetSize { .. }
            | Request::Resize { .. }
            | Request::SetReserve { .. }
            | Request::SetLayer { .. }
    )
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
    request: &Request,
    output: OutputSize,
) -> Result<Option<Plan>, ErrorReply> {
    let next = match request {
        Request::SetEdge { edge, .. } => PanelSettings {
            edge: *edge,
            ..current.clone()
        },
        Request::SetSize { size, reserve, .. } => PanelSettings {
            size: *size,
            reserve: reserve.unwrap_or(current.reserve()),
            ..current.clone()
        },
        Request::Resize { by, reserve, .. } => PanelSettings {
            size: resize_size(current, *by, output).ok_or_else(resize_refused)?,
            reserve: reserve.unwrap_or(current.reserve()),
            ..current.clone()
        },
        Request::SetReserve { reserve, .. } => PanelSettings {
            reserve: *reserve,
            ..current.clone()
        },
        Request::SetLayer { layer, .. } => PanelSettings {
            layer: *layer,
            ..current.clone()
        },
        _ => return Ok(None),
    };
    let layer_changed = matches!(request, Request::SetLayer { .. });
    let geometry = geometry_of(&next, output);
    geometry
        .validate(output)
        .map_err(|error| ErrorReply::new(ErrorKind::InvalidLayout, error.to_string()))?;
    let current_geometry = geometry_of(current, output);
    // A layer change moves no geometry, so it carries no size of its own.
    let needs_size = !layer_changed && needs_new_app_size(current_geometry, geometry, output);
    // D8: only a width change with a duration animates; an edge or gutter
    // change snaps. The request's `ms` or the panel's `tween_ms` is clamped to
    // the tween's ceiling (spec "Duration clamp"), and a zero duration snaps
    // (spec "Snap on zero duration", "Snap when animations are disabled").
    let duration = request_duration(request, current.tween_ms());
    let animate = duration.filter(|ms| *ms > 0 && needs_size);
    // A size command that does not animate still wins over a running tween
    // (spec "Snap interrupts"), even when it names the settings the panel
    // already targets.
    let snaps = duration.is_some() && animate.is_none();
    Ok(Some(Plan {
        layout_changed: current_geometry != geometry,
        settings: next,
        geometry,
        size: needs_size.then(|| geometry.size()),
        layer_changed,
        from_across: current_geometry.across(),
        current_reserve: current.reserve(),
        animate,
        snaps,
    }))
}

/// The duration a layout command animates over (D8): `set-size` and `resize`
/// use their `ms` or the panel's `tween_ms`, clamped; every other command
/// returns `None`.
fn request_duration(request: &Request, panel_ms: u32) -> Option<u32> {
    let ms = match request {
        Request::SetSize { ms, .. } | Request::Resize { ms, .. } => *ms,
        _ => return None,
    };
    Some(super::tween::duration_ms(ms, panel_ms))
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
/// compositor hears what changed. A `set-layer` changes the layer of the
/// live layer surface in place; every other change re-sends state on the
/// layer surface the window docks with.
fn apply_plan(routing: &Rc<Routing>, plan: Plan, panel: &PanelName, bus: &Bus) {
    routing.set_settings(plan.settings);
    if plan.layer_changed {
        // D11: the window keeps its layer surface and stays mapped; the
        // strip follows it into the same layer (spec "Change the layer").
        dock::set_layer(routing, layer_of(routing.settings().layer()));
        bus.publish_geometry_changed(panel);
        return;
    }
    if !plan.layout_changed {
        // D8: a snap wins over a running animation even when it names the
        // state the panel already targets (spec "Snap interrupts").
        if plan.snaps {
            snap(routing, &plan.geometry);
        }
        return;
    }
    routing.set_last_resolved_across(Some(plan.geometry.across()));
    // D8: an animated change starts its tween instead of re-sending the
    // target size; the tween's frames re-send the layer state each frame.
    if let Some(ms) = plan.animate
        && super::tween::start(
            routing,
            plan.from_across,
            plan.current_reserve,
            plan.geometry,
            ms,
        )
    {
        bus.publish_geometry_changed(panel);
        return;
    }
    // D8: the snap sends the layer state outside the commit coordinator and
    // wins over a running tween, so the tween and a frame it queued are
    // dropped (spec "Snap interrupts", "Interrupting an animation").
    let interrupted = super::tween::is_running(routing);
    routing.globals().tween().clear();
    if let Some(layer_surface) = routing.panel_layer_surface() {
        // A change with no new app size of its own (a pure edge move) still
        // lands on its target when it interrupts a tween: the layer surface
        // still shows an intermediate frame.
        let size = plan
            .size
            .or_else(|| interrupted.then(|| plan.geometry.size()));
        if let Some((width, height)) = size {
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

/// Stops a running tween at the state a snap names (D8, spec "Snap
/// interrupts"): the tween and its queued frames go, and the layer surface
/// re-sends the target so the panel lands on it at once. Nothing happens when
/// no tween runs.
fn snap(routing: &Rc<Routing>, geometry: &Geometry) {
    if !super::tween::is_running(routing) {
        return;
    }
    routing.globals().tween().clear();
    if let Some(layer_surface) = routing.panel_layer_surface() {
        let (width, height) = geometry.size();
        layer_surface.send_set_size(width, height);
        let margins = geometry.margins();
        layer_surface.send_set_margin(
            margins.top(),
            margins.right(),
            margins.bottom(),
            margins.left(),
        );
        layer_surface.send_set_anchor(anchor_of(geometry.anchors()));
    }
    publish_gap(routing, geometry);
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
        // D8: the re-apply re-sends the layer state outside the commit
        // coordinator and wins over a running tween, so the tween and a frame
        // it queued are dropped (spec "Interrupting an animation").
        routing.globals().tween().clear();
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
        if routing.hidden.is_hidden() {
            reserve.apply_gap_hidden(gap);
        } else {
            reserve.apply_gap(gap);
        }
    }
    routing.set_held_gap(gap);
}

#[cfg(test)]
mod tests;
