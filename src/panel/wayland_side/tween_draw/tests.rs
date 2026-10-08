use std::num::NonZeroU16;

use super::*;
use crate::layout::Layout as L;

fn layout(side: Side, cols: u16, left: i32, right: i32) -> L {
    L::new(
        side,
        NonZeroU16::new(cols).expect("test columns"),
        0,
        0,
        left,
        right,
    )
}

fn scale(units: u32) -> FractionalScale {
    FractionalScale::from_120ths(units)
}

/// A holder for the plan tests: a tiny canvas stands in for the wide
/// draw, which the crop module's tests cover pixel for pixel.
fn holder(side: Side, start: i32, end: i32, units: u32, gap_tweening: bool) -> TweenDraw {
    let wide = start.max(end);
    // The 12 px gutter sits on the docking edge, so the held 372 strip
    // and the tweening strip arithmetic line up.
    let (left, right) = match side {
        Side::Left => (12, 0),
        Side::Right => (0, 12),
    };
    TweenDraw {
        crop: TweenCrop::new(side, start, end).expect("a valid tween"),
        canvas: Canvas::new(4, 4).expect("test canvas"),
        buffer: None,
        height: 720,
        scale: scale(units),
        layout: layout(side, u16::try_from(wide / 9).unwrap_or(1), left, right),
        gap_side: side,
        gap_zone: 372,
        gap_tweening,
        grid_px: wide,
        stale: false,
    }
}

/// The commit plan orders the actions the frame commits
/// (replace-gtk-with-wayland D7): the
/// layer size, the margins, the viewport source and destination, the
/// reserve's anchor and zone, the wide buffer's attach, then the
/// damage-and-commit — and the damaged rectangle is the wide buffer's.
#[test]
fn the_commit_plan_orders_the_viewporter_actions() {
    let held = holder(Side::Left, 360, 1080, 180, true);
    let plan = held.commit_plan(700, true).expect("a presentable frame");

    assert_eq!(
        plan.actions(),
        &[
            TweenAction::PanelSize(700),
            TweenAction::PanelMargins((0, 0, 0, 12)),
            TweenAction::ViewportSource(plan.frame().source()),
            TweenAction::ViewportDestination(700, 720),
            TweenAction::Reserve {
                side: Side::Left,
                zone: 712
            },
            TweenAction::AttachWide,
            TweenAction::Present,
        ],
        "the order is a to f"
    );
    // The gap tweens: the strip moves with the panel (12 + 700 + 0).
    assert_eq!(plan.damage(), (1620, 1080), "the wide buffer is damaged");
    let source = plan.frame().source();
    assert_eq!(source.x(), 0, "a left-docked crop starts at 0");
    assert_eq!(source.width(), 1050, "700 logical px at 1.5");
}

/// Without a viewporter the plan replaces the viewport writes with the
/// fresh-copy fallback, and damages the crop's own rectangle — the
/// buffer the fallback attaches is the crop, shown at its own size.
#[test]
fn the_commit_plan_falls_back_to_a_fresh_copy_without_a_viewporter() {
    let held = holder(Side::Right, 360, 1080, 180, false);
    let plan = held.commit_plan(700, false).expect("a presentable frame");

    let actions = plan.actions();
    assert_eq!(actions[0], TweenAction::PanelSize(700));
    assert_eq!(actions[1], TweenAction::PanelMargins((0, 12, 0, 0)));
    assert!(
        actions[1..].iter().all(|action| !matches!(
            action,
            TweenAction::ViewportSource(_) | TweenAction::ViewportDestination(_, _)
        )),
        "no viewport writes without a viewport"
    );
    assert_eq!(
        actions[2],
        TweenAction::Reserve {
            side: Side::Right,
            zone: 372
        }
    );
    assert_eq!(actions[3], TweenAction::CopyFresh);
    assert_eq!(actions[4], TweenAction::Present);
    // The gap does not tween: the held strip holds still, on the side
    // the strip rested at.
    assert_eq!(plan.damage(), (1050, 1080), "the fresh crop is damaged");
    let source = plan.frame().source();
    assert_eq!(
        source.x(),
        i32::try_from(plan.frame().wide_width_dev()).expect("test range") - source.width(),
        "a right-docked crop ends at the buffer's right edge"
    );
}

