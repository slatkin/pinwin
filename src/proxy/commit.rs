//! The commit coordinator (replace-terminal-with-app-host D7): the proxy
//! must not commit state the app has not finished, so a commit the proxy
//! needs is either injected on a clean surface or queued for the app's next
//! commit.
//!
//! The app's surface counts as dirty from any request on it that sets pending
//! state after the app's last commit (D7). [`PanelSurfaceCommits`] records
//! those requests — the double-buffered state of `attach`, `set_buffer_scale`,
//! `set_buffer_transform`, `set_opaque_region`, `set_input_region` and
//! `offset`, the pending damage of `damage` and `damage_buffer`, and the
//! app's own `wp_viewport` source and destination on the panel surface.
//! `frame` is the one request that does not count: it carries no surface
//! state and the compositor fires its callback on repaint, so an idle pacer
//! that re-requests the callback without committing would hold the queue
//! forever. `destroy` needs no mark either, because a destroyed surface never
//! commits. While the
//! surface is dirty, a proxy change (the tween's frame step, the focus
//! ring's position, hide and show, a keyboard mode change) waits in a FIFO
//! queue, and
//! [`CommitCoordinator::on_app_commit`] applies the queue in order just
//! before the app's commit is forwarded: the piggyback. On a clean surface
//! the change is applied at once and the proxy sends its own commit: the
//! inject. The coordinator never commits while the surface is dirty.
//!
//! A change is an opaque closure, [`Change`], so this module stays free of
//! the panel's surfaces and protocol state, and its dirty-flag and queue
//! tests run without a compositor (`port-to-rust` D10).
//! [`PanelSurfaceCommits`] is the coordinator's place on the app's panel
//! surface: the `wl_surface` handler the routing installs when the window
//! takes the slot ([`super::routing`]). It watches those requests and the
//! app's commit, and forwards each one exactly as the generated default
//! handler does.
//!
//! Hide, show and the keyboard mode changes request a change (rows 8.2 and
//! 8.3), and so does the width tween (row 10.1).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt;
use std::rc::{Rc, Weak};

use wl_proxy::object::ObjectUtils;
use wl_proxy::protocols::wayland::wl_buffer::WlBuffer;
use wl_proxy::protocols::wayland::wl_callback::WlCallback;
use wl_proxy::protocols::wayland::wl_output::{WlOutput, WlOutputTransform};
use wl_proxy::protocols::wayland::wl_region::WlRegion;
use wl_proxy::protocols::wayland::wl_surface::{WlSurface, WlSurfaceHandler};

use super::hide::{Attach, BufferRelease};
use super::keyboard;
use super::mapping::keyboard_of;
use super::output::PanelSurfaceWatch;
use super::routing::Routing;
use super::viewport;

/// One state change the proxy wants applied to the app's surface with a
/// commit (D7): a tween frame, the ring's position, hide or show. It runs
/// when the coordinator places it; a change must not request another change,
/// because the placement it sees would be a half-applied commit.
pub type Change = Box<dyn FnOnce()>;

/// The dirty flag and the queue of one app surface (D7).
#[derive(Default)]
pub struct CommitCoordinator {
    /// Whether the app has pending state since its last commit: the proxy
    /// must not send a commit of its own while it is set.
    dirty: Cell<bool>,
    /// The changes waiting for the app's next commit, in request order.
    queued: RefCell<VecDeque<Change>>,
}

impl CommitCoordinator {
    /// An empty coordinator for a surface the app has committed (D7).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the app's surface has pending state the proxy must not
    /// commit (D7).
    #[cfg(test)]
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    /// Marks the app's surface dirty (D7): an app request arrived after its
    /// last commit.
    pub fn mark_dirty(&self) {
        self.dirty.set(true);
    }

    /// Places `change` for the app's surface (D7): a clean surface applies it
    /// now and sends the proxy's own commit with `commit` (inject); a dirty
    /// one queues it for the app's next commit (piggyback) and sends
    /// nothing.
    pub fn request(&self, change: Change, commit: impl FnOnce()) {
        if self.dirty.get() {
            self.queued.borrow_mut().push_back(change);
            return;
        }
        change();
        commit();
    }

