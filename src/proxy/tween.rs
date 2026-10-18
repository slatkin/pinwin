//! The animated width transition's state machine (replace-terminal-with-app-host
//! row 10.1, D8): the grow that waits for the app's wide buffer, the shrink
//! that crops the buffer the app already has, and the per-frame values the
//! proxy commits.
//!
//! D8 keeps the method the terminal panel used and the step of
//! [`anim`](super::anim):
//!
//! - **Grow**: the proxy configures the app once to the target width and
//!   holds the current visible width until the app commits a buffer whose
//!   reported size reaches the target — checked at the commit, because a
//!   toolkit reports its new window geometry after its attach (D8 finding 4).
//!   The wait is bounded at 500 ms, after which the panel snaps. Every frame
//!   then crops the docked-edge part of the wide buffer.
//! - **Shrink**: the app keeps the wide buffer it has and the proxy crops it
//!   down each frame; at the end the proxy configures the app to the target
//!   width.
//!
//! Each frame carries the layer surface's size, the margins, the reserve
//! surface's new zone when the gap moves, and the visible [fraction](Fraction)
//! the viewport composition crops to. The driver ([`drive`]) applies them
//! through the commit coordinator (D7) and runs the frame clock.
//!
//! "One size per tween" (D8): while the tween runs, the compositor's
//! intermediate layer-surface configures are acked by the proxy itself and
//! never forwarded ([`route_configure`]), so the app gets exactly one new
//! window size for the whole tween. That configure carries a serial the proxy
//! minted itself ([`MintedSerials`]), and the app's ack of it is swallowed
//! ([`Slot::swallow_ack`]) because the layer surface never saw it.
//!
//! The machine does not read a clock: [`Tween::step`] takes the microseconds
//! since the tween began, so a test drives it without a display
//! (`port-to-rust` D10). Row 10.2 adds the end and interrupt rules:
//! [`duration_ms`] resolves the request's `ms` or the panel's `tween_ms` and
//! clamps it, a retarget continues from [`Tween::visible`], a snap or a
//! re-dock drops the driving tween ([`Slot::clear`]), and the eased clock
//! starts with the change, so a stalled frame clock still lands on the exact
//! target.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use super::anim::{Advance, Tween as Anim};
use crate::geometry::{Edge, Geometry, HeldGap, Margins};

use super::compose::Fraction;

mod drive;

pub(super) use drive::{Slot, advance, is_running, on_app_commit, start, stop};

/// The first serial the proxy mints for a configure it sends the app itself
/// (D8). Serial values are opaque, so this starting point only makes a
/// collision with a compositor's serial unlikely; the ack route reads
/// [`MintedSerials`] membership, never the value.
const FIRST_MINTED_SERIAL: u32 = 1 << 31;

/// How many minted serials the proxy remembers at once (D8). The app may ack
/// a configure long after the tween that sent it ended, so an unacked serial
/// cannot be forgotten at the tween's end; the bound keeps a client that
/// never acks from growing the set without limit.
const MINTED_LIMIT: usize = 64;

/// How long a grow waits for the app's wide buffer before it snaps (D8): the
/// spec's "Grow waits for the app" bound.
pub(super) const GROW_TIMEOUT_US: i64 = 500_000;

/// The longest duration a change animates over (D8, spec "Duration clamp"):
/// a longer request is clamped to it.
pub(super) const MAX_DURATION_MS: u32 = 1000;

/// The duration a change runs with (D8, spec "Duration clamp", "Snap when
/// animations are disabled"): the request's `ms` or the panel's `tween_ms`,
/// clamped to [`MAX_DURATION_MS`]. A zero duration snaps.
pub(super) fn duration_ms(request_ms: Option<u32>, panel_ms: u32) -> u32 {
    request_ms.unwrap_or(panel_ms).min(MAX_DURATION_MS)
}

/// Whether a change can animate at all (D8, spec "Snap when animations are
/// disabled", "No viewport support"): the panel has a window, is shown and
/// has the viewporter its crop needs. A panel without one snaps.
pub(super) const fn animates(has_window: bool, hidden: bool, has_viewporter: bool) -> bool {
    has_window && !hidden && has_viewporter
}

