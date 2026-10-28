use super::*;
use crate::geometry::Gutters;

/// A panel geometry of `across` logical pixels on `edge`, with no
/// gutters (D9).
fn geometry(edge: Edge, across: u32, reserve: ReserveMode) -> Geometry {
    Geometry::new(edge, across, Gutters::new(0, 0, 0, 0), reserve)
}

/// A pushing geometry of `across` pixels on `edge`.
fn push(edge: Edge, across: u32) -> Geometry {
    geometry(edge, across, ReserveMode::Push)
}

/// A covering geometry of `across` pixels on `edge`.
fn cover(edge: Edge, across: u32) -> Geometry {
    geometry(edge, across, ReserveMode::Cover)
}

/// A `none` geometry of `across` pixels on `edge`.
fn none(edge: Edge, across: u32) -> Geometry {
    geometry(edge, across, ReserveMode::None)
}

/// The app surface `across` pixels across the tween axis on `edge`, with
/// 1080 along it: the width-first pair the driver hands the tween (D8).
fn surface(edge: Edge, across: u32) -> (u32, u32) {
    if edge.across_is_width() {
        (across, 1080)
    } else {
        (1080, across)
    }
}

/// The state a change continues from (D8, D5): the panel at `from` pixels
/// across the edge with the app surface still at that length, the gap it
/// holds and the reserve mode it ran in.
fn continuing(edge: Edge, from: u32, resting_gap: HeldGap, reserve: ReserveMode) -> Continuation {
    Continuation {
        from,
        surface: surface(edge, from),
        resting_gap,
        reserve,
    }
}

/// The configure handshake of a 200 ms grow from `from` to `to` on `edge`,
/// with the app surface at its old length.
fn growing(edge: Edge, from: u32, to: u32) -> GrowHandshake {
    GrowHandshake::new(
        push(edge, to),
        continuing(
            edge,
            from,
            HeldGap::start(push(edge, from)),
            ReserveMode::Push,
        ),
        200,
    )
}

/// The tween an answered handshake starts, from the surface the app
/// reported.
fn answered(handshake: GrowHandshake, surface: (u32, u32)) -> Tween {
    handshake.start(surface).expect("an answered grow")
}

/// A 200 ms shrink from `from` to `to` on `edge`, with the app surface
/// still at the wide length.
fn shrinking(edge: Edge, from: u32, to: u32) -> Tween {
    Tween::start(
        push(edge, to),
        continuing(
            edge,
            from,
            HeldGap::start(push(edge, from)),
            ReserveMode::Push,
        ),
        200,
    )
    .expect("a 200 ms shrink")
}

/// A 200 ms collapse of a covering panel from `from` to `to`, holding the
/// strip of the last pushing width of 400 pixels (D5).
fn collapsing(from: u32, to: u32) -> Tween {
    Tween::start(
        push(Edge::Left, to),
        continuing(
            Edge::Left,
            from,
            HeldGap::start(push(Edge::Left, 400)),
            ReserveMode::Cover,
        ),
        200,
    )
    .expect("a 200 ms collapse")
}

/// The frame of a step.
fn frame(step: Step) -> Frame {
    match step {
        Step::Frame(frame) | Step::Finished(frame) => frame,
    }
}

/// Grows and shrinks keep the panel at its start length while a grow
/// waits, then ease to the target. Both directions end exactly on the
/// target, on a left and on a top panel (D8, D9).
#[test]
fn grow_and_shrink_end_on_the_target_on_both_axes() {
    for edge in [Edge::Left, Edge::Top] {
        let handshake = growing(edge, 300, 450);
        assert_eq!(
            GrowHandshake::on_deadline(0),
            None,
            "the handshake waits first"
        );
        let Some(GrowResult::Answered { surface }) = handshake.on_commit(Some(450)) else {
            panic!("the target answers the handshake");
        };
        let mut grow = answered(handshake, surface);
        let first = frame(grow.step(0));
        assert_eq!(first.visible, 300, "the first frame is the start");
        let last = frame(grow.step(200_000));
        assert_eq!(last.visible, 450, "the grow ends on the target on {edge:?}");
        assert_eq!(
            last.layer,
            if edge.across_is_width() {
                (450, 0)
            } else {
                (0, 450)
            },
            "the layer size across the edge on {edge:?}"
        );

        let mut shrink = shrinking(edge, 450, 300);
        let _ = shrink.step(0);
        let last = frame(shrink.step(200_000));
        assert_eq!(
            last.visible, 300,
            "the shrink ends on the target on {edge:?}"
        );
        assert_eq!(shrink.phase(), Phase::Finished);
    }
}