    /// The app's commit (D7): applies the queued changes in order, clears the
    /// dirty flag and only then sends the app's commit through `commit`, so
    /// the proxy's state and the app's arrive in the same frame.
    pub fn on_app_commit(&self, commit: impl FnOnce()) {
        let queued: Vec<Change> = self.queued.borrow_mut().drain(..).collect();
        for change in queued {
            change();
        }
        self.dirty.set(false);
        commit();
    }
}

impl fmt::Debug for CommitCoordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommitCoordinator")
            .field("dirty", &self.dirty.get())
            .field("queued", &self.queued.borrow().len())
            .finish()
    }
}

/// The coordinator on the app's panel surface (D7): the `wl_surface` handler
/// the routing installs when the window takes the slot, on top of the output
/// watch that learns which output the panel docked on (D9).
pub struct PanelSurfaceCommits {
    /// The app's panel surface, kept weak so the handler does not keep its
    /// own object alive (the routing's handler convention).
    surface: Weak<WlSurface>,
    /// The panel's routing: the hidden state and the buffer handlers read it
    /// (row 8.2, D6).
    routing: Rc<Routing>,
    /// The D9 output watch the panel surface carried before this handler: a
    /// handler that dropped it would lose the panel's output.
    watch: PanelSurfaceWatch,
    coordinator: CommitCoordinator,
    /// Whether the launch withholds the on-demand keyboard until the first
    /// buffer maps the surface (row 8.3, spec "No focus steal on launch").
    /// A show or a `set-keyboard` clears it, because each sends the mode
    /// itself.
    defer_keyboard: Cell<bool>,
    /// Whether the current commit carries a buffer the proxy forwarded: the
    /// first such commit maps the launch's layer surface.
    forwarded_buffer: bool,
}

impl PanelSurfaceCommits {
    /// The commits handler for the app's panel surface.
    pub(super) fn new(routing: &Rc<Routing>, surface: &Rc<WlSurface>) -> Self {
        Self {
            surface: Rc::downgrade(surface),
            routing: Rc::clone(routing),
            watch: PanelSurfaceWatch::new(routing, surface),
            coordinator: CommitCoordinator::new(),
            defer_keyboard: Cell::new(keyboard::deferred_until_map(routing.keyboard())),
            forwarded_buffer: false,
        }
    }

    /// Clears the launch deferral (row 8.3): a show or a `set-keyboard` sends
    /// the mode itself, so the first map must not send it again.
    pub(super) fn clear_keyboard_deferral(&self) {
        self.defer_keyboard.set(false);
    }

    /// Whether the launch deferral is still pending (row 8.3): a re-dock that
    /// lands before the first forwarded buffer must keep withholding the
    /// keyboard, so its replacement surface steals no focus (spec "No focus
    /// steal on launch").
    pub(super) fn keyboard_deferred(&self) -> bool {
        self.defer_keyboard.get()
    }

    /// Applies a deferred on-demand mode after the first forwarded buffer
    /// has mapped the surface (row 8.3, spec "No focus steal on launch",
    /// "Click to type"): the map itself carried `none`, so it took no focus,
    /// and the proxy's own commit that follows sets the mode the clicks need.
    fn apply_launch_keyboard(&mut self, surface: &Rc<WlSurface>) {
        if !self.defer_keyboard.replace(false) {
            return;
        }
        let keyboard = self.routing.keyboard();
        if !keyboard::takes_keyboard_when_mapped(keyboard) {
            return;
        }
        let Some(layer_surface) = self.routing.panel_layer_surface() else {
            return;
        };
        self.coordinator.request(
            Box::new(move || {
                layer_surface.send_set_keyboard_interactivity(keyboard_of(keyboard));
            }),
            {
                let surface = Rc::clone(surface);
                move || surface.send_commit()
            },
        );
    }

    /// Marks the panel surface dirty for an app request on another object
    /// that changes its double-buffered state (D7): the app's `wp_viewport`
    /// source and destination belong to the surface it was created for.
    pub(super) fn mark_dirty(&self) {
        self.coordinator.mark_dirty();
    }

    /// Places `change` on the panel surface (D7): the `commit` is the proxy's
    /// own, sent only when the surface is clean.
    pub fn request(&self, change: Change) {
        let Some(surface) = self.surface() else {
            return;
        };
        self.coordinator.request(change, || surface.send_commit());
    }

