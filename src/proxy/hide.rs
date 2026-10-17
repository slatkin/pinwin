//! Hide and show for a live panel (replace-terminal-with-app-host D6, row
//! 8.2): hide unmaps the panel surface with a null buffer and releases the
//! reservation, show maps it again with the state the compositor needs and
//! restores the strip.
//!
//! The surface stays alive: D6 keeps the layer surface and unmaps it instead
//! of destroying the role. Hiding commits a null buffer on the panel surface,
//! moves the reserve's exclusive zone to zero and keeps the held gap for the
//! next show; the proxy drops every app `attach` while hidden and returns the
//! buffer to the app with a `wl_buffer.release`, so the app does not run out
//! of buffers. The buffer the app saw last is kept for the next show: showing
//! re-sends the layer surface's state (size, margins, anchors, exclusive
//! zone, keyboard mode), commits with no buffer so the compositor configures
//! the surface again, and attaches the kept buffer once the app acks that
//! configure (D6 amendments 1 and 2). A release the compositor sends for that
//! proxy-made attach is swallowed, because the app already received the
//! release for the attach the proxy dropped (D6 amendment 3).
//!
//! Every change to the panel's surface goes through the commit coordinator
//! ([`super::commit`], D7): hide and show inject their commit on a clean
//! surface and piggyback on the app's next commit on a dirty one. Neither
//! changes the size the app is configured with: show re-sends the geometry
//! the panel already runs at, so a same-size configure is what the app sees
//! (spec "Size untouched"). A layout change while hidden stores its settings
//! through [`super::apply`] as usual, moves the strip with its zone still
//! released, and takes effect when show re-sends the state; nothing appears
//! meanwhile (spec "Change while hidden").
//!
//! The state lives in the routing, not in the surface handler: a second
//! `take_slot` on the same `wl_surface` installs a fresh
//! [`PanelSurfaceCommits`](super::commit::PanelSurfaceCommits) and would drop
//! a hidden flag stored there. A re-dock (`set-output`, `closed`,
//! `set-layer`) therefore keeps a hidden panel hidden, and the re-created
//! reserve starts with its zone released.
//!
//! The pure state machine ([`Hidden`]) is display-free: the tests cover the
//! hide/show/toggle no-ops, which attaches are dropped while hidden, the
//! buffer kept for show, the release bookkeeping and the syncobj points
//! (`port-to-rust` D10).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::{Rc, Weak};

use wl_proxy::object::{ObjectCoreApi, ObjectUtils};
use wl_proxy::protocols::linux_drm_syncobj_v1::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1;
use wl_proxy::protocols::linux_drm_syncobj_v1::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1;
use wl_proxy::protocols::wayland::wl_buffer::{WlBuffer, WlBufferHandler};
use wl_proxy::protocols::wayland::wl_surface::WlSurface;

use super::commit::PanelSurfaceCommits;
use super::mapping::{anchor_of, keyboard_of};
use super::routing::Routing;
use super::syncobj::Points;
use crate::daemon::events::{Bus, Event};
use crate::daemon::panel::{PanelReply, PanelRequest};
use crate::name::PanelName;

/// The state the panel's hidden flag and the buffers it hides (D6): kept in
/// the routing so it survives a second `take_slot` and every re-dock.
pub(super) struct Hidden {
    /// Whether the panel is hidden. The panel starts at this value
    /// (`--hidden` and the config file's `hidden` set it).
    state: Cell<bool>,
    /// The buffer the next show attaches, with the syncobj points it was
    /// committed with: the last buffer the proxy forwarded upstream, or —
    /// for a panel that started hidden — the first buffer the app drew.
    show_buffer: RefCell<Option<Kept>>,
    /// The app's attach request in the current commit.
    attach: RefCell<PendingAttach>,
    /// The syncobj points the app set for the current commit (D6): held
    /// until a commit that carries a buffer emits them.
    points: RefCell<Points>,
    /// The syncobj object of the panel surface, so the held points can be
    /// emitted.
    syncobj: RefCell<Option<Weak<WpLinuxDrmSyncobjSurfaceV1>>>,
    /// Whether a show is waiting for the app to ack its configure, so the
    /// kept buffer can be attached (D6 amendment 2).
    awaiting_map: Cell<bool>,
    /// The buffers whose attach the proxy dropped since the last commit;
    /// each gets one `wl_buffer.release` after the commit is forwarded (D6).
    dropped: RefCell<Vec<Rc<WlBuffer>>>,
    /// How many attaches the proxy made itself per buffer, whose compositor
    /// release must be swallowed and not forwarded (D6 amendment 3).
    proxy_attached: RefCell<HashMap<u64, u32>>,
}

