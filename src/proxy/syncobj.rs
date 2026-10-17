//! Explicit synchronization for the panel surface (replace-terminal-with-app-host
//! D6, row 8.2): while a panel is hidden the proxy drops the app's `attach`,
//! so the app's acquire and release timeline points must not reach the
//! compositor with a commit that carries no buffer — the protocol raises
//! `no_buffer` for an acquire point without a buffer. The points are held
//! until a commit that carries a buffer emits them (the next drawn frame, or
//! the buffer the proxy attaches on show).
//!
//! The app's `wp_linux_drm_syncobj_manager_v1` bind is forwarded unchanged, so
//! the app keeps creating its own syncobj surfaces and timelines. The proxy
//! only puts a handler on each created surface: for the panel surface it
//! records the points in the routing's hidden state ([`super::hide`]) instead
//! of forwarding them; every other surface keeps the default forward path.
//!
//! [`Points`] is the pure carrier of one pending acquire/release pair, so the
//! hide/show tests exercise the bookkeeping without a compositor
//! (`port-to-rust` D10).

use std::rc::{Rc, Weak};

use wl_proxy::object::ObjectCoreApi;
use wl_proxy::protocols::linux_drm_syncobj_v1::wp_linux_drm_syncobj_manager_v1::{
    WpLinuxDrmSyncobjManagerV1, WpLinuxDrmSyncobjManagerV1Handler,
};
use wl_proxy::protocols::linux_drm_syncobj_v1::wp_linux_drm_syncobj_surface_v1::{
    WpLinuxDrmSyncobjSurfaceV1, WpLinuxDrmSyncobjSurfaceV1Handler,
};
use wl_proxy::protocols::linux_drm_syncobj_v1::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1;
use wl_proxy::protocols::wayland::wl_surface::WlSurface;

use super::routing::Routing;

/// One timeline point: the timeline the app imported and the 64-bit point
/// value split into the two 32-bit halves the protocol carries.
#[derive(Clone, Debug)]
struct Point {
    timeline: Rc<WpLinuxDrmSyncobjTimelineV1>,
    hi: u32,
    lo: u32,
}

impl Point {
    /// Sends this point to the compositor through `surface`.
    fn send(&self, surface: &WpLinuxDrmSyncobjSurfaceV1) {
        surface.send_set_acquire_point(&self.timeline, self.hi, self.lo);
    }

    /// Sends this release point to the compositor through `surface`.
    fn send_release(&self, surface: &WpLinuxDrmSyncobjSurfaceV1) {
        surface.send_set_release_point(&self.timeline, self.hi, self.lo);
    }
}

/// The acquire and release points the app set for the next commit (D6): the
/// proxy holds them while the panel is hidden and emits them only with a
/// commit that carries a buffer.
#[derive(Clone, Debug, Default)]
pub(super) struct Points {
    acquire: Option<Point>,
    release: Option<Point>,
}

impl Points {
    /// Records the acquire point of the app's next commit.
    pub(super) fn set_acquire(
        &mut self,
        timeline: &Rc<WpLinuxDrmSyncobjTimelineV1>,
        hi: u32,
        lo: u32,
    ) {
        self.acquire = Some(Point {
            timeline: Rc::clone(timeline),
            hi,
            lo,
        });
    }

    /// Records the release point of the app's next commit.
    pub(super) fn set_release(
        &mut self,
        timeline: &Rc<WpLinuxDrmSyncobjTimelineV1>,
        hi: u32,
        lo: u32,
    ) {
        self.release = Some(Point {
            timeline: Rc::clone(timeline),
            hi,
            lo,
        });
    }

    /// Whether no point is held, so a commit needs no syncobj request.
    pub(super) fn is_empty(&self) -> bool {
        self.acquire.is_none() && self.release.is_none()
    }

    /// Sends both held points to the compositor, in acquire-then-release
    /// order, so the following commit carries them with its buffer (D6).
    pub(super) fn send(&self, surface: &WpLinuxDrmSyncobjSurfaceV1) {
        if let Some(acquire) = &self.acquire {
            acquire.send(surface);
        }
        if let Some(release) = &self.release {
            release.send_release(surface);
        }
    }
}

