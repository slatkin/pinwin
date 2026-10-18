//! The app's `xdg_surface` handler for one panel (replace-terminal-with-app-host
//! D4): it holds the surface back until its toplevel role appears, then makes
//! the first toplevel the panel's layer surface or forwards every later one
//! unchanged.
//!
//! The mapping between the held `xdg_surface` and the layer surface — the
//! hold-back here, the configure and ack mapping in [`super::super::dock`] —
//! is copied from wl-paper's `apps/wl-paper/src/paper.rs` in
//! <https://github.com/mahkoh/wl-proxy> (GPL-3.0, the licence of this
//! crate). The first dock waits for an output size when a percent size needs
//! one ([`super::super::dock::size`], D9): the toplevel stays local and the
//! panel loop, an output event, an `enter` or the wait's deadline resumes it,
//! so the panel never maps at an invented size and never waits indefinitely.
//!
//! `_slf` names the handler parameters the proxy's own object would carry; a
//! handler that needs its own object keeps it in a `Weak` field, so the
//! xdg-surface handler closes no reference cycle.

use std::rc::Rc;

use wl_proxy::object::{Object, ObjectCoreApi, ObjectUtils};
use wl_proxy::protocols::wayland::wl_surface::WlSurface;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
use wl_proxy::protocols::xdg_shell::xdg_popup::XdgPopup;
use wl_proxy::protocols::xdg_shell::xdg_positioner::XdgPositioner;
use wl_proxy::protocols::xdg_shell::xdg_surface::{XdgSurface, XdgSurfaceHandler};
use wl_proxy::protocols::xdg_shell::xdg_toplevel::XdgToplevel;
use wl_proxy::protocols::xdg_shell::xdg_wm_base::XdgWmBase;

use super::super::commit::PanelSurfaceCommits;
use super::super::dock;
use super::super::dock::size::DockDecision;
use super::super::mapping::layer_of;
use super::super::popup::{ClientXdgPopup, ClientXdgPositioner, offset_anchor_rect};
use super::super::viewport::{apply_steady_crop, settle_forwarded};
use super::super::window::ClientXdgToplevel;
use super::{Route, Routing};
use crate::daemon::panel::AppId;
use crate::geometry::Geometry;

/// One app `xdg_surface` (D4): held back until its toplevel role appears,
/// then either the panel's layer surface or a forwarded normal window.
pub(in crate::proxy) struct ClientXdgSurface {
    routing: Rc<Routing>,
    wl_surface: Rc<WlSurface>,
    xdg_surface: Rc<XdgSurface>,
    xdg_wm_base: Rc<XdgWmBase>,
    /// Whether the compositor has this `xdg_surface` as an upstream object:
    /// false while the surface is held back, true after a pass-through
    /// forward.
    has_server: bool,
    /// The window geometry the app set on this surface, `[x, y, width,
    /// height]` (D5): a popup of the panel window offsets its anchor
    /// rectangle by the panel window's one, and the panel window's buffer is
    /// cropped to it through a proxy-owned viewport (D8). A surface the app
    /// set none on keeps `None`.
    geometry: Option<[i32; 4]>,
    /// The hosted app's `app_id`, recorded when this surface becomes the
    /// panel window (row 11.1, D14).
    app_id: AppId,
    /// The panel toplevel whose first dock waits for an output size (D9),
    /// kept local until [`ClientXdgSurface::resume`] docks or forwards it.
    pending_dock: Option<Rc<XdgToplevel>>,
}

impl ClientXdgSurface {
    /// A held-back surface whose role is not yet known (D4): it carries no
    /// layer surface, has nothing upstream and no window geometry.
    pub(in crate::proxy) fn held_back(
        routing: Rc<Routing>,
        wl_surface: Rc<WlSurface>,
        xdg_surface: Rc<XdgSurface>,
        xdg_wm_base: Rc<XdgWmBase>,
        app_id: AppId,
    ) -> Self {
        Self {
            routing,
            wl_surface,
            xdg_surface,
            xdg_wm_base,
            has_server: false,
            geometry: None,
            app_id,
            pending_dock: None,
        }
    }

