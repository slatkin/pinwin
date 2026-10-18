//! The tween's driver (replace-terminal-with-app-host row 10.1, D8): the
//! per-panel slot that holds the running tween, the frame clock the panel's
//! loop reads, and the per-frame apply.
//!
//! D8 lists what each frame sends — the layer surface's size and margins, the
//! reserve surface's zone when the gap moves, and the viewport source and
//! destination — and D7 decides how the commit happens: the frame is a change
//! on the app's surface, injected with the proxy's own commit when the
//! surface is clean and piggybacked on the app's next commit when it is
//! dirty. The state lives in [`Globals`](super::super::output::Globals) with
//! the panel's other proxy-side state, so a re-dock and the viewport
//! composition see the same tween.
//!
//! The frame clock is the panel loop's timer, the clock the hide/show and
//! tween spikes proved: the loop waits at most a frame interval while a
//! tween runs, and this module says how long it may sleep. A grow first
//! waits for its configure handshake (D8): that wait needs no frames, so it
//! wakes on its deadline (or on the app's commit, which arrives through the
//! state's own dispatch). The handshake's result starts the tween or snaps
//! the change; the tween itself owns no wait.
//!
//! The machines here touch Wayland objects, so the tests are the pure ones
//! next door.

use std::cell::{Cell, RefCell, RefMut};
use std::rc::Rc;
use std::time::{Duration, Instant};

use wl_proxy::object::{ObjectCoreApi, ObjectUtils};

use super::super::commit::PanelSurfaceCommits;
use super::super::dock;
use super::super::routing::Routing;
use super::super::viewport;
use super::{
    Continuation, Fraction, Frame, GROW_HANDSHAKE_US, GrowHandshake, GrowResult, MintedSerials,
    Step, Tween, animates, waits_for_app,
};
use crate::geometry::{Geometry, HeldGap, ReserveMode};

/// How long the panel loop sleeps between two tween frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// One panel's tween state (D8): the tween the driver runs, if any, the
/// serials the proxy minted for its own configures, and the generation a
/// queued frame is told apart by.
pub(in crate::proxy) struct Slot {
    /// The grow's configure handshake (D8): the proxy configured the app and
    /// waits for its wide buffer. `None` when no grow waits.
    pending: RefCell<Option<Pending>>,
    driver: RefCell<Option<Driver>>,
    serials: MintedSerials,
    /// The tween that owns the frames (D8). A frame waits in the commit
    /// coordinator while the app's surface is dirty, and the state it carries
    /// is stale once a newer owner has sent layer state: a hide or the panel
    /// window's end cleared the tween ([`Slot::clear`]), a new or retargeted
    /// tween replaced it ([`Slot::set_driver`]), or a change snapped,
    /// re-applied or re-docked outside the coordinator ([`Slot::clear`]).
    /// Each of those bumps it, so a frame captured before the bump sees
    /// [`Slot::is_current`] fail and does nothing, and the destroyed layer
    /// surface it named can be dropped. A tween that ends naturally keeps the
    /// generation, so its final frame, still queued, is applied.
    generation: Cell<u64>,
}

impl Slot {
    /// A panel that runs no tween.
    pub(in crate::proxy) fn new() -> Self {
        Self {
            pending: RefCell::new(None),
            driver: RefCell::new(None),
            serials: MintedSerials::new(),
            generation: Cell::new(0),
        }
    }

    /// Abandons the tween the slot holds (D8): a hide, the panel window's
    /// end, a snap, a re-apply or a re-dock takes it away, so a frame already
    /// queued in the commit coordinator must not re-send the layer state it
    /// captured over the state the abandoning event sent. A retarget replaces
    /// the driver instead ([`Slot::set_driver`]), which bumps the generation
    /// the same way. The minted serials stay: the app may still ack the
    /// configure the abandoned tween sent, even after a hide or a re-dock.
    pub(in crate::proxy) fn clear(&self) {
        self.bump();
        self.pending.borrow_mut().take();
        self.driver.borrow_mut().take();
    }

