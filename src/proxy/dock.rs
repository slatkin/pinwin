//! The panel window's dock on one output (replace-terminal-with-app-host
//! D11): the layer surface the panel window is served by, the handler that
//! maps its events back to the app's toplevel, and the re-docking an output
//! change requires.
//!
//! The layer surface is created with no output unless the panel's settings
//! name one, so the compositor chooses (D11). A `set-output` checks the
//! name against the outputs the proxy tracks — a name no tracked output
//! carries is refused with error kind `invalid_value` and the panel does
//! not move (spec "Unknown output") — then destroys the current layer
//! surface and creates a new one on the chosen output; the app keeps its
//! window and gets the new surface's configure, and a new size only when a
//! percent size resolves differently on the target output. When the
//! compositor sends `closed` — for example, the monitor was unplugged — the
//! proxy creates a new layer surface with no output instead of closing the
//! app's window (spec "Monitor unplugged"), and the reserve surface follows
//! ([`super::routing`] `recreate_reserve`).
//!
//! The configure/ack mapping and the handler shape are copied from
//! wl-paper's `apps/wl-paper/src/paper.rs` in
//! <https://github.com/mahkoh/wl-proxy> (GPL-3.0, the licence of this
//! crate); the `closed` behaviour and the output pinning are pinwin's.
//!
//! The handler parameters the proxy's own object would carry are named
//! `_slf`, the trait's name with the unused-prefix the codebase's lints
//! require, and the handler keeps the routing weak: the routing holds the
//! panel's layer surface strongly, so a strong reference back would close a
//! reference cycle.

use std::rc::{Rc, Weak};

use wl_proxy::object::{Object, ObjectCoreApi};
use wl_proxy::protocols::wayland::wl_compositor::WlCompositor;
use wl_proxy::protocols::wayland::wl_output::WlOutput;
use wl_proxy::protocols::wayland::wl_shm::WlShm;
use wl_proxy::protocols::wayland::wl_surface::WlSurface;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::{
    ZwlrLayerShellV1, ZwlrLayerShellV1Layer,
};
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::{
    ZwlrLayerSurfaceV1, ZwlrLayerSurfaceV1Handler,
};
use wl_proxy::protocols::xdg_shell::xdg_surface::XdgSurface;
use wl_proxy::protocols::xdg_shell::xdg_toplevel::{XdgToplevel, XdgToplevelState};

use super::mapping::{anchor_of, keyboard_of, layer_of};
use super::reserve::Reserve;
use super::routing::Routing;
use crate::config::PanelSettings;
use crate::geometry::{Geometry, HeldGap};
use crate::ipc::{ErrorKind, ErrorReply};

/// The layer-shell namespace of a panel surface. One namespace keeps every
/// panel out of the special namespaces compositors reserve for their own
/// panels.
const NAMESPACE: &str = "pinwin";

/// What a `set-output` request resolves to (D11, spec "Unknown output").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SetOutputDecision {
    /// The name matches a tracked output: the panel pins to the global it
    /// was bound under.
    Pin(u32),
    /// No tracked output carries the name: the request is refused and the
    /// panel does not move.
    Unknown,
}

/// The `set-output` decision (D11, spec "Unknown output"): a name one
/// tracked output carries pins the panel to that output, and every other
/// name is refused. `tracked` lists each tracked output's global name and
/// the compositor name it has announced, if any — an output that has not
/// announced a name yet matches nothing.
pub(super) fn set_output_decision(
    requested: &str,
    tracked: &[(u32, Option<String>)],
) -> SetOutputDecision {
    for (global, name) in tracked {
        if name.as_deref() == Some(requested) {
            return SetOutputDecision::Pin(*global);
        }
    }
    SetOutputDecision::Unknown
}

/// The app-side objects of the panel window (D4): the surface the layer
/// role is put on and the local xdg role objects the layer surface's events
/// map to.
pub(super) struct PanelWindow {
    pub(super) surface: Rc<WlSurface>,
    pub(super) xdg_surface: Rc<XdgSurface>,
    pub(super) toplevel: Rc<XdgToplevel>,
}

/// Creates the layer surface of the panel window (D4, D11): the role is put
/// on `surface` with the settings' layer and keyboard mode and the
/// geometry's anchors, margins and size, pinned to `output` when one is
/// given — `None` lets the compositor choose (D11). The panel surface
/// claims no zone of its own; the reserve surface owns the strip (D9).
fn create_layer_surface(
    layer_shell: &Rc<ZwlrLayerShellV1>,
    surface: &Rc<WlSurface>,
    output: Option<&Rc<WlOutput>>,
    settings: &PanelSettings,
    geometry: &Geometry,
) -> Rc<ZwlrLayerSurfaceV1> {
    let layer_surface = layer_shell.new_send_get_layer_surface(
        surface,
        output,
        layer_of(settings.layer()),
        NAMESPACE,
    );
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
    layer_surface.send_set_keyboard_interactivity(keyboard_of(settings.keyboard()));
    // D9: the panel surface claims no zone of its own; the reserve surface
    // owns the strip.
    layer_surface.send_set_exclusive_zone(-1);
    layer_surface
}

