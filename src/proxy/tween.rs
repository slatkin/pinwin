//! The animated width transition's state machine (replace-terminal-with-app-host
//! row 10.1, D8): the grow's configure handshake and the animation it starts,
//! the shrink that crops the buffer the app already has, and the per-frame
//! values the proxy commits.
//!
//! D8 keeps the method the terminal panel used and the step of
//! [`anim`](super::anim):
//!
//! - **Grow**: the proxy configures the app once to the target width and
//!   holds the current visible width until the app commits a buffer whose
//!   reported size reaches the target — checked at the commit, because a
//!   toolkit reports its new window geometry after its attach (D8 finding 4).
//!   That wait is the configure handshake ([`GrowHandshake`]) and its answer
//!   is a result of its own: an answered handshake starts the animation from
//!   the commit, and one the app never answers settles on no wide buffer
//!   after [`GROW_HANDSHAKE_US`], which snaps the change. Every frame then
//!   crops the docked-edge part of the wide buffer.
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
use crate::geometry::{Edge, Geometry, HeldGap, Margins, ReserveMode};

use super::compose::Fraction;

mod drive;

pub(super) use drive::{Slot, advance, is_running, on_app_commit, start, stop};

/// The first serial the proxy mints for a configure it sends the app itself
/// (D8). Serial values are opaque, so this starting point only makes a
/// collision with a compositor's serial unlikely; the ack route reads the
/// proxy's own minted values, never a range of someone else's.
const FIRST_MINTED_SERIAL: u32 = 1 << 31;

/// How many minted serials the proxy remembers at once (D8). The app may ack
/// a configure long after the tween that sent it ended, so an unacked serial
/// cannot be forgotten at the tween's end; the bound keeps a client that
/// never acks from growing the set without limit.
const MINTED_LIMIT: usize = 64;

/// How long a grow's configure handshake waits for the app's wide buffer
/// before it settles on no wide buffer (D8): the spec's "Grow waits for the
/// app" bound.
pub(super) const GROW_HANDSHAKE_US: i64 = 500_000;

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

/// What a grow's configure handshake settled on (D8): the app answered with
/// a buffer for the configure, or it never answered within
/// [`GROW_HANDSHAKE_US`]. The result is first-class, so [`Tween`] is only the
/// animation an answer starts and owns no wait or timeout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GrowResult {
    /// The app committed a buffer that reports the target: the grow animates
    /// from that surface.
    Answered { surface: (u32, u32) },
    /// The app never answered within the bound: the change snaps.
    Unanswered,
}

/// The state a change continues from (D8, D5): the length the panel shows,
/// the app surface the crop reads, the gap the panel holds and the reserve
/// mode the change started from. A retarget takes all four from the running
/// change and a fresh change takes them from its plan, so they travel as one
/// value: a path that misses one of them cannot compile.
#[derive(Clone, Copy, Debug)]
pub(super) struct Continuation {
    /// The length across the edge the panel shows now.
    pub(super) from: u32,
    /// The app surface's extent the crop reads, width first (D8).
    pub(super) surface: (u32, u32),
    /// The gap the panel held before the change.
    pub(super) resting_gap: HeldGap,
    /// The reserve mode before the change (D5): only a panel that pushed
    /// before the change moves its strip with the animation.
    pub(super) reserve: ReserveMode,
}

/// The configure handshake of a grow (D8): the proxy configured the app once
/// to the target and holds the panel's current length until the app commits
/// a buffer that reports that size. The handshake owns the bound; the tween
/// it starts only runs the eased animation.
#[derive(Clone, Copy, Debug)]
pub(super) struct GrowHandshake {
    /// The target geometry the frames resolve against.
    geometry: Geometry,
    /// The length the panel shows, the surface the crop reads, the gap it
    /// holds and the reserve mode it started from.
    continuation: Continuation,
    /// The duration the answered grow animates over.
    duration_ms: u32,
}

impl GrowHandshake {
    /// The handshake of a grow from the length `continuation` names to
    /// `geometry.across()` (D8), with the app surface the crop reads until
    /// the answer arrives.
    pub(super) fn new(
        geometry: Geometry,
        mut continuation: Continuation,
        duration_ms: u32,
    ) -> Self {
        continuation.surface = (continuation.surface.0.max(1), continuation.surface.1.max(1));
        Self {
            geometry,
            continuation,
            duration_ms,
        }
    }

    /// The target geometry the answered grow resolves its frames against.
    pub(super) const fn geometry(self) -> Geometry {
        self.geometry
    }

    /// The state the change continues from (D8, D5): a retarget takes all of
    /// it from the running change.
    pub(super) const fn continuation(self) -> Continuation {
        self.continuation
    }

    /// The app surface's extent the crop reads while the handshake waits
    /// (D8).
    pub(super) const fn surface(self) -> (u32, u32) {
        self.continuation.surface
    }

    /// The visible fraction of the app surface the crop shows while the
    /// handshake waits (D8): the panel's current length over the surface's
    /// length across the edge.
    pub(super) fn fraction(self) -> Fraction {
        Fraction::of(
            self.continuation.from,
            across(self.geometry.edge(), self.continuation.surface),
        )
        .unwrap_or_else(Fraction::whole)
    }

