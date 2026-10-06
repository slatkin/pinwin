//! The sctk handler trait impls (split from the state module in row 8.1's
//! dispatch D1): the impls the connection's event queue dispatches into,
//! each body under the shared guard (D5), plus the registry and dispatch
//! delegates the queue needs.

use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::delegate_dispatch2;
use smithay_client_toolkit::delegate_registry;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::registry_handlers;
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, QueueHandle};

use crate::guard::{guard, guard_always};

use super::PanelState;

impl CompositorHandler for PanelState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // The integer preferred buffer scale is the fallback scale source
        // (D5); the fractional preferred scale arrives through the
        // `wp_fractional_scale_v1` dispatch in `buffers`.
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || self.note_integer_scale(new_factor));
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
        surface: &wl_surface::WlSurface,
        time: u32,
    ) {
        // The tween's frame callbacks arrive here (row 6.2): the panel
        // surface's step the driver and commit the eased frame, the
        // reserve's change nothing. Guarded like every handler (D5).
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || {
            let is_panel = self
                .session
                .as_ref()
                .and_then(|session| session.surfaces.as_ref())
                .is_some_and(|surfaces| surfaces.is_panel_surface(surface));
            if is_panel {
                super::super::tween_draw::on_tween_frame(self, time);
            }
        });
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
