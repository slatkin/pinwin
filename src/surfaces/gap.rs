//! The held-gap state and its pure decisions (overlay-expand D2, D3, D5):
//! the side and zone the reserve surface holds, how a validated publish
//! moves them, and what a frame draws. GTK-free so every decision is unit
//! tested here, like [`crate::layout`].
//!
//! The state lives on the panel thread's surface state as a `Cell`; these
//! helpers
//! own the decisions about it — when a publish moves it and what a frame
//! draws from it — while the surface state itself only stores and fetches
//! what they compute.

use crate::layout::{CellSize, Coverage, Layout, OutputSize, Side};

/// The gap the reserve surface holds: its side and exclusive zone in pixels.
/// A zone of zero reserves nothing, which is a covering start's held strip
/// (overlay-expand D5): no pushing layout has ever applied, so there is no
/// earlier pushing width to hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeldGap {
    side: Side,
    zone: i32,
}

impl HeldGap {
    /// The side the reserve surface anchors to.
    #[must_use]
    pub(crate) const fn side(self) -> Side {
        self.side
    }

    /// The exclusive zone the reserve surface carries.
    #[must_use]
    pub(crate) const fn zone(self) -> i32 {
        self.zone
    }
}

/// The held gap a panel starts with (overlay-expand D2, D5): a pushing start
/// reserves its own strip from the first frame; a covering start holds an
/// empty strip on its side until the first pushing apply establishes one.
pub(crate) fn start_held_gap(layout: Layout, cell_w: i32) -> HeldGap {
    let zone = match layout.coverage() {
        Coverage::Push => pushing_strip(layout, cell_w).unwrap_or(0),
        Coverage::Cover => 0,
    };
    HeldGap {
        side: layout.side(),
        zone,
    }
}

/// The held gap after a validated publish (overlay-expand D2, D4): a pushing
/// layout resets both the side and the zone to its own strip; a covering
/// layout never touches the zone and moves the side only on a side switch —
/// the gap follows the panel to the new side at the last pushing width.
pub(super) fn held_gap_after_publish(held: HeldGap, layout: Layout, cell_w: i32) -> HeldGap {
    let zone = match layout.coverage() {
        Coverage::Push => pushing_strip(layout, cell_w).unwrap_or(held.zone),
        Coverage::Cover => held.zone,
    };
    HeldGap {
        side: layout.side(),
        zone,
    }
}

/// The strip a pushing layout reserves at its own width (`cols * cell_w`),
/// `None` where [`pushing_strip_at`] rejects the width.
fn pushing_strip(layout: Layout, cell_w: i32) -> Option<i32> {
    let panel_px = i64::from(layout.cols().get()) * i64::from(cell_w);
    pushing_strip_at(layout, i32::try_from(panel_px).ok()?)
}

/// The gap the reserve surface draws for one frame: its side and exclusive
/// zone (overlay-expand D2, D3). The applied layout drives only the visible
/// panel; the reservation follows the held gap, so a covering layout leaves
/// the strip exactly where the last pushing layout put it. While a publish
/// has flagged the gap as tweening (the surfaces' `gap_tweening` flag) — the one
/// case where a pushing target's strip differs from the strip the gap rests
/// at — the gap tweens with the panel instead, so tiles reflow alongside the
/// growing panel. The caller combines the flag with the tween being live, so
/// a stopped tween falls back to the held strip, which a pushing publish has
/// already staged at the target strip.
pub(crate) fn reserve_gap(
    held: HeldGap,
    gap_tweening: bool,
    layout: Layout,
    panel_px: i32,
) -> HeldGap {
    if gap_tweening
        && layout.coverage() == Coverage::Push
        && let Some(current) = pushing_strip_at(layout, panel_px)
    {
        return HeldGap {
            side: layout.side(),
            zone: current,
        };
    }
    held
}

/// The strip a layout reserves at an explicit panel width, `None` on the
/// checked-arithmetic overflow that a validated apply excludes.
fn pushing_strip_at(layout: Layout, panel_px: i32) -> Option<i32> {
    layout
        .side_geometry(i64::from(panel_px))
        .ok()
        .map(|g| g.reservation())
}

