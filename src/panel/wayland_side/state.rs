//! The panel thread's Wayland dispatch state (replace-gtk-with-wayland D2,
//! D3): the sctk handlers the connection's event queue dispatches into, the
//! bound session's globals and the two layer surfaces, and the grid sizing
//! the configures drive.
//!
//! The state exists in two phases. [`PanelState::headless`] builds the
//! display-free core — the D5 latch, the start handshake, the loop-end flag
//! and the handle state — before any Wayland object exists, which is what the
//! command-handling tests run against (`port-to-rust` D10).
//! [`PanelState::bind`] then binds the globals (D1: `wl_compositor`,
//! `wl_shm` and `zwlr_layer_shell_v1` required, their absence is the spec's
//! "no display" case), creates the panel surface and stores the session. From
//! the bind on, the queue dispatches surface, output and layer events into
//! the handlers below; nothing dispatches before it, so the accessors that
//! reach into the session can only run bound.
//!
//! Panics never cross back into calloop or the compositor (D5): every
//! handler body runs under the shared [`crate::guard`] helpers with the
//! panel's one shared latch, and the stop relays — a compositor-closed panel
//! surface, like a teardown — run under [`guard_always`] so a latched panel
//! still ends.

use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::delegate_dispatch2;
use smithay_client_toolkit::delegate_registry;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::registry_handlers;
use smithay_client_toolkit::shell::wlr_layer::{
    Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, QueueHandle};

use crate::guard::{Poisoned, guard, guard_always};
use crate::layout::{CellSize, OutputSize};
use crate::pty::apply_winsize;

use super::super::handshake::{Handshake, StartOutcome};
use super::Startup;
use super::sizing::{Grid, Sizing};
use super::surfaces::{PanelSurfaces, SurfaceId};

/// The globals and surfaces one bound panel thread session holds (D2). The
/// fields live here and not on [`PanelState`] because every one of them needs
/// the connection: the display-free core is constructible without any of it.
pub(crate) struct Session {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    shell: LayerShell,
    shm: Shm,
    /// The two layer surfaces, `None` once the panel is torn down: the
    /// teardown drops them — the panel leaves the screen at once — and with
    /// them every configure and pty push path, while the connection and the
    /// registry/output handlers it still dispatches to stay alive until the
    /// loop ends.
    surfaces: Option<PanelSurfaces>,
    /// The output the panel's first `wl_surface.enter` named (D3), whose
    /// xdg-output logical size is still awaited.
    pending_output: Option<wl_output::WlOutput>,
    /// The resolved output and its xdg-output logical size (D3). `None` until
    /// both the enter and the logical size have arrived.
    resolved: Option<OutputSize>,
}

/// The wayland panel thread's dispatch state: the display-free core every
/// command and event runs against, plus the session the bind stores. The
/// sctk handler traits are implemented on this type because the connection's
/// event queue dispatches into it.
pub(crate) struct PanelState {
    pub(crate) poisoned: Poisoned,
    pub(crate) handshake: Handshake,
    /// Set by the teardown command (or a closed command channel or a
    /// compositor-closed panel): end the loop and let the thread return.
    pub(crate) done: bool,
    pub(crate) inner: Arc<super::Inner>,
    startup: Startup,
    /// The measured cell metrics the start command carried (D3): the
    /// surfaces' width and the grid derivation both need them from the
    /// first configure, so a required startup input instead of a later
    /// arrival.
    cell: CellSize,
    sizing: Sizing,
    session: Option<Session>,
}

/// Why a start's bind failed: a required global is missing (the spec's
/// "Wayland layer-shell is required" case, D1), or the session failed in a
/// way no layout verdict or missing display explains (D5's internal class).
pub(crate) enum BindFailure {
    NoDisplay,
    Internal,
}

