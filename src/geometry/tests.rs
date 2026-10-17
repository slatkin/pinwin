use super::*;

fn geometry(edge: Edge, across: u32, gutters: Gutters) -> Geometry {
    Geometry::new(edge, across, gutters, ReserveMode::Push)
}

fn covering(edge: Edge, across: u32, gutters: Gutters) -> Geometry {
    Geometry::new(edge, across, gutters, ReserveMode::Cover)
}

fn held(gap: HeldGap) -> (Edge, i32) {
    (gap.edge(), gap.zone())
}

fn output(width: u32, height: u32) -> OutputSize {
    OutputSize::new(width, height).expect("test output size is non-zero")
}

#[test]
fn left_docking() {
    let docked = geometry(Edge::Left, 320, Gutters::new(8, 12, 0, 0));
    assert_eq!(
        docked.margins(),
        Margins::new(0, 0, 0, 8),
        "the panel starts 8 pixels from the left output edge"
    );
    assert_eq!(
        docked.strip(),
        Some(340),
        "the strip reserves 8 + 320 + 12 at the left edge"
    );
    assert_eq!(docked.size(), (320, 0));
    assert_eq!(
        docked.anchors(),
        Anchors::at(Edge::Left),
        "the panel anchors to its edge and both ends of its length"
    );
    assert!(docked.anchors().contains(Edge::Top));
    assert!(docked.anchors().contains(Edge::Bottom));
    assert!(!docked.anchors().contains(Edge::Right));
}

#[test]
fn right_docking() {
    let docked = geometry(Edge::Right, 320, Gutters::new(8, 12, 0, 0));
    assert_eq!(
        docked.margins(),
        Margins::new(0, 12, 0, 0),
        "the panel ends 12 pixels before the right output edge"
    );
    assert_eq!(docked.strip(), Some(340));

    // A pushing apply releases the left reservation: the held gap moves
    // to the right edge at the pushing size resolved there.
    let left = HeldGap::start(geometry(Edge::Left, 320, Gutters::new(8, 12, 0, 0)));
    assert_eq!(held(left), (Edge::Left, 340));
    assert_eq!(held(left.after_publish(docked)), (Edge::Right, 340));
}

#[test]
fn top_docking() {
    let docked = geometry(Edge::Top, 400, Gutters::new(20, 30, 10, 6));
    assert_eq!(
        docked.margins(),
        Margins::new(10, 30, 0, 20),
        "10 below the top edge, 20 from the left edge and 30 before the right edge"
    );
    assert_eq!(docked.size(), (0, 400), "the panel is 400 pixels high");
    assert_eq!(
        docked.strip(),
        Some(416),
        "the strip reserves 10 + 400 + 6 at the top edge"
    );

    // The reserved strip spans the output's full length: the reserve
    // surface anchors to the top edge and both ends with no margins, so
    // the side gutters inset only the panel and the strip is 416 deep
    // across the full output width.
    let gap = HeldGap::start(docked);
    assert!(gap.anchors().contains(Edge::Top));
    assert!(gap.anchors().contains(Edge::Left));
    assert!(gap.anchors().contains(Edge::Right));
    assert!(!gap.anchors().contains(Edge::Bottom));
    assert_eq!(gap.size(), (0, 416));
    assert_eq!(held(gap), (Edge::Top, 416));
}

#[test]
fn vertical_insets() {
    let docked = geometry(Edge::Left, 320, Gutters::new(8, 12, 24, 10));
    assert_eq!(
        docked.margins(),
        Margins::new(24, 0, 10, 8),
        "the panel begins 24 below the output top and ends 10 above its bottom"
    );
    assert_eq!(
        docked.size(),
        (320, 0),
        "the height is what the insets leave"
    );
    assert_eq!(
        docked.strip(),
        Some(340),
        "the side gutters leave the reservation a full-height strip"
    );
    assert_eq!(
        HeldGap::start(docked).size(),
        (340, 0),
        "the strip is 340 deep and spans the full output height"
    );
}

#[test]
fn negative_gutter() {
    let docked = geometry(Edge::Left, 320, Gutters::new(-40, 12, 0, 0));
    assert_eq!(
        docked.margins(),
        Margins::new(0, 0, 0, -40),
        "the panel starts 40 pixels beyond the left output edge"
    );
    assert_eq!(
        docked.strip(),
        Some(292),
        "the reservation is -40 + 320 + 12"
    );
}

#[test]
fn covering_layout_holds_the_gap() {
    let push = geometry(Edge::Left, 320, Gutters::new(8, 12, 0, 0));
    let gap = HeldGap::start(push);
    assert_eq!(held(gap), (Edge::Left, 340));

    let cover = covering(Edge::Left, 1000, Gutters::new(8, 12, 0, 0));
    assert_eq!(cover.size(), (1000, 0), "the panel is 1000 pixels wide");
    assert_eq!(
        held(gap.after_publish(cover)),
        (Edge::Left, 340),
        "a covering apply leaves the reserved strip alone"
    );
}