/// The buffer kept for the next show and the syncobj points it was committed
/// with (D6 amendments 2 and 3).
#[derive(Clone, Debug)]
struct Kept {
    buffer: Rc<WlBuffer>,
    points: Points,
}

/// The app's attach request in the current commit (D6): no request, a null
/// attach, or a buffer.
#[derive(Clone, Debug, Default)]
enum PendingAttach {
    /// No `wl_surface.attach` arrived since the last commit.
    #[default]
    NoRequest,
    /// The app attached a null buffer, which unmaps the surface.
    Null,
    /// The app attached a buffer the proxy may forward, hold or drop.
    Buffer(Rc<WlBuffer>),
}

/// What the proxy does with the app's attach (D6): while shown it forwards
/// it, while hidden it holds the first buffer for show and drops the later
/// ones, releasing each dropped buffer to the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Attach {
    /// The app is visible: the attach goes to the compositor unchanged.
    Forward,
    /// The panel is hidden: the attach stays local. The buffer is kept for
    /// show (the panel's first buffer) or already kept.
    Hold,
    /// The panel is hidden and a buffer is already kept: the attach is
    /// dropped and the buffer goes back to the app with a release.
    Drop,
}

/// What one commit does to the hidden state (D6): the syncobj points to emit
/// before the commit, and the buffers to release to the app after it.
#[derive(Debug, Default)]
pub(super) struct CommitPlan {
    /// The points to emit with this commit's buffer; `None` when the commit
    /// carries no buffer, so the points stay held.
    pub(super) points: Option<Points>,
    /// The buffers whose dropped attach the app gets back after the commit.
    pub(super) release: Vec<Rc<WlBuffer>>,
}

impl Hidden {
    /// The hidden state of a panel that starts hidden or shown.
    pub(super) fn new(hidden: bool) -> Self {
        Self {
            state: Cell::new(hidden),
            show_buffer: RefCell::new(None),
            attach: RefCell::new(PendingAttach::NoRequest),
            points: RefCell::new(Points::default()),
            syncobj: RefCell::new(None),
            awaiting_map: Cell::new(false),
            dropped: RefCell::new(Vec::new()),
            proxy_attached: RefCell::new(HashMap::new()),
        }
    }

    /// Whether the panel is hidden.
    pub(super) fn is_hidden(&self) -> bool {
        self.state.get()
    }

    /// Sets the hidden flag; the surface work is the caller's.
    pub(super) fn set_hidden(&self, hidden: bool) {
        self.state.set(hidden);
    }

    /// Forgets the buffers of a panel window that ended: a new window must
    /// not attach a buffer the old app owned. The hidden flag stays.
    pub(super) fn reset(&self) {
        self.show_buffer.borrow_mut().take();
        *self.attach.borrow_mut() = PendingAttach::NoRequest;
        *self.points.borrow_mut() = Points::default();
        self.syncobj.borrow_mut().take();
        self.awaiting_map.set(false);
        self.dropped.borrow_mut().clear();
        self.proxy_attached.borrow_mut().clear();
    }

