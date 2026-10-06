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

use std::time::Instant;

use crate::layout::{CellSize, Layout, OutputSize, Side};
use crate::surfaces::PublishOutcome;
use crate::surfaces::gap::{HeldGap, gap_tween_decision, reserve_gap_parts, staged_publish};

use super::crop::TweenCrop;
use super::sizing::Grid;
use super::state::PanelState;
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
        // The animate decision (row 6.2) against the applied layout, the
        // GTK path's `should_animate` rule. An animated apply whose columns
        // and coverage the applied layout already has animates nothing (the
        // GTK publish's rule): it stages the end state and applies the
        // geometry, leaving a running tween — already heading for these
        // columns — alone, its deferred push waiting for the finish. Only
        // the snap path runs the stop relay.
        if tween::should_animate(duration_ms, &self.applied, &layout) {
            if Self::animates_width(&self.applied, &layout) {
                return self.apply_animated(output, layout, duration_ms);
            }
            return self.apply_quiet(output, layout);
        }
        self.apply_snap(output, layout)
    }

    /// Whether an animated apply moves the width (row 6.2, the GTK
    /// publish's second rule): only a change in the column count or the
    /// push/cover choice does. An apply the animate rule accepts but that
    /// changes neither restages quietly.
    fn animates_width(applied: &Layout, layout: &Layout) -> bool {
        layout.cols().get() != applied.cols().get() || layout.coverage() != applied.coverage()
    }

    /// The geometry write the snap and quiet applies share: the two
    /// surfaces take the staged apply's geometry while the session still
    /// holds them.
    pub(super) fn write_geometry(&mut self, geometry: SurfaceGeometry) {
        if let Some(surfaces) = self
            .session
            .as_mut()
            .and_then(|session| session.surfaces.as_mut())
        {
            surfaces.apply_geometry(&geometry);
        }
    }

    /// The snap apply (row 3.5, and row 6.2's animated-apply fallbacks): a
    /// zero duration, an unanimateable layout, or an animated apply whose
    /// wide cache could not be built. The stop relay of a running tween
    /// runs after the staged mutation — the GTK publish cancels there too,
    /// so a rejected apply leaves the tween running — and lifts the sizing
    /// defer, so the deferred grid pushes here, once, through the sizing
    /// path: the push the staging could not make while the defer held.
    fn apply_snap(&mut self, output: OutputSize, layout: Layout) -> PublishOutcome {
        let mut push = self.grid_sink();
        match self.apply_snap_against(output, layout, &mut push) {
            Ok(geometry) => {
                self.write_geometry(geometry);
                PublishOutcome::Applied
            }
            Err(outcome) => outcome,
        }
    }

    /// The quiet apply (row 6.2): an animated apply whose columns and
    /// coverage the applied layout already has. It stages the end state and
    /// applies the geometry — the top and bottom gutters may still differ —
    /// and leaves a running tween alone: the tween already heads for these
    /// columns (the GTK publish's rule), so the stop relay does not run,
    /// the wide cache stays and the deferred push waits for the finish.
    fn apply_quiet(&mut self, output: OutputSize, layout: Layout) -> PublishOutcome {
        let mut push = self.grid_sink();
        match self.apply_against(output, layout, &mut push) {
            Ok(geometry) => {
                self.write_geometry(geometry);
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
    ) -> PublishOutcome {
        let mut push = self.grid_sink();
        let Some(staged) = self.stage_animated(output, layout, duration_ms, &mut push) else {
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
            return self.apply_snap(output, layout);
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
mod tests;