/// The gap rule the frames commit: a flagged gap tween moves the strip
/// with the panel, an unflagged one holds the strip the apply staged —
/// the zone the finish writes, so the frames and the finish agree.
#[test]
fn the_commit_plan_follows_the_gap_rule() {
    let tweening = holder(Side::Left, 360, 1080, 180, true);
    let plan = tweening
        .commit_plan(500, true)
        .expect("a presentable frame");
    assert_eq!(
        plan.actions()[4],
        TweenAction::Reserve {
            side: Side::Left,
            zone: 512
        },
        "the strip moves with the panel"
    );

    let held = holder(Side::Left, 360, 1080, 180, false);
    let plan = held.commit_plan(500, true).expect("a presentable frame");
    assert_eq!(
        plan.actions()[4],
        TweenAction::Reserve {
            side: Side::Left,
            zone: 372
        },
        "the strip the apply staged holds still"
    );
}

/// A frame nothing can present is refused rather than committed: a
/// non-positive eased width, a width past the wide buffer, and a zero
/// height (the holder's cached height is never zero, so the last case
/// is reachable only through a hand-built holder).
#[test]
fn unpresentable_frames_are_refused() {
    let held = holder(Side::Left, 360, 1080, 180, true);
    assert!(held.commit_plan(0, true).is_none());
    assert!(held.commit_plan(-5, true).is_none());
    assert!(
        held.commit_plan(2000, true).is_none(),
        "past the wide buffer"
    );

    let mut zero_height = holder(Side::Left, 360, 1080, 180, true);
    zero_height.height = 0;
    assert!(zero_height.commit_plan(500, true).is_none());
}

/// The rebuild-or-keep rule (D7): a retarget whose tween crop and scale
/// the cache was drawn for keeps it, a different wide width or a scale
/// change rebuilds and redraws.
#[test]
fn an_equal_wide_cache_survives_a_retarget_and_a_different_one_rebuilds() {
    let held = holder(Side::Left, 360, 1080, 180, true);

    // The same crop and scale: kept. (The retarget's from is the eased
    // width, so an expanding retarget's crop can be identical.)
    let same = TweenCrop::new(Side::Left, 360, 1080).expect("a valid tween");
    assert!(held.keeps(same, scale(180)));

    // A different wide width: rebuilt.
    let other = TweenCrop::new(Side::Left, 700, 1440).expect("a valid tween");
    assert!(!held.keeps(other, scale(180)));

    // A scale change invalidates the cache.
    assert!(!held.keeps(same, scale(120)));

    // A side never changes mid-tween, but the rule pins it anyway.
    let flipped = TweenCrop::new(Side::Right, 360, 1080).expect("a valid tween");
    assert!(!held.keeps(flipped, scale(180)));
}

/// A retarget that keeps the cache moves the layout and gap state the
/// frames commit: the new target's margins and the new apply's
/// pre-mutation gap read. The canvas, buffer, crop, height and scale
/// stay.
#[test]
fn a_kept_retarget_moves_the_layout_and_gap_state() {
    let mut held = holder(Side::Left, 360, 1080, 180, false);
    let canvas = held.canvas().size();
    held.retarget(layout(Side::Left, 160, 12, 0), Side::Left, 1092, true);

    let plan = held.commit_plan(900, true).expect("a presentable frame");
    assert_eq!(plan.actions()[1], TweenAction::PanelMargins((0, 0, 0, 12)));
    assert_eq!(
        plan.actions()[4],
        TweenAction::Reserve {
            side: Side::Left,
            zone: 912
        },
        "the retargeted gap tweens"
    );
    assert_eq!(held.canvas().size(), canvas, "the cache stays");
    assert!(held.gap_tweening(), "the new apply's flag");
}

