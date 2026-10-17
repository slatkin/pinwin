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

/// One panel's tween state (D8): the tween the driver runs, if any, and the
/// next serial the proxy mints for a configure it sends the app itself.
pub(in crate::proxy) struct Slot {
    driver: RefCell<Option<Driver>>,
    next_serial: Cell<u32>,
}

impl Slot {
    /// A panel that runs no tween.
    pub(in crate::proxy) fn new() -> Self {
        Self {
            driver: RefCell::new(None),
            next_serial: Cell::new(PROXY_SERIAL_BASE),
        }
    }

    /// Forgets a tween whose panel window went away (D8): its frames would
    /// have nothing to apply.
    pub(in crate::proxy) fn clear(&self) {
        self.driver.borrow_mut().take();
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

    /// Keeps the tween a change just started (D8).
    fn set_driver(&self, driver: Driver) {
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
        None => (from, panel_length(routing, geometry)),
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
    globals.tween().clear();
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
    /// The app surface's extent the viewport composition crops (D8) while a
    /// tween runs and the app reports no window geometry.
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
    let Some(driver) = slot.driver().take() else {
        return;
    };
    let gap = driver.tween.final_gap();
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

/// The app surface's extent in surface-local pixels (D8): the window
/// geometry the app reported, or its own viewport destination on the panel
/// surface. `None` when it reports neither.
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
    let routing = Rc::clone(routing);
    commits.request(Box::new(move || {
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