/// Docks the panel window (D4, D11): creates its layer surface on the
/// output named by `output_global` — `None` lets the compositor choose —
/// installs the configure/ack mapping and registers the window with the
/// routing, so the crop and the size re-apply write through it.
fn dock_window(
    routing: &Rc<Routing>,
    layer_shell: &Rc<ZwlrLayerShellV1>,
    window: &PanelWindow,
    geometry: &Geometry,
    output_global: Option<u32>,
) {
    let output = output_global.and_then(|global| routing.globals().output_by_global(global));
    let settings = routing.settings();
    let layer_surface = create_layer_surface(
        layer_shell,
        &window.surface,
        output.as_ref(),
        &settings,
        geometry,
    );
    layer_surface.set_handler(ProxyLayerSurface {
        routing: Rc::downgrade(routing),
        xdg_surface: Rc::clone(&window.xdg_surface),
        xdg_toplevel: Rc::clone(&window.toplevel),
        layer_surface: Rc::downgrade(&layer_surface),
    });
    routing.set_panel_window(
        &window.surface,
        &window.xdg_surface,
        &window.toplevel,
        &layer_surface,
    );
}

/// The initial dock of the panel window, when its toplevel takes the slot
/// (D4): the panel pins to the output its settings name, when the proxy has
/// bound and named it.
pub(super) fn dock_panel_window(
    routing: &Rc<Routing>,
    layer_shell: &Rc<ZwlrLayerShellV1>,
    window: &PanelWindow,
    geometry: &Geometry,
) {
    let pin = routing.pinned_output();
    dock_window(
        routing,
        layer_shell,
        window,
        geometry,
        pin.as_ref().map(|(_, global)| *global),
    );
}

/// Re-docks the panel window on the output named by `output_global` (D11):
/// destroys the layer surface and the reserve it had and creates both again
/// with the same settings, so the app keeps its window and gets the new
/// surface's configure (spec "Monitor unplugged", "Apply layout without
/// restarting the app"). Does nothing when the panel holds no window.
pub(super) fn redock(routing: &Rc<Routing>, output_global: Option<u32>) {
    let Some((surface, xdg_surface, toplevel)) = routing.panel_window() else {
        return;
    };
    let Some(layer_shell) = routing.layer_shell() else {
        return;
    };
    // The old handler goes first: its delete_id would otherwise free the
    // app's toplevel id, and the app must not lose its window (D11).
    if let Some(old) = routing.take_panel_layer_surface() {
        old.unset_handler();
        old.send_destroy();
    }
    // A percent size resolves against the target output's size when it is
    // known, so a size that resolves differently arrives with the new
    // surface's configure (D11, D9).
    let target_size = output_global.and_then(|global| routing.globals().output_size(global));
    let geometry = routing.geometry_against(target_size.unwrap_or_else(|| routing.known_output()));
    let window = PanelWindow {
        surface,
        xdg_surface,
        toplevel,
    };
    dock_window(routing, &layer_shell, &window, &geometry, output_global);
    // The strip follows the panel (D9, D11): re-created with the gap it
    // already holds, on the same output.
    recreate_reserve(routing, output_global);
}

/// Creates the reserve surface with the panel window (D9): it carries the
/// exclusive zone from the held gap the panel starts with, pinned to the
/// panel's output when one is set (D11). A missing compositor, `wl_shm` or
/// layer-shell global -- or a failed buffer -- leaves the panel docked
/// without a strip rather than ending it.
pub(super) fn start_reserve(
    routing: &Rc<Routing>,
    geometry: &Geometry,
    layer: ZwlrLayerShellV1Layer,
    pin: Option<&(Rc<WlOutput>, u32)>,
) {
    let (Some(compositor), Some(shm), Some(layer_shell)) = (
        routing.globals().compositor(),
        routing.globals().shm(),
        routing.layer_shell(),
    ) else {
        eprintln!("pinwin: no compositor, shm or layer-shell global: no strip is reserved");
        return;
    };
    let gap = HeldGap::start(*geometry);
    dock_reserve(routing, &compositor, &shm, &layer_shell, gap, layer, pin);
}

