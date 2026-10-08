//! The tween frames' executor: the per-frame glue that turns one
//! compositor frame callback into the commit plan's Wayland requests, the
//! frame-request bookkeeping and the finish's final geometry.

use super::super::crop::copy_crop;
use super::super::present::{FinishViewport, finish_end_state};
use super::super::sizing::Grid;
use super::super::state::PanelState;
use super::super::surfaces::SurfaceId;
use super::super::tween::FrameStep;
use super::{TweenAction, TweenDraw};

/// One `wl_surface.frame` callback for the panel surface
/// (replace-gtk-with-wayland D7): the compositor's event time steps the
/// tween driver, and the step decides the frame. An eased frame requests
/// the next callback and commits through
/// [`PanelState::request_and_commit_tween_frame`] in that order;
/// the finish relays the stop and applies the final geometry, requesting
/// none.
pub(crate) fn on_tween_frame(state: &mut PanelState, time_ms: u32) {
    match state.tween.frame(time_ms) {
        FrameStep::Idle => {}
        FrameStep::Frame(px) => state.request_and_commit_tween_frame(px),
        FrameStep::Finished(px) => {
            let mut push = state.grid_sink();
            state.tween_finished(px, &mut push);
        }
    }
}

impl PanelState {
    /// Record one frame op (the test seam for the frame order): test builds only.
    #[cfg(test)]
    fn record_frame_op(&self, op: super::super::state::FrameOp) {
        self.frame_ops.borrow_mut().push(op);
    }

    /// One tween frame's Wayland order (replace-gtk-with-wayland D7): the
    /// `wl_surface.frame`
    /// request goes first and the eased commit second. A frame request
    /// binds to the commit that follows it, so a committed frame's callback
    /// fires when the compositor next repaints after that commit — the
    /// callback that drives the next eased frame. Committed first and
    /// requested second, the request would bind to a commit no tween frame
    /// makes: no callback ever fires, and the watchdog snaps the tween to
    /// its target. The tween's begin frame and every eased frame share
    /// this one order.
    pub(crate) fn request_and_commit_tween_frame(&mut self, px: i32) {
        self.request_tween_frame();
        self.commit_tween_frame(px);
    }

    /// Commit one eased tween frame at `px` (replace-gtk-with-wayland D7):
    /// the plan
    /// [`TweenDraw::commit_plan`] decided, executed in its order against
    /// the two surfaces. A scale change since the cache was drawn
    /// invalidates the cache (D7) and the frames stop; the watchdog snaps
    /// the tween at its deadline. A frame the plan refuses is skipped — the
    /// next one or the watchdog ends the tween.
    pub(crate) fn commit_tween_frame(&mut self, px: i32) {
        #[cfg(test)]
        self.record_frame_op(super::super::state::FrameOp::CommitFrame);
        let resolved = self
            .session
            .as_ref()
            .map(|session| session.scale.resolved());
        if let (Some(resolved), Some(holder)) = (resolved, self.tween_draw.as_ref())
            && resolved != holder.scale()
        {
            self.tween_draw = None;
            return;
        }
        // Mid-tween output: the wide cache is a snapshot of the grid at the
        // tween's start, and a repaint request while the tween runs marks
        // the holder stale — the next frame, this one, redraws the cache
        // from the live terminal before presenting it. A refused redraw
        // skips the frame; the next one or the watchdog retries.
        if self.tween_draw.as_ref().is_some_and(TweenDraw::is_stale) {
            // The seat's focus flag is the accent's source (D8): the
            // redraw reads the renderer's flag, so it syncs from the
            // shared cell the seat links hold first.
            let focused = self
                .seat_links
                .as_ref()
                .is_some_and(|links| links.focused.get());
            let Some(render) = self.render.clone() else {
                return;
            };
            render.renderer.borrow_mut().set_focused(focused);
            let Some(session) = self.session.as_mut() else {
                return;
            };
            let Some(surfaces) = session.surfaces.as_mut() else {
                return;
            };
            let Some(holder) = self.tween_draw.as_mut() else {
                return;
            };
            if !holder.redraw(&render, surfaces.pool_mut()) {
                return;
            }
        }
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(surfaces) = session.surfaces.as_mut() else {
            return;
        };
        let Some(holder) = self.tween_draw.as_ref() else {
            return;
        };
        let viewporter = session.scale.panel_viewporter();
        let Some(plan) = holder.commit_plan(px, viewporter) else {
            return;
        };
        for action in plan.actions() {
            match *action {
                TweenAction::PanelSize(width) => surfaces.tween_panel_size(width),
                TweenAction::PanelMargins(margins) => surfaces.tween_panel_margins(margins),
                TweenAction::ViewportSource(rect) => session.scale.set_source(
                    SurfaceId::Panel,
                    rect.x(),
                    rect.y(),
                    rect.width(),
                    rect.height(),
                ),
                TweenAction::ViewportDestination(width, height) => {
                    session
                        .scale
                        .set_destination(SurfaceId::Panel, width, height);
                }
                TweenAction::Reserve { side, zone } => surfaces.tween_reserve(side, zone),
                TweenAction::AttachWide => {
                    if let Some(buffer) = holder.buffer() {
                        surfaces.tween_attach(buffer);
                    }
                }
                TweenAction::CopyFresh => {
                    // A fresh pool buffer at the crop size, the crop copied
                    // into it (D7's no-viewporter fallback): a memory copy,
                    // no glyph work. A refused buffer or copy skips the
                    // frame; the next one or the watchdog retries.
                    let source = plan.frame().source();
                    let (Ok(width), Ok(height)) = (
                        u32::try_from(source.width()),
                        u32::try_from(source.height()),
                    ) else {
                        return;
                    };
                    let Ok((buffer, bytes)) = surfaces.tween_fresh_buffer(width, height) else {
                        return;
                    };
                    let Ok(()) = copy_crop(holder.canvas(), plan.frame(), bytes) else {
                        return;
                    };
                    surfaces.tween_attach(&buffer);
                }
                TweenAction::Present => {
                    let (width, height) = plan.damage();
                    surfaces.tween_present(width, height);
                }
            }
        }
    }