    /// The panel window's current layer surface when this surface is the
    /// panel window (D4, D11): the routing's live reference, so popups and
    /// acks stay on the surface that is actually docked across a re-dock.
    fn panel_layer_surface(&self) -> Option<Rc<ZwlrLayerSurfaceV1>> {
        if !self.routing.is_panel_surface(self.wl_surface.unique_id()) {
            return None;
        }
        self.routing.panel_layer_surface()
    }

    /// Forwards this surface and its new toplevel to the compositor unchanged
    /// (D4: `get_toplevel` while the slot is full). This surface can no
    /// longer be the panel's, so an app viewport it created before the
    /// window mapping settles now instead of waiting for the slot (D8).
    fn forward(&mut self, id: &Rc<XdgToplevel>) {
        self.xdg_wm_base
            .send_get_xdg_surface(&self.xdg_surface, &self.wl_surface);
        self.has_server = true;
        self.xdg_surface.send_get_toplevel(id);
        settle_forwarded(&self.routing, self.wl_surface.unique_id());
    }

    /// Forwards a toplevel that waited for its output size (D9): the local
    /// handler and the local role go, so the window becomes the normal window
    /// the proxy forwards.
    fn forward_pending(&mut self, id: &Rc<XdgToplevel>) {
        id.unset_handler();
        id.set_forward_to_server(true);
        self.forward(id);
    }

    /// Makes the panel toplevel local to the proxy (D4): the app sees a
    /// normal window, but every request on this role is answered by the proxy,
    /// and its `app_id` reaches the tray.
    fn prepare_panel(&self, id: &Rc<XdgToplevel>) {
        if id.version() >= XdgToplevel::MSG__WM_CAPABILITIES__SINCE {
            id.send_wm_capabilities(&[]);
        }
        id.set_forward_to_server(false);
        id.set_handler(ClientXdgToplevel::new(
            Rc::clone(&self.routing),
            id,
            id.unique_id(),
            self.app_id.clone(),
        ));
    }

    /// Forgets the waiting dock and frees the slot it held (D9).
    fn abandon_pending(&mut self, id: &Rc<XdgToplevel>) {
        self.pending_dock = None;
        self.routing.clear_pending_dock(id.unique_id());
        self.routing.release(id.unique_id());
    }

    /// Resumes a waiting first dock (D9): a known output size docks the
    /// window, an invalid geometry forwards it as a normal window, and a wait
    /// that reached its deadline takes any known output size or forwards the
    /// window when no output reported one. Nothing happens while the wait is
    /// still open.
    pub(super) fn resume(&mut self, routing: &Rc<Routing>, timed_out: bool) {
        let Some(id) = self.pending_dock.clone() else {
            return;
        };
        let Some(layer_shell) = routing.layer_shell() else {
            self.abandon_pending(&id);
            self.forward_pending(&id);
            return;
        };
        let decision = match routing.dock_decision() {
            ready @ DockDecision::Ready { .. } => ready,
            DockDecision::Wait if timed_out => {
                let Some(decision) = routing.fallback_dock_decision() else {
                    self.abandon_pending(&id);
                    self.forward_pending(&id);
                    return;
                };
                decision
            }
            DockDecision::Wait => return,
        };
        let DockDecision::Ready { geometry, output } = decision else {
            return;
        };
        if let Some(output) = output
            && let Err(error) = geometry.validate(output)
        {
            eprintln!(
                "pinwin: the panel geometry cannot apply: {error}; the window opens as a normal window"
            );
            self.abandon_pending(&id);
            self.forward_pending(&id);
            return;
        }
        self.pending_dock = None;
        routing.clear_pending_dock(id.unique_id());
        self.take_slot(&layer_shell, &id, geometry);
        // The app's initial commit may have gone upstream while the window
        // waited, so the layer surface's state needs one commit of its own to
        // reach the compositor. The coordinator injects it on the clean
        // surface (D7); without it the app would wait for a configure that
        // never comes.
        if let Ok(commits) = self.wl_surface.try_get_handler_ref::<PanelSurfaceCommits>() {
            commits.request(Box::new(|| {}));
        }
    }