    /// The gap the reserve holds once the grow ends (D5): a pushing target's
    /// own strip at the target length, a covering target's resting gap.
    pub(super) fn final_gap(self) -> HeldGap {
        final_gap(self.continuation.resting_gap, self.geometry)
    }

    /// The handshake's answer to the app's commit (D8 finding 4): `Answered`
    /// once the length the app reports across the tween axis reaches the
    /// target, `None` while it does not.
    pub(super) fn on_commit(self, reported: Option<u32>) -> Option<GrowResult> {
        let reported = reported?;
        if reported < self.geometry.across() {
            return None;
        }
        let reported = reported.max(1);
        let mut surface = self.continuation.surface;
        if self.geometry.edge().across_is_width() {
            surface.0 = reported;
        } else {
            surface.1 = reported;
        }
        Some(GrowResult::Answered { surface })
    }

    /// The handshake's result at `elapsed_us` microseconds after the change
    /// (D8): `Unanswered` once the bound is spent, `None` before it. The
    /// bound belongs to the handshake, not to the tween it starts.
    pub(super) fn on_deadline(elapsed_us: i64) -> Option<GrowResult> {
        (elapsed_us >= GROW_HANDSHAKE_US).then_some(GrowResult::Unanswered)
    }

    /// The animation an answered handshake starts from the surface it
    /// reports (D8). The caller starts its clock at the commit.
    pub(super) fn start(self, surface: (u32, u32)) -> Option<Tween> {
        Tween::start(
            self.geometry,
            Continuation {
                surface,
                ..self.continuation
            },
            self.duration_ms,
        )
    }

    /// The frame an unanswered handshake snaps to (D8): the target layer
    /// state with no wide buffer to crop, so the whole old surface shows.
    pub(super) fn snap(self) -> Frame {
        let visible = self.geometry.across();
        Frame {
            visible,
            layer: self.geometry.size_at(visible),
            margins: self.geometry.margins(),
            gap: self.final_gap(),
            fraction: Fraction::of(
                visible,
                across(self.geometry.edge(), self.continuation.surface),
            )
            .unwrap_or_else(Fraction::whole),
        }
    }
}

/// The serials the proxy minted for its own configures (D8). The layer
/// surface never saw these, so an ack that names one is swallowed. Every
/// value from [`FIRST_MINTED_SERIAL`] up to the counter was minted, so a
/// serial the bound forgot, or one a second ack names again, keeps the
/// proxy's route instead of reaching the layer surface. A compositor's
/// opaque serial is swallowed only where it collides with a minted value,
/// which the high starting point makes unlikely.
#[derive(Debug)]
pub(super) struct MintedSerials {
    next: Cell<u32>,
    live: RefCell<VecDeque<u32>>,
    /// Whether the counter wrapped, so the whole range from
    /// [`FIRST_MINTED_SERIAL`] up has been minted at least once.
    wrapped: Cell<bool>,
}