    /// The app's attach (D6): forward it while shown, hold the kept buffer
    /// and drop the others while hidden.
    pub(super) fn on_app_attach(&self, buffer: Option<&Rc<WlBuffer>>) -> Attach {
        *self.attach.borrow_mut() = match buffer {
            Some(buffer) => PendingAttach::Buffer(Rc::clone(buffer)),
            None => PendingAttach::Null,
        };
        if !self.state.get() {
            if let Some(buffer) = buffer {
                // The app reuses the buffer, so the release the compositor
                // sends is the app's own and must be forwarded.
                self.forget_proxy_attach(buffer.unique_id());
            }
            return Attach::Forward;
        }
        let Some(buffer) = buffer else {
            return Attach::Hold;
        };
        let kept = self
            .show_buffer
            .borrow()
            .as_ref()
            .map(|kept| kept.buffer.unique_id());
        match kept {
            None => Attach::Hold,
            Some(id) if id == buffer.unique_id() => Attach::Hold,
            Some(_) => {
                self.dropped.borrow_mut().push(Rc::clone(buffer));
                Attach::Drop
            }
        }
    }

    /// The plan of one commit (D6): the points a buffer-carrying commit
    /// emits, the kept buffer a visible commit records, and the releases the
    /// dropped attaches owe the app.
    pub(super) fn commit_plan(&self) -> CommitPlan {
        let attach = std::mem::take(&mut *self.attach.borrow_mut());
        let release: Vec<Rc<WlBuffer>> = self.dropped.borrow_mut().drain(..).collect();
        let mut plan = CommitPlan {
            points: None,
            release,
        };
        let PendingAttach::Buffer(buffer) = attach else {
            return plan;
        };
        if self.state.get() {
            let kept = self
                .show_buffer
                .borrow()
                .as_ref()
                .map(|kept| kept.buffer.unique_id());
            match kept {
                Some(id) if id == buffer.unique_id() => {
                    let points = std::mem::take(&mut *self.points.borrow_mut());
                    if let Some(kept) = self.show_buffer.borrow_mut().as_mut() {
                        kept.points = points;
                    }
                }
                Some(_) => {
                    // The dropped attach's points are moot: the buffer goes
                    // back to the app without ever reaching the compositor.
                    std::mem::take(&mut *self.points.borrow_mut());
                }
                None => {
                    let points = std::mem::take(&mut *self.points.borrow_mut());
                    *self.show_buffer.borrow_mut() = Some(Kept { buffer, points });
                }
            }
            return plan;
        }
        // A visible buffer commit: the compositor can take the points now,
        // and the buffer becomes the one the next show attaches.
        let points = std::mem::take(&mut *self.points.borrow_mut());
        plan.points = Some(points.clone());
        *self.show_buffer.borrow_mut() = Some(Kept { buffer, points });
        self.awaiting_map.set(false);
        plan
    }

    /// Emits held points through the panel surface's syncobj object (D6):
    /// callers only call this with a commit that carries a buffer.
    pub(super) fn emit_points(&self, points: &Points) {
        if points.is_empty() {
            return;
        }
        if let Some(syncobj) = self.syncobj.borrow().as_ref().and_then(Weak::upgrade) {
            points.send(&syncobj);
        }
    }

    /// The buffer the next show attaches, with its points (D6 amendment 2).
    pub(super) fn show_buffer(&self) -> Option<(Rc<WlBuffer>, Points)> {
        self.show_buffer
            .borrow()
            .as_ref()
            .map(|kept| (Rc::clone(&kept.buffer), kept.points.clone()))
    }

    /// Marks a show that waits for the app's configure ack.
    pub(super) fn mark_awaiting_map(&self) {
        self.awaiting_map.set(true);
    }

    /// Whether a show is waiting for the app's configure ack.
    pub(super) fn awaiting_map(&self) -> bool {
        self.awaiting_map.get()
    }

    /// Clears the show's wait for a configure ack.
    pub(super) fn clear_awaiting_map(&self) {
        self.awaiting_map.set(false);
    }

    /// Whether this commit has an app attach pending, so a queued show
    /// re-attach must not overwrite it.
    pub(super) fn has_pending_attach(&self) -> bool {
        !matches!(*self.attach.borrow(), PendingAttach::NoRequest)
    }

