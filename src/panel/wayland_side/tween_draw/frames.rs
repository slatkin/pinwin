//! The tween frames' executor (split from the `tween_draw` module in row
//! 8.1's dispatch D1): the per-frame glue that turns one compositor frame
//! callback into the commit plan's Wayland requests, the frame-request
//! bookkeeping and the finish's final geometry.

use super::super::crop::copy_crop;
use super::super::sizing::Grid;
use super::super::state::PanelState;
use super::super::surfaces::SurfaceId;
use super::super::tween::FrameStep;
use super::TweenAction;

/// One `wl_surface.frame` callback for the panel surface (row 6.2): the
/// compositor's event time steps the tween driver, and the step decides the
/// frame. An eased frame commits through [`commit_tween_frame`] and
/// requests the next callback; the finish relays the stop and applies the
/// final geometry, requesting none.
pub(crate) fn on_tween_frame(state: &mut PanelState, time_ms: u32) {
    match state.tween.frame(time_ms) {
        FrameStep::Idle => {}
        FrameStep::Frame(px) => {
            state.commit_tween_frame(px);
            state.request_tween_frame();
        }
        FrameStep::Finished(px) => {
            let fd = state.startup.fd;
            state.tween_finished(px, &mut |grid| {
                super::super::state::apply_pty_size(fd, grid);
            });
        }
    }
}

impl PanelState {
    /// Commit one eased tween frame at `px` (row 6.2): the plan
    /// [`TweenDraw::commit_plan`] decided, executed in its order against
    /// the two surfaces. A scale change since the cache was drawn
    /// invalidates the cache (D7) and the frames stop; the watchdog snaps
    /// the tween at its deadline. A frame the plan refuses is skipped — the
    /// next one or the watchdog ends the tween.
    pub(crate) fn commit_tween_frame(&mut self, px: i32) {
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
    /// committed frame requests the next one. The queue handle lives on the
    /// session, so the apply's begin frame can request one too. The same
    /// committed frame requests its presentation feedback (row 6.3), tagged
    /// with the tween's generation so a late `presented` from a tween that
    /// already stopped cannot reach the next tween's log; without the
    /// presentation-time global the request is skipped and the frame log
    /// rides on the callbacks' times alone.
    pub(crate) fn request_tween_frame(&self) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(surfaces) = session.surfaces.as_ref() else {
            return;
        };
        surfaces.request_frame(&session.qh);
        // The presentation feedback feeds only a frame log (row 6.3): with
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

    /// The tween's finish action (row 6.1, wired in row 6.2): the stop
    /// relay first — drop the wide cache, lift the sizing defer and push
    /// the deferred grid once through the sizing path, the new columns
    /// derived from the latest configure height (the GTK path's
    /// `on_tween_stopped`) — then the final geometry, the same path a plain
    /// apply writes. The push sink is a parameter so the display-free tests
    /// observe it (`port-to-rust` D10); the production callers sink the pty
    /// winsize. The pty read's tween flag has no mirror here yet: the
    /// pty source joins this thread in row 8.1 and reads the driver's own
    /// state, which the finish has already cleared. The headless core has
    /// no session, so the geometry write is exercised only on niri (row
    /// 10.1); the relay is display-free and tested in the apply module.
    pub(crate) fn tween_finished(&mut self, target_px: i32, push: &mut dyn FnMut(Grid)) {
        // The final frame commits at the target before the cache drops
        // (row 6.1): the viewport's source and destination are persistent
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
    }
}