impl MintedSerials {
    /// No serial minted yet.
    pub(super) fn new() -> Self {
        Self {
            next: Cell::new(FIRST_MINTED_SERIAL),
            live: RefCell::new(VecDeque::new()),
            wrapped: Cell::new(false),
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
                self.wrapped.set(true);
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
    /// does. A live serial is forgotten with every earlier minted serial,
    /// because an ack consumes them all. A serial the bound already forgot,
    /// or one that a repeated ack names, is still swallowed: forwarding it
    /// would ack a serial the layer surface never configured.
    pub(super) fn swallow(&self, serial: u32) -> bool {
        let mut live = self.live.borrow_mut();
        let Some(index) = live.iter().position(|minted| *minted == serial) else {
            return self.was_minted(serial);
        };
        live.drain(..=index);
        true
    }

    /// Whether the proxy minted `serial` since the last [`Self::reset`] (D8):
    /// `next` names the first value not yet minted, and a counter that
    /// wrapped has minted the whole range.
    fn was_minted(&self, serial: u32) -> bool {
        serial >= FIRST_MINTED_SERIAL && (self.wrapped.get() || serial < self.next.get())
    }

    /// Forgets every minted serial (D8): the app's `xdg_surface` is gone, so
    /// no ack can still name one, and the next window's configures start a
    /// fresh range.
    pub(super) fn reset(&self) {
        self.live.borrow_mut().clear();
        self.next.set(FIRST_MINTED_SERIAL);
        self.wrapped.set(false);
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
    /// The eased width moves from the start length to the target.
    Running,
    /// The eased width reached the target; the driver applies this frame and
    /// drops the tween.
    Finished,
}

/// What one [`Tween::step`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
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
    to: u32,
    /// The length the tween started from, the app surface the visible
    /// fraction is taken over, the gap the panel held and the reserve mode
    /// the change started from. The surface is the window geometry the app
    /// reports, its own viewport destination, or the size the panel
    /// configures; width first, height second — the surface-local order the
    /// proxy's window geometry, viewport destination and panel size all
    /// carry, so [`Tween::across`] picks the axis by the edge and a top or
    /// bottom panel crops against the height.
    continuation: Continuation,
    phase: Phase,
    anim: Anim,
    /// Whether the gap moves with the panel (D9): only a pushing target whose
    /// strip differs from the resting strip.
    gap_tweening: bool,
    visible: u32,
}

impl Tween {
    /// The tween of a change from the length `continuation` names to
    /// `geometry.across()` over `duration_ms`. The continuation carries the
    /// app's current extent in surface-local pixels, width first, the source
    /// of the visible fraction's denominator, and the panel's mode before the
    /// change, which decides whether the strip moves with the frames (D5).
    /// `None` for a change with no distance, no duration or a size the
    /// easing's integer step cannot carry.
    ///
    /// A grow starts here only after its configure handshake answered
    /// ([`GrowHandshake::start`]); a shrink starts moving at once.
    pub(super) fn start(
        geometry: Geometry,
        mut continuation: Continuation,
        duration_ms: u32,
    ) -> Option<Self> {
        let from = continuation.from;
        let to = geometry.across();
        if from == to || duration_ms == 0 {
            return None;
        }
        continuation.surface = (continuation.surface.0.max(1), continuation.surface.1.max(1));
        let mut anim = Anim::begin(
            i32::try_from(from).ok()?,
            i32::try_from(to).ok()?,
            duration_ms.min(MAX_DURATION_MS),
        );
        // D8: the eased clock starts with the change, not with its first
        // frame, so a frame clock that stalls for the duration still lands on
        // the target (spec "Stalled frame clock").
        anim.advance(0);
        Some(Self {
            geometry,
            to,
            gap_tweening: HeldGap::tweens_with_panel(
                continuation.resting_gap.zone(),
                continuation.reserve,
                true,
                geometry,
                to,
            ),
            continuation,
            phase: Phase::Running,
            anim,
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

    /// Whether the tween still owns the frames: it moves and has not yet
    /// applied its last frame.
    pub(super) const fn is_tweening(self) -> bool {
        !matches!(self.phase, Phase::Finished)
    }

    /// The length the panel shows now (D8): what a retarget continues from
    /// (spec "Retarget").
    pub(super) const fn visible(self) -> u32 {
        self.visible
    }

    /// The state a retarget continues from (D8, D5): the length the panel
    /// shows now and the gap it holds now, on the surface it crops, with the
    /// reserve mode the change started from. A running strip tween therefore
    /// hands the retarget the strip it applied, not the one it started from.
    pub(super) fn retarget_continuation(self) -> Continuation {
        Continuation {
            from: self.visible(),
            resting_gap: self.current_gap(),
            ..self.continuation
        }
    }

    /// The gap the panel holds now (D8): what a retarget to a covering target
    /// keeps still.
    pub(super) fn current_gap(self) -> HeldGap {
        self.continuation
            .resting_gap
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
        self.continuation.surface
    }

    /// The app surface's length across the tween axis (D8).
    fn across(self) -> u32 {
        across(self.geometry.edge(), self.continuation.surface)
    }

    /// The visible fraction of the app surface the viewport composition
    /// crops to now (D8): the eased length over the surface's length across
    /// the edge.
    pub(super) fn fraction(self) -> Fraction {
        Fraction::of(self.visible, self.across()).unwrap_or_else(Fraction::whole)
    }

    /// The gap the reserve holds once the tween ends (D5): a pushing target's
    /// own strip at the target length, a covering target's resting gap.
    pub(super) fn final_gap(self) -> HeldGap {
        final_gap(self.continuation.resting_gap, self.geometry)
    }

    /// One step at `elapsed_us` microseconds after the tween began (D8).
    ///
    /// The tween eases with [`crate::anim`] and finishes exactly on the
    /// target, so the last frame's length is the target and nothing re-rounds
    /// it.
    pub(super) fn step(&mut self, elapsed_us: i64) -> Step {
        match self.phase {
            Phase::Running => match self.anim.advance(elapsed_us) {
                Advance::Frame(px) => {
                    self.visible = px.max(0).unsigned_abs().clamp(
                        self.continuation.from.min(self.to),
                        self.continuation.from.max(self.to),
                    );
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

    /// This frame's values (D8). The last frame carries the end gap, which
    /// the target mode decides (D5): a collapse to a pushing width sets the
    /// strip on that frame even though the frames before it held it.
    fn frame(self) -> Frame {
        let gap = if matches!(self.phase, Phase::Finished) {
            self.final_gap()
        } else {
            self.continuation
                .resting_gap
                .reserve(self.gap_tweening, self.geometry, self.visible)
        };
        Frame {
            visible: self.visible,
            layer: self.geometry.size_at(self.visible),
            margins: self.geometry.margins(),
            gap,
            fraction: self.fraction(),
        }
    }
}

/// The gap a change leaves at its target (D5): a pushing target lands on its
/// own strip, so a collapse from a covering panel still moves the strip once
/// the animation ends; a covering target holds the resting gap.
fn final_gap(resting_gap: HeldGap, geometry: Geometry) -> HeldGap {
    resting_gap.reserve(true, geometry, geometry.across())
}

#[cfg(test)]
mod tests;