/// A fresh holder draws the live grid into the wide canvas through the
/// render state and plans its begin frame — driven here with the thread's
/// renderer over the test family
/// (font-gated, like the crop module's draw tests).
#[test]
fn a_fresh_holder_draws_from_the_render_state() {
    use crate::fontconfig::FontConfig;
    use crate::guard::Poisoned;
    use crate::panel::wayland_side::renderer::FontSetup;
    use crate::render::font::FontBook;
    use crate::render::text_pass::test_support;
    use crate::term::{PngDecoder, PtySink, Terminal};

    struct NullSink;
    impl PtySink for NullSink {
        fn write_pty(&mut self, _data: &[u8]) {}
    }
    struct NoDecoder;
    impl PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
            None
        }
    }

    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(test_support::FAMILY) {
        println!("skipped: {} is not installed", test_support::FAMILY);
        return;
    }
    let config = FontConfig {
        family: Some(test_support::FAMILY.to_owned()),
        size: test_support::SIZE,
    };
    let setup = FontSetup::resolve_over(book, &config).expect("the family resolves");
    let cell = setup.cell().expect("the cell fits");
    let renderer =
        Renderer::new(setup, 1.5, test_support::THEME, None).expect("the renderer builds");

    let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
    assert!(terminal.push_size(8, 4, cell.width().get(), cell.height().get()));
    terminal.push_pty_data(test_support::HIDE_CURSOR);
    let mut render = TweenRender {
        terminal: Rc::new(RefCell::new(terminal)),
        renderer: Rc::new(RefCell::new(renderer)),
    };

    // A 40 -> 120 column tween at a 9 px cell: the grid drawn is the
    // live 360 px one, the wide canvas is the 1080 px target.
    let crop = TweenCrop::new(Side::Left, 360, 1080).expect("a valid tween");
    let held = TweenDraw::draw(
        crop,
        720,
        scale(180),
        layout(Side::Left, 120, 0, 12),
        Side::Left,
        372,
        true,
        360,
        &mut render,
    )
    .expect("the wide cache draws");
    assert_eq!(held.canvas().size().0, 1620, "1080 logical px at 1.5");
    assert!(held.buffer().is_none(), "the upload is the session's step");

    let plan = held.commit_plan(360, true).expect("the begin frame plans");
    assert_eq!(plan.actions()[0], TweenAction::PanelSize(360));
    assert_eq!(plan.damage(), (1620, 1080));
}

/// The finish's final frame (replace-gtk-with-wayland D7): the plan at the
/// target is the whole wide buffer — the viewport source and destination
/// end at the exact target, and the fallback's fresh copy covers the
/// buffer too. The finish commits it before dropping the cache (the
/// render seam makes it reachable).
#[test]
fn the_finishs_final_frame_plan_shows_the_full_wide_buffer_at_the_target() {
    let held = holder(Side::Left, 360, 1080, 180, true);
    let plan = held.commit_plan(1080, true).expect("a presentable frame");
    let source = plan.frame().source();
    assert_eq!(
        (source.x(), source.y(), source.width(), source.height()),
        (0, 0, 1620, 1080),
        "the whole wide buffer"
    );
    assert_eq!(plan.actions()[0], TweenAction::PanelSize(1080));
    assert_eq!(
        plan.actions()[3],
        TweenAction::ViewportDestination(1080, 720)
    );

    // The fallback copies the full crop: its source covers the buffer.
    let held = holder(Side::Right, 360, 1080, 180, false);
    let plan = held.commit_plan(1080, false).expect("a presentable frame");
    assert_eq!(plan.actions()[3], TweenAction::CopyFresh);
    let source = plan.frame().source();
    assert_eq!(
        (source.x(), source.width()),
        (0, 1620),
        "the fresh copy shows the whole buffer"
    );
}