    /// Forgets a panel window that ended (D8): the tween goes and so do the
    /// minted serials, because the app's `xdg_surface` is gone and no ack can
    /// still name one.
    pub(in crate::proxy) fn reset(&self) {
        self.clear();
        self.serials.reset();
    }

    /// Forgets a tween that reached its target (D8): the state it ended on is
    /// the steady state, so its last frame — which may still wait in the
    /// commit coordinator — stays valid and must still be applied. A frame of
    /// a later owner is told apart by that owner's own bump. The final
    /// configure's serial stays live: the app acks it after the tween ends.
    fn finish(&self) {
        self.driver.borrow_mut().take();
    }

    /// The generation a frame is queued for (D8).
    pub(super) fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Whether a frame queued for `generation` still applies (D8): false once
    /// the slot has cleared the tween it belonged to.
    pub(super) fn is_current(&self, generation: u64) -> bool {
        self.generation.get() == generation
    }

    /// Invalidates every frame queued for the tween the slot holds (D8).
    fn bump(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }

    /// The next serial of the proxy's own configures (D8), remembered so the
    /// app's ack is recognised by its membership and swallowed.
    pub(super) fn mint(&self) -> u32 {
        self.serials.mint()
    }

    /// Whether an app ack answers a configure the proxy minted itself (D8):
    /// the layer surface never saw that serial, so the ack must not be
    /// forwarded. A serial the proxy did not mint passes through.
    pub(in crate::proxy) fn swallow_ack(&self, serial: u32) -> bool {
        self.serials.swallow(serial)
    }

    /// Whether a tween runs (D8): a waiting configure handshake counts, so
    /// the layer surface acks the compositor's intermediate configures while
    /// the grow waits too.
    fn is_running(&self) -> bool {
        self.pending.borrow().is_some()
            || self
                .driver
                .borrow()
                .as_ref()
                .is_some_and(|driver| driver.tween.is_tweening())
    }

    /// The waiting handshake, so the frame step and a commit can look at it.
    fn pending(&self) -> RefMut<'_, Option<Pending>> {
        self.pending.borrow_mut()
    }

    /// The running tween's driver, so the frame step and a commit can look
    /// at it.
    fn driver(&self) -> RefMut<'_, Option<Driver>> {
        self.driver.borrow_mut()
    }

    /// The state a retarget continues from (D8, D5): the waiting handshake's
    /// or the running tween's. The reserve mode is the one the change started
    /// from, so a retarget of a covering panel keeps holding its strip.
    fn running(&self) -> Option<Continuation> {
        if let Some(pending) = self.pending.borrow().as_ref() {
            return Some(pending.handshake.continuation());
        }
        self.driver
            .borrow()
            .as_ref()
            .map(|driver| driver.tween.continuation())
    }

    /// The gap the reserve holds once the running change ends (D8): the
    /// waiting handshake's or the running tween's.
    fn final_gap(&self) -> Option<HeldGap> {
        if let Some(pending) = self.pending.borrow().as_ref() {
            return Some(pending.handshake.final_gap());
        }
        self.driver
            .borrow()
            .as_ref()
            .map(|driver| driver.tween.final_gap())
    }

    /// Keeps the tween a change just started (D8), invalidating the frames
    /// of the tween it replaces.
    fn set_driver(&self, driver: Driver) {
        self.bump();
        self.pending.borrow_mut().take();
        *self.driver.borrow_mut() = Some(driver);
    }

    /// Keeps the configure handshake a grow just started (D8), invalidating
    /// the frames of the tween it replaces.
    fn set_pending(&self, pending: Pending) {
        self.bump();
        self.driver.borrow_mut().take();
        *self.pending.borrow_mut() = Some(pending);
    }
}

/// A grow waiting for its configure handshake (D8) and the instant its
/// elapsed microseconds count from.
struct Pending {
    handshake: GrowHandshake,
    begun: Instant,
}

/// One running tween and the instant its elapsed microseconds count from.
struct Driver {
    tween: Tween,
    begun: Instant,
}

/// Whether a tween runs on the panel (D8): the layer surface's configure
/// handler acks the compositor's intermediate configures while it does.
pub(in crate::proxy) fn is_running(routing: &Routing) -> bool {
    routing.globals().tween().is_running()
}

