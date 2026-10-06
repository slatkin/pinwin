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

use std::os::fd::RawFd;
use std::time::Instant;

use crate::layout::{CellSize, Layout, OutputSize, Side};
use crate::surfaces::PublishOutcome;
use crate::surfaces::gap::{HeldGap, gap_tween_decision, reserve_gap_parts, staged_publish};

use super::crop::TweenCrop;
use super::sizing::Grid;
use super::state::{PanelState, apply_pty_size};
use super::surfaces::{grid_width_px, panel_margins};
use super::tween;
use super::tween_draw::TweenDraw;

/// One validated apply, staged (overlay-expand D4): the layout to apply and
/// the held gap it leaves behind. A rejected apply produces no value, so the
/// caller cannot partially apply one.
#[derive(Debug)]
pub(crate) struct StagedApply {
    pub(crate) layout: Layout,
    pub(crate) held_gap: HeldGap,
}

/// One validated animated apply, staged (row 6.2): everything the wide
/// cache and the tween frames need, read before the apply mutated the
/// applied layout and held gap. Constructed only by
/// [`PanelState::stage_animated`] (`port-to-rust` D6).
#[derive(Debug)]
pub(crate) struct StagedTween {
    /// The width the tween eases from: the current one, the running
    /// tween's eased width or the applied width at rest.
    pub(crate) from_px: i32,
    /// The width the tween eases to: the staged layout's pixel width.
    pub(crate) target_px: i32,
    /// The tween's crop plan, decided from the two widths.
    pub(crate) crop: TweenCrop,
    /// The live grid's pixel width: the columns the terminal actually
    /// runs, which the deferred applies have not changed, times the cell
    /// width. The wide draw keys the docked-edge offset against it.
    pub(crate) grid_px: i32,
    /// The gap state the frames commit the reserve with (overlay-expand
    /// D3): the side and zone the strip rested at before this apply
    /// mutated the held gap, and the gap-tween decision.
    pub(crate) gap_side: Side,
    pub(crate) gap_zone: i32,
    pub(crate) gap_tweening: bool,
    /// The tweening layout: the staged target, whose margins the frames
    /// commit and whose side and gutters the animate rule pinned.
    pub(crate) layout: Layout,
    /// The tween's duration, clamped to 1000 ms.
    pub(crate) duration_ms: u32,
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
    /// The production apply (row 3.5, animated in row 6.2), the shape of
    /// the GTK side's `dispatch_apply`: validate against the resolved
    /// output, stage, move the held gap, push the grid through the row 3.3
    /// sizing path — deferred to the tween's finish when the apply animates
    /// — and write the geometry onto the two surfaces. Answers `NotLive`
    /// while the output is unresolved (the GTK side's missing monitor) or
    /// the surfaces are gone (torn down, the GTK side's closed),
    /// `InvalidLayout` for a layout the output refuses, and `Applied`
    /// otherwise.
    pub(crate) fn apply(&mut self, layout: Layout, duration_ms: u32) -> PublishOutcome {
        // The width animation is row 6.2 (decision 7): a positive duration
        // between layouts that match in side and gutters tweens on the
        // frame callbacks; every other apply takes the row 3.5 end state.
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
        // The animate decision (row 6.2) against the applied layout, the
        // GTK path's `should_animate` rule. An animated apply whose columns
        // and coverage the applied layout already has animates nothing (the
        // GTK publish's rule): it stages and applies the geometry, leaving
        // a running tween alone.
        if tween::should_animate(duration_ms, &self.applied, &layout) {
            let cols_changed = layout.cols().get() != self.applied.cols().get();
            let coverage_changed = layout.coverage() != self.applied.coverage();
            if cols_changed || coverage_changed {
                return self.apply_animated(output, layout, duration_ms, fd);
            }
        }
        self.apply_snap(output, layout, fd)
    }

    /// The snap apply (row 3.5, and row 6.2's animated-apply fallbacks): a
    /// zero duration, an unanimateable layout, or an animated apply whose
    /// wide cache could not be built. The stop relay of a running tween
    /// runs after the staged mutation — the GTK publish cancels there too,
    /// so a rejected apply leaves the tween running — and lifts the sizing
    /// defer, so the deferred grid pushes here, once, through the sizing
    /// path: the push the staging could not make while the defer held.
    fn apply_snap(&mut self, output: OutputSize, layout: Layout, fd: RawFd) -> PublishOutcome {
        match self.apply_snap_against(output, layout, &mut |grid| apply_pty_size(fd, grid)) {
            Ok(geometry) => {
                if let Some(surfaces) = self
                    .session
                    .as_mut()
                    .and_then(|session| session.surfaces.as_mut())
                {
                    surfaces.apply_geometry(&geometry);
                }
                PublishOutcome::Applied
            }
            Err(outcome) => outcome,
        }
    }