    /// The coordinated surface, when the app still has it: the requests are
    /// forwarded through the handler's own weak handle, as the output watch
    /// does.
    fn surface(&self) -> Option<Rc<WlSurface>> {
        self.surface.upgrade()
    }
}

/// Marks the panel surface dirty for an app request on another object that
/// changes its double-buffered state (D7): the app's `wp_viewport` source and
/// destination belong to the surface it was created for.
pub(super) fn mark_app_viewport_dirty(routing: &Routing, surface_id: u64) {
    if !routing.is_panel_surface(surface_id) {
        return;
    }
    let Some((surface, _, _)) = routing.panel_window() else {
        return;
    };
    if let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>() {
        commits.mark_dirty();
    }
}

/// The generated default handler of `wl_surface` (D7). Its request methods
/// are what the surface does without the coordinator: they forward to the
/// compositor when the object is allowed to, and log a failure. The
/// coordinator hands every request it watches to this, so only the dirty
/// flag and the queue are new.
struct ForwardToServer;

impl WlSurfaceHandler for ForwardToServer {}

impl WlSurfaceHandler for PanelSurfaceCommits {
    fn handle_enter(&mut self, _slf: &Rc<WlSurface>, output: &Rc<WlOutput>) {
        // The panel's output tracking is the D9 watch's own; a handler that
        // dropped it would lose the size the crop resolves against.
        if let Some(surface) = self.surface() {
            WlSurfaceHandler::handle_enter(&mut self.watch, &surface, output);
        }
    }

    fn handle_attach(
        &mut self,
        _slf: &Rc<WlSurface>,
        buffer: Option<&Rc<WlBuffer>>,
        x: i32,
        y: i32,
    ) {
        self.coordinator.mark_dirty();
        let Some(surface) = self.surface() else {
            return;
        };
        // D6: a hidden panel forwards no attach and returns every buffer it
        // removed to the app, so the app does not run out of buffers.
        let decision = self.routing.hidden.on_app_attach(buffer);
        self.forwarded_buffer = buffer.is_some() && decision == Attach::Forward;
        if let Some(buffer) = buffer {
            // D6 amendment 3: a release for a proxy-made attach is swallowed.
            buffer.set_handler(BufferRelease::new(&self.routing, buffer));
        }
        if decision == Attach::Forward {
            ForwardToServer.handle_attach(&surface, buffer, x, y);
        }
    }