#[test]
fn side_switch_moves_the_gap() {
    // The pushing size is `25%` of a 1920 pixel wide output, which is 480
    // pixels; the held gap keeps the resolved pixels, not the percent.
    let push = geometry(Edge::Right, 480, Gutters::new(0, 0, 0, 0));
    let gap = HeldGap::start(push);
    assert_eq!(held(gap), (Edge::Right, 480));

    let moved = gap.after_publish(covering(Edge::Left, 480, Gutters::new(0, 0, 0, 0)));
    assert_eq!(
        held(moved),
        (Edge::Left, 480),
        "a covering side switch moves the reservation to the new edge"
    );
}

#[test]
fn the_reserve_draws_from_the_held_gap_not_the_geometry() {
    // A pushing 360 pixel panel with a 12 pixel far gutter holds 372.
    let gap = HeldGap::start(geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0)));
    assert_eq!(held(gap), (Edge::Left, 372));

    // A covering geometry on the same edge with a tween idle draws the
    // held strip, not its own wider strip.
    let cover = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(gap.reserve(false, cover, 1080)), (Edge::Left, 372));

    // A pushing geometry with no tween running agrees with the held gap:
    // the apply has already reset it to the geometry's own strip.
    let push = geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(gap.reserve(false, push, 360)), (Edge::Left, 372));
}

#[test]
fn a_flagged_gap_tween_moves_the_gap_with_the_panel() {
    let gap = HeldGap::start(geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0)));
    assert_eq!(held(gap), (Edge::Left, 372));

    // Mid-tween at a 700 pixel panel: the gap shows the strip at the
    // eased width, 0 + 700 + 12.
    let tweening = geometry(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(gap.reserve(true, tweening, 700)), (Edge::Left, 712));

    // The gap follows the tweening geometry's edge.
    let tweening = geometry(Edge::Right, 1080, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(gap.reserve(true, tweening, 700)), (Edge::Right, 712));
}

#[test]
fn a_covering_tween_holds_the_gap() {
    let gap = HeldGap::start(geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0)));
    let tweening = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
    assert_eq!(
        held(gap.reserve(true, tweening, 700)),
        (Edge::Left, 372),
        "a covering target never moves the gap, flag or not"
    );
}

#[test]
fn the_gap_tween_decision_compares_against_the_strip_the_gap_rests_at() {
    // Pushing 360 -> 1080 pixels: the gap rests at 372 and the target
    // strip is 1092, so the gap tweens.
    let expanding = geometry(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
    assert!(HeldGap::tweens_with_panel(372, true, expanding, 1080));

    // Shrinking a covering excursion back to the original pushing width:
    // the target strip is the strip the gap rests at, so it holds still.
    let shrinking_back = geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0));
    assert!(!HeldGap::tweens_with_panel(372, true, shrinking_back, 360));

    // A covering target never tweens the gap.
    let cover = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
    assert!(!HeldGap::tweens_with_panel(372, true, cover, 1080));

    // A snap apply (zero duration, no animation, a side or gutter change)
    // applies the strip in one step.
    assert!(!HeldGap::tweens_with_panel(372, false, expanding, 1080));

    // A pushing target whose strip the gap already rests at does not tween.
    assert!(!HeldGap::tweens_with_panel(1092, true, expanding, 1080));
}

#[test]
fn the_starting_held_gap_follows_the_reserve_mode() {
    let push = geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(HeldGap::start(push)), (Edge::Left, 372));

    // A covering start reserves nothing: no earlier pushing width to hold.
    let cover = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(HeldGap::start(cover)), (Edge::Left, 0));
}

#[test]
fn the_held_gap_follows_a_publish_by_reserve_mode() {
    let gap = HeldGap::start(geometry(Edge::Left, 360, Gutters::new(0, 12, 0, 0)));
    assert_eq!(held(gap), (Edge::Left, 372));

    // A pushing publish on the same edge resets the zone to its own strip.
    let wider = geometry(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(gap.after_publish(wider)), (Edge::Left, 1092));

    // A pushing publish on the other edge moves both, at the strip that
    // edge resolves.
    let other = geometry(Edge::Right, 360, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(gap.after_publish(other)), (Edge::Right, 372));

    // A covering publish on the same edge touches nothing.
    let cover = covering(Edge::Left, 1080, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(gap.after_publish(cover)), (Edge::Left, 372));

    // A covering publish on the other edge moves only the edge.
    let cover = covering(Edge::Right, 1080, Gutters::new(0, 12, 0, 0));
    assert_eq!(held(gap.after_publish(cover)), (Edge::Right, 372));
}