    /// Turns this surface into the panel's layer surface (D4: `get_toplevel`
    /// while the slot is empty). Copied from wl-paper's `paper.rs`
    /// `ClientXdgSurface::handle_get_toplevel` (GPL-3.0); the geometry comes
    /// from pinwin's settings, and the layer surface's creation and handler
    /// live in [`super::super::dock`] (D11).
    fn take_slot(
        &mut self,
        layer_shell: &Rc<ZwlrLayerShellV1>,
        id: &Rc<XdgToplevel>,
        geometry: Geometry,
    ) {
        let window = dock::PanelWindow {
            surface: Rc::clone(&self.wl_surface),
            xdg_surface: Rc::clone(&self.xdg_surface),
            toplevel: Rc::clone(id),
        };
        // D11: the settings' output pins the panel when the proxy has bound
        // it; otherwise the compositor chooses.
        dock::dock_panel_window(&self.routing, layer_shell, &window, &geometry);
        // D9, D11: the reserve surface is created with the panel window and
        // owns the exclusive zone the panel surface does not claim, pinned
        // to the panel's output when one is set.
        dock::start_reserve(
            &self.routing,
            &geometry,
            layer_of(self.routing.layer()),
            self.routing.pinned_output().as_ref(),
        );
        // D9: the surface watch learns which output the compositor docked
        // the panel on; the D7 commits handler sits on top of it and sees
        // each pending-state request and the app's commit.
        self.wl_surface
            .set_handler(PanelSurfaceCommits::new(&self.routing, &self.wl_surface));
        // D9: the dock's size is the resolution a later output event compares
        // against, so the panel does not re-send an unchanged size when its
        // own output's size arrives.
        self.routing
            .set_last_resolved_across(Some(geometry.across()));
        // A window geometry the app set before its toplevel crops from here.
        apply_steady_crop(&self.routing, self.geometry);
    }
}

impl XdgSurfaceHandler for ClientXdgSurface {
    fn handle_destroy(&mut self, _slf: &Rc<XdgSurface>) {
        if let Some(id) = self.pending_dock.take() {
            self.abandon_pending(&id);
        }
        self.xdg_surface.unset_handler();
        if self.has_server {
            self.xdg_surface.send_destroy();
            self.has_server = false;
        } else {
            self.xdg_surface.delete_id();
        }
    }

    fn handle_get_toplevel(&mut self, _slf: &Rc<XdgSurface>, id: &Rc<XdgToplevel>) {
        let Some(layer_shell) = self.routing.layer_shell() else {
            self.forward(id);
            return;
        };
        let owner = id.core().client();
        if self.routing.route(id.unique_id(), owner.as_ref()) == Route::PassedThrough {
            self.forward(id);
            return;
        }
        match self.routing.dock_decision() {
            DockDecision::Ready { geometry, output } => {
                // D9: validate before applying. A geometry the output cannot
                // hold never docks — the window passes through as a normal
                // window and the slot stays empty (spec "Validate a change
                // before applying it"). A pixel size with no output yet has
                // nothing to validate against.
                if let Some(output) = output
                    && let Err(error) = geometry.validate(output)
                {
                    eprintln!(
                        "pinwin: the panel geometry cannot apply: {error}; the window opens as a normal window"
                    );
                    self.routing.release(id.unique_id());
                    self.forward(id);
                    return;
                }
                self.prepare_panel(id);
                self.take_slot(&layer_shell, id, geometry);
            }
            DockDecision::Wait => {
                // D9: a percent size waits for its output instead of docking
                // at an invented one; an output event, the panel's `enter`
                // or the wait's deadline resumes it.
                self.prepare_panel(id);
                self.pending_dock = Some(Rc::clone(id));
                self.routing.defer_dock(&self.xdg_surface, id.unique_id());
            }
        }
    }