/// Starts the tween of a validated change from `from` to `geometry.across()`
/// (D8). `false` when the change snaps instead: no duration, no viewporter to
/// crop through, a hidden panel, no panel window, or a size the easing cannot
/// carry. A running tween is retargeted from the length it shows now (spec
/// "Retarget"); a change that has to grow the app sends its one new configure
/// here.
pub(in crate::proxy) fn start(
    routing: &Rc<Routing>,
    from: u32,
    current_reserve: ReserveMode,
    geometry: Geometry,
    duration_ms: u32,
) -> bool {
    let globals = routing.globals();
    if duration_ms == 0
        || !animates(
            routing.panel_window().is_some(),
            routing.hidden.is_hidden(),
            globals.viewporter().is_some(),
        )
    {
        return false;
    }
    // D8: a running change continues from the length it shows now and from
    // the gap it holds, so a retarget does not jump (spec "Retarget").
    let running = {
        let slot = globals.tween();
        slot.running()
    };
    let continuation = if let Some(running) = running {
        Continuation {
            surface: app_surface(routing).unwrap_or(running.surface),
            ..running
        }
    } else {
        let surface = match app_surface(routing) {
            Some(surface) => surface,
            // D8: a change that grows the app and cannot see its size
            // never knows when the wide buffer arrives, so it snaps.
            None if geometry.across() > from => return false,
            // D8: the pair is width first, like every other surface
            // extent the composition reads, so a top or bottom panel's
            // length across its edge is the height.
            None => {
                let length = panel_length(routing, geometry);
                if geometry.edge().across_is_width() {
                    (from, length)
                } else {
                    (length, from)
                }
            }
        };
        Continuation {
            from,
            surface,
            resting_gap: routing
                .held_gap()
                .unwrap_or_else(|| HeldGap::start(geometry)),
            reserve: current_reserve,
        }
    };
    // D8: a change that grows the app configures it once to the target and
    // then waits for the configure handshake; a shrink, or a retarget inside
    // the buffer it has, configures it when the last frame arrives.
    if waits_for_app(geometry.edge(), continuation.surface, geometry.across()) {
        globals.tween().set_pending(Pending {
            handshake: GrowHandshake::new(geometry, continuation, duration_ms),
            begun: Instant::now(),
        });
        let serial = globals.tween().mint();
        send_configure(routing, geometry, serial);
        return true;
    }
    let Some(tween) = Tween::start(geometry, continuation, duration_ms) else {
        return false;
    };
    globals.tween().set_driver(Driver {
        tween,
        begun: Instant::now(),
    });
    true
}

/// Applies the frames a running tween owes now, and returns how long the
/// panel loop may sleep before the next one (D8). `None` when no change runs.
///
/// A waiting configure handshake returns the time left before it settles on
/// no wide buffer; a running tween returns one frame interval. The last frame
/// is applied before the tween is dropped, so the viewport composition still
/// crops to its fraction and the steady state then takes over.
pub(in crate::proxy) fn advance(routing: &Rc<Routing>) -> Option<Duration> {
    let globals = routing.globals();
    // D8: a grow first settles its configure handshake. An app that never
    // answers within the bound snaps to the target; the tween it never
    // started owns no timeout.
    let pending = {
        let slot = globals.tween();
        let pending = slot.pending();
        pending
            .as_ref()
            .map(|pending| (pending.handshake, micros(pending.begun.elapsed())))
    };
    if let Some((handshake, elapsed)) = pending {
        if GrowHandshake::on_deadline(elapsed).is_some() {
            globals.tween().clear();
            apply(routing, handshake.snap());
            return None;
        }
        let left = (GROW_HANDSHAKE_US - elapsed).max(0);
        return Some(Duration::from_micros(u64::try_from(left).unwrap_or(0)));
    }
    let (frame, finished, resize_app, geometry) = {
        let slot = globals.tween();
        let mut driver = slot.driver();
        let driver = driver.as_mut()?;
        let elapsed = micros(driver.begun.elapsed());
        let resize_app = driver.tween.shrinks_app();
        let geometry = driver.tween.geometry();
        match driver.tween.step(elapsed) {
            Step::Frame(frame) => (frame, false, resize_app, geometry),
            Step::Finished(frame) => (frame, true, resize_app, geometry),
        }
    };
    apply(routing, frame);
    if !finished {
        return Some(FRAME_INTERVAL);
    }
    // D8: a change that leaves the app wider than the target ends by
    // configuring it to the target width. A grow already did at its start,
    // and a snapped grow never needs it.
    if resize_app {
        let serial = globals.tween().mint();
        send_configure(routing, geometry, serial);
    }
    // D8: the tween ended on its target, so the last frame it queued is the
    // steady state and stays valid; a frame of a later owner is dropped by
    // that owner's own bump.
    globals.tween().finish();
    None
}