/// Re-docks the reserve on the output named by `output_global` (D11):
/// destroys the surface it had and creates a new one carrying the gap it
/// already holds. `None` lets the compositor choose. Does nothing when no
/// reserve exists.
pub(super) fn recreate_reserve(routing: &Rc<Routing>, output_global: Option<u32>) {
    let Some(gap) = routing.held_gap() else {
        return;
    };
    let (Some(compositor), Some(shm), Some(layer_shell)) = (
        routing.globals().compositor(),
        routing.globals().shm(),
        routing.layer_shell(),
    ) else {
        eprintln!("pinwin: no compositor, shm or layer-shell global: no strip is reserved");
        return;
    };
    if let Some(old) = routing.take_reserve() {
        old.destroy();
    }
    let pin = output_global.and_then(|global| {
        routing
            .globals()
            .output_by_global(global)
            .map(|output| (output, global))
    });
    dock_reserve(
        routing,
        &compositor,
        &shm,
        &layer_shell,
        gap,
        layer_of(routing.settings().layer()),
        pin.as_ref(),
    );
}

/// Creates one reserve surface and registers it with the routing, together
/// with its held gap and output (D9, D11): the body `start_reserve` and
/// `recreate_reserve` share.
fn dock_reserve(
    routing: &Rc<Routing>,
    compositor: &Rc<WlCompositor>,
    shm: &Rc<WlShm>,
    layer_shell: &Rc<ZwlrLayerShellV1>,
    gap: HeldGap,
    layer: ZwlrLayerShellV1Layer,
    pin: Option<&(Rc<WlOutput>, u32)>,
) {
    let reserve = Reserve::create(
        compositor,
        shm,
        layer_shell,
        gap,
        layer,
        pin.map(|(output, _)| output),
        routing.weak(),
    );
    routing.register_reserve(reserve, gap, pin.map(|(_, global)| *global));
}

/// The compositor closed the reserve surface (D11, spec "Monitor
/// unplugged"): the strip follows the panel. A strip that shared the panel's
/// output is re-created where the compositor chooses, since the panel's
/// surface is closing too; a strip that sat elsewhere moves to the panel's
/// output.
pub(super) fn on_reserve_closed(routing: &Rc<Routing>) {
    let panel = routing.globals().panel_output();
    if panel.is_some() && routing.reserve_output() == panel {
        recreate_reserve(routing, None);
    } else {
        recreate_reserve(routing, panel);
    }
}

/// The reserve surface entered an output: remember where the strip is and
/// move it when it is not on the panel's output (D11).
pub(super) fn on_reserve_entered(routing: &Rc<Routing>, output: &Rc<WlOutput>) {
    let global = routing.globals().global_of_output(output);
    routing.set_reserve_output(global);
    follow_reserve(routing);
}

/// Moves the strip to the panel's output when it entered another one (D9:
/// the reservation applies only to the panel's monitor; D11).
pub(super) fn follow_reserve(routing: &Rc<Routing>) {
    let Some(panel) = routing.globals().panel_output() else {
        return;
    };
    if routing.reserve_output() == Some(panel) {
        return;
    }
    recreate_reserve(routing, Some(panel));
}

/// The `set-output` flow (D11, spec "Unknown output"): a name one tracked
/// output carries pins the panel to it and re-docks the window; any other
/// name is refused with `invalid_value` and the panel does not move. A
/// geometry the target output cannot hold is refused with `invalid_layout`
/// and the panel stays where it is (spec "Panel commands").
pub(super) fn answer_set_output(routing: &Rc<Routing>, requested: &str) -> Result<(), ErrorReply> {
    let tracked = routing.globals().tracked_output_names();
    let global = match set_output_decision(requested, &tracked) {
        SetOutputDecision::Pin(global) => global,
        SetOutputDecision::Unknown => {
            return Err(ErrorReply::new(
                ErrorKind::InvalidValue,
                format!("no output named '{requested}'"),
            ));
        }
    };
    if let Some(size) = routing.globals().output_size(global)
        && let Err(error) = routing.geometry_against(size).validate(size)
    {
        return Err(ErrorReply::new(ErrorKind::InvalidLayout, error.to_string()));
    }
    routing.set_output_name(Some(requested.to_owned()));
    redock(routing, Some(global));
    Ok(())
}

/// The panel's layer surface handler (D4, D11): the compositor's configure
/// becomes the app's `xdg_toplevel.configure` and `xdg_surface.configure`
/// with the same serial, and `closed` re-docks the panel window on a new
/// layer surface with no output instead of closing the app's window.
/// `handle_configure` and the handler shape are copied from wl-paper's
/// `paper.rs` `ProxyZwlrLayerSurfaceV1` (GPL-3.0).
struct ProxyLayerSurface {
    routing: Weak<Routing>,
    xdg_surface: Rc<XdgSurface>,
    xdg_toplevel: Rc<XdgToplevel>,
    /// The layer surface this handler answers, kept weak so the handler does
    /// not keep its own object alive.
    layer_surface: Weak<ZwlrLayerSurfaceV1>,
}