    /// The animated apply (row 6.2, D7): stage with the push deferred,
    /// begin the tween from the current width, draw the grid once into the
    /// wide cache — kept from an equal retarget, drawn fresh otherwise —
    /// commit the begin frame at the from width and request the first
    /// frame callback. The frames then carry the size, margins, viewport
    /// crop, reserve zone and buffer until the tween's finish applies the
    /// final geometry and pushes the deferred grid.
    fn apply_animated(
        &mut self,
        output: OutputSize,
        layout: Layout,
        duration_ms: u32,
        fd: RawFd,
    ) -> PublishOutcome {
        let Some(staged) = self.stage_animated(output, layout, duration_ms, &mut |grid| {
            apply_pty_size(fd, grid);
        }) else {
            return PublishOutcome::InvalidLayout;
        };

        // The stop relay before the begin (Anim::begin's rule): the
        // previous tween's wide cache is taken, and an equal one survives
        // the retarget below.
        let previous = self.tween_draw.take();
        // The frame log (row 6.3): built on the state, which reads the
        // environment once and picks the session's presentation-time
        // source.
        let frame_log = self.new_frame_log();
        self.tween.begin(
            staged.from_px,
            staged.target_px,
            staged.duration_ms,
            Instant::now(),
            frame_log,
        );

        // The wide cache (D7): a retarget keeps an equal one, otherwise the
        // grid is drawn once into a fresh wide canvas. The upload and the
        // scale read are the session's; the draw itself needs the render
        // state, which row 8.1 fills — a cache that cannot be built or kept
        // snaps the apply, because a tween has nothing to present.
        let height = self.sizing.height();
        let cache = self.build_tween_cache(previous, &staged, height);
        let Some(holder) = cache else {
            return self.apply_snap(output, layout, fd);
        };
        self.tween_draw = Some(holder);
        self.commit_tween_frame(staged.from_px);
        self.request_tween_frame();
        PublishOutcome::Applied
    }

    /// Build the wide cache for a staged animated apply (row 6.2, D7): a
    /// retarget keeps an equal cache — its layout and gap state move to the
    /// new apply's — and anything else draws the live grid once into a
    /// fresh wide canvas, uploaded into a pool buffer when the compositor
    /// has a viewport for the panel. `None` is a cache the tween cannot
    /// present with: no session or surfaces left, no configure height yet,
    /// no render state on this thread (row 8.1 fills it), or a canvas or
    /// buffer the draw refused. The caller snaps the apply.
    fn build_tween_cache(
        &mut self,
        previous: Option<TweenDraw>,
        staged: &StagedTween,
        height: Option<u32>,
    ) -> Option<TweenDraw> {
        let session = self.session.as_mut()?;
        let surfaces = session.surfaces.as_mut()?;
        let scale = session.scale.resolved();
        let viewporter = session.scale.panel_viewporter();
        if let Some(mut kept) = previous.filter(|previous| previous.keeps(staged.crop, scale)) {
            kept.retarget(
                staged.layout,
                staged.gap_side,
                staged.gap_zone,
                staged.gap_tweening,
            );
            return Some(kept);
        }
        let render = self.render.as_mut()?;
        let mut fresh = TweenDraw::draw(
            staged.crop,
            height?,
            scale,
            staged.layout,
            staged.gap_side,
            staged.gap_zone,
            staged.gap_tweening,
            staged.grid_px,
            render,
        )?;
        if viewporter && !fresh.upload(surfaces.pool_mut()) {
            return None;
        }
        Some(fresh)
    }