/// The driver hands the tween a width-first surface pair, so a top or
/// bottom panel takes its length across the edge from the height: the
/// first frame of a shrink and of a grow crops against the panel's own
/// length, not the output's width (D8, spec "Content does not slide").
#[test]
fn a_top_panel_reads_the_across_length_from_the_height() {
    for edge in [Edge::Top, Edge::Bottom] {
        let mut shrink = shrinking(edge, 450, 300);
        let first = frame(shrink.step(0));
        assert_eq!(first.visible, 450);
        assert_eq!(
            first.fraction,
            Fraction::of(450, 450).expect("the whole height"),
            "the first shrink frame crops the height on {edge:?}"
        );
        // t = 0.5 -> ease(0.5) = 0.875 -> 450 - round(150 * 0.875).
        let mid = frame(shrink.step(100_000));
        assert_eq!(mid.visible, 319);
        assert_eq!(
            mid.fraction,
            Fraction::of(319, 450).expect("the docked part of the height"),
            "the mid shrink frame crops the height on {edge:?}"
        );

        let handshake = growing(edge, 300, 450);
        let Some(GrowResult::Answered { surface: wide }) = handshake.on_commit(Some(450)) else {
            panic!("the target answers the handshake");
        };
        let mut grow = answered(handshake, wide);
        assert_eq!(
            grow.surface(),
            surface(edge, 450),
            "the wide surface keeps the width first on {edge:?}"
        );
        let first = frame(grow.step(0));
        assert_eq!(
            first.fraction,
            Fraction::of(300, 450).expect("the start part of the height"),
            "the first grow frame crops the height on {edge:?}"
        );
        let last = frame(grow.step(200_000));
        assert_eq!(
            last.fraction,
            Fraction::of(450, 450).expect("the whole height"),
            "the grow ends on the whole height on {edge:?}"
        );
        assert_eq!(
            last.layer,
            (0, 450),
            "the layer size across the edge on {edge:?}"
        );
    }
}

/// A grow holds the panel's current length until the configure handshake
/// answers with a buffer whose reported length reaches the target (D8
/// finding 4), and then eases from that length. The handshake owns the
/// wait; the tween it starts owns none.
#[test]
fn a_grow_starts_only_at_a_commit_that_reports_the_target() {
    let handshake = growing(Edge::Left, 400, 600);
    assert_eq!(
        handshake.on_commit(Some(575)),
        None,
        "an intermediate size waits"
    );
    assert_eq!(handshake.on_commit(None), None, "no report waits");
    assert_eq!(
        GrowHandshake::on_deadline(499_999),
        None,
        "still inside the bound"
    );
    let Some(GrowResult::Answered { surface }) = handshake.on_commit(Some(600)) else {
        panic!("the target starts the grow");
    };
    assert_eq!(surface, (600, 1080), "the wide surface is the crop");
    let mut tween = answered(handshake, surface);
    assert_eq!(tween.phase(), Phase::Running);
    assert_eq!(tween.surface(), (600, 1080), "the wide surface is the crop");
    let first = frame(tween.step(0));
    assert_eq!(first.visible, 400);
    assert_eq!(first.fraction, Fraction::of(400, 600).expect("a fraction"));
    // t = 0.5 -> ease(0.5) = 0.875 -> 400 + round(200 * 0.875).
    let mid = frame(tween.step(100_000));
    assert_eq!(mid.visible, 575);
    let last = frame(tween.step(200_000));
    assert_eq!(last.visible, 600);
    assert_eq!(
        last.fraction,
        Fraction::of(600, 600).expect("the whole wide surface")
    );
}