    fn handle_damage(&mut self, _slf: &Rc<WlSurface>, x: i32, y: i32, width: i32, height: i32) {
        self.coordinator.mark_dirty();
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_damage(&surface, x, y, width, height);
        }
    }

    fn handle_frame(&mut self, _slf: &Rc<WlSurface>, callback: &Rc<WlCallback>) {
        // D7: a frame callback carries no surface state and the compositor
        // fires it on repaint even when the app never commits, so it must not
        // leave the surface dirty.
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_frame(&surface, callback);
        }
    }

    fn handle_set_opaque_region(&mut self, _slf: &Rc<WlSurface>, region: Option<&Rc<WlRegion>>) {
        self.coordinator.mark_dirty();
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_set_opaque_region(&surface, region);
        }
    }

    fn handle_set_input_region(&mut self, _slf: &Rc<WlSurface>, region: Option<&Rc<WlRegion>>) {
        self.coordinator.mark_dirty();
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_set_input_region(&surface, region);
        }
    }

    fn handle_set_buffer_transform(&mut self, _slf: &Rc<WlSurface>, transform: WlOutputTransform) {
        self.coordinator.mark_dirty();
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_set_buffer_transform(&surface, transform);
        }
    }

    fn handle_offset(&mut self, _slf: &Rc<WlSurface>, x: i32, y: i32) {
        self.coordinator.mark_dirty();
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_offset(&surface, x, y);
        }
    }

    fn handle_damage_buffer(
        &mut self,
        _slf: &Rc<WlSurface>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
        self.coordinator.mark_dirty();
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_damage_buffer(&surface, x, y, width, height);
        }
    }

    fn handle_set_buffer_scale(&mut self, _slf: &Rc<WlSurface>, scale: i32) {
        self.coordinator.mark_dirty();
        // D8: the viewporter's source rectangle is in the coordinates after
        // the buffer scale, so the crop's bound is the buffer divided by it.
        self.routing
            .globals()
            .viewport()
            .buffers()
            .set_panel_scale(scale);
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_set_buffer_scale(&surface, scale);
        }
    }

    fn handle_commit(&mut self, _slf: &Rc<WlSurface>) {
        // The queued changes apply just before the app's own commit is
        // forwarded (D7): one commit carries both, and the dirty flag clears
        // with it. The hidden state decides what the commit carries (D6):
        // held syncobj points only with a buffer, and the releases the
        // dropped attaches owe the app after it.
        let Some(surface) = self.surface() else {
            return;
        };
        self.coordinator.on_app_commit(|| {
            let plan = self.routing.hidden.commit_plan();
            // D8: the crop is clamped to the buffer the compositor holds
            // after this commit, whose size the proxy knows from the request
            // that created it. While hidden that is the buffer show attaches,
            // not the app's attach, which never reaches the compositor (D6).
            self.routing
                .globals()
                .viewport()
                .buffers()
                .set_panel_buffer(self.routing.hidden.compositor_buffer());
            if let Some(points) = plan.points {
                self.routing.hidden.emit_points(&points);
            }
            // D8: a grow starts moving once this commit reports the target
            // size, so the crop below holds the docked part of the wide
            // buffer from this frame on.
            super::tween::on_app_commit(&self.routing);
            // D8: the viewport state applies with the app's own commit, so
            // the composed crop rides this one. The buffer the commit
            // attaches is what the crop is bounded by, so the values are
            // composed here rather than at the attach.
            viewport::recompose(&self.routing);
            ForwardToServer.handle_commit(&surface);
            for buffer in plan.release {
                buffer.send_release();
            }
        });
        if std::mem::take(&mut self.forwarded_buffer) {
            self.apply_launch_keyboard(&surface);
        }
    }
}