    /// The snap apply's decision and state half, against an output the
    /// caller names and with the pushes observed through `push`
    /// (`port-to-rust` D10): stage, then the stop relay of a running tween
    /// — drop the wide cache, cancel the driver, lift the sizing defer and
    /// push the deferred grid once through the sizing path. When no tween
    /// ran, the staging's own push already landed and this one is the
    /// sizing's no-op, so either way the grid pushes exactly once.
    pub(crate) fn apply_snap_against(
        &mut self,
        output: OutputSize,
        layout: Layout,
        push: &mut dyn FnMut(Grid),
    ) -> Result<SurfaceGeometry, PublishOutcome> {
        let geometry = self.apply_against(output, layout, push)?;
        self.tween_draw = None;
        self.tween.cancel();
        self.sizing.defer_pushes(false);
        let cols = self.applied.cols();
        self.sizing.apply_columns(cols, push);
        Ok(geometry)
    }

    /// The animated apply's staging half (row 6.2): the reads
    /// `Surfaces::publish` makes before it mutates the applied layout and
    /// held gap — the current width, the strip the gap rests at, the
    /// gap-tween decision — then the sizing defer and the staging, whose
    /// `apply_columns` records the target columns without pushing. Returns
    /// everything the wide cache and the tween frames need; `None` is a
    /// rejected apply, which restores the defer a running tween holds and
    /// leaves the tween running (the GTK publish's reject-before-mutate
    /// order, overlay-expand D4).
    pub(crate) fn stage_animated(
        &mut self,
        output: OutputSize,
        layout: Layout,
        duration_ms: u32,
        push: &mut dyn FnMut(Grid),
    ) -> Option<StagedTween> {
        let applied_px = grid_width_px(self.applied.cols(), self.cell)?;
        let target_px = grid_width_px(layout.cols(), self.cell)?;
        let from_px = self.tween.current_px(applied_px);
        let crop = TweenCrop::new(layout.side(), from_px, target_px)?;
        let grid_px = grid_width_px(self.sizing.live_cols(), self.cell)?;
        // The gap the strip rests at right now, before this apply mutates
        // the held gap (exactly `Surfaces::publish`'s read): a retarget's
        // flagged gap tween moves the strip with the panel, so the decision
        // compares the target strip against the moving one.
        let gap_active = self
            .tween_draw
            .as_ref()
            .is_some_and(TweenDraw::gap_tweening)
            && self.tween.is_active();
        let (gap_side, gap_zone) = reserve_gap_parts(
            self.held.side(),
            self.held.zone(),
            gap_active,
            self.applied,
            from_px,
        );
        let gap_tweening = gap_tween_decision(gap_zone, true, layout, target_px);

        // The grid push defers to the tween's finish (D7): the sizing
        // records the target columns, and the finish derives and pushes
        // once from the latest height.
        self.sizing.defer_pushes(true);
        if self.apply_against(output, layout, push).is_err() {
            self.sizing.defer_pushes(self.tween.is_active());
            return None;
        }
        Some(StagedTween {
            from_px,
            target_px,
            crop,
            grid_px,
            gap_side,
            gap_zone,
            gap_tweening,
            layout,
            duration_ms: duration_ms.min(1000),
        })
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
        self.applied = staged.layout;
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

    /// The animated apply's staging defers the grid push (row 6.2): the
    /// staging records the target columns without deriving or pushing, and
    /// the finish pushes once through the sizing path — the new columns,
    /// the rows of the latest configure height.
    #[test]
    fn an_animated_apply_defers_the_push_and_the_finish_pushes_once() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        let mut pushed: Vec<Grid> = Vec::new();

        let staged = state
            .stage_animated(
                output(1920, 1080),
                layout(Side::Left, 120, 0, 0, 0, 12),
                200,
                &mut |grid| pushed.push(grid),
            )
            .expect("the layout fits the output");
        assert!(pushed.is_empty(), "a staged animated apply pushes nothing");
        assert_eq!(staged.from_px, 360, "the applied width at rest");
        assert_eq!(staged.target_px, 1080, "the staged target width");
        assert_eq!(staged.crop.wide_px(), 1080, "the wide width is the larger");
        assert_eq!(staged.grid_px, 360, "the live grid is the applied one");
        assert!(staged.gap_tweening, "a pushing expand tweens the gap");
        assert_eq!(staged.gap_zone, 360, "the strip the gap rests at");
        assert_eq!(staged.duration_ms, 200);

        // The finish pushes once, through the sizing path.
        state.tween_finished(1080, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.len(), 1, "exactly one push");
        assert_eq!(pushed[0].cols(), 120, "the tweened columns");
        assert_eq!(pushed[0].rows(), 1080 / 16, "the recorded height's rows");
    }