impl PanelState {
    /// The display-free core, before the session is bound (D10): the command
    /// tests and the pre-bind window of [`super::run_thread`] both start
    /// here. `cell` is the measured cell metrics the start command carried
    /// (D3); without them no configure could derive a grid and the panel
    /// would silently never map, so they are a required input.
    pub(crate) fn headless(
        handshake: Handshake,
        poisoned: Poisoned,
        inner: Arc<super::Inner>,
        startup: Startup,
        cell: CellSize,
    ) -> Self {
        PanelState {
            poisoned,
            handshake,
            done: false,
            inner,
            startup,
            cell,
            sizing: Sizing::new(startup.layout.cols(), cell),
            session: None,
        }
    }

    /// Bind the session (D1, D2): the required globals, the output state and
    /// the panel surface's creation and initial commit. A missing required
    /// global is [`BindFailure::NoDisplay`]; a failed pool or a caught panic
    /// is [`BindFailure::Internal`] (D5 latches the shared flag — the caller
    /// runs this under the guard).
    pub(crate) fn bind(
        &mut self,
        globals: &GlobalList,
        qh: &QueueHandle<Self>,
    ) -> Result<(), BindFailure> {
        let compositor =
            CompositorState::bind(globals, qh).map_err(|_bind| BindFailure::NoDisplay)?;
        let shm = Shm::bind(globals, qh).map_err(|_bind| BindFailure::NoDisplay)?;
        let shell = LayerShell::bind(globals, qh).map_err(|_bind| BindFailure::NoDisplay)?;
        // The panel surface is created with no output, so the compositor
        // places it on the focused output (D3); the first enter names it.
        let surface = compositor.create_surface_with_data(qh, None, 1, SurfaceId::Panel);
        let panel = shell.create_layer_surface(qh, surface, Layer::Overlay, Some("pinwin"), None);
        let surfaces = PanelSurfaces::new(
            panel,
            &shm,
            self.startup.layout,
            self.startup.keyboard,
            self.cell,
        )
        .map_err(|_pool| BindFailure::Internal)?;
        self.session = Some(Session {
            registry: RegistryState::new(globals),
            outputs: OutputState::new(globals, qh),
            compositor,
            shell,
            shm,
            surfaces: Some(surfaces),
            pending_output: None,
            resolved: None,
        });
        Ok(())
    }

    /// A closed compositor connection maps the panel onto the dead state
    /// (D2): the handle stops posting (`NotRunning` without blocking), and a
    /// still-pending start handshake fails — the panel never went live, the
    /// same mapping the GTK side's loop-returned path reports.
    pub(crate) fn connection_closed(&mut self) {
        self.inner.live.store(false, Ordering::Relaxed);
        self.handshake.report(StartOutcome::NoDisplay);
    }