/// The app's bind of the compositor's explicit-synchronization factory: the
/// bind is forwarded by the registry, and every syncobj surface the app
/// creates gets a handler so the panel surface's points can be held (D6).
pub(super) struct ClientSyncobjManager {
    routing: Rc<Routing>,
    /// The manager this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    manager: Weak<WpLinuxDrmSyncobjManagerV1>,
}

impl ClientSyncobjManager {
    /// The handler of one app's manager bind.
    pub(super) fn new(routing: Rc<Routing>, manager: &Rc<WpLinuxDrmSyncobjManagerV1>) -> Self {
        Self {
            routing,
            manager: Rc::downgrade(manager),
        }
    }
}

impl WpLinuxDrmSyncobjManagerV1Handler for ClientSyncobjManager {
    fn handle_get_surface(
        &mut self,
        _slf: &Rc<WpLinuxDrmSyncobjManagerV1>,
        id: &Rc<WpLinuxDrmSyncobjSurfaceV1>,
        surface: &Rc<WlSurface>,
    ) {
        id.set_handler(ClientSyncobjSurface {
            routing: Rc::clone(&self.routing),
            surface: Rc::downgrade(surface),
            syncobj: Rc::downgrade(id),
        });
        if let Some(manager) = self.manager.upgrade() {
            manager.send_get_surface(id, surface);
        }
    }
}

/// One app syncobj surface (D6): the panel surface's acquire and release
/// points are held for the next buffer-carrying commit; another surface
/// (a passed-through window's) forwards them as the default handler does.
struct ClientSyncobjSurface {
    routing: Rc<Routing>,
    /// The app surface the object extends, kept weak so the handler does not
    /// keep its own object alive through the surface graph.
    surface: Weak<WlSurface>,
    /// The syncobj surface, kept weak so the handler does not keep its own
    /// object alive.
    syncobj: Weak<WpLinuxDrmSyncobjSurfaceV1>,
}

impl ClientSyncobjSurface {
    /// Whether this object extends the panel's surface, read at the time of
    /// each request (the surface can take the panel's slot after the object
    /// was created, as a viewport can).
    fn is_panel(&self) -> Option<u64> {
        let surface = self.surface.upgrade()?;
        let id = surface.unique_id();
        self.routing.is_panel_surface(id).then_some(id)
    }
}

impl WpLinuxDrmSyncobjSurfaceV1Handler for ClientSyncobjSurface {
    fn handle_set_acquire_point(
        &mut self,
        _slf: &Rc<WpLinuxDrmSyncobjSurfaceV1>,
        timeline: &Rc<WpLinuxDrmSyncobjTimelineV1>,
        point_hi: u32,
        point_lo: u32,
    ) {
        if self.is_panel().is_some() {
            if let Some(syncobj) = self.syncobj.upgrade() {
                self.routing.hidden.set_syncobj(&syncobj);
            }
            self.routing
                .hidden
                .remember_acquire(timeline, point_hi, point_lo);
            return;
        }
        if let Some(syncobj) = self.syncobj.upgrade() {
            syncobj.send_set_acquire_point(timeline, point_hi, point_lo);
        }
    }

    fn handle_set_release_point(
        &mut self,
        _slf: &Rc<WpLinuxDrmSyncobjSurfaceV1>,
        timeline: &Rc<WpLinuxDrmSyncobjTimelineV1>,
        point_hi: u32,
        point_lo: u32,
    ) {
        if self.is_panel().is_some() {
            if let Some(syncobj) = self.syncobj.upgrade() {
                self.routing.hidden.set_syncobj(&syncobj);
            }
            self.routing
                .hidden
                .remember_release(timeline, point_hi, point_lo);
            return;
        }
        if let Some(syncobj) = self.syncobj.upgrade() {
            syncobj.send_set_release_point(timeline, point_hi, point_lo);
        }
    }
}

impl std::fmt::Debug for ClientSyncobjManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSyncobjManager")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ClientSyncobjSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSyncobjSurface")
            .finish_non_exhaustive()
    }
}