/// A configure handshake the app never answers settles on no wide buffer at
/// the bound (D8, spec "Grow waits for the app"), and its snap frame is the
/// target's layer state while the old surface still draws.
#[test]
fn a_handshake_without_a_wide_buffer_snaps_at_the_bound() {
    let handshake = growing(Edge::Left, 400, 600);
    assert_eq!(GrowHandshake::on_deadline(0), None);
    assert_eq!(GrowHandshake::on_deadline(GROW_HANDSHAKE_US - 1), None);
    assert_eq!(
        GrowHandshake::on_deadline(GROW_HANDSHAKE_US),
        Some(GrowResult::Unanswered),
        "the bound settles on no wide buffer"
    );
    let snap = handshake.snap();
    assert_eq!(snap.visible, 600, "the snap lands on the target");
    assert_eq!(snap.layer, (600, 0), "the snapped layer is the target");
    assert_eq!(
        snap.gap,
        HeldGap::start(push(Edge::Left, 400)).after_publish(push(Edge::Left, 600)),
        "the snapped strip is the target's, not an intermediate one"
    );
    assert_eq!(
        snap.fraction,
        Fraction::of(600, 400).expect("the whole old surface"),
        "the snap shows the whole old surface it still has"
    );
}

/// A shrink keeps the app's wide buffer and crops the docked part of it
/// down each frame, so the content stays against the edge (D8, spec
/// "Content does not slide").
#[test]
fn a_shrink_crops_the_wide_buffer_from_the_docked_edge() {
    let mut tween = shrinking(Edge::Right, 600, 400);
    let first = frame(tween.step(0));
    assert_eq!(first.visible, 600);
    assert_eq!(
        first.fraction,
        Fraction::of(600, 600).expect("the whole wide surface")
    );
    // t = 0.5 -> ease = 0.875 -> 600 - 175.
    let mid = frame(tween.step(100_000));
    assert_eq!(mid.visible, 425);
    assert_eq!(
        mid.fraction,
        Fraction::of(425, 600).expect("the docked part of the wide surface")
    );
    assert_eq!(mid.layer, (425, 0), "the panel's own width follows");
    let last = frame(tween.step(200_000));
    assert_eq!(last.visible, 400);
    assert_eq!(
        last.fraction,
        Fraction::of(400, 600).expect("the last crop keeps the right edge")
    );
    assert_eq!(tween.phase(), Phase::Finished);
    assert!(
        tween.shrinks_app(),
        "the app is still wider than the target"
    );
}

/// A pushing target moves the strip with the panel (D8, spec "Animate on
/// request"): every frame's gap is the strip at that frame's length.
#[test]
fn a_pushing_gap_moves_with_the_panel() {
    let handshake = growing(Edge::Left, 400, 600);
    let Some(GrowResult::Answered { surface }) = handshake.on_commit(Some(600)) else {
        panic!("the target answers the handshake");
    };
    let mut tween = answered(handshake, surface);
    let _ = tween.step(0);
    let mid = frame(tween.step(100_000));
    assert_eq!(mid.visible, 575);
    assert_eq!(mid.gap.zone(), 575, "the strip is the visible length");
    let last = frame(tween.step(200_000));
    assert_eq!(last.gap.zone(), 600, "the strip ends on the target");
}

/// A covering target holds the strip still (D8, spec "Covering expand
/// holds tiles still"): the gap rests at the last pushing strip in every
/// frame, and the panel grows over it.
#[test]
fn a_covering_gap_holds_still() {
    let resting = HeldGap::start(push(Edge::Left, 400));
    let handshake = GrowHandshake::new(
        cover(Edge::Left, 600),
        continuing(Edge::Left, 400, resting, ReserveMode::Push),
        200,
    );
    let Some(GrowResult::Answered { surface }) = handshake.on_commit(Some(600)) else {
        panic!("the target answers the handshake");
    };
    let mut tween = answered(handshake, surface);
    let _ = tween.step(0);
    let mid = frame(tween.step(100_000));
    assert_eq!(mid.visible, 575, "the panel still grows");
    assert_eq!(mid.gap.zone(), 400, "the strip holds still");
    assert_eq!(mid.gap.edge(), Edge::Left);
    let last = frame(tween.step(200_000));
    assert_eq!(last.visible, 600);
    assert_eq!(last.gap.zone(), 400, "the final frame leaves the strip");
    assert_eq!(tween.final_gap(), resting, "a covering target ends held");
}