#[test]
fn percent_of_the_output() {
    let screen = output(2560, 1080);
    let quarter = Size::percent(25).expect("25 percent is a valid size");

    // A side panel measures its percent against the output's width.
    assert_eq!(quarter.resolve(Edge::Left, screen), 640);
    assert_eq!(quarter.resolve(Edge::Right, screen), 640);

    // A top or bottom panel measures it against the output's height:
    // 25% of 1080 is 270, not 640.
    assert_eq!(quarter.resolve(Edge::Top, screen), 270);
    assert_eq!(quarter.resolve(Edge::Bottom, screen), 270);

    // A pixel size does not depend on the output.
    let pixels = Size::pixels(640).expect("640 pixels is a valid size");
    assert_eq!(pixels.resolve(Edge::Top, screen), 640);

    // A percent rounds down, and 1% of a narrow output still resolves to
    // one pixel rather than the zero that no size may be.
    let one = Size::percent(1).expect("1 percent is a valid size");
    assert_eq!(one.resolve(Edge::Left, output(60, 100)), 1);
    let third = Size::percent(33).expect("33 percent is a valid size");
    assert_eq!(third.resolve(Edge::Left, output(101, 100)), 33);
}

#[test]
fn percent_follows_the_output() {
    let quarter = Size::percent(25).expect("25 percent is a valid size");

    // The same setting resolves again when the output changes.
    assert_eq!(quarter.resolve(Edge::Left, output(2560, 1080)), 640);
    assert_eq!(quarter.resolve(Edge::Left, output(1920, 1080)), 480);
}

#[test]
fn invalid_size() {
    // A size is whole pixels or 1 to 100 percent (spec "Size values");
    // the text form `wide` is refused by the command line's parser
    // (row 4.x), so here it is a value no constructor can build.
    assert_eq!(Size::pixels(0), None);
    assert_eq!(Size::percent(0), None);
    assert_eq!(Size::percent(150), None);
    assert!(Size::percent(100).is_some());
}

#[test]
fn the_gutters_follow_the_screen_directions() {
    let gutters = Gutters::new(8, 12, 24, 10);
    assert_eq!(gutters.near(Edge::Left), 8);
    assert_eq!(gutters.far(Edge::Left), 12);
    assert_eq!(gutters.near(Edge::Right), 12);
    assert_eq!(gutters.far(Edge::Right), 8);
    assert_eq!(gutters.near(Edge::Top), 24);
    assert_eq!(gutters.far(Edge::Top), 10);
    assert_eq!(gutters.near(Edge::Bottom), 10);
    assert_eq!(gutters.far(Edge::Bottom), 24);

    // The length runs down the output on a side edge and across it on a
    // top or bottom edge, so its ends are Top/Bottom and Left/Right.
    assert_eq!(gutters.along_start(Edge::Left), 24);
    assert_eq!(gutters.along_end(Edge::Left), 10);
    assert_eq!(gutters.along_start(Edge::Top), 8);
    assert_eq!(gutters.along_end(Edge::Top), 12);
}

#[test]
fn no_visible_length() {
    let output = output(2560, 1080);
    let gutters = Gutters::new(0, 12, 600, 600);

    // A left panel's length is the output's height less the Top and Bottom
    // gutters: 1080 - 600 - 600 leaves less than one pixel.
    let push = geometry(Edge::Left, 320, gutters);
    assert_eq!(push.validate(output), Err(InvalidLayout::NoLength));

    // A covering panel rejects the same invisible length.
    let cover = covering(Edge::Left, 320, gutters);
    assert_eq!(cover.validate(output), Err(InvalidLayout::NoLength));
}

#[test]
fn too_wide_for_the_output() {
    let output = output(1920, 1080);

    // A 20 pixel near gutter and a 12 pixel far gutter push the strip to
    // 1952 pixels of the output's 1920: no output width is left for other
    // windows.
    let push = geometry(Edge::Left, 1920, Gutters::new(20, 12, 0, 0));
    assert_eq!(push.validate(output), Err(InvalidLayout::NoWidth));

    // A strip exactly as wide as the output leaves no room either.
    let exact = geometry(Edge::Left, 1920, Gutters::new(0, 0, 0, 0));
    assert_eq!(exact.validate(output), Err(InvalidLayout::NoWidth));

    // A strip one pixel narrower leaves room beside the panel.
    let fits = geometry(Edge::Left, 1919, Gutters::new(0, 0, 0, 0));
    assert_eq!(fits.validate(output), Ok(()));
}

#[test]
fn covering_panel_wider_than_the_output() {
    let output = output(1920, 1080);

    let wider = covering(Edge::Left, 1921, Gutters::new(0, 0, 0, 0));
    assert_eq!(wider.validate(output), Err(InvalidLayout::NoWidth));

    // Exactly the output's width: a covering panel draws over the tiles and
    // holds the last pushing strip, so the wider strip it would reserve
    // never fails the change.
    let fits = covering(Edge::Left, 1920, Gutters::new(0, 0, 0, 0));
    assert_eq!(fits.validate(output), Ok(()));
}

#[test]
fn a_negative_reservation_is_refused() {
    let output = output(1920, 1080);

    // A -400 near gutter with a 320 pixel panel and a 12 pixel far gutter
    // reserves -68 pixels: pushing cannot hold a negative strip.
    let push = geometry(Edge::Left, 320, Gutters::new(-400, 12, 0, 0));
    assert_eq!(push.validate(output), Err(InvalidLayout::NoReserve));

    // A covering panel never checks the strip it would reserve.
    let cover = covering(Edge::Left, 320, Gutters::new(-400, 12, 0, 0));
    assert_eq!(cover.validate(output), Ok(()));
}