/// The app's commit settles a grow's configure handshake (D8 finding 4:
/// checked at the commit, because a toolkit reports its new window geometry
/// after its attach). An answer starts the tween from that commit; an
/// intermediate size leaves the handshake waiting.
pub(in crate::proxy) fn on_app_commit(routing: &Routing) {
    let globals = routing.globals();
    let edge = {
        let slot = globals.tween();
        let pending = slot.pending();
        pending
            .as_ref()
            .map(|pending| pending.handshake.geometry().edge())
    };
    let Some(edge) = edge else {
        return;
    };
    let reported = app_surface(routing).map(|(width, height)| {
        if edge.across_is_width() {
            width
        } else {
            height
        }
    });
    let slot = globals.tween();
    let mut waiting = slot.pending();
    let Some(pending) = waiting.as_mut() else {
        return;
    };
    let Some(GrowResult::Answered { surface }) = pending.handshake.on_commit(reported) else {
        return;
    };
    // D8: the answered grow animates from this commit, so its eased clock
    // starts now; a size the easing cannot carry leaves the handshake to its
    // deadline, which snaps.
    let Some(tween) = pending.handshake.start(surface) else {
        return;
    };
    let begun = Instant::now();
    drop(waiting);
    slot.set_driver(Driver { tween, begun });
}

impl Routing {
    /// The app surface's extent, width first, the viewport composition crops
    /// (D8) while a change runs and the app reports no window geometry: a
    /// waiting handshake's or the running tween's.
    pub(in crate::proxy) fn tween_surface(&self) -> Option<(u32, u32)> {
        let globals = self.globals();
        let slot = globals.tween();
        if let Some(surface) = slot
            .pending()
            .as_ref()
            .map(|pending| pending.handshake.surface())
        {
            return Some(surface);
        }
        let driver = slot.driver();
        driver.as_ref().map(|driver| driver.tween.surface())
    }
}

/// Stops a running tween at its target (D8, spec "Show and hide a panel": a
/// running animation first ends at its target): the held gap moves to where
/// the tween would have left it and no frame runs afterwards. The driver
/// does not re-send the layer state, so a hide's own commit is the last one.
pub(in crate::proxy) fn stop(routing: &Routing) {
    let globals = routing.globals();
    let slot = globals.tween();
    let Some(gap) = slot.final_gap() else {
        return;
    };
    // D8: the hide's own commit is the one that ends the panel, so a frame
    // still queued for the tween the stop abandons must not re-send the layer
    // state it captured.
    slot.clear();
    routing.set_held_gap(gap);
    if let Some(reserve) = routing.reserve() {
        if routing.hidden.is_hidden() {
            reserve.apply_gap_hidden(gap);
        } else {
            reserve.apply_gap(gap);
        }
    }
}

impl Routing {
    /// The visible fraction the viewport composition crops to (D8): a waiting
    /// handshake's, the running tween's, or none when no change runs.
    pub(in crate::proxy) fn tween_fraction(&self) -> Option<Fraction> {
        let globals = self.globals();
        let slot = globals.tween();
        if let Some(fraction) = slot
            .pending()
            .as_ref()
            .map(|pending| pending.handshake.fraction())
        {
            return Some(fraction);
        }
        let driver = slot.driver();
        driver.as_ref().map(|driver| driver.tween.fraction())
    }
}