    /// Tear the panel down: drop the two layer surfaces and end the loop.
    /// The teardown command, the closed command channel and the startup
    /// watchdog's fire all end here, so the panel leaves the screen at once
    /// and no later configure can push a pty size — the only push path runs
    /// through the configure handling the dropped surfaces take with them.
    /// Nothing here can panic, so the stop relays call it outside the guard
    /// and the loop still ends on a latched flag.
    pub(crate) fn tear_down(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.surfaces = None;
        }
        self.done = true;
    }

    /// The panel surface entered an output (D3): the first enter names the
    /// panel's output. The reserve is created on it and the start handshake
    /// completes once the output's xdg-output logical size is known.
    fn on_panel_enter(&mut self, output: &wl_output::WlOutput, qh: &QueueHandle<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.resolved.is_some() {
            return;
        }
        session.pending_output = Some(output.clone());
        self.try_resolve_output(qh);
    }

    /// Resolve the pending output (D3): with the xdg-output logical size in
    /// hand, create the reserve on the same output and complete the start
    /// handshake. Called again from the output handlers, because the logical
    /// size can arrive after the enter.
    fn try_resolve_output(&mut self, qh: &QueueHandle<Self>) {
        let Some(output) = self
            .session
            .as_ref()
            .and_then(|session| session.pending_output.clone())
        else {
            return;
        };
        let Some(session) = &mut self.session else {
            return;
        };
        // The info arrives with the output's wl_output/xdg-output events; a
        // missing one means the compositor has not finished describing the
        // output yet, and a later update retries.
        let Some(info) = session.outputs.info(&output) else {
            return;
        };
        // A torn-down session (the watchdog's fire dropped the surfaces)
        // attaches no reserve and completes no handshake: its start already
        // failed and its loop is already ending.
        let Some(surfaces) = session.surfaces.as_mut() else {
            return;
        };
        // A logical size of zero is a compositor that has not decided its
        // layout yet: the same retry as a missing info.
        let Some((width, height)) = info.logical_size else {
            return;
        };
        let Some(size) = OutputSize::new(width, height) else {
            return;
        };
        // The reserve is created on the same output (D3). A region the
        // compositor refused degrades to no reservation: the panel still
        // runs, the tiles just are not held back (port-to-rust D3 — the
        // library never exits over an environment failure).
        if let Ok(region) = Region::new(&session.compositor) {
            let surface =
                session
                    .compositor
                    .create_surface_with_data(qh, None, 1, SurfaceId::Reserve);
            let reserve = session.shell.create_layer_surface(
                qh,
                surface,
                Layer::Bottom,
                Some("pinwin-reserve"),
                Some(&output),
            );
            surfaces.attach_reserve(reserve, region);
        }
        session.pending_output = None;
        session.resolved = Some(size);
        // The start handshake completes here (D3), like the GTK side's
        // first-draw monitor resolution: the panel's output and its logical
        // size are live.
        self.handshake.report(StartOutcome::Started);
    }

    /// One layer-surface configure (rows 3.3 and 3.4): the panel's configure
    /// maps the surface and drives the grid sizing; the reserve's maps the
    /// reservation.
    fn on_configure(&mut self, layer: &LayerSurface, configure: &LayerSurfaceConfigure) {
        // The pty fd the pushes reach: copied before the session borrow, the
        // way the push closure below captures it.
        let fd = self.startup.fd;
        let Some(session) = &mut self.session else {
            return;
        };
        // A torn-down session has no surfaces left, so no late configure can
        // reach the pty push.
        let Some(surfaces) = session.surfaces.as_mut() else {
            return;
        };
        let (width, height) = configure.new_size;
        if surfaces.is_panel(layer) {
            surfaces.panel_configured(width, height);
            self.configure_grid(height, &mut |grid| apply_pty_size(fd, grid));
        } else if surfaces.is_reserve(layer) {
            surfaces.reserve_configured(width, height);
        }
    }

    /// The grid size decision for one configure height (D3, row 3.3): the
    /// rows derive from the height, the columns from the applied layout, and
    /// the push goes to the supplied sink — the pty winsize in production —
    /// only when the derived grid changed. The sink is a parameter so the
    /// display-free tests can observe the pushes the state drives
    /// (`port-to-rust` D10).
    fn configure_grid(&mut self, height: u32, push: &mut dyn FnMut(Grid)) {
        self.sizing.configure(height, push);
    }

    /// The compositor closed a layer surface. The panel's surface ending is
    /// a stop relay (D5): the panel is dead, a pending start fails and the
    /// loop ends even on a latched flag. The reserve's ending only drops the
    /// reservation; the panel stays.
    fn on_layer_closed(&mut self, layer: &LayerSurface) {
        let Some(session) = &mut self.session else {
            return;
        };
        // A torn-down session has no surfaces left to close.
        let Some(surfaces) = session.surfaces.as_mut() else {
            return;
        };
        if surfaces.is_panel(layer) {
            // The compositor destroyed the panel: the panel is dead (D2).
            self.inner.live.store(false, Ordering::Relaxed);
            // A resolved handshake drops this; a pending one fails, the same
            // mapping a loop-returned thread reports.
            self.handshake.report(StartOutcome::NoDisplay);
            self.done = true;
        } else if surfaces.is_reserve(layer) {
            surfaces.reserve_closed();
        }
    }
}