    /// Takes a pending app attach out and owes its buffer a release: a hide
    /// replaces it with the null attach in the same commit, so the buffer
    /// never reaches the compositor.
    pub(super) fn release_pending_attach(&self) {
        let attach = std::mem::take(&mut *self.attach.borrow_mut());
        if let PendingAttach::Buffer(buffer) = attach {
            self.dropped.borrow_mut().push(buffer);
        }
    }

    /// Records an attach the proxy made itself, whose compositor release it
    /// must swallow (D6 amendment 3).
    pub(super) fn on_proxy_attach(&self, buffer: &Rc<WlBuffer>) {
        *self
            .proxy_attached
            .borrow_mut()
            .entry(buffer.unique_id())
            .or_insert(0) += 1;
    }

    /// Clears a proxy-made attach's release debt: the app attached the
    /// buffer again, so the next release belongs to the app.
    pub(super) fn forget_proxy_attach(&self, buffer: u64) {
        self.proxy_attached.borrow_mut().remove(&buffer);
    }

    /// Whether a compositor release for `buffer` belongs to a proxy-made
    /// attach and must be swallowed (D6 amendment 3).
    pub(super) fn swallow_release(&self, buffer: u64) -> bool {
        let mut attached = self.proxy_attached.borrow_mut();
        match attached.get_mut(&buffer) {
            Some(count) if *count > 0 => {
                *count -= 1;
                if *count == 0 {
                    attached.remove(&buffer);
                }
                true
            }
            Some(_) | None => false,
        }
    }

    /// Records the acquire point of the app's current commit (D6).
    pub(super) fn remember_acquire(
        &self,
        timeline: &Rc<WpLinuxDrmSyncobjTimelineV1>,
        hi: u32,
        lo: u32,
    ) {
        self.points.borrow_mut().set_acquire(timeline, hi, lo);
    }

    /// Records the release point of the app's current commit (D6).
    pub(super) fn remember_release(
        &self,
        timeline: &Rc<WpLinuxDrmSyncobjTimelineV1>,
        hi: u32,
        lo: u32,
    ) {
        self.points.borrow_mut().set_release(timeline, hi, lo);
    }

    /// Remembers the panel surface's syncobj object, through which the held
    /// points are emitted.
    pub(super) fn set_syncobj(&self, syncobj: &Rc<WpLinuxDrmSyncobjSurfaceV1>) {
        *self.syncobj.borrow_mut() = Some(Rc::downgrade(syncobj));
    }
}

impl fmt::Debug for Hidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hidden")
            .field("hidden", &self.state.get())
            .field("show_buffer", &self.show_buffer.borrow().is_some())
            .field("awaiting_map", &self.awaiting_map.get())
            .field("dropped", &self.dropped.borrow().len())
            .finish_non_exhaustive()
    }
}

/// The panel surface buffer handler (D6 amendment 3): a compositor release
/// for a buffer the proxy attached itself is swallowed; every other release
/// reaches the app as the default handler forwards it.
pub(super) struct BufferRelease {
    routing: Weak<Routing>,
    /// The buffer this handler was installed on, kept weak so the handler
    /// does not keep its own object alive.
    buffer: Weak<WlBuffer>,
}

impl BufferRelease {
    /// The handler of `buffer` on the panel surface.
    pub(super) fn new(routing: &Rc<Routing>, buffer: &Rc<WlBuffer>) -> Self {
        Self {
            routing: Rc::downgrade(routing),
            buffer: Rc::downgrade(buffer),
        }
    }
}

impl WlBufferHandler for BufferRelease {
    fn handle_release(&mut self, _slf: &Rc<WlBuffer>) {
        let Some(buffer) = self.buffer.upgrade() else {
            return;
        };
        let swallow = self
            .routing
            .upgrade()
            .is_some_and(|routing| routing.hidden.swallow_release(buffer.unique_id()));
        if !swallow {
            buffer.send_release();
        }
    }
}

impl fmt::Debug for BufferRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferRelease").finish_non_exhaustive()
    }
}

