//! The present step of the panel thread's frames (replace-gtk-with-wayland
//! row 8.1, dispatch D4c): the pure decisions that turn the renderer's
//! [`FrameOutcome`] into the Wayland requests a frame commits, the frame
//! input the live frames build from the panel's state, and the executor the
//! configure path and the loop's repaint hook run.
//!
//! Two decisions live here and are recorded on their functions:
//!
//! - The canvas copy into a pool slot is always the full canvas, and the
//!   damage is only for the compositor (the copy in `draw_frame_at`). The
//!   pool rotates
//!   its slots, so the slot a hand-out serves may hold the frame before the
//!   last one; only a full copy guarantees the buffer's bytes are the
//!   canvas'. The rectangles stay the compositor's damage hint: compositors
//!   accumulate damage per buffer, and the buffer content is always
//!   current, so the hint only has to cover the changes since that buffer's
//!   last commit. Row 10.3 measures the copy against the frame budget on
//!   the stutter laptop.
//! - A present the pool refused leaves the repaint request latched
//!   ([`latch_after_service`]): the buffer releases arrive as Wayland
//!   events the source dispatches, and the next loop pass retries the
//!   frame. There is no separate release handler to hook — sctk tracks the
//!   releases inside the slot pool.
//!
//! The planner, the frame input, the draw offset, the output disposition
//! and the finish's end-state actions are pure and unit tested here
//! without a display (`port-to-rust` D10). The executor touches the
//! queue-bound pool and the surfaces, so a live session is what exercises
//! it (row 10.1); the executor runs on the panel thread, reached through
//! the dispatch state's frame and repaint hooks.

use crate::fontconfig::ThemeColours;
use crate::layout::{Accent, Side};
use crate::render::OutputScale;
use crate::render::frame_gate::FrameOutcome;
use crate::render::geom::{DeviceRect, FrameInput};

use super::buffers::{self, FractionalScale};
use super::crop::upload_wide;
use super::state::PanelState;
use super::toggle::Visibility;

/// What a frame's present step does with the renderer's outcome (dispatch
/// D4c): nothing at all when the frame drew nothing or a tween owns the
/// commits, or the buffer rectangles to damage under the attach-and-commit.
/// Constructed only by [`present_plan`] (`port-to-rust` D6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresentPlan {
    /// No commit: the surface's committed state still shows the frame.
    Nothing,
    /// Attach the drawn buffer, damage these device rectangles — one
    /// `damage_buffer` each — and commit.
    Frame(Vec<DeviceRect>),
}

/// The present plan for one renderer outcome (D4c): a `Clean` frame commits
/// nothing at all — the gate drew nothing, so the committed state is already
/// current — and a `Damage` frame attaches its buffer with one
/// `damage_buffer` per rectangle. A tween owns the panel surface's size,
/// viewport and buffer commits until it finishes (row 6.2), so while one
/// runs the live frames present nothing.
#[must_use]
pub fn present_plan(
    outcome: &FrameOutcome,
    buffer: (u32, u32),
    tween_owns_commits: bool,
) -> PresentPlan {
    if tween_owns_commits {
        return PresentPlan::Nothing;
    }
    match outcome {
        FrameOutcome::Clean => PresentPlan::Nothing,
        FrameOutcome::Damage(damage) => {
            let rects = damage
                .rects()
                .iter()
                .filter_map(|rect| clamp_to_buffer(*rect, buffer))
                .collect();
            PresentPlan::Frame(rects)
        }
    }
}

/// Clamp one damage rectangle to the buffer it is damaged in: a rectangle
/// past the buffer's edge or an extent that overflows it cannot be a
/// `damage_buffer` argument, so it shrinks to the overlap or drops.
#[must_use]
fn clamp_to_buffer(rect: DeviceRect, buffer: (u32, u32)) -> Option<DeviceRect> {
    let (Ok(width), Ok(height)) = (i32::try_from(buffer.0), i32::try_from(buffer.1)) else {
        return None;
    };
    if rect.x() >= width || rect.y() >= height {
        return None;
    }
    DeviceRect::new(
        rect.x(),
        rect.y(),
        rect.w().min(width - rect.x()),
        rect.h().min(height - rect.y()),
    )
}

/// What the loop's service step does with a repaint request (dispatch D4c):
/// while a tween owns the commits the request marks the tween's wide cache
/// stale — the next tween frame redraws it from the live terminal — and
/// outside a tween the request draws a live frame. Constructed only by
/// [`output_disposition`] (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputDisposition {
    /// The running tween's wide cache is a snapshot; mark it stale.
    MarkHolderStale,
    /// Draw a live frame of the terminal into a pool buffer and present it.
    DrawLiveFrame,
}

/// The disposition of one repaint request (D4c): the tween's wide cache is
/// the snapshot the old path redrew on terminal output (`note_terminal_output`
/// then the cache drop), so a request while a tween runs marks the holder
/// stale instead of drawing a live frame over the tween's commits; outside a
/// tween the request is a live frame.
#[must_use]
pub fn output_disposition(tween_owns_commits: bool) -> OutputDisposition {
    if tween_owns_commits {
        OutputDisposition::MarkHolderStale
    } else {
        OutputDisposition::DrawLiveFrame
    }
}