/// The app surface's extent in surface-local pixels, width first (D8): the
/// window geometry the app reported, or its own viewport destination on the
/// panel surface. `None` when it reports neither.
fn app_surface(routing: &Routing) -> Option<(u32, u32)> {
    if let Some([_, _, width, height]) = routing.globals().viewport().window() {
        return Some((extent(width), extent(height)));
    }
    let (surface, _, _) = routing.panel_window()?;
    let destination = routing
        .globals()
        .viewport()
        .app(surface.unique_id())
        .destination()?;
    Some((extent(destination.0), extent(destination.1)))
}

/// A surface length as at least one surface pixel.
fn extent(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(1).max(1)
}

/// The panel's length along its edge on `output`, less the side gutters
/// (D9): the length a configure carries when the app reports no window
/// geometry.
fn panel_length(routing: &Routing, geometry: Geometry) -> u32 {
    let edge = geometry.edge();
    let Some(output) = routing.dock_output() else {
        return 1;
    };
    let gutters = geometry.gutters();
    let length = i64::from(output.length(edge))
        - i64::from(gutters.along_start(edge))
        - i64::from(gutters.along_end(edge));
    u32::try_from(length).unwrap_or(1).max(1)
}

/// The one configure a tween sends the app (D8): the target size across the
/// tween axis and the length it already has along it.
fn send_configure(routing: &Routing, geometry: Geometry, serial: u32) {
    let Some((_, xdg_surface, toplevel)) = routing.panel_window() else {
        return;
    };
    let (width, height) = app_configure_size(routing, geometry);
    dock::send_app_configure(&toplevel, &xdg_surface, width, height, serial);
}

/// The window size the app is configured with at the tween's target (D8):
/// the target across the edge and the length the app already has along it —
/// its window geometry's, or the panel's when it reports none.
fn app_configure_size(routing: &Routing, geometry: Geometry) -> (u32, u32) {
    let edge = geometry.edge();
    let along = match routing.globals().viewport().window() {
        Some([_, _, width, height]) => {
            let value = if edge.across_is_width() {
                height
            } else {
                width
            };
            extent(value)
        }
        None => panel_length(routing, geometry),
    };
    if edge.across_is_width() {
        (geometry.across(), along)
    } else {
        (along, geometry.across())
    }
}

/// One frame applied through the commit coordinator (D7, D8): the layer
/// surface's size and margins, the reserve's zone when the gap moves, the
/// held gap the rest of the panel reads, and the viewport values, then the
/// coordinator's commit.
fn apply(routing: &Rc<Routing>, frame: Frame) {
    let Some((surface, _, _)) = routing.panel_window() else {
        return;
    };
    let Some(layer_surface) = routing.panel_layer_surface() else {
        return;
    };
    let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>() else {
        return;
    };
    let generation = routing.globals().tween().generation();
    let routing = Rc::clone(routing);
    commits.request(Box::new(move || {
        // D8: a frame the slot cleared its tween out from under is stale —
        // a hide, a re-dock or a new tween may have sent newer layer state —
        // so it must not re-send the state it captured.
        if !routing.globals().tween().is_current(generation) {
            return;
        }
        let (width, height) = frame.layer_size();
        layer_surface.send_set_size(width, height);
        let margins = frame.margins();
        layer_surface.send_set_margin(
            margins.top(),
            margins.right(),
            margins.bottom(),
            margins.left(),
        );
        if let Some(reserve) = routing.reserve() {
            reserve.apply_gap(frame.gap());
        }
        routing.set_held_gap(frame.gap());
        viewport::recompose(&routing);
    }));
}