/// Apply a pushed grid to the pty: the winsize ioctl with the grid and its
/// pixel size, `SIGWINCH` raised inside [`apply_winsize`] on success. The
/// terminal's grid push joins here when the terminal moves onto this thread
/// (replace-gtk-with-wayland row 8.1); until then the pty carries the size
/// alone, which is what the child's `TIOCGWINSZ` reads.
fn apply_pty_size(fd: RawFd, grid: Grid) {
    let Ok(rows) = i32::try_from(grid.rows()) else {
        return;
    };
    let (Ok(cell_w), Ok(cell_h)) = (
        u32::try_from(grid.cell_width()),
        u32::try_from(grid.cell_height()),
    ) else {
        return;
    };
    let _ = apply_winsize(fd, i32::from(grid.cols()), rows, cell_w, cell_h);
}

impl CompositorHandler for PanelState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
        // The renderer redraws at the new scale (row 4.x); nothing draws yet.
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
        // The renderer consumes the transform with the scale (row 4.x).
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // The tween's frame callbacks arrive here (row 6.1); no frames are
        // requested yet.
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || {
            // The panel's first enter names its output (D3); the reserve's
            // enters arrive after the resolution and change nothing.
            let is_panel = self
                .session
                .as_ref()
                .and_then(|session| session.surfaces.as_ref())
                .is_some_and(|surfaces| surfaces.is_panel_surface(surface));
            if is_panel {
                self.on_panel_enter(output, qh);
            }
        });
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
        // The panel stays on its original monitor (the spec's "Dock at one
        // edge of the panel's monitor"); a leave has no state to undo.
    }
}

impl OutputHandler for PanelState {
    fn output_state(&mut self) -> &mut OutputState {
        // Only reachable once the session is bound: outputs are bound at the
        // bind and their events dispatch through the queue the bind inserted,
        // so no event can name an output while the session is `None`.
        &mut self
            .session
            .as_mut()
            .expect("output events dispatch only after the bind")
            .outputs
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || {
            // A new output's description may complete a pending resolution.
            self.try_resolve_output(qh);
        });
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || {
            // The pending output's logical size may have arrived (D3).
            self.try_resolve_output(qh);
        });
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // The compositor closes the layer surfaces when their output goes
        // away; the `closed` handler maps that onto the dead panel.
    }
}

impl ShmHandler for PanelState {
    fn shm_state(&mut self) -> &mut Shm {
        // Same bound-session argument as `output_state` above.
        &mut self
            .session
            .as_mut()
            .expect("wl_shm events dispatch only after the bind")
            .shm
    }
}

impl LayerShellHandler for PanelState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        // A stop relay (D5): the panel's death must end the loop even on a
        // latched flag, or the thread leaks its connection.
        let poisoned = self.poisoned.clone();
        let _ = guard_always(&poisoned, || self.on_layer_closed(layer));
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || self.on_configure(layer, &configure));
    }
}

impl ProvidesRegistryState for PanelState {
    fn registry(&mut self) -> &mut RegistryState {
        // Same bound-session argument as `output_state` above: the registry
        // exists from the bind on, and registry events dispatch only after
        // the queue that carries them is inserted.
        &mut self
            .session
            .as_mut()
            .expect("registry events dispatch only after the bind")
            .registry
    }

    registry_handlers![OutputState];
}

delegate_registry!(PanelState);

delegate_dispatch2!(PanelState);

#[cfg(test)]
mod tests {
    use super::super::surfaces::{panel_anchor, panel_margins};
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::layout::{Keyboard, Side};
    use crate::panel::handshake::wait_for_start;
    use std::num::NonZeroU16;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    /// A startup for the tests; the thread does not touch the pty fd until
    /// the surfaces push a grid, so a placeholder fd is fine here.
    fn startup() -> Startup {
        Startup {
            fd: -1,
            layout: crate::layout::Layout::new(
                Side::Left,
                NonZeroU16::new(40).expect("test columns"),
                0,
                0,
                0,
                0,
            ),
            keyboard: Keyboard::OnDemand,
            accent: None,
        }
    }

