//! The layout apply on the wayland surfaces (replace-gtk-with-wayland row
//! 3.5): the shape of the GTK side's `Surfaces::publish`
//! ([`crate::surfaces`]) without the tween — validation, the held gap,
//! covering layouts and side switches, applied at the final width.
//!
//! The pure decisions stay in [`crate::surfaces::gap`]: `staged_publish`
//! validates and stages, `start_held_gap` seeds the held strip the panel
//! state starts from, and the held gap is what the reserve surface follows —
//! a covering apply leaves the strip where the last pushing layout put it,
//! and a side switch moves it to the new side at the held width
//! (overlay-expand D2). No tween runs yet, so the drawn gap is exactly the
//! held one and `gap::reserve_gap`'s tweening case does not apply; group 6
//! (decision 7) routes the animated applies through it.
//!
//! `PanelState::apply_against` is the display-free seam (`port-to-rust`
//! D10): the output and the grid-push sink are parameters, so the tests
//! drive the same staging, held-gap and pty-push path the production
//! `PanelState::apply` runs against the resolved output and the pty fd.

use crate::layout::{CellSize, Layout, OutputSize, Side};
use crate::surfaces::PublishOutcome;
use crate::surfaces::gap::{HeldGap, staged_publish};

use super::sizing::Grid;
use super::state::{PanelState, apply_pty_size};
use super::surfaces::{grid_width_px, panel_margins};

/// One validated apply, staged (overlay-expand D4): the layout to apply and
/// the held gap it leaves behind. A rejected apply produces no value, so the
/// caller cannot partially apply one.
#[derive(Debug)]
pub(crate) struct StagedApply {
    pub(crate) layout: Layout,
    pub(crate) held_gap: HeldGap,
}

/// Validate a layout the way `Surfaces::publish` does and stage the mutation
/// a validated one may make: [`staged_publish`] runs the metrics and layout
/// checks, so `None` is a rejected apply that leaves the applied state — the
/// layout, the column count and the held gap — untouched.
fn stage(output: OutputSize, cell: CellSize, held: HeldGap, layout: Layout) -> Option<StagedApply> {
    let (layout, _cols, held_gap) = staged_publish(
        output.width().get(),
        output.height().get(),
        cell.width().get(),
        cell.height().get(),
        held,
        layout,
    )?;
    Some(StagedApply { layout, held_gap })
}

/// The geometry one staged apply pushes onto the two layer surfaces: the
/// panel re-anchors to the layout's docking side with its margins and pixel
/// width, the reserve re-anchors to the held gap's side with its zone
/// (overlay-expand D2). The pure mapping is unit tested here
/// (`port-to-rust` D10); [`super::surfaces::PanelSurfaces::apply_geometry`]
/// writes it onto the real surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfaceGeometry {
    /// The panel's docking side: which horizontal anchor set.
    pub(crate) panel_side: Side,
    /// The panel's margins in the `set_margin` order (top, right, bottom,
    /// left).
    pub(crate) panel_margins: (i32, i32, i32, i32),
    /// The panel's pixel width for `set_size`. `None` only for a width that
    /// does not fit `i32`, which a validated layout excludes.
    pub(crate) panel_width: Option<i32>,
    /// The reserve's docking side: the held gap's side.
    pub(crate) reserve_side: Side,
    /// The reserve's exclusive zone: the held gap's zone.
    pub(crate) reserve_zone: i32,
}

/// The surface geometry of one staged apply.
fn surface_geometry(staged: &StagedApply, cell: CellSize) -> SurfaceGeometry {
    SurfaceGeometry {
        panel_side: staged.layout.side(),
        panel_margins: panel_margins(staged.layout),
        panel_width: grid_width_px(staged.layout.cols(), cell),
        reserve_side: staged.held_gap.side(),
        reserve_zone: staged.held_gap.zone(),
    }
}