/// Whether the repaint request survives the service step (D4c): only a
/// present the pool refused latches it again — the buffer releases the
/// source dispatches wake the loop, and the next pass retries. A drawn,
/// clean or idle step consumed the request.
#[must_use]
pub fn latch_after_service(requested: bool, busy: bool) -> bool {
    requested && busy
}

/// One viewport action of the tween finish's end state (row 6.2, dispatch
/// D4c): the finish's last eased frame left a source crop and a destination
/// on the panel's viewport; the live frames that follow need the crop unset
/// and the destination at the final logical size. Constructed only by
/// [`finish_end_state`] (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishViewport {
    /// Reset the viewport source to its default — the whole buffer — with
    /// the protocol's all `-1` arguments.
    UnsetSource,
    /// Set the viewport destination to the final logical size.
    Destination(i32, i32),
}

/// What one draw step did (dispatch D4c): nothing to draw on a thread or
/// size that cannot produce a frame, a gate that drew nothing, a frame
/// attached and committed, or a present the pool refused — the caller
/// latches the repaint request again for the latter ([`latch_after_service`])
/// and leaves the panel unmapped for the next configure on a first map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceOutcome {
    /// No frame could be built: no render state, no session, or a size no
    /// buffer could hold.
    Idle,
    /// The gate drew nothing; the committed state is current.
    Clean,
    /// The frame was drawn, copied into a pool slot and committed.
    Drawn,
    /// The pool had no slot, or the attach was refused; the frame's draw
    /// happened, and the next service pass retries.
    Busy,
}

/// The viewport end state the tween's finish leaves the panel surface in
/// (D4c): without a viewporter nothing is pending — the frames presented
/// fresh buffers at their own size — and with one the source crop the last
/// eased frame set is unset and the destination moves to the final logical
/// size, so the live buffer the finish's repaint draws is shown 1:1. A
/// configure height the thread has not seen yet leaves the destination at
/// the last eased frame's; the next configure draws and sets it.
#[must_use]
pub fn finish_end_state(
    viewporter: bool,
    final_height: Option<u32>,
    target_px: i32,
) -> Vec<FinishViewport> {
    if !viewporter {
        return Vec::new();
    }
    let mut actions = vec![FinishViewport::UnsetSource];
    if let (Some(height), Some(width)) = (
        final_height.and_then(|height| i32::try_from(height).ok()),
        Some(target_px).filter(|width| *width > 0),
    ) {
        actions.push(FinishViewport::Destination(width, height));
    }
    actions
}

/// The drawing shift that keeps the grid against the docked edge (the GTK
/// path's `Surfaces::draw_offset_at`, dispatch D4c): a right-docked panel
/// shifts by the surface width minus the width of the grid it draws, so a
/// widened grid whose stale content the vt has not repainted yet stays
/// glued to the docked edge instead of parking at the edge opposite it.
/// The drawn grid is the stale pre-resize width while one is recorded —
/// the narrowest since the last terminal output — and the terminal's live
/// grid otherwise (`drawn_grid_px`'s rule). A left-docked panel shifts by
/// nothing, and so does a frame without a width yet. The shift snaps once,
/// here, to a whole device pixel at the frame's scale (`snap-grid-edges`
/// D7), the one snapped value the frame and the pointer mapping share.
#[must_use]
pub fn docked_edge_offset(
    side: Side,
    panel_width_px: i32,
    live_grid_px: i32,
    stale_grid_px: i32,
    scale: f64,
) -> f64 {
    let drawn_grid_px = if stale_grid_px > 0 {
        stale_grid_px
    } else {
        live_grid_px
    };
    let raw = match side {
        Side::Right if panel_width_px > 0 => panel_width_px - drawn_grid_px,
        _ => 0,
    };
    OutputScale::new(scale).snap_edge(f64::from(raw))
}

/// The frame input one live frame builds from the panel's state (D4c): the
/// logical configure size, the device size the resolved scale rounds it to,
/// and the docked-edge draw offset. `None` when the size has no buffer — a
/// dimension that does not survive the scaling or the protocol's `i32` —
/// which the caller skips; the next frame retries.
#[must_use]
pub fn frame_input(
    logical: (u32, u32),
    scale: FractionalScale,
    side: Side,
    live_grid_px: i32,
    stale_grid_px: i32,
    focused: bool,
    theme: ThemeColours,
    accent: Option<Accent>,
) -> Option<FrameInput> {
    let device = buffers::device_size(logical, scale)?;
    let width_px = i32::try_from(logical.0).ok()?;
    let offset = docked_edge_offset(side, width_px, live_grid_px, stale_grid_px, scale.as_f64());
    Some(FrameInput::new(
        logical.0, logical.1, device.0, device.1, offset, focused, theme, accent,
    ))
}