/// The answer to `toggle`, `show` and `hide` (spec "Panel commands"): the
/// state machine is here, the surface work goes through the commit
/// coordinator. `None` for every other request, which the caller answers.
pub(super) fn answer(
    routing: &Rc<Routing>,
    request: &PanelRequest,
    panel: &PanelName,
    bus: &Bus,
) -> Option<PanelReply> {
    match request {
        PanelRequest::Toggle => Some(toggle(routing, panel, bus)),
        PanelRequest::Show => Some(show(routing, panel, bus)),
        PanelRequest::Hide => Some(hide(routing, panel, bus)),
        _ => None,
    }
}

/// `toggle` hides a shown panel and shows a hidden one (spec "Hide", "Show a
/// hidden panel").
fn toggle(routing: &Rc<Routing>, panel: &PanelName, bus: &Bus) -> PanelReply {
    if routing.hidden.is_hidden() {
        show(routing, panel, bus)
    } else {
        hide(routing, panel, bus)
    }
}

/// Hides a shown panel (spec "Hide", "Hide releases the reservation"): the
/// surface commits a null buffer, the reserve's zone moves to zero, and the
/// event goes out once. A hidden panel is unchanged (spec "Panel commands").
fn hide(routing: &Rc<Routing>, panel: &PanelName, bus: &Bus) -> PanelReply {
    if routing.hidden.is_hidden() {
        return PanelReply::Done;
    }
    routing.hidden.set_hidden(true);
    if let Some((surface, _, _)) = routing.panel_window()
        && let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>()
    {
        let surface = Rc::clone(&surface);
        let routing = Rc::clone(routing);
        let reserve = routing.reserve();
        commits.request(Box::new(move || {
            routing.hidden.release_pending_attach();
            surface.send_attach(None, 0, 0);
            if let Some(reserve) = reserve {
                reserve.release();
            }
        }));
    }
    bus.publish(&Event::Hidden {
        panel: panel.clone(),
    });
    PanelReply::Done
}

/// Shows a hidden panel (spec "Show a hidden panel", "Size untouched"): the
/// layer surface's state goes again so the compositor sends a configure, the
/// proxy commits with no buffer, and the reserve gets its zone back in the
/// same frame. A shown panel is unchanged (spec "Show a shown panel").
fn show(routing: &Rc<Routing>, panel: &PanelName, bus: &Bus) -> PanelReply {
    if !routing.hidden.is_hidden() {
        return PanelReply::Done;
    }
    routing.hidden.set_hidden(false);
    if let Some((surface, _, _)) = routing.panel_window()
        && let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>()
    {
        let routing = Rc::clone(routing);
        commits.request(Box::new(move || {
            send_layer_state(&routing);
            routing.hidden.mark_awaiting_map();
            if let (Some(reserve), Some(gap)) = (routing.reserve(), routing.held_gap()) {
                reserve.apply_gap(gap);
            }
        }));
    }
    bus.publish(&Event::Shown {
        panel: panel.clone(),
    });
    PanelReply::Done
}

/// The show's attach of the kept buffer (D6 amendments 2 and 3): called when
/// the app acks the configure show asked for, and skipped when the app has
/// already attached a buffer in the commit that is about to go out.
pub(super) fn on_ack(routing: &Rc<Routing>, surface: &Rc<WlSurface>) {
    if routing.hidden.is_hidden() || !routing.hidden.awaiting_map() {
        return;
    }
    routing.hidden.clear_awaiting_map();
    let Some((buffer, points)) = routing.hidden.show_buffer() else {
        return;
    };
    let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>() else {
        return;
    };
    let routing = Rc::clone(routing);
    let surface = Rc::clone(surface);
    commits.request(Box::new(move || {
        if routing.hidden.has_pending_attach() {
            return;
        }
        surface.send_attach(Some(&buffer), 0, 0);
        routing.hidden.emit_points(&points);
        routing.hidden.on_proxy_attach(&buffer);
    }));
}