/// A duration in whole microseconds for the tween's step.
fn micros(elapsed: Duration) -> i64 {
    i64::try_from(elapsed.as_micros()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{Continuation, Driver, GrowHandshake, Pending, Slot, Tween};
    use crate::geometry::{Edge, Geometry, Gutters, HeldGap, ReserveMode};

    /// A pushing geometry of `across` pixels on `edge` with no gutters (D9).
    fn push(edge: Edge, across: u32) -> Geometry {
        Geometry::new(edge, across, Gutters::new(0, 0, 0, 0), ReserveMode::Push)
    }

    /// The tween an answered 200 ms grow of the panel from 400 to 600 pixels
    /// starts (D8).
    fn tween() -> Tween {
        Tween::start(
            push(Edge::Left, 600),
            Continuation {
                from: 400,
                surface: (400, 1080),
                resting_gap: HeldGap::start(push(Edge::Left, 400)),
                reserve: ReserveMode::Push,
            },
            200,
        )
        .expect("a 200 ms grow")
    }

    /// A 200 ms grow of the panel from 400 to 600 pixels still waiting for
    /// its configure handshake (D8).
    fn pending() -> Pending {
        Pending {
            handshake: GrowHandshake::new(
                push(Edge::Left, 600),
                Continuation {
                    from: 400,
                    surface: (400, 1080),
                    resting_gap: HeldGap::start(push(Edge::Left, 400)),
                    reserve: ReserveMode::Push,
                },
                200,
            ),
            begun: Instant::now(),
        }
    }

    /// A tween that ends naturally keeps the generation, so its final frame —
    /// which the dirty app surface may leave in the commit coordinator — still
    /// applies (D8, spec "Exact final width", "Animation completes").
    #[test]
    fn a_natural_finish_keeps_its_queued_frame() {
        let slot = Slot::new();
        let queued = slot.generation();
        slot.finish();
        assert!(
            slot.is_current(queued),
            "the final frame of a finished tween still applies"
        );
    }

    /// A tween that ends naturally keeps its final minted serial: the app
    /// acks the configure it sent after the tween drops (D8).
    #[test]
    fn a_natural_finish_keeps_its_minted_serial() {
        let slot = Slot::new();
        let minted = slot.mint();
        slot.finish();
        assert!(
            slot.swallow_ack(minted),
            "the app acks the final configure after the tween ends"
        );
    }

    /// A panel window that ends forgets its minted serials: the app's
    /// `xdg_surface` is gone, so no ack can still name one (D8).
    #[test]
    fn a_reset_forgets_the_minted_serials() {
        let slot = Slot::new();
        let minted = slot.mint();
        slot.reset();
        assert!(!slot.swallow_ack(minted), "the old xdg_surface is gone");
    }

    /// A waiting configure handshake counts as a running change, holds the
    /// crop at the old surface, and a tween that answers it or a clear drops
    /// the frames it queued (D8).
    #[test]
    fn a_pending_handshake_runs_until_its_answer() {
        let slot = Slot::new();
        assert!(!slot.is_running(), "a new slot runs nothing");
        let queued = slot.generation();
        slot.set_pending(pending());
        assert!(slot.is_running(), "a waiting handshake runs");
        assert_eq!(
            slot.running()
                .map(|running| (running.from, running.surface)),
            Some((400, (400, 1080))),
            "the handshake holds the old length and surface"
        );
        assert_eq!(
            slot.final_gap(),
            Some(HeldGap::start(push(Edge::Left, 400)).after_publish(push(Edge::Left, 600))),
            "the handshake ends on the target's strip"
        );
        slot.set_driver(Driver {
            tween: tween(),
            begun: Instant::now(),
        });
        assert!(
            !slot.is_current(queued),
            "the answered grow drops the handshake's frames"
        );
        assert!(slot.is_running(), "the answered tween runs");

        let abandoned = Slot::new();
        abandoned.set_pending(pending());
        abandoned.clear();
        assert!(
            !abandoned.is_running(),
            "a clear drops the waiting handshake"
        );
    }

    /// A frame queued by a tween is stale once a new tween takes the slot or a
    /// clear abandons it, which is what a hide, a snap, a re-apply and a
    /// re-dock do (D8, spec "Snap interrupts").
    #[test]
    fn a_new_tween_and_a_clear_drop_queued_frames() {
        let renamed = Slot::new();
        let queued = renamed.generation();
        renamed.set_driver(Driver {
            tween: tween(),
            begun: Instant::now(),
        });
        assert!(!renamed.is_current(queued), "a new tween drops old frames");

        let superseded = Slot::new();
        let queued = superseded.generation();
        superseded.clear();
        assert!(
            !superseded.is_current(queued),
            "a snap or re-dock drops queued frames"
        );
    }
}