/// The app surface's length across the tween axis (D8): the width on a left
/// or right panel, the height on a top or bottom one.
const fn across(edge: Edge, surface: (u32, u32)) -> u32 {
    if edge.across_is_width() {
        surface.0
    } else {
        surface.1
    }
}

/// Whether a change must configure the app to its target and wait for the
/// wide buffer (D8, spec "Grow waits for the app"): a target longer than the
/// app's surface across the tween axis. A shrink, and a retarget that stays
/// inside the buffer the app already has, crop that buffer instead.
pub(super) fn waits_for_app(edge: Edge, surface: (u32, u32), target: u32) -> bool {
    across(edge, surface) < target
}

/// The serials the proxy minted for its own configures and the app has not
/// acked (D8). The layer surface never saw these, so an ack that names one is
/// swallowed. Membership, not a value range, decides the route: a
/// compositor's serials are opaque and may be larger than any minted one.
#[derive(Debug)]
pub(super) struct MintedSerials {
    next: Cell<u32>,
    live: RefCell<VecDeque<u32>>,
}

impl MintedSerials {
    /// No serial minted yet.
    pub(super) fn new() -> Self {
        Self {
            next: Cell::new(FIRST_MINTED_SERIAL),
            live: RefCell::new(VecDeque::new()),
        }
    }

    /// The next serial for a configure the proxy sends the app itself. The
    /// value is never reused while it is still live, so a wrapped counter
    /// cannot mint a serial twice.
    pub(super) fn mint(&self) -> u32 {
        let mut live = self.live.borrow_mut();
        loop {
            let serial = self.next.get();
            self.next.set(if serial == u32::MAX {
                FIRST_MINTED_SERIAL
            } else {
                serial + 1
            });
            if live.contains(&serial) {
                continue;
            }
            live.push_back(serial);
            while live.len() > MINTED_LIMIT {
                live.pop_front();
            }
            return serial;
        }
    }

    /// Whether `serial` names a configure the proxy minted (D8): true when it
    /// does, and the serial is forgotten with every earlier minted serial,
    /// because an ack consumes them all.
    pub(super) fn swallow(&self, serial: u32) -> bool {
        let mut live = self.live.borrow_mut();
        let Some(index) = live.iter().position(|minted| *minted == serial) else {
            return false;
        };
        live.drain(..=index);
        true
    }

    /// Forgets every minted serial (D8): the app's `xdg_surface` is gone, so
    /// no ack can still name one.
    pub(super) fn reset(&self) {
        self.live.borrow_mut().clear();
    }
}

/// What the proxy does with one compositor layer-surface configure (D8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigureRoute {
    /// A tween runs: the proxy acks the intermediate configure itself and
    /// forwards nothing, so the app gets one new size for the whole tween.
    AckOnly,
    /// The configure goes to the app as its window configure.
    Forward,
}

/// The route of one compositor configure (D8).
pub(super) fn route_configure(tweening: bool) -> ConfigureRoute {
    if tweening {
        ConfigureRoute::AckOnly
    } else {
        ConfigureRoute::Forward
    }
}

/// Where the tween stands (D8). Row 10.2 adds the interrupt and retarget
/// transitions between these phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// A grow: the app was configured to the target and the panel holds its
    /// current width until the app commits a wide buffer.
    Waiting,
    /// The eased width moves from the start length to the target.
    Running,
    /// The eased width reached the target; the driver applies this frame and
    /// drops the tween.
    Finished,
}

/// What one [`Tween::step`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// The grow still waits for the app's wide buffer: nothing changes.
    Held,
    /// A frame to apply at the eased width.
    Frame(Frame),
    /// The last frame: the eased width reached the target exactly.
    Finished(Frame),
}

/// One frame's values (D8): what the driver sends before its commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Frame {
    visible: u32,
    layer: (u32, u32),
    margins: Margins,
    gap: HeldGap,
    fraction: Fraction,
}

