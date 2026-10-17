//! The commit coordinator (replace-terminal-with-app-host D7): the proxy
//! must not commit state the app has not finished, so a commit the proxy
//! needs is either injected on a clean surface or queued for the app's next
//! commit.
//!
//! The app's surface counts as dirty from any request on it after the app's
//! last commit. The spike's approximation is the four requests that change
//! double-buffered surface state — `attach`, `damage`, `damage_buffer` and
//! `set_buffer_scale` — which [`PanelSurfaceCommits`] records. While the
//! surface is dirty, a proxy change (the tween's frame step, the focus
//! ring's position, hide and show) waits in a FIFO queue, and
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
//! No consumer requests a change yet: hide and show are row 8.2 and the
//! tween is row 10.x. The module stays public until row 12.x, as
//! [`super::slot`] does, so the API those rows call does not report as dead
//! code meanwhile.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt;
use std::rc::{Rc, Weak};

use wl_proxy::protocols::wayland::wl_buffer::WlBuffer;
use wl_proxy::protocols::wayland::wl_output::WlOutput;
use wl_proxy::protocols::wayland::wl_surface::{WlSurface, WlSurfaceHandler};

use super::output::PanelSurfaceWatch;
use super::routing::Routing;

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
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    /// Marks the app's surface dirty (D7): a request that changes
    /// double-buffered state arrived after the app's last commit.
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
    /// The D9 output watch the panel surface carried before this handler: a
    /// handler that dropped it would lose the panel's output.
    watch: PanelSurfaceWatch,
    coordinator: CommitCoordinator,
}

impl PanelSurfaceCommits {
    /// The commits handler for the app's panel surface.
    pub(super) fn new(routing: &Rc<Routing>, surface: &Rc<WlSurface>) -> Self {
        Self {
            surface: Rc::downgrade(surface),
            watch: PanelSurfaceWatch::new(routing, surface),
            coordinator: CommitCoordinator::new(),
        }
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
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_attach(&surface, buffer, x, y);
        }
    }

    fn handle_damage(&mut self, _slf: &Rc<WlSurface>, x: i32, y: i32, width: i32, height: i32) {
        self.coordinator.mark_dirty();
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_damage(&surface, x, y, width, height);
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
        if let Some(surface) = self.surface() {
            ForwardToServer.handle_set_buffer_scale(&surface, scale);
        }
    }

    fn handle_commit(&mut self, _slf: &Rc<WlSurface>) {
        // The queued changes apply just before the app's own commit is
        // forwarded (D7): one commit carries both, and the dirty flag clears
        // with it.
        self.coordinator.on_app_commit(|| {
            if let Some(surface) = self.surface() {
                ForwardToServer.handle_commit(&surface);
            }
        });
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
    use wl_proxy::protocols::wayland::wl_surface::{WlSurface, WlSurfaceHandler};
    use wl_proxy::state::State;

    use super::{Change, CommitCoordinator, PanelSurfaceCommits};
    use crate::config::PanelSettings;
    use crate::proxy::routing::Routing;

    /// The `wl_surface` version a test mints its object with.
    const SURFACE_VERSION: u32 = 6;

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

    /// The commits handler on a minted `wl_surface` and that surface: the
    /// state has no server, so nothing is sent anywhere and no compositor is
    /// needed (`port-to-rust` D10).
    fn commits() -> (Rc<WlSurface>, PanelSurfaceCommits) {
        let state = State::builder(Baseline::V6)
            .without_server()
            .build()
            .expect("a client-side state needs no compositor");
        let surface = state.create_object::<WlSurface>(SURFACE_VERSION);
        let routing = Routing::new(PanelSettings::built_in());
        let handler = PanelSurfaceCommits::new(&routing, &surface);
        (surface, handler)
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

    /// Each of the four requests that change double-buffered state leaves the
    /// surface dirty (D7, spike 1.3).
    #[test]
    fn each_pending_state_request_marks_the_surface_dirty() {
        type Request = fn(&mut PanelSurfaceCommits, &Rc<WlSurface>);
        let requests: [(&str, Request); 4] = [
            ("attach", |handler, surface| {
                handler.handle_attach(surface, None, 0, 0);
            }),
            ("damage", |handler, surface| {
                handler.handle_damage(surface, 0, 0, 1, 1);
            }),
            ("damage_buffer", |handler, surface| {
                handler.handle_damage_buffer(surface, 0, 0, 1, 1);
            }),
            ("set_buffer_scale", |handler, surface| {
                handler.handle_set_buffer_scale(surface, 1);
            }),
        ];
        for (name, request) in requests {
            let (surface, mut handler) = commits();
            assert!(!handler.coordinator.is_dirty(), "a fresh surface is clean");
            request(&mut handler, &surface);
            assert!(
                handler.coordinator.is_dirty(),
                "wl_surface.{name} leaves pending state"
            );
        }
    }
}
