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

use std::cell::{Cell, RefCell};
use std::os::fd::RawFd;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use smithay_client_toolkit::compositor::{CompositorState, Region};
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::shell::wlr_layer::{
    Layer, LayerShell, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::Shm;
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_output;

use crate::guard::Poisoned;
use crate::layout::{CellSize, Layout, OutputSize};
use crate::pty::apply_winsize;
use crate::surfaces::gap::{HeldGap, start_held_gap};
use crate::term::Terminal;

use super::super::handshake::{Handshake, StartOutcome};
use super::Startup;
use super::activation::Activation;
use super::buffers::{Scale, viewport_destination};
use super::frame_log::{FrameLog, Presentation};
use super::seat::SeatLinks;
use super::seat::SeatSide;
use super::sizing::{Grid, Sizing};
use super::surfaces::{PanelSurfaces, SurfaceId};
use super::tween::TweenDriver;
use super::tween_draw::{TweenDraw, TweenRender};

mod handlers;
mod seat_handlers;
mod session;

/// The globals and surfaces one bound panel thread session holds (D2). The
/// fields live here and not on [`PanelState`] because every one of them needs
/// the connection: the display-free core is constructible without any of it.
pub(crate) struct Session {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    shell: LayerShell,
    shm: Shm,
    /// The output-scale state (D5): the optional fractional/viewporter
    /// globals, the per-surface objects and the preferred-scale sources.
    /// Declared before the surfaces so an implicit drop — a thread whose
    /// loop ends without a teardown — destroys the per-surface scale
    /// objects before the surfaces they belong to, the protocol's order.
    pub(crate) scale: Scale,
    /// The optional presentation-time global (row 6.3): the frame log's
    /// on-screen timestamps when the compositor offers the protocol, the
    /// frame callbacks' times otherwise. Its absence degrades the frame
    /// log, never the start (D1).
    pub(crate) presentation: Presentation,
    /// The optional xdg-activation global (row 7.2, replace-gtk-with-wayland
    /// D4): the focus request's transport; its absence degrades the request
    /// to a no-op, never the start (D1).
    pub(crate) activation: Activation,
    /// The globals the session bound (row 8.1's seat wiring): the list the
    /// pointer's cursor-shape device binds from when the pointer capability
    /// arrives (row 5.5).
    pub(crate) globals: wayland_client::globals::GlobalList,
    /// The seat state (row 8.1's seat wiring): the `wl_seat` handler the
    /// capability events dispatch through, and the source the keyboard and
    /// the pointer are created from.
    pub(crate) seat: SeatState,
    /// The seat side the seat handlers route into (row 8.1's seat wiring):
    /// the keyboard, pointer and focus hooks rows 5.1 to 5.5 built, over
    /// the thread's links.
    pub(crate) seat_side: SeatSide,
    /// The seat's capability objects (row 8.1's seat wiring): the keyboard
    /// and the pointer the capability events created, dropped on removal,
    /// on a removed seat and at teardown.
    pub(crate) seat_objects: seat_handlers::SeatObjects,
    /// The loop handle the keyboard's repeat source installs itself
    /// through (row 5.2): `new_capability` runs from the loop's dispatch,
    /// so the handle the bind stores must reach it. `EventLoop::try_new`
    /// leaves the lifetime to the caller, and the 'static one is what the
    /// toolkit's repeat source holds.
    pub(crate) loop_handle: calloop::LoopHandle<'static, PanelState>,
    /// The two layer surfaces, `None` once the panel is torn down: the
    /// teardown drops them — the panel leaves the screen at once — and with
    /// them every configure and pty push path, while the connection and the
    /// registry/output handlers it still dispatches to stay alive until the
    /// loop ends.
    pub(crate) surfaces: Option<PanelSurfaces>,
    /// The output the panel's first `wl_surface.enter` named (D3), whose
    /// xdg-output logical size is still awaited.
    pending_output: Option<wl_output::WlOutput>,
    /// The resolved output and its xdg-output logical size (D3). `None` until
    /// both the enter and the logical size have arrived.
    pub(crate) resolved: Option<OutputSize>,
    /// The queue handle the session dispatches through (row 6.2): the tween
    /// frames' `wl_surface.frame` requests go through it, from the apply's
    /// begin frame as well as from the frame handler.
    pub(crate) qh: QueueHandle<PanelState>,
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
    pub(crate) startup: Startup,
    /// The cell the thread measured from its own font (row 8.1, D3): the
    /// surfaces' width and the grid derivation both need it from the first
    /// configure. The thread loads the font before it builds the state, so
    /// the value is the measurement, not a startup input; the tests pass a
    /// synthetic one through the same parameter.
    pub(crate) cell: CellSize,
    /// The shared terminal the thread owns (row 8.1): the pty read source
    /// feeds it, the seat links and the tween's render bundle hold clones.
    /// `None` on a headless state — the tests — and set by the production
    /// thread before the bind, so every push and feed after sees it.
    pub(crate) terminal: Option<Rc<RefCell<Terminal>>>,
    /// The repaint request the terminal's callbacks latch (row 8.1): the
    /// terminal's `queue_draw` closure sets it, and the loop reads and
    /// clears it after each dispatch, where the draw step turns it into a
    /// frame (dispatch D4c).
    pub(crate) repaint: Rc<Cell<bool>>,
    /// The panel surface's latest logical size (dispatch D4c): the last
    /// configure's, or the tween finish's final size — the frame input the
    /// live draws build from. `None` before the first configure, which is
    /// when no live frame exists yet.
    pub(crate) panel_size: Option<(u32, u32)>,
    /// The pixel width of the stale pre-resize grid still on screen after
    /// a widening push (dispatch D4c, the GTK path's `note_grid_widened`):
    /// the drawn grid the docked-edge offset keys against until the
    /// terminal produces output for the new width, which clears it. Shared
    /// with the byte path's output closure, which clears it.
    pub(crate) stale_grid_px: Rc<Cell<i32>>,
    /// The docked-edge draw offset the latest drawn frame used, snapped to
    /// a whole device pixel (dispatch D4c): the value the pointer mapping
    /// adjusts pointer x by, published per frame through the seat links'
    /// shared cell.
    pub(crate) draw_offset: Rc<Cell<f64>>,
    /// The seat links the thread built from its pieces (row 8.1, dispatch
    /// D4c): the draw reads the focus flag the accent draws from through
    /// them, and dispatch D5 wires the Wayland handlers to the same value.
    /// `None` on a headless state — the tests.
    pub(crate) seat_links: Option<SeatLinks>,
    pub(crate) sizing: Sizing,
    /// The held gap the reserve surface draws (overlay-expand D2, D5):
    /// seeded from the startup layout's own choice — a pushing start holds
    /// its own strip, a covering start holds zero — and moved only by a
    /// validated apply (row 3.5, `crate::surfaces::gap`).
    pub(crate) held: HeldGap,
    /// The applied layout (row 3.5): the last staged apply's layout, whose
    /// side and gutters the tween's finish geometry reads. A tween only runs
    /// between layouts that match in side and left/right gutters, so the
    /// finish restores the applied margins exactly.
    pub(crate) applied: Layout,
    /// The width tween's driver (row 6.1, [`super::tween`]): the frame
    /// callbacks and the watchdog timer drive it from row 6.2's animated
    /// apply on.
    pub(crate) tween: TweenDriver,
    /// The running tween's wide cache (row 6.2, [`super::tween_draw`]):
    /// `Some` exactly while a tween runs and its wide buffer is presentable,
    /// dropped when the tween stops.
    pub(crate) tween_draw: Option<TweenDraw>,
    /// The render state the tween's wide draw reads (row 6.2,
    /// [`super::tween_draw`]): the terminal and the handle to the thread's
    /// one renderer. The thread fills it at start (row 8.1), so an animated
    /// apply draws its wide cache instead of snapping.
    pub(crate) render: Option<TweenRender>,
    pub(crate) session: Option<Session>,
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
    /// here. `cell` is the cell the thread measured from its font (D3,
    /// row 8.1); without a cell no configure could derive a grid and the
    /// panel would silently never map, so it is a required input.
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
            terminal: None,
            repaint: Rc::new(Cell::new(false)),
            panel_size: None,
            stale_grid_px: Rc::new(Cell::new(0)),
            draw_offset: Rc::new(Cell::new(0.0)),
            seat_links: None,
            sizing: Sizing::new(startup.layout.cols(), cell),
            held: start_held_gap(startup.layout, cell.width().get()),
            applied: startup.layout,
            tween: TweenDriver::default(),
            tween_draw: None,
            render: None,
            session: None,
        }
    }

    /// A closed compositor connection maps the panel onto the dead state
    /// (D2): the handle stops posting (`NotRunning` without blocking), and a
    /// still-pending start handshake fails — the panel never went live, the
    /// same mapping the GTK side's loop-returned path reports.
    pub(crate) fn connection_closed(&mut self) {
        self.inner.live.store(false, Ordering::Relaxed);
        self.handshake.report(StartOutcome::NoDisplay);
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
            session.scale.attach(SurfaceId::Reserve, &surface, qh);
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
        // The push sink the configures drive (row 8.1): it owns clones of
        // the shared terminal and the repaint flag, so the session borrow
        // below does not alias it.
        let mut push = self.grid_sink();
        let Some(session) = &mut self.session else {
            return;
        };
        // A torn-down session has no surfaces left, so no late configure can
        // reach the pty push.
        if session.surfaces.is_none() {
            return;
        }
        let (width, height) = configure.new_size;
        // Which surface the configure names, decided while the session
        // borrow is alive; the arms re-borrow per step, because the panel's
        // draw step needs the rest of the state while it runs.
        let is_panel = session
            .surfaces
            .as_ref()
            .is_some_and(|surfaces| surfaces.is_panel(layer));
        let is_reserve = !is_panel
            && session
                .surfaces
                .as_ref()
                .is_some_and(|surfaces| surfaces.is_reserve(layer));
        if is_panel {
            if self.tween_draw.is_some() {
                // A tween owns the panel surface's size, viewport and buffer
                // commits until it finishes (row 6.2): this configure only
                // runs the `on-demand` switch and records the height for the
                // deferred grid push — the sizing's defer mode holds the
                // push until the finish — while the tween frames keep
                // committing the cached height, the same staleness the GTK
                // path had; the tween's finish and the next configure catch
                // up.
                self.panel_size = Some((width, height));
                if let Some(surfaces) = self
                    .session
                    .as_mut()
                    .and_then(|session| session.surfaces.as_mut())
                {
                    surfaces.panel_configured_under_tween();
                }
                self.configure_grid(height, &mut push);
                return;
            }
            // The viewporter destination is the logical size (D5): set
            // before the buffer commit the configure handler makes. A
            // zero-sized configure sets no destination — the viewport
            // protocol's fatal `bad_value` covers a zero or negative
            // dimension — and the panel simply stays unmapped until a real
            // configure arrives.
            if let Some(session) = self.session.as_mut()
                && let Some((width_i, height_i)) = viewport_destination(width, height)
            {
                session
                    .scale
                    .set_destination(SurfaceId::Panel, width_i, height_i);
            }
            // The configure draws the frame it maps the panel with
            // (dispatch D4c): the renderer's canvas resizes to the device
            // size — which invalidates the gate — and the full frame goes
            // into a pool buffer at that device size, committed against
            // the destination above. The `on-demand` switch follows the
            // first buffer commit, so it runs only when one landed.
            self.panel_size = Some((width, height));
            let drawn = self.draw_frame_at(width, height);
            if drawn == super::present::ServiceOutcome::Drawn
                && let Some(surfaces) = self
                    .session
                    .as_mut()
                    .and_then(|session| session.surfaces.as_mut())
            {
                surfaces.panel_configured();
            }
            self.configure_grid(height, &mut push);
        } else if is_reserve && let Some(session) = self.session.as_mut() {
            if let Some((width_i, height_i)) = viewport_destination(width, height) {
                session
                    .scale
                    .set_destination(SurfaceId::Reserve, width_i, height_i);
            }
            if let Some(surfaces) = session.surfaces.as_mut() {
                surfaces.reserve_configured(width, height);
            }
        }
    }

    /// The grid size decision for one configure height (D3, row 3.3): the
    /// rows derive from the height, the columns from the applied layout, and
    /// the push goes to the supplied sink — the thread's grid push (row
    /// 8.1) — only when the derived grid changed. The sink is a parameter so
    /// the display-free tests can observe the pushes the state drives
    /// (`port-to-rust` D10).
    pub(crate) fn configure_grid(&mut self, height: u32, push: &mut dyn FnMut(Grid)) {
        self.sizing.configure(height, push);
    }

    /// Take the repaint request the terminal's callbacks latched (row 8.1):
    /// the loop reads and clears it after each dispatch, where the draw
    /// step (dispatch D4c) turns it into a frame.
    pub(crate) fn take_repaint_request(&self) -> bool {
        self.repaint.replace(false)
    }

    /// Move the renderer to the session's resolved scale (D5): called after
    /// the bind — the preferred scale arrives with the surfaces' events,
    /// dispatched only from the loop on — and from the two scale-note
    /// handlers, so the metrics the frames and the wide draw read follow
    /// the compositor's preferred scale.
    pub(crate) fn sync_renderer_scale(&mut self) {
        let Some(scale) = self
            .session
            .as_ref()
            .map(|session| session.scale.resolved())
        else {
            return;
        };
        if let Some(render) = &self.render {
            render.renderer.borrow_mut().set_scale(scale.as_f64());
        }
    }

    /// Note the integer preferred buffer scale the compositor reported for
    /// one surface: the fallback source of the resolution order (D5), read
    /// only while no fractional preferred scale has arrived.
    pub(crate) fn note_integer_scale(&mut self, factor: i32) {
        if let Some(session) = &mut self.session {
            session.scale.note_integer(factor);
        }
        self.sync_renderer_scale();
    }

    /// Note the fractional preferred scale the `wp_fractional_scale_v1`
    /// object of one surface reported, in 1/120 units: the primary source
    /// of the resolution order (D5).
    pub(crate) fn note_preferred_scale(&mut self, units_120: u32) {
        if let Some(session) = &mut self.session {
            session.scale.note_preferred_scale(units_120);
        }
        self.sync_renderer_scale();
    }

    /// The frame log a new tween begins with (row 6.3,
    /// [`super::frame_log`]): `Some` only under `PINWIN_FRAMELOG=1`,
    /// sourced from the presentation-time protocol when that global is
    /// bound and from the frame callbacks otherwise. The environment is
    /// read once per tween, as the GTK path read it once per begin.
    #[must_use]
    pub(crate) fn new_frame_log(&self) -> Option<FrameLog> {
        let presentation = self
            .session
            .as_ref()
            .is_some_and(|session| session.presentation.is_bound());
        FrameLog::begin(FrameLog::enabled(), presentation)
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
            // The compositor closed the reserve: its viewport and
            // fractional-scale objects die with it, before the surface the
            // drop inside `reserve_closed` destroys, so reserve churn leaks
            // neither.
            session.scale.detach(SurfaceId::Reserve);
            surfaces.reserve_closed();
        }
    }
}

/// Apply a pushed grid to the pty: the winsize ioctl with the grid and its
/// pixel size, `SIGWINCH` raised inside [`apply_winsize`] on success. The
/// single grid push sink in [`super::glue`] runs this after the terminal's
/// own `push_size`, so the terminal and the pty change together (row 8.1);
/// the child's `TIOCGWINSZ` reads the result.
pub(crate) fn apply_pty_size(fd: RawFd, grid: Grid) {
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