impl PanelState {
    /// The production apply (row 3.5), the shape of the GTK side's
    /// `dispatch_apply`: validate against the resolved output, stage, move
    /// the held gap, push the grid through the row 3.3 sizing path, and
    /// write the geometry onto the two surfaces. Answers `NotLive` while the
    /// output is unresolved (the GTK side's missing monitor) or the surfaces
    /// are gone (torn down, the GTK side's closed), `InvalidLayout` for a
    /// layout the output refuses, and `Applied` otherwise.
    pub(crate) fn apply(&mut self, layout: Layout, _duration_ms: u32) -> PublishOutcome {
        // The width animation is group 6 (decision 7): `duration_ms` drives
        // the tween there. The row 3.5 apply takes the end state directly.
        let Some(output) = self.session.as_ref().and_then(|session| session.resolved) else {
            return PublishOutcome::NotLive;
        };
        // A torn-down session has no surfaces to write, even though its
        // resolved output still is: the panel is gone (NotLive, the same
        // lifecycle verdict the GTK side's closed surfaces give).
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.surfaces.is_none())
        {
            return PublishOutcome::NotLive;
        }
        let fd = self.startup.fd;
        let geometry =
            match self.apply_against(output, layout, &mut |grid| apply_pty_size(fd, grid)) {
                Ok(geometry) => geometry,
                Err(outcome) => return outcome,
            };
        if let Some(surfaces) = self
            .session
            .as_mut()
            .and_then(|session| session.surfaces.as_mut())
        {
            surfaces.apply_geometry(&geometry);
        }
        PublishOutcome::Applied
    }

    /// The apply's decision and state half, against an output the caller
    /// names and with the grid pushes observed through `push`
    /// (`port-to-rust` D10): validate and stage, move the held gap, and push
    /// the derived grid through the row 3.3 sizing path — a layout apply is
    /// one of the two push sources, and a repeated apply of the same columns
    /// derives the same grid and pushes nothing. Returns the geometry the
    /// caller writes onto the surfaces; `Err` is the publish verdict and
    /// stages nothing.
    pub(crate) fn apply_against(
        &mut self,
        output: OutputSize,
        layout: Layout,
        push: &mut dyn FnMut(Grid),
    ) -> Result<SurfaceGeometry, PublishOutcome> {
        let Some(staged) = stage(output, self.cell, self.held, layout) else {
            return Err(PublishOutcome::InvalidLayout);
        };
        self.held = staged.held_gap;
        self.sizing.apply_columns(staged.layout.cols(), push);
        Ok(surface_geometry(&staged, self.cell))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Inner, Startup};
    use super::*;
    use crate::guard::Poisoned;
    use crate::layout::Keyboard;
    use crate::panel::handshake::Handshake;
    use crate::surfaces::gap::start_held_gap;
    use std::num::NonZeroU16;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    /// A startup for the tests; the thread does not touch the pty fd until
    /// the surfaces push a grid, so a placeholder fd is fine here.
    fn startup() -> Startup {
        Startup {
            fd: -1,
            layout: layout(Side::Left, 40, 0, 0, 0, 0),
            keyboard: Keyboard::OnDemand,
            accent: None,
        }
    }

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

    /// The startup cell metrics the tests carry, the way a start command
    /// would (D3).
    fn cell() -> CellSize {
        CellSize::new(9, 16).expect("test cell size is non-zero")
    }

    fn output(width: i32, height: i32) -> OutputSize {
        OutputSize::new(width, height).expect("test output size is non-zero")
    }

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<Inner> {
        Arc::new(Inner {
            id: 0,
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        })
    }

    fn headless_state() -> PanelState {
        let (tx, _rx) = mpsc::channel();
        PanelState::headless(
            Handshake::new(tx),
            Poisoned::new(),
            live_inner(),
            startup(),
            cell(),
        )
    }

    /// An invalid layout is rejected with the applied state untouched: the
    /// verdict is `InvalidLayout`, no grid is pushed and the held gap stays
    /// at the startup one — a rejected covering apply stages nothing
    /// (overlay-expand D4).
    #[test]
    fn an_invalid_layout_is_rejected_and_stages_nothing() {
        let mut state = headless_state();
        // A covering 120-column layout is 1080px wide at a 9px cell, past
        // the 600px output: rejected.
        let too_wide = layout(Side::Left, 120, 0, 0, 0, 0).covering();
        let mut pushed: Vec<Grid> = Vec::new();
        let outcome =
            state.apply_against(output(600, 1080), too_wide, &mut |grid| pushed.push(grid));
        assert_eq!(outcome, Err(PublishOutcome::InvalidLayout));
        assert!(pushed.is_empty(), "a rejected apply pushes no grid");
        assert_eq!(
            state.held,
            start_held_gap(startup().layout, cell().width().get()),
            "the held gap is untouched"
        );
    }

    /// A valid pushing apply moves the reserve zone and the panel size to
    /// the target layout's own strip and width, moves the held gap with
    /// them, and pushes the widened grid through the sizing path.
    #[test]
    fn a_valid_pushing_apply_moves_the_zone_and_the_size() {
        let mut state = headless_state();
        // A configure delivered the startup height, so the rows are live.
        state.sizing.configure(1080, &mut |_| {});

        let target = layout(Side::Left, 120, 0, 0, 8, 12);
        let mut pushed: Vec<Grid> = Vec::new();
        let geometry = state
            .apply_against(output(1920, 1080), target, &mut |grid| pushed.push(grid))
            .expect("the layout fits the output");

        assert_eq!(geometry.panel_side, Side::Left);
        assert_eq!(geometry.panel_margins, (0, 0, 0, 8), "the edge gutter");
        assert_eq!(geometry.panel_width, Some(1080), "the final width");
        assert_eq!(geometry.reserve_side, Side::Left);
        assert_eq!(
            geometry.reserve_zone,
            8 + 1080 + 12,
            "the reserve holds the target's own strip"
        );
        assert_eq!(pushed.len(), 1, "the widened grid is pushed");
        assert_eq!(pushed[0].cols(), 120, "the columns come from the layout");
        assert_eq!(pushed[0].rows(), 1080 / 16, "the rows stay from the height");
        assert_eq!(
            state.held.zone(),
            1100,
            "a pushing apply resets the held strip to its own"
        );
    }

    /// A covering apply holds the strip: the reserve keeps the last pushing
    /// layout's side and zone (overlay-expand D2) while the panel takes the
    /// covering layout's width, and the held gap is untouched.
    #[test]
    fn a_covering_apply_holds_the_strip() {
        let mut state = headless_state();
        // A pushing start holds its own strip: 40 columns at 9px.
        assert_eq!(state.held.zone(), 360);
        // A configure delivered the startup height, so the rows are live.
        state.sizing.configure(1080, &mut |_| {});

        let covering = layout(Side::Left, 120, 0, 0, 0, 0).covering();
        let mut pushed: Vec<Grid> = Vec::new();
        let geometry = state
            .apply_against(output(1920, 1080), covering, &mut |grid| pushed.push(grid))
            .expect("the covering layout fits the output");

        assert_eq!(geometry.panel_side, Side::Left);
        assert_eq!(geometry.panel_width, Some(1080), "the covering width");
        assert_eq!(geometry.reserve_side, Side::Left, "the held side");
        assert_eq!(
            geometry.reserve_zone, 360,
            "the strip stays where the last pushing layout put it"
        );
        assert_eq!(state.held.zone(), 360, "the held gap is untouched");
        assert_eq!(pushed.len(), 1, "the grid still follows the columns");
    }

    /// A side switch re-anchors both surfaces: the panel to the new side,
    /// the reserve with it at the held width — the shuffle belongs to
    /// whoever moved the panel (overlay-expand D2).
    #[test]
    fn a_side_switch_re_anchors_both_surfaces() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});

        let switched = layout(Side::Right, 120, 0, 0, 0, 0).covering();
        let geometry = state
            .apply_against(output(1920, 1080), switched, &mut |_| {})
            .expect("the layout fits the output");

        assert_eq!(geometry.panel_side, Side::Right, "the panel re-anchors");
        assert_eq!(geometry.panel_margins, (0, 0, 0, 0));
        assert_eq!(
            geometry.reserve_side,
            Side::Right,
            "the gap follows the panel to the new side"
        );
        assert_eq!(geometry.reserve_zone, 360, "at the last pushing width");

        // A pushing switch also moves the strip to the new side's own.
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        let geometry = state
            .apply_against(
                output(1920, 1080),
                layout(Side::Right, 40, 0, 0, 0, 0),
                &mut |_| {},
            )
            .expect("the layout fits the output");
        assert_eq!(geometry.reserve_side, Side::Right);
        assert_eq!(geometry.reserve_zone, 360, "the new side's own strip");
    }

    /// A repeat apply of the same layout derives the same grid and pushes
    /// nothing more: the pty size has one source (D3) and a repeated apply
    /// is not one of them.
    #[test]
    fn a_repeat_apply_of_the_same_layout_pushes_no_extra_grid() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        let target = layout(Side::Left, 120, 0, 0, 0, 0);

        let mut pushed: Vec<Grid> = Vec::new();
        let first = state
            .apply_against(output(1920, 1080), target, &mut |grid| pushed.push(grid))
            .expect("the layout fits the output");
        let second = state
            .apply_against(output(1920, 1080), target, &mut |grid| pushed.push(grid))
            .expect("the layout fits the output");

        assert_eq!(pushed.len(), 1, "the repeat apply pushes nothing");
        assert_eq!(first.panel_width, second.panel_width);
        assert_eq!(first.reserve_zone, second.reserve_zone);
    }

    /// An apply before the first configure records the columns without a
    /// push: there is no height to derive rows from, and the next configure
    /// derives with the applied columns (the sizing path's rule).
    #[test]
    fn an_apply_before_the_first_configure_records_the_columns() {
        let mut state = headless_state();
        let mut pushed: Vec<Grid> = Vec::new();
        let geometry = state
            .apply_against(
                output(1920, 1080),
                layout(Side::Left, 120, 0, 0, 0, 0),
                &mut |grid| pushed.push(grid),
            )
            .expect("the layout fits the output");
        assert!(pushed.is_empty(), "no height, no push");
        assert_eq!(geometry.panel_width, Some(1080));

        state.sizing.configure(1080, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.len(), 1, "the next configure derives");
        assert_eq!(pushed[0].cols(), 120, "with the applied columns");
    }

    /// The production apply without a bound session is `NotLive` (D10): the
    /// headless core has no resolved output to validate against, the same
    /// lifecycle verdict the GTK side's missing monitor gives.
    #[test]
    fn an_apply_without_a_session_is_not_live() {
        let mut state = headless_state();
        assert_eq!(
            state.apply(startup().layout, 0),
            PublishOutcome::NotLive,
            "no session, no publish"
        );
        assert!(!state.done, "an apply does not end the thread");
    }
}
