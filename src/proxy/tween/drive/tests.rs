//! The tween driver's pure tests (D8, D5): the slot's generation, the
//! waiting handshake, the gap a retarget continues from and the frames a new
//! tween or a clear drops.

use std::time::Instant;

use super::{Continuation, Driver, Frame, GrowHandshake, Pending, Slot, Step, Tween};
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

/// The frame of a step.
fn frame(step: Step) -> Frame {
    match step {
        Step::Frame(frame) | Step::Finished(frame) => frame,
    }
}

/// A retarget of a running strip tween to a covering target holds the
/// strip the panel shows now, not the strip it held before the first
/// change (D8 "Retarget", D5 rule A).
#[test]
fn a_retarget_to_a_covering_target_holds_the_strip_it_shows_now() {
    let slot = Slot::new();
    let mut tween = tween();
    let mid = frame(tween.step(100_000));
    assert!(
        mid.gap.zone() > 400,
        "the strip moved with the push: {mid:?}"
    );
    let applied = tween.current_gap();
    slot.set_driver(Driver {
        tween,
        begun: Instant::now(),
    });
    let mut retarget = Tween::start(
        Geometry::new(
            Edge::Left,
            300,
            Gutters::new(0, 0, 0, 0),
            ReserveMode::Cover,
        ),
        slot.running().expect("a running tween"),
        200,
    )
    .expect("a retarget");
    let first = frame(retarget.step(0));
    assert_eq!(
        first.gap.zone(),
        applied.zone(),
        "the retarget holds the strip the panel shows now"
    );
    let last = frame(retarget.step(200_000));
    assert_eq!(
        last.gap.zone(),
        applied.zone(),
        "the covering target ends on that strip"
    );
}

/// A retarget back to a length whose strip equals the old resting strip
/// still follows the strip, because the panel pushed before the change
/// (D8 "Retarget", D5 rule A).
#[test]
fn a_retarget_back_to_the_resting_strip_follows_the_strip() {
    let slot = Slot::new();
    let mut tween = tween();
    let mid = frame(tween.step(100_000));
    assert!(
        mid.gap.zone() > 400,
        "the strip moved with the push: {mid:?}"
    );
    let applied = tween.current_gap();
    let visible = tween.visible();
    slot.set_driver(Driver {
        tween,
        begun: Instant::now(),
    });
    let mut retarget = Tween::start(
        push(Edge::Left, 400),
        slot.running().expect("a running tween"),
        200,
    )
    .expect("a retarget");
    let first = frame(retarget.step(0));
    assert_eq!(first.visible, visible);
    assert_eq!(
        first.gap.zone(),
        applied.zone(),
        "the strip follows the shrink from where it is"
    );
    let last = frame(retarget.step(200_000));
    assert_eq!(last.gap.zone(), 400, "it ends on the resting strip");
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