/// A collapse from a covering panel to the held strip keeps the strip in
/// every frame and at the end (D5, spec "Collapse back to the held
/// strip").
#[test]
fn a_collapse_to_the_held_strip_holds_it_in_every_frame() {
    let mut tween = collapsing(1100, 400);
    assert_eq!(
        tween.final_gap().zone(),
        400,
        "the end stays at the held strip"
    );
    for elapsed in [0, 50_000, 100_000, 150_000, 200_000] {
        let frame = frame(tween.step(elapsed));
        assert_eq!(
            frame.gap.zone(),
            400,
            "the strip holds at {elapsed} microseconds"
        );
    }
    assert_eq!(tween.phase(), Phase::Finished);
}

/// A collapse from a covering panel to a new pushing width holds the strip
/// during the animation and sets it when the animation ends (D5, spec
/// "Collapse to a new pushing width").
#[test]
fn a_collapse_to_a_new_pushing_width_lands_on_it_at_the_end() {
    let mut tween = collapsing(1100, 500);
    assert_eq!(
        tween.final_gap().zone(),
        500,
        "the target strip is the new width"
    );
    let first = frame(tween.step(0));
    assert_eq!(first.visible, 1100, "the collapse starts at the old width");
    assert_eq!(first.gap.zone(), 400, "the first frame holds the strip");
    for elapsed in [50_000, 100_000, 150_000] {
        let frame = frame(tween.step(elapsed));
        assert!(
            frame.visible > 500 && frame.visible < 1100,
            "the panel is still collapsing at {elapsed} microseconds, visible {}",
            frame.visible
        );
        assert_eq!(
            frame.gap.zone(),
            400,
            "the strip holds during the animation"
        );
    }
    let last = frame(tween.step(200_000));
    assert_eq!(last.visible, 500);
    assert_eq!(
        last.gap.zone(),
        500,
        "the strip becomes the new width at the end"
    );
}

/// A push to none holds the strip during the animation and releases it
/// when the animation ends (D5, spec "Push to none").
#[test]
fn a_push_to_none_holds_the_strip_and_releases_it_at_the_end() {
    let mut tween = Tween::start(
        none(Edge::Left, 300),
        continuing(
            Edge::Left,
            400,
            HeldGap::start(push(Edge::Left, 400)),
            ReserveMode::Push,
        ),
        200,
    )
    .expect("a 200 ms shrink to none");
    assert_eq!(tween.final_gap().zone(), 0, "the end releases the strip");
    let first = frame(tween.step(0));
    assert_eq!(first.visible, 400, "the shrink starts at the old width");
    assert_eq!(first.gap.zone(), 400, "the first frame holds the strip");
    for elapsed in [50_000, 100_000, 150_000] {
        let frame = frame(tween.step(elapsed));
        assert_eq!(
            frame.gap.zone(),
            400,
            "the strip holds at {elapsed} microseconds"
        );
    }
    let last = frame(tween.step(200_000));
    assert_eq!(last.visible, 300);
    assert_eq!(last.gap.zone(), 0, "the strip becomes zero at the end");
}

/// A cover to none follows the same rule: the strip holds during the
/// animation and becomes zero when it ends (D5).
#[test]
fn a_cover_to_none_holds_the_strip_and_releases_it_at_the_end() {
    let mut tween = Tween::start(
        none(Edge::Left, 900),
        continuing(
            Edge::Left,
            1100,
            HeldGap::start(push(Edge::Left, 400)),
            ReserveMode::Cover,
        ),
        200,
    )
    .expect("a 200 ms shrink to none");
    assert_eq!(tween.final_gap().zone(), 0, "the end releases the strip");
    let first = frame(tween.step(0));
    assert_eq!(first.visible, 1100, "the shrink starts at the old width");
    assert_eq!(first.gap.zone(), 400, "the first frame holds the strip");
    for elapsed in [50_000, 100_000, 150_000] {
        let frame = frame(tween.step(elapsed));
        assert_eq!(
            frame.gap.zone(),
            400,
            "the strip holds at {elapsed} microseconds"
        );
    }
    let last = frame(tween.step(200_000));
    assert_eq!(last.visible, 900);
    assert_eq!(last.gap.zone(), 0, "the strip becomes zero at the end");
}

