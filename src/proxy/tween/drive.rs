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
//! tween runs, and this module says how long it may sleep. The grow's 500 ms
//! wait needs no frames, so it wakes on its deadline (or on the app's
//! commit, which arrives through the state's own dispatch).
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
use super::{Fraction, Frame, GROW_TIMEOUT_US, PROXY_SERIAL_BASE, Step, Tween};
use crate::geometry::{Geometry, HeldGap};

/// How long the panel loop sleeps between two tween frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// One panel's tween state (D8): the tween the driver runs, if any, the next
/// serial the proxy mints for a configure it sends the app itself, and the
/// generation a queued frame is told apart by.
pub(in crate::proxy) struct Slot {
    driver: RefCell<Option<Driver>>,
    next_serial: Cell<u32>,
    /// The tween that owns the frames (D8). A frame waits in the commit
    /// coordinator while the app's surface is dirty, and the state it carries
    /// is stale once a newer owner has sent layer state: a hide or the panel
    /// window's end cleared the tween ([`Slot::clear`]), a new tween replaced
    /// it ([`Slot::set_driver`]), or a change snapped, re-applied or re-docked
    /// outside the coordinator ([`Slot::supersede`]). Each of those bumps it,
    /// so a frame captured before the bump sees [`Slot::is_current`] fail and
    /// does nothing. A tween that ends naturally keeps the generation, so its
    /// final frame, still queued, is applied.
    generation: Cell<u64>,
}

impl Slot {
    /// A panel that runs no tween.
    pub(in crate::proxy) fn new() -> Self {
        Self {
            driver: RefCell::new(None),
            next_serial: Cell::new(PROXY_SERIAL_BASE),
            generation: Cell::new(0),
        }
    }

    /// Abandons the tween the slot holds (D8): a hide or the panel window's
    /// end takes it away before it reached its target, so a frame already
    /// queued in the commit coordinator must not re-send the layer state it
    /// captured over the state the abandoning event sent.
    pub(in crate::proxy) fn clear(&self) {
        self.bump();
        self.driver.borrow_mut().take();
    }

    /// Forgets a tween that reached its target (D8): the state it ended on is
    /// the steady state, so its last frame — which may still wait in the
    /// commit coordinator — stays valid and must still be applied. A frame of
    /// a later owner is told apart by [`Slot::supersede`]'s bump.
    fn finish(&self) {
        self.driver.borrow_mut().take();
    }

    /// Invalidates every frame the slot's tween queued (D8): a change that
    /// sent layer state outside the commit coordinator — a snap apply, a
    /// re-apply after an output change, or a re-dock — has taken the panel
    /// over, so a frame still waiting must not re-send the state it captured.
    pub(in crate::proxy) fn supersede(&self) {
        self.bump();
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

    /// The next serial of the proxy's own configures (D8), always at or above
    /// the proxy's base so the app's ack is recognised and swallowed.
    fn mint(&self) -> u32 {
        let serial = self.next_serial.get();
        self.next_serial
            .set(serial.wrapping_add(1).max(PROXY_SERIAL_BASE));
        serial
    }

    /// Whether a tween runs (D8).
    fn is_running(&self) -> bool {
        self.driver
            .borrow()
            .as_ref()
            .is_some_and(|driver| driver.tween.is_tweening())
    }

    /// The running tween's driver, so the frame step and a commit can look
    /// at it.
    fn driver(&self) -> RefMut<'_, Option<Driver>> {
        self.driver.borrow_mut()
    }