impl ZwlrLayerSurfaceV1Handler for ProxyLayerSurface {
    fn delete_id(&mut self, _slf: &Rc<ZwlrLayerSurfaceV1>) {
        // Drop this handler first: it holds the app's toplevel and the
        // panel's xdg_surface, so unsetting it breaks the handler cycle
        // (wl-paper), and then free the app's toplevel id, which died with
        // the layer surface the compositor destroyed.
        if let Some(layer_surface) = self.layer_surface.upgrade() {
            layer_surface.unset_handler();
        }
        self.xdg_toplevel.delete_id();
    }

    fn handle_configure(
        &mut self,
        _slf: &Rc<ZwlrLayerSurfaceV1>,
        serial: u32,
        width: u32,
        height: u32,
    ) {
        let states = toplevel_states(&self.xdg_toplevel);
        if self.xdg_toplevel.version() >= XdgToplevel::MSG__CONFIGURE_BOUNDS__SINCE {
            self.xdg_toplevel
                .send_configure_bounds(width.cast_signed(), height.cast_signed());
        }
        self.xdg_toplevel
            .send_configure(width.cast_signed(), height.cast_signed(), &states);
        self.xdg_surface.send_configure(serial);
    }

    fn handle_closed(&mut self, _slf: &Rc<ZwlrLayerSurfaceV1>) {
        // D11 (spec "Monitor unplugged"): the panel's output went away. The
        // app keeps its window: the proxy re-docks it on a new layer surface
        // with no output, and the strip follows, instead of forwarding a
        // close to the app.
        if let Some(routing) = self.routing.upgrade() {
            redock(&routing, None);
        }
    }
}

/// The tiled and constrained states the panel window is configured with (D4,
/// copied from wl-paper): the app may lay its content out as a tiled window,
/// and each state is only sent when its version supports it. The states are
/// a native-endian `u32` array, as the protocol requires.
fn toplevel_states(toplevel: &XdgToplevel) -> Vec<u8> {
    // Eight states at four bytes each: the tiled and constrained sets.
    let mut states = Vec::with_capacity(8 * 4);
    if toplevel.version() >= XdgToplevel::ENM__STATE_TILED_TOP__SINCE {
        for state in [
            XdgToplevelState::TILED_LEFT,
            XdgToplevelState::TILED_TOP,
            XdgToplevelState::TILED_RIGHT,
            XdgToplevelState::TILED_BOTTOM,
        ] {
            states.extend_from_slice(&state.0.to_ne_bytes());
        }
    }
    if toplevel.version() >= XdgToplevel::ENM__STATE_CONSTRAINED_TOP__SINCE {
        for state in [
            XdgToplevelState::CONSTRAINED_LEFT,
            XdgToplevelState::CONSTRAINED_TOP,
            XdgToplevelState::CONSTRAINED_RIGHT,
            XdgToplevelState::CONSTRAINED_BOTTOM,
        ] {
            states.extend_from_slice(&state.0.to_ne_bytes());
        }
    }
    states
}

#[cfg(test)]
mod tests {
    use super::{SetOutputDecision, set_output_decision};

    fn tracked() -> Vec<(u32, Option<String>)> {
        vec![
            (10, Some("DP-1".to_owned())),
            (11, Some("DP-2".to_owned())),
            (12, None),
        ]
    }

    /// A name one tracked output carries pins the panel to that output's
    /// global (D11, spec "Opens on the chosen output"): the right output is
    /// picked among several, whatever the announcement order was.
    #[test]
    fn a_known_name_pins_its_output() {
        let tracked = tracked();
        assert_eq!(
            set_output_decision("DP-2", &tracked),
            SetOutputDecision::Pin(11)
        );
        assert_eq!(
            set_output_decision("DP-1", &tracked),
            SetOutputDecision::Pin(10)
        );
    }

    /// A name no tracked output carries is refused and the panel does not
    /// move (spec "Unknown output"): an unknown name, a nameless output and
    /// an empty output list all refuse.
    #[test]
    fn an_unknown_name_is_refused() {
        assert_eq!(
            set_output_decision("DP-3", &tracked()),
            SetOutputDecision::Unknown,
            "a name no output carries does not move the panel"
        );
        assert_eq!(
            set_output_decision("DP-2", &[]),
            SetOutputDecision::Unknown,
            "no tracked outputs at all refuses the request"
        );
        assert_eq!(
            set_output_decision("DP-2", &[(12, None)]),
            SetOutputDecision::Unknown,
            "an output that has not announced a name yet matches nothing"
        );
    }
}