/// While a tween runs every compositor configure is acked by the proxy
/// and none reaches the app, so the app gets one new size for the whole
/// tween (D8 finding 3, spec "One new size for the app").
#[test]
fn a_tween_forwards_one_app_size() {
    let handshake = growing(Edge::Left, 400, 600);
    // The one configure the proxy sends when the grow starts.
    let mut app_sizes = vec![handshake.geometry().across()];
    let Some(GrowResult::Answered { surface }) = handshake.on_commit(Some(600)) else {
        panic!("the target answers the handshake");
    };
    let mut tween = answered(handshake, surface);
    assert_eq!(
        route_configure(tween.is_tweening()),
        ConfigureRoute::AckOnly,
        "the grow swallows the compositor's configures"
    );
    for elapsed in [0, 50_000, 100_000, 150_000] {
        let _ = tween.step(elapsed);
        if route_configure(tween.is_tweening()) == ConfigureRoute::Forward {
            app_sizes.push(tween.target());
        }
    }
    let _ = tween.step(200_000);
    assert_eq!(tween.phase(), Phase::Finished);
    assert_eq!(
        route_configure(tween.is_tweening()),
        ConfigureRoute::Forward,
        "the steady path forwards again after the tween"
    );
    assert_eq!(app_sizes, [600], "exactly one size for the whole tween");
}

/// A frame queued for a tween the slot has since cleared is stale and
/// does nothing, so it cannot re-send its layer state over a hide, a
/// re-dock or a newer tween; a frame of the tween the slot still owns
/// applies (D8).
#[test]
fn a_cleared_tween_drops_the_frames_it_queued() {
    let slot = Slot::new();
    let queued = slot.generation();
    assert!(slot.is_current(queued), "the tween's frame applies");
    slot.clear();
    assert!(!slot.is_current(queued), "a cleared tween's frame is stale");
    slot.clear();
    assert!(
        !slot.is_current(queued),
        "a second clear keeps the old frame stale"
    );
}

/// A serial the proxy minted is swallowed, a repeated ack of it is still
/// swallowed, and a compositor's is not (D8 findings 3): only values the
/// proxy minted take the proxy's route.
#[test]
fn the_app_ack_of_a_minted_configure_is_not_forwarded() {
    let slot = Slot::new();
    let minted = slot.mint();
    assert!(
        !slot.swallow_ack(minted.wrapping_add(1)),
        "a serial the proxy did not mint passes through"
    );
    assert!(slot.swallow_ack(minted), "the minted serial is swallowed");
    assert!(
        slot.swallow_ack(minted),
        "a repeated ack of a minted serial is still not forwarded"
    );
}

/// An ack consumes the serials of every earlier minted configure too
/// (D8): the later ack supersedes them, but a late ack that names an
/// earlier one is still swallowed rather than forwarded.
#[test]
fn an_ack_of_a_later_minted_serial_still_swallows_the_earlier_ones() {
    let slot = Slot::new();
    let first = slot.mint();
    let second = slot.mint();
    assert!(slot.swallow_ack(second));
    assert!(
        slot.swallow_ack(first),
        "the earlier minted serial stays swallowed once consumed"
    );
}

/// A client that never acks cannot grow the minted set without limit
/// (D8): the oldest serials leave it first, and their acks are still
/// swallowed, as is the newest minted serial's.
#[test]
fn minted_serials_are_bounded() {
    let slot = Slot::new();
    let oldest = slot.mint();
    for _ in 1..MINTED_LIMIT {
        let _ = slot.mint();
    }
    let newest = slot.mint();
    assert!(
        slot.swallow_ack(oldest),
        "the oldest minted serial stays swallowed once the bound forgets it"
    );
    assert!(
        slot.swallow_ack(newest),
        "the newest minted serial still reaches the proxy"
    );
}