impl Frame {
    /// The layer surface's size for `set_size`: the visible length across
    /// the edge and 0 along it, which the compositor fills between the end
    /// anchors.
    pub(super) const fn layer_size(self) -> (u32, u32) {
        self.layer
    }

    /// The layer surface's margins, unchanged by a width tween but re-sent
    /// with each frame as D8 lists it.
    pub(super) const fn margins(self) -> Margins {
        self.margins
    }

    /// The gap the reserve surface carries this frame (D8, D9): it follows a
    /// pushing panel and holds still for a covering one.
    pub(super) const fn gap(self) -> HeldGap {
        self.gap
    }
}

/// One running width tween (D8). Built only by [`Tween::start`], so the
/// phase and the eased step always agree (`port-to-rust` D6).
#[derive(Clone, Copy, Debug)]
pub(super) struct Tween {
    /// The target geometry: the edge, the target across and the gutters the
    /// frames resolve their margins and strip against.
    geometry: Geometry,
    from: u32,
    to: u32,
    /// The app surface's extent the visible fraction is taken over (D8):
    /// the window geometry the app reports, its own viewport destination, or
    /// the size the panel configures. Width first, height second — the
    /// surface-local order the proxy's window geometry, viewport destination
    /// and panel size all carry; [`Tween::across`] picks the axis by the
    /// edge, so a top or bottom panel crops against the height.
    surface: (u32, u32),
    phase: Phase,
    anim: Anim,
    /// The gap the panel held before the change: a covering target keeps it
    /// and a pushing one moves it.
    resting_gap: HeldGap,
    /// Whether the gap moves with the panel (D9): only a pushing target whose
    /// strip differs from the resting strip.
    gap_tweening: bool,
    visible: u32,
}

impl Tween {
    /// The tween of a change from `from` to `geometry.across()` over
    /// `duration_ms`. `surface` is the app's current extent in surface-local
    /// pixels, width first, the source of the visible fraction's denominator.
    /// `grow` starts in the wait for the app's wide buffer; a shrink starts
    /// moving at once.
    /// `None` for a change with no distance, no duration or a size the
    /// easing's integer step cannot carry.
    ///
    /// `wait` starts the tween in the wait for the app's wide buffer, so the
    /// caller has configured the app to the target; a change inside the
    /// buffer the app already has starts moving at once.
    pub(super) fn start(
        geometry: Geometry,
        from: u32,
        surface: (u32, u32),
        resting_gap: HeldGap,
        wait: bool,
        duration_ms: u32,
    ) -> Option<Self> {
        let to = geometry.across();
        if from == to || duration_ms == 0 {
            return None;
        }
        let mut anim = Anim::begin(
            i32::try_from(from).ok()?,
            i32::try_from(to).ok()?,
            duration_ms.min(MAX_DURATION_MS),
        );
        // D8: the eased clock starts with the change, not with its first
        // frame, so a frame clock that stalls for the duration still lands on
        // the target (spec "Stalled frame clock"). A waiting grow starts its
        // clock when the app's buffer arrives.
        if !wait {
            anim.advance(0);
        }
        Some(Self {
            geometry,
            from,
            to,
            surface: (surface.0.max(1), surface.1.max(1)),
            phase: if wait { Phase::Waiting } else { Phase::Running },
            anim,
            resting_gap,
            gap_tweening: HeldGap::tweens_with_panel(resting_gap.zone(), true, geometry, to),
            visible: from,
        })
    }

    /// The target geometry the tween resolves its frames against.
    pub(super) const fn geometry(self) -> Geometry {
        self.geometry
    }

    /// The length across the edge the panel is moving to. Rows 10.2's
    /// retarget reads it.
    #[cfg(test)]
    pub(super) const fn target(self) -> u32 {
        self.to
    }

    /// Where the tween stands (D8).
    #[cfg(test)]
    pub(super) const fn phase(self) -> Phase {
        self.phase
    }

    /// Whether the tween still owns the frames: it runs while it waits for
    /// the app's wide buffer, moves and has not yet applied its last frame.
    pub(super) const fn is_tweening(self) -> bool {
        !matches!(self.phase, Phase::Finished)
    }

