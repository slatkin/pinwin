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
//! proxy-made attach is swallowed when the app already received the release
//! for its own attach of the buffer, and forwarded when the proxy held that
//! attach without releasing it (D6 amendment 3).
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

use super::buffers::forward_destroy;
use super::commit::PanelSurfaceCommits;
use super::mapping::{anchor_of, keyboard_of};
use super::ring;
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
    /// The app's attach request in the current commit, with whether the proxy
    /// forwarded it: only a forwarded attach gives the commit a buffer (D6).
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
    /// Whether the app's last attach of this buffer was held while hidden and
    /// never released: the compositor's release for the proxy's own attach is
    /// then the app's release, not a duplicate (D6 amendment 3).
    owed_release: bool,
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
    Buffer {
        buffer: Rc<WlBuffer>,
        /// Whether the proxy forwarded the attach upstream. A hidden attach
        /// never reaches the compositor, so the commit that follows carries
        /// no buffer even if the panel shows in between (D6): the held
        /// syncobj points must not go out with it (`no_buffer`).
        forwarded: bool,
    },
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
        let decision = self.attach_decision(buffer);
        *self.attach.borrow_mut() = match buffer {
            Some(buffer) => PendingAttach::Buffer {
                buffer: Rc::clone(buffer),
                forwarded: decision == Attach::Forward,
            },
            None => PendingAttach::Null,
        };
        let Some(buffer) = buffer else {
            return decision;
        };
        match decision {
            Attach::Forward => {
                // The app reuses the buffer, so the release the compositor
                // sends is the app's own and must be forwarded.
                self.forget_proxy_attach(buffer.unique_id());
                self.clear_owed_release(buffer.unique_id());
            }
            Attach::Hold => self.mark_owed_release(buffer.unique_id()),
            Attach::Drop => self.dropped.borrow_mut().push(Rc::clone(buffer)),
        }
        decision
    }

    /// What the proxy does with the app's attach (D6), before it is recorded.
    fn attach_decision(&self, buffer: Option<&Rc<WlBuffer>>) -> Attach {
        if !self.state.get() {
            return Attach::Forward;
        }
        let Some(buffer) = buffer else {
            return Attach::Hold;
        };
        match self.kept_id() {
            None => Attach::Hold,
            Some(id) if id == buffer.unique_id() => Attach::Hold,
            Some(_) => Attach::Drop,
        }
    }

    /// The buffer the next show attaches, if one is already kept.
    fn kept_id(&self) -> Option<u64> {
        self.show_buffer
            .borrow()
            .as_ref()
            .map(|kept| kept.buffer.unique_id())
    }

    /// The buffer any crop of the panel surface must stay inside after this
    /// commit (D6, D8): the kept buffer. While hidden the compositor never
    /// receives the app's attaches, so the buffer it sees at the next show is
    /// the kept one; while shown the kept buffer is the last forwarded
    /// attach. A commit's crop is composed against it, because a source past
    /// the buffer the compositor holds is the fatal `out_of_buffer`.
    pub(super) fn compositor_buffer(&self) -> Option<u64> {
        self.kept_id()
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
        let PendingAttach::Buffer { buffer, forwarded } = attach else {
            return plan;
        };
        if !forwarded {
            // The attach never reached the compositor, whatever the panel's
            // state is now: this commit carries no buffer, so the held points
            // must not go out (the protocol's `no_buffer`).
            let kept = self.kept_id();
            match kept {
                None => {
                    let points = std::mem::take(&mut *self.points.borrow_mut());
                    *self.show_buffer.borrow_mut() = Some(Kept {
                        buffer,
                        points,
                        owed_release: true,
                    });
                }
                Some(id) if id == buffer.unique_id() => {
                    let points = std::mem::take(&mut *self.points.borrow_mut());
                    if let Some(kept) = self.show_buffer.borrow_mut().as_mut() {
                        kept.points = points;
                        kept.owed_release = true;
                    }
                }
                Some(_) => {
                    // The dropped attach's points are moot: the buffer goes
                    // back to the app without ever reaching the compositor.
                    std::mem::take(&mut *self.points.borrow_mut());
                }
            }
            return plan;
        }
        // A forwarded buffer commit: the compositor can take the points now,
        // and the buffer becomes the one the next show attaches.
        let points = std::mem::take(&mut *self.points.borrow_mut());
        plan.points = Some(points.clone());
        let previous = self.show_buffer.borrow_mut().replace(Kept {
            buffer,
            points,
            owed_release: false,
        });
        if let Some(previous) = previous
            && previous.owed_release
        {
            // A held buffer the show no longer attaches: the app gets its
            // release now, because the compositor never saw that attach.
            plan.release.push(previous.buffer);
        }
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
        if let PendingAttach::Buffer { buffer, .. } = attach {
            self.dropped.borrow_mut().push(buffer);
        }
    }

    /// Records an attach the proxy made itself (D6 amendments 2 and 3): the
    /// compositor's release for it is swallowed, unless the app's held attach
    /// of the same buffer is still owed one.
    pub(super) fn on_proxy_attach(&self, buffer: &Rc<WlBuffer>) {
        let id = buffer.unique_id();
        if self.kept_owes_release(id) {
            return;
        }
        *self.proxy_attached.borrow_mut().entry(id).or_insert(0) += 1;
    }

    /// Clears a proxy-made attach's release debt: the app attached the
    /// buffer again, so the next release belongs to the app.
    pub(super) fn forget_proxy_attach(&self, buffer: u64) {
        self.proxy_attached.borrow_mut().remove(&buffer);
    }

    /// Whether a compositor release for `buffer` belongs to a proxy-made
    /// attach and must be swallowed (D6 amendment 3). A release that reaches
    /// the app settles the kept buffer's held attach, so a later show
    /// swallows instead.
    pub(super) fn swallow_release(&self, buffer: u64) -> bool {
        let mut attached = self.proxy_attached.borrow_mut();
        if let Some(count) = attached.get_mut(&buffer) {
            *count -= 1;
            if *count == 0 {
                attached.remove(&buffer);
            }
            return true;
        }
        self.clear_owed_release(buffer);
        false
    }

    /// Whether the kept buffer's held attach still owes the app a release.
    fn kept_owes_release(&self, buffer: u64) -> bool {
        self.show_buffer
            .borrow()
            .as_ref()
            .is_some_and(|kept| kept.buffer.unique_id() == buffer && kept.owed_release)
    }

    /// Marks the kept buffer's held attach as owed a release (D6 amendment 3).
    fn mark_owed_release(&self, buffer: u64) {
        if let Some(kept) = self.show_buffer.borrow_mut().as_mut()
            && kept.buffer.unique_id() == buffer
        {
            kept.owed_release = true;
        }
    }

    /// Clears the kept buffer's owed release: the app got its release.
    fn clear_owed_release(&self, buffer: u64) {
        if let Some(kept) = self.show_buffer.borrow_mut().as_mut()
            && kept.buffer.unique_id() == buffer
        {
            kept.owed_release = false;
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
/// reaches the app as the default handler forwards it. The handler also
/// frees the buffer's size entry when the app destroys it (D8): the attach
/// it replaced is the one that recorded the size.
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

    fn handle_destroy(&mut self, _slf: &Rc<WlBuffer>) {
        let Some(buffer) = self.buffer.upgrade() else {
            return;
        };
        // D8: the panel's attach replaced the tracked handler, so the size
        // entry must leave here. Without this, every buffer the panel ever
        // attached — including each one the app draws with while hidden —
        // would keep its entry for the life of the daemon.
        if let Some(routing) = self.routing.upgrade() {
            routing
                .globals()
                .viewport()
                .buffers()
                .forget(buffer.unique_id());
        }
        forward_destroy(&buffer);
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
    // D8: a hide ends a running tween at its target before the panel unmaps.
    super::tween::stop(routing);
    routing.hidden.set_hidden(true);
    // D10: a hidden panel shows no ring, and the hide gives up the keyboard
    // focus the ring followed.
    ring::on_panel_hidden(routing);
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
/// same frame. The re-sent state carries the settings' keyboard mode, so an
/// `on-demand` or `exclusive` panel takes the keyboard when the show maps it
/// and a `none` panel does not (spec "Show takes focus", "Show in none
/// mode"). A shown panel is unchanged (spec "Show a shown panel").
pub(super) fn show(routing: &Rc<Routing>, panel: &PanelName, bus: &Bus) -> PanelReply {
    if !routing.hidden.is_hidden() {
        return PanelReply::Done;
    }
    routing.hidden.set_hidden(false);
    // D10: the show keeps the ring off until a keyboard enter reports the
    // focus it may take (spec "Show takes focus").
    ring::on_panel_hidden(routing);
    if let Some((surface, _, _)) = routing.panel_window()
        && let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>()
    {
        // The show sends the keyboard mode itself before the map, so the
        // launch deferral (row 8.3) must not send it again.
        commits.clear_keyboard_deferral();
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

// The path is explicit because `src/proxy.rs` includes this file with
// `#[path]`, so a child module searches in `src/proxy/` rather than beside
// this file (as in `src/daemon/events.rs`).
#[cfg(test)]
#[path = "hide/tests.rs"]
mod tests;