impl fmt::Debug for PanelSurfaceCommits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PanelSurfaceCommits")
            .field("coordinator", &self.coordinator)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use wl_proxy::baseline::Baseline;
    use wl_proxy::fixed::Fixed;
    use wl_proxy::object::{ObjectCoreApi, ObjectUtils};
    use wl_proxy::protocols::viewporter::wp_viewport::{WpViewport, WpViewportHandler};
    use wl_proxy::protocols::viewporter::wp_viewporter::{WpViewporter, WpViewporterHandler};
    use wl_proxy::protocols::wayland::wl_buffer::{WlBuffer, WlBufferHandler};
    use wl_proxy::protocols::wayland::wl_callback::WlCallback;
    use wl_proxy::protocols::wayland::wl_output::WlOutputTransform;
    use wl_proxy::protocols::wayland::wl_region::WlRegion;
    use wl_proxy::protocols::wayland::wl_surface::{WlSurface, WlSurfaceHandler};
    use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
    use wl_proxy::protocols::xdg_shell::xdg_surface::XdgSurface;
    use wl_proxy::protocols::xdg_shell::xdg_toplevel::XdgToplevel;
    use wl_proxy::state::State;

    use super::{Change, CommitCoordinator, PanelSurfaceCommits};
    use crate::config::PanelSettings;
    use crate::proxy::hide::BufferRelease;
    use crate::proxy::routing::Routing;
    use crate::proxy::viewport::{ClientWpViewport, ClientWpViewporter};

    /// The `wl_surface` version a test mints its object with.
    const SURFACE_VERSION: u32 = 6;

    /// The `wl_buffer` version a test mints its object with.
    const BUFFER_VERSION: u32 = 1;

    /// The order in which changes and commits happen (D7).
    fn recorder() -> Rc<RefCell<Vec<&'static str>>> {
        Rc::new(RefCell::new(Vec::new()))
    }

    /// A change that records its own name when the coordinator applies it.
    fn change(log: &Rc<RefCell<Vec<&'static str>>>, name: &'static str) -> Change {
        let log = Rc::clone(log);
        Box::new(move || log.borrow_mut().push(name))
    }

    /// A commit that records itself: the proxy's own (inject) or the app's
    /// forwarded one (piggyback).
    fn commit(log: &Rc<RefCell<Vec<&'static str>>>) -> impl FnOnce() {
        let log = Rc::clone(log);
        move || log.borrow_mut().push("commit")
    }

    /// The commits handler on a minted `wl_surface` and that surface, from a
    /// state the test can mint more objects in: the state has no server, so
    /// nothing is sent anywhere and no compositor is needed
    /// (`port-to-rust` D10).
    fn commits() -> (Rc<State>, Rc<WlSurface>, PanelSurfaceCommits) {
        let state = State::builder(Baseline::V6)
            .without_server()
            .build()
            .expect("a client-side state needs no compositor");
        let surface = state.create_object::<WlSurface>(SURFACE_VERSION);
        let routing = Routing::new(PanelSettings::built_in());
        let handler = PanelSurfaceCommits::new(&routing, &surface);
        (state, surface, handler)
    }

    /// A clean surface injects: the change applies at once and the proxy
    /// sends one commit of its own (D7).
    #[test]
    fn a_clean_surface_injects_the_change_and_one_commit() {
        let coordinator = CommitCoordinator::new();
        let log = recorder();
        coordinator.request(change(&log, "change"), commit(&log));
        assert_eq!(*log.borrow(), ["change", "commit"]);
    }

    /// A dirty surface queues: nothing applies and nothing is sent, then the
    /// queued change applies before the app's commit (D7).
    #[test]
    fn a_dirty_surface_queues_until_the_app_commits() {
        let coordinator = CommitCoordinator::new();
        let log = recorder();
        coordinator.mark_dirty();
        coordinator.request(change(&log, "queued"), commit(&log));
        assert!(
            log.borrow().is_empty(),
            "a dirty surface applies no change and sends no commit"
        );
        coordinator.on_app_commit(commit(&log));
        assert_eq!(*log.borrow(), ["queued", "commit"]);
    }

    /// Two queued changes keep their request order at the app's commit (D7).
    #[test]
    fn two_queued_changes_keep_their_order() {
        let coordinator = CommitCoordinator::new();
        let log = recorder();
        coordinator.mark_dirty();
        coordinator.request(change(&log, "first"), commit(&log));
        coordinator.request(change(&log, "second"), commit(&log));
        assert!(log.borrow().is_empty(), "both changes wait");
        coordinator.on_app_commit(commit(&log));
        assert_eq!(*log.borrow(), ["first", "second", "commit"]);
    }

    /// The app's commit clears the dirty flag, so the next change injects
    /// again (D7).
    #[test]
    fn the_app_commit_clears_the_dirty_flag() {
        let coordinator = CommitCoordinator::new();
        let log = recorder();
        coordinator.mark_dirty();
        coordinator.request(change(&log, "queued"), commit(&log));
        coordinator.on_app_commit(commit(&log));
        assert!(!coordinator.is_dirty(), "the app's commit leaves it clean");
        log.borrow_mut().clear();
        coordinator.request(change(&log, "after"), commit(&log));
        assert_eq!(*log.borrow(), ["after", "commit"]);
    }

    /// Every request the app sends on the panel surface that sets pending
    /// state leaves it dirty until the app's commit (D7): the double-buffered
    /// state, the pending damage, the offset, and the region and transform
    /// setters.
    #[test]
    fn every_app_request_marks_the_surface_dirty() {
        type Request = fn(&mut PanelSurfaceCommits, &Rc<State>, &Rc<WlSurface>);
        let requests: [(&str, Request); 8] = [
            ("attach", |handler, _state, surface| {
                handler.handle_attach(surface, None, 0, 0);
            }),
            ("damage", |handler, _state, surface| {
                handler.handle_damage(surface, 0, 0, 1, 1);
            }),
            ("set_opaque_region", |handler, state, surface| {
                let region = state.create_object::<WlRegion>(1);
                handler.handle_set_opaque_region(surface, Some(&region));
            }),
            ("set_input_region", |handler, _state, surface| {
                handler.handle_set_input_region(surface, None);
            }),
            ("set_buffer_transform", |handler, _state, surface| {
                handler.handle_set_buffer_transform(surface, WlOutputTransform(0));
            }),
            ("set_buffer_scale", |handler, _state, surface| {
                handler.handle_set_buffer_scale(surface, 1);
            }),
            ("damage_buffer", |handler, _state, surface| {
                handler.handle_damage_buffer(surface, 0, 0, 1, 1);
            }),
            ("offset", |handler, _state, surface| {
                handler.handle_offset(surface, 0, 0);
            }),
        ];
        for (name, request) in requests {
            let (state, surface, mut handler) = commits();
            assert!(!handler.coordinator.is_dirty(), "a fresh surface is clean");
            request(&mut handler, &state, &surface);
            assert!(
                handler.coordinator.is_dirty(),
                "wl_surface.{name} leaves pending state"
            );
        }
    }

    /// `wl_surface.frame` sets no surface state, so it must not leave the
    /// surface dirty (D7): the compositor fires the callback on repaint even
    /// when the app never commits, and an app stopped after a frame request
    /// would otherwise hold every queued proxy change forever.
    #[test]
    fn an_app_frame_request_leaves_the_surface_clean() {
        let (state, surface, mut handler) = commits();
        let callback = state.create_object::<WlCallback>(1);
        handler.handle_frame(&surface, &callback);
        assert!(
            !handler.coordinator.is_dirty(),
            "wl_surface.frame sets no pending state"
        );
    }

    /// The app's commit is the only thing that clears the surface's dirty flag
    /// (D7): every other request keeps the proxy waiting for it.
    #[test]
    fn the_app_commit_clears_the_surface_dirty_flag() {
        let (_state, surface, mut handler) = commits();
        handler.handle_set_input_region(&surface, None);
        assert!(
            handler.coordinator.is_dirty(),
            "the request leaves it dirty"
        );
        handler.handle_commit(&surface);
        assert!(
            !handler.coordinator.is_dirty(),
            "the app's commit leaves it clean"
        );
    }

    /// The app's own `wp_viewport` source and destination on the panel surface
    /// are double-buffered surface state (D7): either leaves it dirty until the
    /// app commits, so a proxy commit cannot drag the values out early.
    #[test]
    fn an_app_viewport_request_on_the_panel_marks_the_surface_dirty() {
        type Request = fn(&mut ClientWpViewport, &Rc<WpViewport>);
        let requests: [(&str, Request); 2] = [
            ("set_source", |handler, viewport| {
                handler.handle_set_source(
                    viewport,
                    Fixed::from_f64_lossy(0.0),
                    Fixed::from_f64_lossy(0.0),
                    Fixed::from_f64_lossy(10.0),
                    Fixed::from_f64_lossy(10.0),
                );
            }),
            ("set_destination", |handler, viewport| {
                handler.handle_set_destination(viewport, 10, 10);
            }),
        ];
        for (name, request) in requests {
            let (state, surface, handler) = commits();
            let routing = Rc::clone(&handler.routing);
            let xdg_surface = state.create_object::<XdgSurface>(1);
            let toplevel = state.create_object::<XdgToplevel>(1);
            let layer_surface = state.create_object::<ZwlrLayerSurfaceV1>(4);
            routing.set_panel_window(&surface, &xdg_surface, &toplevel, &layer_surface);
            // The viewport path reaches the coordinator through the surface's
            // installed handler, as the routing installs it (D7).
            surface.set_handler(handler);

            let viewporter = state.create_object::<WpViewporter>(1);
            let viewport = state.create_object::<WpViewport>(1);
            let mut app_viewporter = ClientWpViewporter::new(&viewporter, &routing);
            WpViewporterHandler::handle_get_viewport(
                &mut app_viewporter,
                &viewporter,
                &viewport,
                &surface,
            );
            let commits = surface
                .try_get_handler_ref::<PanelSurfaceCommits>()
                .expect("the surface carries the commit handler");
            assert!(!commits.coordinator.is_dirty(), "a fresh surface is clean");
            let mut app_viewport = viewport
                .try_get_handler_mut::<ClientWpViewport>()
                .expect("the app viewport carries its local handler");
            request(&mut app_viewport, &viewport);
            assert!(
                commits.coordinator.is_dirty(),
                "wp_viewport.{name} leaves pending state"
            );
        }
    }

    /// Destroying the app's viewport on the panel surface changes the
    /// surface's pending state too: the protocol removes the surface's crop
    /// and scale on its next commit, and the recompose after the destroy
    /// writes the proxy's own crop (D7, D8). A proxy commit sent before that
    /// would draw the whole buffer for a frame.
    #[test]
    fn an_app_viewport_destroy_on_the_panel_marks_the_surface_dirty() {
        let (state, surface, handler) = commits();
        let routing = Rc::clone(&handler.routing);
        let xdg_surface = state.create_object::<XdgSurface>(1);
        let toplevel = state.create_object::<XdgToplevel>(1);
        let layer_surface = state.create_object::<ZwlrLayerSurfaceV1>(4);
        routing.set_panel_window(&surface, &xdg_surface, &toplevel, &layer_surface);
        surface.set_handler(handler);

        let viewporter = state.create_object::<WpViewporter>(1);
        let viewport = state.create_object::<WpViewport>(1);
        let mut app_viewporter = ClientWpViewporter::new(&viewporter, &routing);
        WpViewporterHandler::handle_get_viewport(
            &mut app_viewporter,
            &viewporter,
            &viewport,
            &surface,
        );
        let commits = surface
            .try_get_handler_ref::<PanelSurfaceCommits>()
            .expect("the surface carries the commit handler");
        assert!(!commits.coordinator.is_dirty(), "a fresh surface is clean");
        let mut app_viewport = viewport
            .try_get_handler_mut::<ClientWpViewport>()
            .expect("the app viewport carries its local handler");
        app_viewport.handle_destroy(&viewport);
        assert!(
            commits.coordinator.is_dirty(),
            "wp_viewport.destroy leaves pending state"
        );
    }

    /// A buffer the panel surface attached still frees its size entry when
    /// the app destroys it (D8, D6 amendment 3): the attach swaps in the
    /// release handler, and a handler that dropped the destroy bookkeeping
    /// would keep an entry for every buffer the app ever drew into the panel.
    #[test]
    fn a_panel_buffer_leaves_the_size_map_when_it_is_destroyed() {
        let (state, surface, mut handler) = commits();
        let buffer = state.create_object::<WlBuffer>(BUFFER_VERSION);
        let globals = handler.routing.globals();
        let buffers = globals.viewport().buffers();
        buffers.record(&buffer, 640, 480);
        buffers.set_panel_buffer(Some(buffer.unique_id()));

        handler.handle_attach(&surface, Some(&buffer), 0, 0);
        assert_eq!(buffers.panel_size(), Some((640, 480)));

        let mut release = buffer
            .try_get_handler_mut::<BufferRelease>()
            .expect("the panel attach installs the release handler");
        WlBufferHandler::handle_destroy(&mut *release, &buffer);
        assert_eq!(
            buffers.panel_size(),
            None,
            "a destroyed panel buffer leaves the size map"
        );
    }

    /// While hidden the crop is bounded by the buffer show attaches, not the
    /// one the app attached and the proxy dropped (D6, D8): the compositor
    /// never received that attach, and a source past the buffer it holds is
    /// the fatal `out_of_buffer`.
    #[test]
    fn a_hidden_commit_bounds_the_crop_by_the_kept_buffer() {
        let (state, surface, mut handler) = commits();
        handler.routing.hidden.set_hidden(true);
        let globals = handler.routing.globals();
        let buffers = globals.viewport().buffers();
        let kept = state.create_object::<WlBuffer>(BUFFER_VERSION);
        let dropped = state.create_object::<WlBuffer>(BUFFER_VERSION);
        buffers.record(&kept, 640, 480);
        buffers.record(&dropped, 800, 600);

        handler.handle_attach(&surface, Some(&kept), 0, 0);
        handler.handle_commit(&surface);
        assert_eq!(
            buffers.panel_size(),
            Some((640, 480)),
            "the buffer show attaches bounds the crop"
        );

        handler.handle_attach(&surface, Some(&dropped), 0, 0);
        handler.handle_commit(&surface);
        assert_eq!(
            buffers.panel_size(),
            Some((640, 480)),
            "a dropped hidden attach never reaches the compositor"
        );
    }
}
