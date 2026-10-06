//! The bound session's two edges (split from the state module in row
//! 8.1's dispatch D1): the bind that creates the globals, the surfaces
//! and the session, and the teardown that drops them.

use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::shell::wlr_layer::{Layer, LayerShell};
use smithay_client_toolkit::shm::Shm;
use wayland_client::QueueHandle;
use wayland_client::globals::GlobalList;

use super::super::activation::Activation;
use super::super::buffers::Scale;
use super::super::frame_log::Presentation;
use super::super::surfaces::{PanelSurfaces, SurfaceId};
use super::{BindFailure, PanelState, Session};

impl PanelState {
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
        let mut scale = Scale::bind(globals, qh);
        let presentation = Presentation::bind(globals, qh);
        let activation = Activation::bind(globals, qh);
        // The panel surface is created with no output, so the compositor
        // places it on the focused output (D3); the first enter names it.
        let surface = compositor.create_surface_with_data(qh, None, 1, SurfaceId::Panel);
        // The per-surface fractional-scale and viewport objects (D1):
        // optional globals, their absence degrades the scale, never the start.
        scale.attach(SurfaceId::Panel, &surface, qh);
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
            scale,
            presentation,
            activation,
            qh: qh.clone(),
        });
        Ok(())
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
            // The per-surface scale objects die with their surfaces, and
            // before them: dropping the objects sends their `destroy`
            // requests, so the viewport and fractional-scale objects of the
            // panel and of a still-live reserve cannot outlive the surfaces
            // they are attached to.
            session.scale.detach(SurfaceId::Panel);
            session.scale.detach(SurfaceId::Reserve);
            session.surfaces = None;
        }
        self.done = true;
    }
}