impl PanelState {
    /// Draw one live frame of the terminal at the logical size and present
    /// it (dispatch D4c, the executor): the frame input from the state's
    /// sizing, the renderer's gated draw, then the plan's requests — the
    /// pool slot at the device size, the full canvas copied into its bytes,
    /// the buffer attached, one `damage_buffer` per planned rectangle and
    /// the commit. The stale pre-resize width keys the docked-edge offset
    /// until the terminal produces output for the new width.
    pub fn draw_frame_at(&mut self, width: u32, height: u32) -> ServiceOutcome {
        let Some(render) = self.render.clone() else {
            return ServiceOutcome::Idle;
        };
        let Some(session) = self.session.as_ref() else {
            return ServiceOutcome::Idle;
        };
        let scale = session.scale.resolved();
        let live_grid_px =
            super::surfaces::grid_width_px(self.sizing.live_cols(), self.cell).unwrap_or(0);
        let stale_grid_px = self.stale_grid_px.get();
        let side = self.applied.side();
        // The seat's focus flag is the accent's source (D8, dispatch D4c):
        // the draw syncs the renderer's flag from the shared cell the seat
        // links hold, so a focus enter the seat latched reaches the next
        // frame — and the wide draw's cache rebuilds read the same flag.
        let focused = self
            .seat_links
            .as_ref()
            .is_some_and(|links| links.focused.get());
        let (theme, accent) = {
            let mut renderer = render.renderer.borrow_mut();
            renderer.set_focused(focused);
            (renderer.theme(), renderer.accent())
        };
        let Some(input) = frame_input(
            (width, height),
            scale,
            side,
            live_grid_px,
            stale_grid_px,
            focused,
            theme,
            accent,
        ) else {
            return ServiceOutcome::Idle;
        };
        // The offset the pointer mapping adjusts pointer x by (the seat
        // links' shared cell): the one snapped value the frame and the
        // mapping share.
        self.draw_offset.set(input.draw_offset());
        let outcome = render
            .renderer
            .borrow_mut()
            .draw(&mut render.terminal.borrow_mut(), &input);
        let (device_w, device_h) = input.device_size();
        let plan = present_plan(&outcome, (device_w, device_h), self.tween_draw.is_some());
        let PresentPlan::Frame(rects) = plan else {
            // Nothing may be committed: a clean gate, or a tween that owns
            // the commits (the configure path never reaches this under a
            // tween, and the service step does not call this there).
            return ServiceOutcome::Clean;
        };
        let renderer = render.renderer.borrow();
        let Some(canvas) = renderer.canvas() else {
            // A canvas no allocator produced: nothing could have been
            // attached that this would leave stale (see `Renderer::draw`).
            return ServiceOutcome::Idle;
        };
        let Some(session) = self.session.as_mut() else {
            return ServiceOutcome::Idle;
        };
        let Some(surfaces) = session.surfaces.as_mut() else {
            return ServiceOutcome::Idle;
        };
        let Ok((buffer, bytes)) = surfaces.pool_mut().buffer(device_w, device_h) else {
            // The pool could not provide a slot (both ping-pong buffers and
            // every bounded spare still held): the frame retries on the
            // buffer releases the source dispatches ([`latch_after_service`]).
            return ServiceOutcome::Busy;
        };
        // The full canvas, always: the handed slot may hold the frame
        // before the last one (see the module docs).
        if upload_wide(canvas, bytes).is_err() {
            return ServiceOutcome::Busy;
        }
        let surface = surfaces.panel_wl_surface();
        if buffer.attach_to(surface).is_err() {
            return ServiceOutcome::Busy;
        }
        for rect in &rects {
            surface.damage_buffer(rect.x(), rect.y(), rect.w(), rect.h());
        }
        surface.commit();
        ServiceOutcome::Drawn
    }

    /// The loop's service of a repaint request (dispatch D4c, the
    /// executor): read and clear the flag the terminal's callbacks and the
    /// seat's `queue_draw` latch, then either mark the tween's wide cache
    /// stale — a request while a tween owns the commits is terminal output
    /// the snapshot must catch up with — or draw and present a live frame
    /// at the panel's latest size. A present the pool refused latches the
    /// request again: the buffer releases arrive as Wayland events the
    /// source dispatches, and the next pass retries. A request before the
    /// first configure consumes itself: the configure draw paints the
    /// panel.
    pub fn service_repaint_request(&mut self) {
        let requested = self.take_repaint_request();
        if !requested {
            return;
        }
        // A hidden panel consumes the request without a commit (row 9.1,
        // replace-gtk-with-wayland D4): the terminal and the pty keep
        // running while the panel is hidden, and the show's configure draws
        // the current grid — a live frame committed here would remap the
        // panel behind the host's back.
        if self.visibility == Visibility::Hidden {
            return;
        }
        match output_disposition(self.tween_draw.is_some()) {
            OutputDisposition::MarkHolderStale => {
                if let Some(holder) = self.tween_draw.as_mut() {
                    holder.note_output();
                }
            }
            OutputDisposition::DrawLiveFrame => {
                let Some((width, height)) = self.panel_size else {
                    return;
                };
                let busy = self.draw_frame_at(width, height) == ServiceOutcome::Busy;
                if latch_after_service(true, busy) {
                    self.repaint.set(true);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
