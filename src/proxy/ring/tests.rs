use super::{Bar, RingState, bars};

/// The standard ring: every side of the panel, with the corners taken by
/// the perpendicular bars (D10).
#[test]
fn a_side_panel_draws_all_four_sides() {
    assert_eq!(
        bars((480, 1080), 2),
        [
            Bar {
                x: 0,
                y: 0,
                width: 480,
                height: 2
            },
            Bar {
                x: 478,
                y: 2,
                width: 2,
                height: 1076
            },
            Bar {
                x: 0,
                y: 1078,
                width: 480,
                height: 2
            },
            Bar {
                x: 0,
                y: 2,
                width: 2,
                height: 1076
            },
        ]
    );
}

/// A top or bottom panel is wide and short: the same four sides, with the
/// vertical bars between the two horizontal ones.
#[test]
fn a_wide_panel_draws_all_four_sides() {
    assert_eq!(
        bars((1920, 270), 3),
        [
            Bar {
                x: 0,
                y: 0,
                width: 1920,
                height: 3
            },
            Bar {
                x: 1917,
                y: 3,
                width: 3,
                height: 264
            },
            Bar {
                x: 0,
                y: 267,
                width: 1920,
                height: 3
            },
            Bar {
                x: 0,
                y: 3,
                width: 3,
                height: 264
            },
        ]
    );
}

/// Every bar stays inside the panel and no two bars share a pixel,
/// whatever the ring width is.
#[test]
fn the_bars_stay_inside_the_panel_without_overlapping() {
    for size in [(480, 1080), (1920, 270), (100, 100), (37, 11), (1, 1)] {
        for width in 1..=6 {
            assert_disjoint(size, width);
        }
    }
}

/// Asserts one panel size's four bars stay inside it and share nothing.
fn assert_disjoint(size: (u32, u32), width: u32) {
    let bars = bars(size, width);
    for (index, bar) in bars.iter().enumerate() {
        assert!(
            inside(*bar, size),
            "bar {index} of {bars:?} leaves the panel {size:?}"
        );
        for other in &bars[index + 1..] {
            assert!(
                !overlaps(*bar, *other),
                "bars {bar:?} and {other:?} share a pixel on {size:?}"
            );
        }
    }
}

/// A panel smaller than twice the ring width still gives four disjoint
/// bars: the horizontal sides split the height and the vertical sides
/// have no room left.
#[test]
fn a_panel_smaller_than_the_ring_keeps_the_bars_disjoint() {
    let degenerate = bars((10, 10), 20);
    assert_eq!(
        degenerate[0],
        Bar {
            x: 0,
            y: 0,
            width: 10,
            height: 10
        },
        "the top bar takes the whole degenerate panel"
    );
    assert!(degenerate[1].is_empty(), "the right bar has no room");
    assert!(degenerate[2].is_empty(), "the bottom bar has no room left");
    assert!(degenerate[3].is_empty(), "the left bar has no room left");
    let narrow = bars((5, 100), 10);
    assert_eq!(
        narrow[1].width, 0,
        "a panel narrower than the ring has no right bar"
    );
    assert_eq!(narrow[3].width, 5, "the left bar is clipped to the panel");
}

/// The ring follows the keyboard focus (spec "Accent appears while
/// focused"): an enter shows it, a leave hides it, and a repeated event
/// changes nothing.
#[test]
fn the_ring_follows_the_keyboard_focus() {
    let mut state = RingState::new(true, false);
    assert!(!state.is_visible(), "a ring starts hidden");
    assert!(state.focus(true), "the enter shows the ring");
    assert!(state.is_visible());
    assert!(!state.focus(true), "a repeated enter changes nothing");
    assert!(state.focus(false), "the leave hides the ring");
    assert!(!state.is_visible());
    assert!(!state.focus(false), "a repeated leave changes nothing");
}

/// A hidden panel has no ring, and its hide gives up the focus, so a
/// later show does not resurrect a stale one (spec "Hide").
#[test]
fn hiding_the_panel_hides_the_ring_and_drops_the_focus() {
    let mut state = RingState::new(true, false);
    assert!(state.focus(true));
    assert!(state.sync(true, true), "the hide takes the ring away");
    assert!(!state.is_visible());
    assert!(!state.sync(true, false), "the show alone shows nothing");
    assert!(!state.is_visible());
    assert!(
        !state.focus(false),
        "no focus event is pending after the show"
    );
    assert!(state.focus(true), "a new enter shows the ring again");
    assert!(state.is_visible());
}

/// A ring setting of `off` never shows anything (spec "Accent disabled"),
/// and turning a shown ring off hides it.
#[test]
fn a_disabled_ring_never_shows() {
    let mut state = RingState::new(false, false);
    assert!(!state.focus(true), "a disabled ring ignores the focus");
    assert!(!state.is_visible());

    let mut shown = RingState::new(true, false);
    assert!(shown.focus(true));
    assert!(shown.is_visible());
    assert!(shown.sync(false, false), "disabling hides the shown ring");
    assert!(!shown.is_visible());
    assert!(!shown.focus(true), "a disabled ring takes no focus");
    assert!(!shown.is_visible());
}

/// Whether `bar` lies inside a `size` panel.
fn inside(bar: Bar, size: (u32, u32)) -> bool {
    let left = i64::from(bar.x);
    let top = i64::from(bar.y);
    let right = left + i64::from(bar.width);
    let bottom = top + i64::from(bar.height);
    left >= 0 && top >= 0 && right <= i64::from(size.0) && bottom <= i64::from(size.1)
}

/// Whether two bars share at least one pixel; an empty bar shares
/// nothing.
fn overlaps(a: Bar, b: Bar) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let a_right = i64::from(a.x) + i64::from(a.width);
    let a_bottom = i64::from(a.y) + i64::from(a.height);
    let b_right = i64::from(b.x) + i64::from(b.width);
    let b_bottom = i64::from(b.y) + i64::from(b.height);
    i64::from(a.x) < b_right
        && i64::from(b.x) < a_right
        && i64::from(a.y) < b_bottom
        && i64::from(b.y) < a_bottom
}