    /// A snap apply during a running tween pushes at once (row 6.2): the
    /// stop relay drops the wide cache, cancels the driver and lifts the
    /// sizing defer, so the deferred grid lands in the same apply.
    #[test]
    fn a_snap_apply_during_a_tween_pushes_at_once() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        let mut pushed: Vec<Grid> = Vec::new();
        let _ = state.stage_animated(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            200,
            &mut |grid| pushed.push(grid),
        );
        state.tween.begin(360, 1080, 200, Instant::now(), None);
        assert!(
            state.tween.is_active(),
            "the test tweens like the apply would"
        );

        // A zero-duration apply of the same layout: the relay lifts the
        // defer and the deferred grid pushes at once.
        let geometry = state
            .apply_snap_against(
                output(1920, 1080),
                layout(Side::Left, 120, 0, 0, 0, 12),
                &mut |grid| pushed.push(grid),
            )
            .expect("the layout fits the output");
        assert_eq!(pushed.len(), 1, "the snap pushes at once");
        assert_eq!(pushed[0].cols(), 120);
        assert_eq!(geometry.panel_width, Some(1080));
        assert!(!state.tween.is_active(), "the snap cancelled the tween");
        assert!(state.tween_draw.is_none(), "the wide cache dropped");
    }

    /// A snap apply whose staging the output rejects leaves a running tween
    /// alone — the relay runs after the staged mutation, the GTK publish's
    /// reject-before-mutate order (overlay-expand D4) — and pushes nothing.
    #[test]
    fn a_rejected_snap_apply_leaves_the_tween_running() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        let mut pushed: Vec<Grid> = Vec::new();
        let _ = state.stage_animated(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            200,
            &mut |grid| pushed.push(grid),
        );
        state.tween.begin(360, 1080, 200, Instant::now(), None);

        // A covering 400-column layout is 3600px wide, past the output.
        let too_wide = layout(Side::Left, 400, 0, 0, 0, 12).covering();
        assert_eq!(
            state.apply_snap_against(output(1920, 1080), too_wide, &mut |grid| pushed.push(grid)),
            Err(PublishOutcome::InvalidLayout)
        );
        assert!(state.tween.is_active(), "the tween keeps running");
        assert!(pushed.is_empty(), "a rejected apply pushes nothing");
    }

    /// A rejected animated staging restores the defer a running tween
    /// holds: the tween keeps deferring its pushes, the way the GTK
    /// publish's rejected apply leaves the tween's resize deferred.
    #[test]
    fn a_rejected_animated_staging_keeps_the_running_tweens_defer() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        let mut pushed: Vec<Grid> = Vec::new();
        let _ = state.stage_animated(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            200,
            &mut |grid| pushed.push(grid),
        );
        state.tween.begin(360, 1080, 200, Instant::now(), None);

        // A covering 400-column layout is rejected; the staging restores
        // the defer the running tween holds.
        let too_wide = layout(Side::Left, 400, 0, 0, 0, 12).covering();
        assert!(
            state
                .stage_animated(output(1920, 1080), too_wide, 200, &mut |grid| pushed
                    .push(grid))
                .is_none(),
            "the rejected staging stages nothing"
        );
        assert!(state.tween.is_active());
        assert!(pushed.is_empty(), "the defer still holds the push");

        // And a mid-tween configure pushes nothing either, while a height
        // lands for the finish to derive from.
        state.sizing.configure(1040, &mut |grid| pushed.push(grid));
        assert!(
            pushed.is_empty(),
            "the defer still holds the configure's push"
        );
        state.tween_finished(1080, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.len(), 1, "the finish pushes once, at the new height");
        assert_eq!(pushed[0].rows(), 1040 / 16);
    }

    /// A snap apply with no tween running pushes exactly once: the
    /// staging's own push lands and the relay's is the sizing's no-op.
    #[test]
    fn a_snap_apply_without_a_tween_pushes_once() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        let mut pushed: Vec<Grid> = Vec::new();
        let geometry = state
            .apply_snap_against(
                output(1920, 1080),
                layout(Side::Left, 120, 0, 0, 0, 12),
                &mut |grid| pushed.push(grid),
            )
            .expect("the layout fits the output");
        assert_eq!(pushed.len(), 1);
        assert_eq!(geometry.panel_width, Some(1080));
    }
}