/// Abandoning a tween keeps its minted serials: the app may ack the
/// configure after a hide, a snap or a re-dock (D8), and that ack must
/// still be swallowed rather than forwarded to the layer surface.
#[test]
fn a_cancelled_tween_still_swallows_its_minted_serial() {
    let slot = Slot::new();
    let minted = slot.mint();
    slot.clear();
    assert!(
        slot.swallow_ack(minted),
        "the app may ack a configure its tween abandoned"
    );
}

/// The tween's held gap at the end is the state a plain apply would
/// leave: a pushing target's own strip, a covering target's resting gap
/// (spec "Exact final width").
#[test]
fn the_final_gap_matches_a_plain_apply() {
    let pushing = growing(Edge::Left, 400, 600);
    assert_eq!(
        pushing.final_gap(),
        HeldGap::start(push(Edge::Left, 400)).after_publish(push(Edge::Left, 600))
    );
    let resting = HeldGap::start(push(Edge::Left, 400));
    let covering = GrowHandshake::new(
        cover(Edge::Left, 600),
        continuing(Edge::Left, 400, resting, ReserveMode::Push),
        200,
    );
    assert_eq!(covering.final_gap(), resting);
}

/// No duration and no distance start no tween (D8: a snap), and an app
/// surface of no pixels never divides the fraction.
#[test]
fn a_change_that_cannot_animate_starts_no_tween() {
    assert!(
        Tween::start(
            push(Edge::Left, 600),
            continuing(
                Edge::Left,
                400,
                HeldGap::start(push(Edge::Left, 400)),
                ReserveMode::Push,
            ),
            0
        )
        .is_none()
    );
    assert!(
        Tween::start(
            push(Edge::Left, 400),
            continuing(
                Edge::Left,
                400,
                HeldGap::start(push(Edge::Left, 400)),
                ReserveMode::Push,
            ),
            200
        )
        .is_none()
    );
    let tween = Tween::start(
        push(Edge::Left, 600),
        Continuation {
            from: 400,
            surface: (0, 0),
            resting_gap: HeldGap::start(push(Edge::Left, 400)),
            reserve: ReserveMode::Push,
        },
        200,
    )
    .expect("a shrink");
    assert_eq!(
        tween.fraction(),
        Fraction::of(400, 1).expect("the surface is at least one pixel")
    );
}

/// A request's duration is its `ms` or the panel's `tween_ms`, clamped to
/// one second (D8, spec "Duration clamp", "Snap when animations are
/// disabled").
#[test]
fn a_duration_is_resolved_and_clamped() {
    assert_eq!(duration_ms(None, 200), 200, "the panel's tween_ms");
    assert_eq!(duration_ms(Some(150), 200), 150, "the request wins");
    assert_eq!(
        duration_ms(Some(60_000), 200),
        MAX_DURATION_MS,
        "a long request clamps"
    );
    assert_eq!(
        duration_ms(None, 60_000),
        MAX_DURATION_MS,
        "a long panel duration clamps"
    );
    assert_eq!(duration_ms(Some(0), 200), 0, "zero disables the animation");
    assert_eq!(duration_ms(None, 0), 0, "a zero panel duration snaps");
}

/// A 60000 ms change still ends at one second (D8, spec "Duration
/// clamp"): the eased tween is still moving just before the clamp and
/// finished exactly on it.
#[test]
fn a_clamped_tween_ends_after_one_second() {
    let handshake = GrowHandshake::new(
        push(Edge::Left, 600),
        continuing(
            Edge::Left,
            400,
            HeldGap::start(push(Edge::Left, 400)),
            ReserveMode::Push,
        ),
        60_000,
    );
    let Some(GrowResult::Answered { surface }) = handshake.on_commit(Some(600)) else {
        panic!("the target answers the handshake");
    };
    let mut tween = answered(handshake, surface);
    let _ = tween.step(0);
    assert!(
        matches!(tween.step(999_999), Step::Frame(_)),
        "the clamped second still runs"
    );
    let last = frame(tween.step(1_000_000));
    assert_eq!(last.visible, 600, "the clamp ends on the target");
}