    /// Keeps the tween a change just started (D8), invalidating the frames
    /// of the tween it replaces.
    fn set_driver(&self, driver: Driver) {
        self.bump();
        *self.driver.borrow_mut() = Some(driver);
    }
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
/// (D8). `false` when the change snaps instead: no duration, a hidden panel,
/// no panel window, a running tween (row 10.2 retargets it), or a size the
/// easing cannot carry. A grow sends the app its one new configure here.
pub(in crate::proxy) fn start(
    routing: &Rc<Routing>,
    from: u32,
    geometry: Geometry,
    duration_ms: u32,
) -> bool {
    if duration_ms == 0 || routing.hidden.is_hidden() || routing.panel_window().is_none() {
        return false;
    }
    let globals = routing.globals();
    if globals.tween().is_running() {
        return false;
    }
    let grow = geometry.across() > from;
    let surface = match app_surface(routing) {
        Some(surface) => surface,
        // D8: a grow that cannot see the app's size never knows when the
        // wide buffer arrives, so it snaps.
        None if grow => return false,
        // D8: the pair is width first, like every other surface extent the
        // composition reads, so a top or bottom panel's length across its
        // edge is the height.
        None => {
            let length = panel_length(routing, geometry);
            if geometry.edge().across_is_width() {
                (from, length)
            } else {
                (length, from)
            }
        }
    };
    let resting = routing
        .held_gap()
        .unwrap_or_else(|| HeldGap::start(geometry));
    let Some(tween) = Tween::start(geometry, from, surface, resting, grow, duration_ms) else {
        return false;
    };
    globals.tween().set_driver(Driver {
        tween,
        begun: Instant::now(),
    });
    // D8: a grow configures the app once to the target and then waits; a
    // shrink configures it when the last frame arrives.
    if grow {
        let serial = globals.tween().mint();
        send_configure(routing, geometry, serial);
    }
    true
}

/// Applies the frames a running tween owes now, and returns how long the
/// panel loop may sleep before the next one (D8). `None` when no tween runs.
///
/// A waiting grow returns the time left before its 500 ms snap; a running
/// tween returns one frame interval. The last frame is applied before the
/// tween is dropped, so the viewport composition still crops to its fraction
/// and the steady state then takes over.
pub(in crate::proxy) fn advance(routing: &Rc<Routing>) -> Option<Duration> {
    let globals = routing.globals();
    let (frame, finished, shrink, geometry) = {
        let slot = globals.tween();
        let mut driver = slot.driver();
        let driver = driver.as_mut()?;
        let elapsed = micros(driver.begun.elapsed());
        let shrink = driver.tween.is_shrink();
        let geometry = driver.tween.geometry();
        match driver.tween.step(elapsed) {
            Step::Held => {
                let left = (GROW_TIMEOUT_US - elapsed).max(0);
                return Some(Duration::from_micros(u64::try_from(left).unwrap_or(0)));
            }
            Step::Frame(frame) => (frame, false, shrink, geometry),
            Step::Finished(frame) => (frame, true, shrink, geometry),
        }
    };
    apply(routing, frame);
    if !finished {
        return Some(FRAME_INTERVAL);
    }
    // D8: a shrink ends by configuring the app to the target width. A grow
    // already did at its start, and a snapped grow never needs it.
    if shrink {
        let serial = globals.tween().mint();
        send_configure(routing, geometry, serial);
    }
    // D8: the tween ended on its target, so the last frame it queued is the
    // steady state and stays valid; a frame of a later owner is dropped by
    // that owner's own bump.
    globals.tween().finish();
    None
}

/// The app's commit: a waiting grow starts moving once the app reports the
/// target length across the tween axis (D8 finding 4: checked at the commit,
/// because a toolkit reports its new window geometry after its attach).
pub(in crate::proxy) fn on_app_commit(routing: &Routing) {
    let globals = routing.globals();
    let reported = {
        let slot = globals.tween();
        let driver = slot.driver();
        let Some(driver) = driver.as_ref() else {
            return;
        };
        app_surface(routing).map(|(width, height)| {
            if driver.tween.geometry().edge().across_is_width() {
                width
            } else {
                height
            }
        })
    };
    let slot = globals.tween();
    let mut driver = slot.driver();
    if let Some(driver) = driver.as_mut() {
        driver.tween.app_commit(reported);
    }
}

impl Routing {
    /// The app surface's extent, width first, the viewport composition crops
    /// (D8) while a tween runs and the app reports no window geometry.
    pub(in crate::proxy) fn tween_surface(&self) -> Option<(u32, u32)> {
        let globals = self.globals();
        let slot = globals.tween();
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
    let gap = {
        let driver = slot.driver();
        let Some(driver) = driver.as_ref() else {
            return;
        };
        driver.tween.final_gap()
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
    /// The visible fraction the viewport composition crops to (D8): the
    /// running tween's, or none when no tween runs.
    pub(in crate::proxy) fn tween_fraction(&self) -> Option<Fraction> {
        let globals = self.globals();
        let slot = globals.tween();
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
    let output = routing.known_output();
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

    use super::{Driver, Slot, Tween};
    use crate::geometry::{Edge, Geometry, Gutters, HeldGap, ReserveMode};

    /// A pushing geometry of `across` pixels on `edge` with no gutters (D9).
    fn push(edge: Edge, across: u32) -> Geometry {
        Geometry::new(edge, across, Gutters::new(0, 0, 0, 0), ReserveMode::Push)
    }

    /// A 200 ms grow of the panel from 400 to 600 pixels (D8).
    fn tween() -> Tween {
        Tween::start(
            push(Edge::Left, 600),
            400,
            (400, 1080),
            HeldGap::start(push(Edge::Left, 400)),
            true,
            200,
        )
        .expect("a 200 ms grow")
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

    /// A frame queued by a tween is stale once a new tween takes the slot, a
    /// hide abandons the tween, or a direct send supersedes it (D8).
    #[test]
    fn a_new_tween_a_hide_and_a_supersede_drop_queued_frames() {
        let renamed = Slot::new();
        let queued = renamed.generation();
        renamed.set_driver(Driver {
            tween: tween(),
            begun: Instant::now(),
        });
        assert!(!renamed.is_current(queued), "a new tween drops old frames");

        let hidden = Slot::new();
        let queued = hidden.generation();
        hidden.clear();
        assert!(!hidden.is_current(queued), "a hide drops queued frames");

        let superseded = Slot::new();
        let queued = superseded.generation();
        superseded.supersede();
        assert!(
            !superseded.is_current(queued),
            "a direct layer-state send drops queued frames"
        );
    }
}