    /// Request the panel surface's next `wl_surface.frame` callback (row
    /// 6.2): the tween's frames are driven by these callbacks, so every
    /// committed frame requests the next one — issued before the commit it
    /// belongs to, by [`Self::request_and_commit_tween_frame`]
    /// (replace-gtk-with-wayland D7).
    /// The queue handle lives on the session, so the apply's begin frame
    /// can request one too. The same committed frame requests its
    /// presentation feedback (replace-gtk-with-wayland D7), tagged with the
    /// tween's generation
    /// so a late `presented` from a tween that already stopped cannot reach
    /// the next tween's log; without the presentation-time global the
    /// request is skipped and the frame log rides on the callbacks' times
    /// alone.
    pub(crate) fn request_tween_frame(&self) {
        #[cfg(test)]
        self.record_frame_op(super::super::state::FrameOp::RequestFrame);
        let Some(session) = &self.session else {
            return;
        };
        let Some(surfaces) = session.surfaces.as_ref() else {
            return;
        };
        surfaces.request_frame(&session.qh);
        // The presentation feedback feeds only a frame log
        // (replace-gtk-with-wayland D7): with
        // no log the generation filter would drop every sample, so no
        // request is made and the log rides on the callbacks' times alone.
        if self.tween.has_log() {
            session.presentation.feedback(
                surfaces.panel_wl_surface(),
                &session.qh,
                self.tween.generation(),
            );
        }
    }

    /// The tween's finish action (replace-gtk-with-wayland D7): the stop
    /// relay first — drop the wide cache, lift the sizing defer and push
    /// the deferred grid once through the sizing path, the new columns
    /// derived from the latest configure height — then the final geometry,
    /// the same path a plain apply writes. The viewport end state follows:
    /// the last eased frame left a source crop and a destination on the
    /// panel's viewport, and the live buffer the finish's repaint draws
    /// needs the crop unset and the destination at the final logical size,
    /// so the surface no longer shows the crop. The finish latches the
    /// repaint request, which the draw step turns into that live frame at
    /// the final size. The push sink is a parameter so the display-free
    /// tests observe it (`port-to-rust` D10); the production callers sink
    /// the pty winsize. The headless core has no session, so the geometry
    /// write is exercised only on niri; the relay is
    /// display-free and tested in the apply module.
    pub(crate) fn tween_finished(&mut self, target_px: i32, push: &mut dyn FnMut(Grid)) {
        // The final frame commits at the target before the cache drops
        // (replace-gtk-with-wayland D7): the viewport's source and
        // destination are persistent
        // state, so the last eased crop would pin the surface against the
        // finish's `set_size` — a blank panel.
        self.commit_tween_frame(target_px);
        self.tween_draw = None;
        self.sizing.defer_pushes(false);
        let cols = self.applied.cols();
        self.sizing.apply_columns(cols, push);
        let geometry = super::super::apply::SurfaceGeometry {
            panel_side: self.applied.side(),
            panel_margins: super::super::surfaces::panel_margins(self.applied),
            panel_width: Some(target_px),
            reserve_side: self.held.side(),
            reserve_zone: self.held.zone(),
        };
        self.write_geometry(geometry);
        let height = self.sizing.height();
        let viewporter = self
            .session
            .as_ref()
            .is_some_and(|session| session.scale.panel_viewporter());
        for action in finish_end_state(viewporter, height, target_px) {
            if let Some(session) = self.session.as_mut() {
                match action {
                    FinishViewport::UnsetSource => {
                        // The protocol's all `-1` arguments reset the source
                        // to its default, the whole buffer.
                        session.scale.set_source(SurfaceId::Panel, -1, -1, -1, -1);
                    }
                    FinishViewport::Destination(width, height) => {
                        session
                            .scale
                            .set_destination(SurfaceId::Panel, width, height);
                    }
                }
            }
        }
        // The frame input the live draw builds from: the final width and
        // the latest configure height.
        if let (Some(height), Ok(width)) = (height, u32::try_from(target_px)) {
            self.panel_size = Some((width, height));
        }
        // The live grid must replace the crop on screen: the draw step
        // turns this request into the frame at the final size.
        self.repaint.set(true);
    }
}