/// [`reserve_gap`] over the held gap's plain parts, for a caller that holds
/// the side and zone as separate values: the wayland tween's per-frame crop
/// plan (`replace-gtk-with-wayland` D7) reads them from the panel state,
/// and the decision itself stays here, stated once.
pub fn reserve_gap_parts(
    held_side: Side,
    held_zone: i32,
    gap_tweening: bool,
    layout: Layout,
    panel_px: i32,
) -> (Side, i32) {
    let held = reserve_gap(
        HeldGap {
            side: held_side,
            zone: held_zone,
        },
        gap_tweening,
        layout,
        panel_px,
    );
    (held.side(), held.zone())
}

/// The decision half of a publish (overlay-expand D4), split out
/// so the reject-before-mutate ordering is unit testable without a display:
/// validate the staged layout against the live output metrics first, and only
/// a validated layout yields the staged mutation — the applied layout and the
/// held gap. `None` is a rejected publish, which leaves the applied
/// layout and the held gap untouched.
pub(crate) fn staged_publish(
    output: OutputSize,
    cell: CellSize,
    held_gap: HeldGap,
    layout: Layout,
) -> Option<(Layout, HeldGap)> {
    layout.validate(cell, output).ok()?;
    Some((
        layout,
        held_gap_after_publish(held_gap, layout, cell.width().get()),
    ))
}