    /// The startup cell metrics the tests carry, the way a start command
    /// would (D3): required, because a metrics-less state never pushes.
    fn cell() -> CellSize {
        CellSize::new(9, 16).expect("test cell size is non-zero")
    }

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<super::super::Inner> {
        Arc::new(super::super::Inner {
            id: 0,
            poisoned: GuardPoisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        })
    }

    fn headless_state(handshake: Handshake) -> PanelState {
        PanelState::headless(handshake, Poisoned::new(), live_inner(), startup(), cell())
    }

    /// A closed compositor connection maps the panel onto the dead state
    /// (D2): the handle stops posting, and a still-pending start handshake
    /// fails with `NoDisplay`.
    #[test]
    fn a_closed_connection_marks_the_panel_dead() {
        let (tx, rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        state.connection_closed();
        assert!(
            !state.inner.live.load(Ordering::Relaxed),
            "the panel is dead"
        );
        assert_eq!(
            wait_for_start(&rx),
            Err(crate::panel::PinwinError::NoDisplay),
            "a still-pending start fails"
        );
    }

    /// A closed connection after the start handshake was resolved marks the
    /// panel dead and adds no second report: the first one is the only one.
    #[test]
    fn a_closed_connection_after_a_resolved_start_reports_nothing_new() {
        let (tx, rx) = mpsc::channel();
        let handshake = Handshake::new(tx);
        handshake.report(StartOutcome::Started);
        let mut state = headless_state(handshake);
        state.connection_closed();
        assert!(
            !state.inner.live.load(Ordering::Relaxed),
            "the panel is dead"
        );
        assert_eq!(
            rx.try_recv().expect("the first report"),
            StartOutcome::Started
        );
        assert!(
            rx.try_recv().is_err(),
            "the closed connection adds no second report"
        );
    }

    /// The panel's configure drives the grid sizing through the real push
    /// seam (row 3.3): the startup metrics the start command carries reach
    /// the state's sizing, so a configure derives and pushes the startup grid
    /// through the state's [`PanelState::configure_grid`]. The pushes the
    /// sizing decides are observed through the same sink the pty winsize
    /// path fills in production.
    #[test]
    fn a_configure_with_the_startup_metrics_pushes_the_startup_grid() {
        let (tx, _rx) = mpsc::channel();
        let mut state = headless_state(Handshake::new(tx));
        let mut pushed: Vec<Grid> = Vec::new();
        state.configure_grid(1080, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.len(), 1, "the configure pushes the startup grid");
        let grid = pushed[0];
        assert_eq!(grid.cols(), 40, "the columns come from the startup layout");
        assert_eq!(grid.rows(), 1080 / 16, "the rows come from the height");
        assert_eq!(
            (grid.cell_width(), grid.cell_height()),
            (9, 16),
            "the startup metrics reached the sizing"
        );
    }

    /// The pure anchor and margin mappings the bind applies are the
    /// docking-side ones (D3); the tests live in the surfaces module beside
    /// them. Here only the headless core's shape is pinned: no session, no
    /// pushed grid.
    #[test]
    fn a_headless_state_holds_no_session() {
        let (tx, _rx) = mpsc::channel();
        let state = headless_state(Handshake::new(tx));
        assert!(state.session.is_none(), "headless means unbound");
        assert!(!state.done, "a headless state still runs");
    }

    /// The bind applies the startup geometry to the panel surface it
    /// creates (row 3.1). The mapping itself is tested in the surfaces
    /// module; this pins the handlers' surface matching helpers against the
    /// anchor they are derived from.
    #[test]
    fn the_surface_geometry_helpers_stay_aligned() {
        // The bind sets these through `PanelSurfaces::new`; the values must
        // stay the docking-side ones.
        assert_eq!(panel_margins(startup().layout), (0, 0, 0, 0));
        assert_eq!(
            panel_anchor(startup().layout.side()),
            panel_anchor(Side::Left)
        );
    }
}