/// A change waits for the app only when its target is longer than the
/// buffer the app has; a retarget inside that buffer crops instead (D8,
/// spec "Retarget", "One new size for the app").
#[test]
fn a_change_waits_only_when_the_app_must_grow() {
    assert!(waits_for_app(Edge::Left, (400, 1080), 600));
    assert!(!waits_for_app(Edge::Left, (600, 1080), 500));
    assert!(!waits_for_app(Edge::Left, (600, 1080), 600));
    assert!(waits_for_app(Edge::Top, (1080, 400), 600));
    assert!(!waits_for_app(Edge::Top, (1080, 600), 500));
}

/// A retarget continues from the length the panel shows now, keeps the
/// strip it holds, and lands on the new target exactly (D8, spec
/// "Retarget", "Exact final width").
#[test]
fn a_retarget_continues_from_the_current_length() {
    let mut shrink = shrinking(Edge::Left, 600, 400);
    let _ = shrink.step(0);
    let mid = frame(shrink.step(100_000));
    assert_eq!(mid.visible, 425);
    let surface = shrink.surface();
    // A client asks for a larger size while the shrink runs: the app's
    // wide buffer serves the new target, so no new configure is needed
    // until the last frame (spec "Retarget").
    assert!(!waits_for_app(Edge::Left, surface, 500));
    let mut retarget = Tween::start(push(Edge::Left, 500), shrink.retarget_continuation(), 200)
        .expect("a retarget");
    let first = frame(retarget.step(0));
    assert_eq!(first.visible, 425, "no jump on the retarget");
    let last = frame(retarget.step(200_000));
    assert_eq!(last.visible, 500, "the retarget lands exactly");
    assert_eq!(last.gap.zone(), 500, "the strip follows the retarget");
    assert!(
        retarget.shrinks_app(),
        "the app ends configured to the new target"
    );
}

/// A zero duration starts no tween, so no later frame changes the panel
/// after a snap (D8, spec "Snap interrupts").
#[test]
fn a_snap_starts_no_frames() {
    assert!(
        Tween::start(
            push(Edge::Left, 300),
            Continuation {
                from: 425,
                surface: surface(Edge::Left, 600),
                resting_gap: HeldGap::start(push(Edge::Left, 425)),
                reserve: ReserveMode::Push,
            },
            0
        )
        .is_none(),
        "the snap runs no frame"
    );
}

/// A frame clock that stalls for the duration and 100 ms lands on the
/// exact target on the next tick (D8, spec "Stalled frame clock").
#[test]
fn a_stalled_clock_snaps_to_the_exact_target() {
    // A shrink starts its eased clock with the change, so a stall before
    // its first frame still spends the duration.
    let mut shrink = shrinking(Edge::Right, 600, 400);
    let last = frame(shrink.step(300_000));
    assert_eq!(last.visible, 400, "the stalled shrink lands exactly");
    assert_eq!(shrink.phase(), Phase::Finished);
    // A grow starts its clock when the app's buffer arrives, and a stall
    // after that still ends exactly.
    let handshake = growing(Edge::Left, 400, 600);
    let Some(GrowResult::Answered { surface }) = handshake.on_commit(Some(600)) else {
        panic!("the target answers the handshake");
    };
    let mut grow = answered(handshake, surface);
    let _ = grow.step(0);
    let last = frame(grow.step(300_000));
    assert_eq!(last.visible, 600, "the stalled grow lands exactly");
    assert_eq!(last.layer, (600, 0));
    assert_eq!(grow.phase(), Phase::Finished);
}

/// A change snaps when it has no viewporter to crop through, no window or
/// a hidden panel (D8, spec "No viewport support", "Snap when animations
/// are disabled").
#[test]
fn a_change_animates_only_with_a_shown_window_and_viewporter() {
    assert!(animates(true, false, true));
    assert!(!animates(true, false, false), "no viewporter snaps");
    assert!(!animates(false, false, true), "no window snaps");
    assert!(!animates(true, true, true), "a hidden panel snaps");
}