/// Whether an animated apply tweens the gap with the panel (overlay-expand
/// D3): only a pushing target whose strip differs from the strip the gap
/// rests at right now. The strip the gap rests at is read before the apply
/// mutates the held gap, so a covering excursion's shrink-back — whose
/// target strip equals the held one — holds the gap still, while a pure
/// pushing expand, whose strip is new, reflows the tiles alongside.
/// `pub(crate)`: the wayland apply's staging half reads the same decision
/// (`replace-gtk-with-wayland` D7), so it stays stated once here.
pub(crate) fn gap_tween_decision(
    gap_rests_at: i32,
    animate: bool,
    layout: Layout,
    target_px: i32,
) -> bool {
    animate
        && layout.coverage() == Coverage::Push
        && pushing_strip_at(layout, target_px).is_some_and(|strip| strip != gap_rests_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU16;

    fn layout(side: Side, cols: u16, top: i32, bottom: i32, left: i32, right: i32) -> Layout {
        Layout::new(
            side,
            NonZeroU16::new(cols).expect("test column count is non-zero"),
            top,
            bottom,
            left,
            right,
        )
    }

    /// The reserve surface draws from the held gap, not the applied layout
    /// (overlay-expand D2): a covering 120-column layout over a 40-column held
    /// strip keeps the strip exactly where the last pushing layout put it, even
    /// though the covering layout's own geometry would reserve far more.
    #[test]
    fn the_reserve_draws_from_the_held_gap_not_the_applied_layout() {
        let held = HeldGap {
            side: Side::Left,
            zone: 372,
        };
        // Covering, tween idle: the held strip, on the held side.
        let covering = layout(Side::Left, 120, 0, 0, 0, 12).covering();
        assert_eq!(
            reserve_gap(held, false, covering, 1080),
            HeldGap {
                side: Side::Left,
                zone: 372
            }
        );
        // A pushing applied layout with no tween running also draws from the
        // held gap: the publish has already reset it to the layout's own strip,
        // so the two agree.
        let pushing = layout(Side::Left, 120, 0, 0, 0, 12);
        assert_eq!(
            reserve_gap(held, false, pushing, 1080),
            HeldGap {
                side: Side::Left,
                zone: 372
            }
        );
    }

    /// The one place the strip moves with the panel: a publish that flagged the
    /// gap as tweening (a pushing target whose strip differs from the strip the
    /// gap rests at) tweens the gap alongside the eased panel width
    /// (overlay-expand D3), so tiles reflow instead of jumping.
    #[test]
    fn a_flagged_gap_tween_moves_the_gap_with_the_panel() {
        let held = HeldGap {
            side: Side::Left,
            zone: 372,
        };
        // Pushing 40 -> 120 columns at a 9px cell, mid-tween at 700px of panel:
        // the gap shows the strip at the eased width, left 0 + 700 + right 12.
        let tweening = layout(Side::Left, 120, 0, 0, 0, 12);
        assert_eq!(
            reserve_gap(held, true, tweening, 700),
            HeldGap {
                side: Side::Left,
                zone: 712
            }
        );
        // The gap follows the tweening layout's side.
        let tweening = layout(Side::Right, 120, 0, 0, 0, 12);
        assert_eq!(
            reserve_gap(held, true, tweening, 700),
            HeldGap {
                side: Side::Right,
                zone: 712
            }
        );
    }

    /// A covering target never moves the gap, tween flag or not: the panel eases
    /// over the tiles while the strip stays at the last pushing width. The flag
    /// is never set for a covering target, so this pins the belt-and-braces
    /// guard inside `reserve_gap`.
    #[test]
    fn a_covering_tween_holds_the_gap() {
        let held = HeldGap {
            side: Side::Left,
            zone: 372,
        };
        let tweening = layout(Side::Left, 120, 0, 0, 0, 12).covering();
        assert_eq!(
            reserve_gap(held, true, tweening, 700),
            HeldGap {
                side: Side::Left,
                zone: 372
            }
        );
    }

    /// The gap-tween decision (overlay-expand D3): an animated pushing target
    /// whose strip differs from the strip the gap rests at tweens the gap with
    /// the panel; a target whose strip equals it, a covering target and a snap
    /// apply never do.
    #[test]
    fn the_gap_tween_decision_compares_against_the_strip_the_gap_rests_at() {
        // Pushing 40 -> 120 columns at a 9px cell: the gap rests at the held
        // 372 strip and the target strip is 1092, so the gap tweens.
        let expanding = layout(Side::Left, 120, 0, 0, 0, 12);
        assert!(gap_tween_decision(372, true, expanding, 1080));

        // Shrinking a covering excursion back to the original pushing width:
        // the gap rests at the held 372 strip and the target strip is 372, so
        // the gap holds still while the panel narrows over it.
        let shrinking_back = layout(Side::Left, 40, 0, 0, 0, 12);
        assert!(!gap_tween_decision(372, true, shrinking_back, 360));

        // A covering target never tweens the gap.
        let covering = layout(Side::Left, 120, 0, 0, 0, 12).covering();
        assert!(!gap_tween_decision(372, true, covering, 1080));

        // A snap apply (zero duration, disabled animations, a side or gutter
        // change) applies the strip in one step instead of tweening it.
        assert!(!gap_tween_decision(372, false, expanding, 1080));

        // A pushing target whose strip the gap already rests at does not tween.
        assert!(!gap_tween_decision(1092, true, expanding, 1080));
    }

    /// The held gap a panel starts with (overlay-expand D5): a pushing start
    /// reserves its own strip from the first frame; a covering start holds an
    /// empty strip on its side until the first pushing apply establishes one.
    #[test]
    fn the_starting_held_gap_follows_the_startup_choice() {
        // Pushing 40 columns at a 9px cell, left 0 right 12: strip 372.
        let pushing = layout(Side::Left, 40, 0, 0, 0, 12);
        assert_eq!(
            start_held_gap(pushing, 9),
            HeldGap {
                side: Side::Left,
                zone: 372
            }
        );
        // A covering start reserves nothing: zone zero, no earlier pushing
        // width to hold.
        let covering = layout(Side::Left, 120, 0, 0, 0, 12).covering();
        assert_eq!(
            start_held_gap(covering, 9),
            HeldGap {
                side: Side::Left,
                zone: 0
            }
        );
    }

    /// A validated pushing publish resets both the side and the zone to its own
    /// strip; a covering publish never touches the zone and moves the side only
    /// on a side switch, where the gap follows at the last pushing width
    /// (overlay-expand D2).
    #[test]
    fn the_held_gap_follows_a_validated_publish_by_choice() {
        let held = HeldGap {
            side: Side::Left,
            zone: 372,
        };

        // A pushing publish on the same side resets the zone to its own strip
        // (120 columns at a 9px cell, left 0 right 12: 1092).
        let pushing = layout(Side::Left, 120, 0, 0, 0, 12);
        assert_eq!(
            held_gap_after_publish(held, pushing, 9),
            HeldGap {
                side: Side::Left,
                zone: 1092
            }
        );

        // A pushing publish on the other side moves both.
        let pushing = layout(Side::Right, 40, 0, 0, 0, 12);
        assert_eq!(
            held_gap_after_publish(held, pushing, 9),
            HeldGap {
                side: Side::Right,
                zone: 372
            }
        );

        // A covering publish on the same side touches nothing.
        let covering = layout(Side::Left, 120, 0, 0, 0, 12).covering();
        assert_eq!(held_gap_after_publish(held, covering, 9), held);

        // A covering publish on the other side moves the side at the held
        // width: the shuffle belongs to whoever moved the panel.
        let covering = layout(Side::Right, 120, 0, 0, 0, 12).covering();
        assert_eq!(
            held_gap_after_publish(held, covering, 9),
            HeldGap {
                side: Side::Right,
                zone: 372
            }
        );
    }

    /// A rejected covering apply stages nothing (spec Requirement
    /// **Rejected-layout-safety**, task 1.2): the decision half validates
    /// first and stages second, so a covering layout the output cannot hold
    /// yields `None` — no applied layout, no held gap move —
    /// and the publish that drives it returns `InvalidLayout` before any
    /// field is set (overlay-expand D4).
    #[test]
    fn a_rejected_covering_publish_stages_nothing() {
        fn output(width: i32, height: i32) -> OutputSize {
            OutputSize::new(width, height).expect("test output is non-degenerate")
        }
        fn cell(width: i32, height: i32) -> CellSize {
            CellSize::new(width, height).expect("test cell is non-degenerate")
        }

        // A pushing 40-column start at a 9px cell holds its own 372 strip.
        let held = start_held_gap(layout(Side::Left, 40, 0, 0, 0, 12), 9);
        let held = held_gap_after_publish(held, layout(Side::Left, 40, 0, 0, 0, 12), 9);

        // A covering 120-column layout is 1080px wide at that cell, past the
        // 600px output: rejected, so nothing is staged.
        let too_wide = layout(Side::Left, 120, 0, 0, 0, 12).covering();
        assert_eq!(
            staged_publish(output(600, 1080), cell(9, 16), held, too_wide),
            None
        );

        // For contrast, the same covering layout against a wide-enough
        // output stages the pair — and the held gap is not part of
        // it: a covering publish never moves the gap.
        let staged =
            staged_publish(output(1920, 1080), cell(9, 16), held, too_wide).expect("layout fits");
        assert_eq!(staged.0, too_wide);
        assert_eq!(staged.1, held);
    }

    /// A layout the output metrics refuse stages nothing either
    /// (`staged_publish` validates before it stages, overlay-expand D4):
    /// the reachable `Layout::validate` rejects — no reserve, no width,
    /// no row and the checked-arithmetic overflow — all yield `None`, and
    /// for contrast the same layout against roomier metrics stages.
    #[test]
    fn a_publish_the_layout_refuses_stages_nothing() {
        fn output(width: i32, height: i32) -> OutputSize {
            OutputSize::new(width, height).expect("test output is non-degenerate")
        }
        fn cell(width: i32, height: i32) -> CellSize {
            CellSize::new(width, height).expect("test cell is non-degenerate")
        }

        let held = start_held_gap(layout(Side::Left, 40, 0, 0, 0, 12), 9);

        // A pushing 600-column layout at a 1px cell exactly covers the
        // 600px output: no reserve is left, so the publish rejects.
        assert_eq!(
            staged_publish(
                output(600, 1080),
                cell(1, 16),
                held,
                layout(Side::Left, 600, 0, 0, 0, 0)
            ),
            None
        );
        // Top and bottom gutters leave no row for even one cell.
        assert_eq!(
            staged_publish(
                output(1920, 1080),
                cell(9, 16),
                held,
                layout(Side::Left, 1, 1000, 1000, 0, 0)
            ),
            None
        );
        // Negative gutters reject the same way.
        assert_eq!(
            staged_publish(
                output(1920, 1080),
                cell(9, 16),
                held,
                layout(Side::Left, 1, 0, 0, -100, -100)
            ),
            None
        );
        // A 65535-column layout at a 65536px cell overflows the checked
        // arithmetic before any verdict runs.
        assert_eq!(
            staged_publish(
                output(1920, 1080),
                cell(65536, 16),
                held,
                layout(Side::Left, 65535, 0, 0, 0, 0)
            ),
            None
        );

        // For contrast: the same overflow-free layout at a sane cell stages.
        let staged = staged_publish(
            output(1920, 1080),
            cell(9, 16),
            held,
            layout(Side::Left, 40, 0, 0, 0, 12),
        )
        .expect("layout fits");
        assert_eq!(staged.0, layout(Side::Left, 40, 0, 0, 0, 12));
    }
}