/// Re-sends the layer surface's state a show needs (D6 amendment 1): a
/// compositor that cleared the cached state on the unmap commit would
/// otherwise answer with a default configure. The values are the panel's
/// current geometry, so a show after a layout change applies that change and
/// a show without one keeps the size the app already has (spec "Size
/// untouched").
fn send_layer_state(routing: &Routing) {
    let Some(layer) = routing.panel_layer_surface() else {
        return;
    };
    let geometry = routing.geometry();
    let (width, height) = geometry.size();
    layer.send_set_size(width, height);
    let margins = geometry.margins();
    layer.send_set_margin(
        margins.top(),
        margins.right(),
        margins.bottom(),
        margins.left(),
    );
    layer.send_set_anchor(anchor_of(geometry.anchors()));
    let settings = routing.settings();
    layer.send_set_keyboard_interactivity(keyboard_of(settings.keyboard()));
    // D9: the panel surface still claims no zone of its own; the reserve
    // carries the strip.
    layer.send_set_exclusive_zone(-1);
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::time::Duration;

    use wl_proxy::baseline::Baseline;
    use wl_proxy::object::ObjectCoreApi;
    use wl_proxy::protocols::linux_drm_syncobj_v1::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1;
    use wl_proxy::protocols::wayland::wl_buffer::WlBuffer;
    use wl_proxy::state::State;

    use super::{Attach, Hidden};
    use crate::config::PanelSettings;
    use crate::daemon::events::{Bus, Event};
    use crate::daemon::panel::{PanelReply, PanelRequest};
    use crate::name::PanelName;
    use crate::proxy::routing::Routing;

    /// A display-free state: the test mints proxy objects without a
    /// compositor (`port-to-rust` D10).
    fn state() -> Rc<State> {
        State::builder(Baseline::V6)
            .without_server()
            .build()
            .expect("a client-side state needs no compositor")
    }

    /// One `wl_buffer` object the app could attach.
    fn buffer(state: &Rc<State>) -> Rc<WlBuffer> {
        state.create_object::<WlBuffer>(1)
    }

    /// One imported timeline, which a held point names.
    fn timeline(state: &Rc<State>) -> Rc<WpLinuxDrmSyncobjTimelineV1> {
        state.create_object::<WpLinuxDrmSyncobjTimelineV1>(1)
    }

    /// The panel's routing with the built-in settings.
    fn routing() -> Rc<Routing> {
        Routing::new(PanelSettings::built_in())
    }

    /// A panel name the events name.
    fn notes() -> PanelName {
        PanelName::parse("notes").expect("a panel name")
    }

    /// A toggle hides a shown panel and shows a hidden one, each real change
    /// publishing one event (spec "Hide", "Show a hidden panel").
    #[test]
    fn a_toggle_hides_then_shows_with_one_event_each() {
        let bus = Bus::new();
        let events = bus.subscribe();
        let routing = routing();
        let panel = notes();
        assert_eq!(
            super::answer(&routing, &PanelRequest::Toggle, &panel, &bus),
            Some(PanelReply::Done)
        );
        assert!(routing.hidden.is_hidden());
        assert_eq!(
            events.next(Duration::from_millis(100)),
            Ok(Event::Hidden {
                panel: panel.clone()
            })
        );
        assert_eq!(
            super::answer(&routing, &PanelRequest::Toggle, &panel, &bus),
            Some(PanelReply::Done)
        );
        assert!(!routing.hidden.is_hidden());
        assert_eq!(
            events.next(Duration::from_millis(100)),
            Ok(Event::Shown {
                panel: panel.clone()
            })
        );
        assert_eq!(
            events.next(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
    }

    /// Showing a shown panel and hiding a hidden one change nothing and
    /// publish nothing (spec "Show a shown panel").
    #[test]
    fn showing_a_shown_panel_and_hiding_a_hidden_one_are_no_ops() {
        let bus = Bus::new();
        let events = bus.subscribe();
        let routing = routing();
        let panel = notes();
        assert_eq!(
            super::answer(&routing, &PanelRequest::Show, &panel, &bus),
            Some(PanelReply::Done)
        );
        assert_eq!(
            events.next(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        assert_eq!(
            super::answer(&routing, &PanelRequest::Hide, &panel, &bus),
            Some(PanelReply::Done)
        );
        assert_eq!(
            events.next(Duration::from_millis(100)),
            Ok(Event::Hidden {
                panel: panel.clone()
            })
        );
        assert_eq!(
            super::answer(&routing, &PanelRequest::Hide, &panel, &bus),
            Some(PanelReply::Done)
        );
        assert_eq!(
            events.next(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
    }

    /// A visible commit forwards its attach and becomes the buffer the next
    /// show attaches; showing is a forward, not a drop (D6 amendments 2, 3).
    #[test]
    fn a_visible_buffer_becomes_the_one_show_attaches() {
        let state = state();
        let hidden = Hidden::new(false);
        let first = buffer(&state);
        assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Forward);
        let plan = hidden.commit_plan();
        assert!(plan.release.is_empty(), "a visible attach is not released");
        let (kept, _points) = hidden
            .show_buffer()
            .expect("a visible commit keeps its buffer");
        assert_eq!(kept.unique_id(), first.unique_id());
    }

    /// While hidden the first buffer is kept for show and each later one is
    /// dropped and released after the commit (D6, "Hide releases the
    /// reservation").
    #[test]
    fn a_hidden_panel_keeps_the_first_buffer_and_releases_the_next() {
        let state = state();
        let hidden = Hidden::new(true);
        let first = buffer(&state);
        assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Hold);
        let plan = hidden.commit_plan();
        assert!(plan.release.is_empty(), "the kept buffer is not released");
        let (kept, _points) = hidden.show_buffer().expect("the first buffer is kept");
        assert_eq!(kept.unique_id(), first.unique_id());

        let second = buffer(&state);
        assert_eq!(hidden.on_app_attach(Some(&second)), Attach::Drop);
        let plan = hidden.commit_plan();
        assert_eq!(plan.release.len(), 1, "the dropped attach owes one release");
        assert_eq!(plan.release[0].unique_id(), second.unique_id());
        let (kept, _points) = hidden.show_buffer().expect("the kept buffer stays");
        assert_eq!(kept.unique_id(), first.unique_id());
    }

    /// Reattaching the kept buffer while hidden neither drops nor releases
    /// it: the proxy holds it for show (D6 amendment 2).
    #[test]
    fn a_hidden_reattach_of_the_kept_buffer_is_held() {
        let state = state();
        let hidden = Hidden::new(true);
        let first = buffer(&state);
        assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Hold);
        let _ = hidden.commit_plan();
        assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Hold);
        let plan = hidden.commit_plan();
        assert!(plan.release.is_empty());
    }

    /// The app gets exactly one release per proxy-made attach, and the next
    /// release goes through again (D6 amendment 3).
    #[test]
    fn a_proxy_attach_owns_one_swallowed_release() {
        let state = state();
        let hidden = Hidden::new(false);
        let buffer = buffer(&state);
        hidden.on_proxy_attach(&buffer);
        assert!(hidden.swallow_release(buffer.unique_id()));
        assert!(
            !hidden.swallow_release(buffer.unique_id()),
            "the second release belongs to the app"
        );
    }

    /// Holding points across a hidden commit keeps them for the next
    /// buffer-carrying commit; a visible buffer commit emits them (D6). A
    /// hidden commit carries no buffer because the proxy dropped the attach.
    #[test]
    fn held_points_wait_for_a_buffer_carrying_commit() {
        let state = state();
        let hidden = Hidden::new(true);
        let timeline = timeline(&state);
        hidden.remember_acquire(&timeline, 0, 1);
        hidden.remember_release(&timeline, 0, 2);
        let plan = hidden.commit_plan();
        assert!(
            plan.points.is_none(),
            "a commit without a buffer emits no points"
        );

        let buffer = buffer(&state);
        hidden.set_hidden(false);
        assert_eq!(hidden.on_app_attach(Some(&buffer)), Attach::Forward);
        let plan = hidden.commit_plan();
        let points = plan.points.expect("a buffer-carrying commit emits points");
        assert!(!points.is_empty());
        let (_kept, kept_points) = hidden.show_buffer().expect("the buffer is kept");
        assert!(!kept_points.is_empty(), "the kept buffer keeps its points");
    }
}