    /// The length the panel shows now (D8): what a retarget continues from
    /// (spec "Retarget").
    pub(super) const fn visible(self) -> u32 {
        self.visible
    }

    /// The gap the panel holds now (D8): what a retarget to a covering target
    /// keeps still.
    pub(super) fn current_gap(self) -> HeldGap {
        self.resting_gap
            .reserve(self.gap_tweening, self.geometry, self.visible)
    }

    /// Whether the app still draws wider than the target, so the last frame
    /// must configure it down (D8): a shrink, or a retarget that ends inside a
    /// wider buffer.
    pub(super) fn shrinks_app(self) -> bool {
        self.across() > self.to
    }

    /// The app surface's extent the composition crops (D8), while no window
    /// geometry reports a fresh one.
    pub(super) const fn surface(self) -> (u32, u32) {
        self.surface
    }

    /// The app surface's length across the tween axis (D8).
    fn across(self) -> u32 {
        across(self.geometry.edge(), self.surface)
    }

    /// The visible fraction of the app surface the viewport composition
    /// crops to now (D8): the eased length over the surface's length across
    /// the edge.
    pub(super) fn fraction(self) -> Fraction {
        Fraction::of(self.visible, self.across()).unwrap_or_else(Fraction::whole)
    }

    /// The gap the reserve holds once the tween ends: a pushing target's own
    /// strip at the target length, a covering target's resting gap.
    pub(super) fn final_gap(self) -> HeldGap {
        self.resting_gap
            .reserve(self.gap_tweening, self.geometry, self.to)
    }

    /// One step at `elapsed_us` microseconds after the tween began (D8).
    ///
    /// A waiting grow holds for at most [`GROW_TIMEOUT_US`] and then snaps to
    /// the target. A running tween eases with [`crate::anim`] and finishes
    /// exactly on the target, so the last frame's length is the target and
    /// nothing re-rounds it.
    pub(super) fn step(&mut self, elapsed_us: i64) -> Step {
        match self.phase {
            Phase::Waiting => {
                if elapsed_us >= GROW_TIMEOUT_US {
                    self.visible = self.to;
                    self.phase = Phase::Finished;
                    return Step::Finished(self.frame());
                }
                Step::Held
            }
            Phase::Running => match self.anim.advance(elapsed_us) {
                Advance::Frame(px) => {
                    self.visible = px
                        .max(0)
                        .unsigned_abs()
                        .clamp(self.from.min(self.to), self.from.max(self.to));
                    Step::Frame(self.frame())
                }
                Advance::Finished => {
                    self.visible = self.to;
                    self.phase = Phase::Finished;
                    Step::Finished(self.frame())
                }
            },
            Phase::Finished => Step::Finished(self.frame()),
        }
    }

    /// The app committed a surface: a waiting grow starts moving once the
    /// length the app reports across the tween axis reaches the target (D8
    /// finding 4). The eased clock starts at `elapsed_us` microseconds after
    /// the change, so a stalled frame clock still spends the duration from
    /// the commit. `false` for a shrink or a grow that already waits no
    /// more.
    pub(super) fn app_commit(&mut self, reported: Option<u32>, elapsed_us: i64) -> bool {
        if self.phase != Phase::Waiting {
            return false;
        }
        let Some(reported) = reported else {
            return false;
        };
        if reported < self.to {
            return false;
        }
        let reported = reported.max(1);
        if self.geometry.edge().across_is_width() {
            self.surface.0 = reported;
        } else {
            self.surface.1 = reported;
        }
        self.phase = Phase::Running;
        self.anim.advance(elapsed_us);
        true
    }

    /// This frame's values (D8).
    fn frame(self) -> Frame {
        Frame {
            visible: self.visible,
            layer: self.geometry.size_at(self.visible),
            margins: self.geometry.margins(),
            gap: self
                .resting_gap
                .reserve(self.gap_tweening, self.geometry, self.visible),
            fraction: self.fraction(),
        }
    }
}

#[cfg(test)]
mod tests;