    fn handle_set_window_geometry(
        &mut self,
        _slf: &Rc<XdgSurface>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
        // The window geometry drives the popup anchor offset (D5) and the
        // panel window's viewport crop (D8); every surface keeps
        // what the app asked for, and a held surface sends nothing upstream
        // yet.
        self.geometry = Some([x, y, width, height]);
        if self.has_server {
            self.xdg_surface
                .send_set_window_geometry(x, y, width, height);
        }
        // D5/D8: the panel window's buffer is cropped to the window geometry
        // through the proxy-owned viewport, so a client-side title bar and
        // shadow stay outside the panel. The values are double-buffered
        // viewport state: they apply with the app's next commit (D7 is row
        // 8.1).
        if self.panel_layer_surface().is_some() {
            apply_steady_crop(&self.routing, self.geometry);
        }
    }

    fn handle_get_popup(
        &mut self,
        _slf: &Rc<XdgSurface>,
        id: &Rc<XdgPopup>,
        parent: Option<&Rc<XdgSurface>>,
        positioner: &Rc<XdgPositioner>,
    ) {
        // The popup's parent decides its route (D5): a popup of the panel
        // window becomes a popup of the panel's layer surface, moved by the
        // window geometry, and every other popup is forwarded unchanged.
        // Copied from wl-paper's `paper.rs`
        // `ClientXdgSurface::handle_get_popup` (GPL-3.0); the geometry the
        // offset moves by is the one [`Self::handle_set_window_geometry`]
        // keeps.
        if !self.has_server {
            // The upstream popup is created from this surface's upstream
            // `xdg_surface`, so the held-back surface goes first.
            self.xdg_wm_base
                .send_get_xdg_surface(&self.xdg_surface, &self.wl_surface);
            self.has_server = true;
        }
        if let Some([x, y, width, height]) = self.geometry {
            self.xdg_surface
                .send_set_window_geometry(x, y, width, height);
        }
        let mut layer_surface = None;
        let mut proxy_parent = parent;
        let mut parent_geometry = None;
        if let Some(parent) = parent
            && let Ok(parent_handler) = parent.try_get_handler_ref::<ClientXdgSurface>()
            && let Some(parent_layer) = parent_handler.panel_layer_surface()
        {
            layer_surface = Some(parent_layer);
            proxy_parent = None;
            parent_geometry = parent_handler.geometry;
        }
        let anchor = positioner
            .try_get_handler_ref::<ClientXdgPositioner>()
            .ok()
            .and_then(|handler| handler.anchor_rect());
        if let Some(geometry) = parent_geometry
            && let Some([ax, ay, aw, ah]) = anchor
        {
            let [x, y, width, height] = offset_anchor_rect([ax, ay, aw, ah], Some(geometry));
            positioner.send_set_anchor_rect(x, y, width, height);
        }
        self.xdg_surface
            .send_get_popup(id, proxy_parent, positioner);
        if parent_geometry.is_some()
            && let Some([ax, ay, aw, ah]) = anchor
        {
            // The upstream positioner keeps the app's rectangle: the offset
            // was only for this popup's creation (wl-paper).
            positioner.send_set_anchor_rect(ax, ay, aw, ah);
        }
        if let Some(layer_surface) = layer_surface {
            layer_surface.send_get_popup(id);
        }
        if let Some(geometry) = parent_geometry {
            id.set_handler(ClientXdgPopup::new(id, geometry));
        }
    }

    fn handle_ack_configure(&mut self, _slf: &Rc<XdgSurface>, serial: u32) {
        // D8: a minted configure's ack is swallowed; any other ack is forwarded.
        if self.routing.globals().tween().swallow_ack(serial) {
            return;
        }
        if let Some(layer) = self.panel_layer_surface() {
            // The app acks the layer surface's configure through its local
            // xdg_surface (D4); after a re-dock it reaches the live surface
            // (D11).
            layer.send_ack_configure(serial);
            // D6: a show waits for this ack to attach the kept buffer.
            super::super::hide::on_ack(&self.routing, &self.wl_surface);
        } else if self.has_server {
            self.xdg_surface.send_ack_configure(serial);
        }
    }
}
