//! The width tween's holder and its frame glue (replace-gtk-with-wayland
//! D7): the state one running tween keeps on the
//! panel thread — the crop plan, the wide canvas drawn once at the tween's
//! start, the pool buffer it was uploaded into, the configure height and
//! scale they were decided at, and the gap state the frames commit the
//! reserve with — plus the two pieces that turn it into Wayland requests:
//! the pure per-frame commit plan and the thin executor
//! `PanelState::commit_tween_frame` runs it.
//!
//! The decisions are pure and unit tested here without a compositor
//! (`port-to-rust` D10): [`TweenDraw::commit_plan`] maps an eased width to
//! the ordered commit actions — the layer size, the margins, the viewport
//! source and destination, the reserve's anchor and zone, the buffer attach
//! (the cached wide buffer with a viewport, a fresh copy of the crop
//! without one) and the damage-and-commit — and [`TweenDraw::keeps`]
//! decides whether a retarget keeps the cache or redraws it. The executor
//! and the render seam ([`TweenRender`], the terminal and the renderer the
//! thread assembles at its start) are the compile-only half: they run only
//! in a live session.
//!
//! The gap state the frames commit is the read `Surfaces::publish` makes
//! before it mutates the held gap (overlay-expand D3): the side and zone
//! the strip rests at, and the apply's gap-tween flag. While the flag is
//! set, [`crate::surfaces::gap::reserve_gap_parts`] moves the strip with
//! the panel and the held values are unused; while it is clear, the held
//! values are the strip the apply staged — the decision
//! ([`crate::surfaces::gap::gap_tween_decision`]) only clears the flag when
//! the target strip equals the strip the gap rests at, so the frames and
//! the finish's final geometry agree in every case.
//!
//! Panics never cross back into calloop or the compositor (D5): the frame
//! handler in [`super::state`] runs this module's glue under the shared
//! guard, and the decisions here are plain field operations that cannot
//! panic.

use std::cell::RefCell;
use std::rc::Rc;

use smithay_client_toolkit::shm::slot::Buffer;

use crate::layout::{Layout, Side};
use crate::render::canvas::Canvas;

use super::buffers::{BufferPool, FractionalScale};
use super::crop::{CropFrame, CropRect, TweenCrop, frame_geometry, upload_wide};
use super::renderer::Renderer;

mod frames;

pub(crate) use frames::on_tween_frame;

#[cfg(test)]
mod tests;

/// The render state the tween's wide draw reads (replace-gtk-with-wayland
/// D7): the
/// terminal whose grid is drawn from — the one render piece the thread
/// owns outside the renderer — and a handle to the thread's one
/// [`Renderer`], which owns the text pass, the image pass, the metrics,
/// the theme, the accent and the focus flag the wide draw reads. There is
/// exactly one text pass and one image pass per thread: the renderer is
/// their only owner, and the wide draw borrows them through the handle
/// for the one draw of a fresh tween's cache (the borrow cannot contend:
/// the renderer is borrowed only here and by the live frames outside a
/// tween, sequentially on the one thread). The thread fills the bundle at
/// its start — the terminal it owns and the renderer over the font
/// it measured — so an animated apply draws its wide cache instead of
/// snapping. The fields are `pub(crate)`: the thread's start assembles the
/// bundle, and the D6 field-privacy rule guards the crate's public API,
/// not this internal one.
#[derive(Clone)]
pub(crate) struct TweenRender {
    /// The terminal whose grid the wide draw paints. Shared, because the
    /// seat and the pty source that join this thread hold the same
    /// terminal.
    pub(crate) terminal: Rc<RefCell<crate::term::Terminal>>,
    /// The thread's one renderer, borrowed for the wide draw.
    pub(crate) renderer: Rc<RefCell<Renderer>>,
}

/// One commit action of a tween frame, in the order the frame commits it
/// (replace-gtk-with-wayland D7): the layer size, the margins, the viewport
/// source and
/// destination, the reserve's anchor and zone, the buffer attach, then the
/// damage-and-commit. The executor maps each onto its Wayland request; the
/// plan is pure and tested here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TweenAction {
    /// The panel's `set_size` at the eased width; the height stays the
    /// compositor's, between the anchors.
    PanelSize(i32),
    /// The panel's `set_margin(top, right, bottom, left)`.
    PanelMargins((i32, i32, i32, i32)),
    /// The viewport's `set_source`, the docked-edge crop in buffer (device)
    /// pixels. Only with a viewport.
    ViewportSource(CropRect),
    /// The viewport's `set_destination`, the eased logical size. Only with
    /// a viewport.
    ViewportDestination(i32, i32),
    /// The reserve's anchor and exclusive zone, from the held-gap rule.
    Reserve { side: Side, zone: i32 },
    /// Attach the cached wide buffer. Only with a viewport.
    AttachWide,
    /// Without a viewport: allocate a fresh pool buffer at the crop size,
    /// copy the crop into it and attach it — a memory copy, no glyph work.
    CopyFresh,
    /// Damage the presented buffer's rectangle and commit the frame.
    Present,
}

/// One tween frame's plan: the ordered commit actions, the crop rectangle
/// the frame presents — which the copy reads — and the buffer rectangle to
/// damage. Constructed only by [`TweenDraw::commit_plan`]
/// (`port-to-rust` D6).
#[derive(Debug)]
pub(crate) struct TweenFramePlan {
    actions: Vec<TweenAction>,
    frame: CropFrame,
    damage: (u32, u32),
}

impl TweenFramePlan {
    /// The ordered commit actions, first to last.
    #[must_use]
    pub(crate) fn actions(&self) -> &[TweenAction] {
        &self.actions
    }

    /// The crop rectangle this frame presents.
    #[must_use]
    pub(crate) fn frame(&self) -> &CropFrame {
        &self.frame
    }

    /// The buffer rectangle to damage: the wide buffer's, or the fresh
    /// crop's without a viewport.
    #[must_use]
    pub(crate) fn damage(&self) -> (u32, u32) {
        self.damage
    }
}

/// The width tween's state on the panel thread (replace-gtk-with-wayland
/// D7): the crop plan,
/// the wide canvas and its uploaded buffer, the height and scale they were
/// decided at, and the gap state the frames commit the reserve with. Lives
/// on [`PanelState`](super::state::PanelState) for exactly one running
/// tween and is dropped when it stops, so a stopped tween cannot leave a
/// stale cache behind.
pub(crate) struct TweenDraw {
    crop: TweenCrop,
    /// The wide canvas the grid was drawn into once at the tween's start
    /// (D7): the theme background fills it and the grid sits against the
    /// docked edge. The no-viewporter fallback copies its crops per frame.
    canvas: Canvas,
    /// The pool buffer the canvas was uploaded into, re-attached every
    /// frame; `Some` only when the compositor has a viewport for the panel
    /// surface. Without one, every frame copies its crop into a fresh
    /// buffer instead and this stays `None`.
    buffer: Option<Buffer>,
    /// The configure height the crop was decided at, in logical pixels: the
    /// frames keep committing it, so a mid-tween configure's new height
    /// waits for the finish and the next configure.
    height: u32,
    /// The scale the crop geometry and the canvas were decided at; a scale
    /// change mid-tween invalidates the cache and the watchdog snaps.
    scale: FractionalScale,
    /// The tweening layout (the staged target): the frames' margins and the
    /// gap rule's layout read it. A tween only runs between layouts that
    /// match in side and gutters, so it is the applied layout's twin.
    layout: Layout,
    /// The gap state the frames commit the reserve with (overlay-expand
    /// D3): the side and zone the strip rested at before the apply mutated
    /// the held gap, and the apply's gap-tween flag.
    gap_side: Side,
    gap_zone: i32,
    gap_tweening: bool,
    /// The live grid's pixel width the cache was drawn against: the redraw
    /// keys the docked-edge offset against the same grid.
    grid_px: i32,
    /// Whether terminal output has made the cache stale since it was drawn:
    /// the next frame this holder presents redraws it from the
    /// live terminal first.
    stale: bool,
}

impl TweenDraw {
    /// Draw a fresh tween's wide cache (D7): the live grid — `grid_px`
    /// wide, the columns the tween defers the resize of — drawn once into
    /// a canvas as wide as the larger of the start and end widths, against
    /// the docked edge. `None` when the draw refuses: a zero or oversized
    /// canvas, or a draw offset the painter cannot place. The pool upload
    /// ([`TweenDraw::upload`]) follows where the compositor has a viewport;
    /// the pool is queue-bound, so this constructor itself stays
    /// display-free.
    #[must_use]
    pub(crate) fn draw(
        crop: TweenCrop,
        height: u32,
        scale: FractionalScale,
        layout: Layout,
        gap_side: Side,
        gap_zone: i32,
        gap_tweening: bool,
        grid_px: i32,
        render: &mut TweenRender,
    ) -> Option<Self> {
        let draw = crop.wide_draw(height, grid_px, scale)?;
        // The renderer is the bundle's other half: its passes, metrics,
        // theme, accent and focus flag are what the wide draw reads. The
        // borrow cannot contend — see [`TweenRender`].
        let mut renderer = render.renderer.borrow_mut();
        let canvas = renderer.paint_wide(draw, &mut render.terminal.borrow_mut())?;
        Some(TweenDraw {
            crop,
            canvas,
            buffer: None,
            height,
            scale,
            layout,
            gap_side,
            gap_zone,
            gap_tweening,
            grid_px,
            stale: false,
        })
    }

    /// Upload the cached canvas into a pool buffer at the canvas' device
    /// size — the viewporter path's one upload at the tween's start (D7).
    /// `false` is a refused buffer or a refused upload: the caller snaps,
    /// because a viewporter frame without the wide buffer has nothing to
    /// attach.
    pub(crate) fn upload(&mut self, pool: &mut BufferPool) -> bool {
        let (width, height) = self.canvas.size();
        let Ok((buffer, bytes)) = pool.buffer(width, height) else {
            return false;
        };
        if upload_wide(&self.canvas, bytes).is_err() {
            return false;
        }
        self.buffer = Some(buffer);
        true
    }

    /// Whether this holder's wide cache can serve a retarget (D7): the same
    /// wide width at the same scale drew the same canvas — the live grid
    /// and the height it was drawn at cannot have changed while the tween
    /// ran, because the grid push is deferred and the height waits for the
    /// finish. An equal cache is kept; a different one is rebuilt and
    /// redrawn.
    #[must_use]
    pub(crate) fn keeps(&self, crop: TweenCrop, scale: FractionalScale) -> bool {
        self.crop == crop && self.scale == scale
    }

    /// Retarget the holder onto a new tween whose wide cache it keeps (D7):
    /// the layout the frames commit the margins and the gap rule with moves
    /// to the new target, and the gap state moves to the new apply's
    /// pre-mutation read. The canvas, its buffer, the crop, the height and
    /// the scale stay.
    pub(crate) fn retarget(
        &mut self,
        layout: Layout,
        gap_side: Side,
        gap_zone: i32,
        gap_tweening: bool,
    ) {
        self.layout = layout;
        self.gap_side = gap_side;
        self.gap_zone = gap_zone;
        self.gap_tweening = gap_tweening;
    }

    /// The scale the crop geometry and the canvas were decided at.
    #[must_use]
    pub(crate) fn scale(&self) -> FractionalScale {
        self.scale
    }

    /// The apply's gap-tween flag, the read a retarget's gap decision
    /// combines with the running tween.
    #[must_use]
    pub(crate) fn gap_tweening(&self) -> bool {
        self.gap_tweening
    }

    /// The cached canvas, for the no-viewporter fallback's copies.
    #[must_use]
    pub(crate) fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    /// The uploaded wide buffer, for the executor's per-frame attach.
    #[must_use]
    pub(crate) fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    /// Mark the cache stale: terminal output arrived while the
    /// tween runs, and the next frame this holder presents redraws the
    /// cache from the live terminal instead of presenting the snapshot.
    pub(crate) fn note_output(&mut self) {
        self.stale = true;
    }

    /// Whether the cache is stale ([`Self::note_output`] and not yet
    /// redrawn).
    #[must_use]
    pub(crate) fn is_stale(&self) -> bool {
        self.stale
    }

    /// Redraw the cache from the live terminal: the same crop,
    /// height, scale and grid width the first draw used, through the
    /// renderer's passes into a fresh wide canvas, re-uploaded into the
    /// pool buffer when the compositor has a viewport. `false` is a draw
    /// or upload that was refused: the previous canvas and buffer stay in
    /// place and the frame is skipped, so the next one retries.
    pub(crate) fn redraw(&mut self, render: &TweenRender, pool: &mut BufferPool) -> bool {
        let Some(draw) = self.crop.wide_draw(self.height, self.grid_px, self.scale) else {
            return false;
        };
        let canvas = {
            let mut renderer = render.renderer.borrow_mut();
            renderer.paint_wide(draw, &mut render.terminal.borrow_mut())
        };
        let Some(canvas) = canvas else {
            return false;
        };
        let previous = std::mem::replace(&mut self.canvas, canvas);
        if self.buffer.is_some() && !self.upload(pool) {
            self.canvas = previous;
            return false;
        }
        self.stale = false;
        true
    }

    /// One tween frame's commit plan at the eased width `px`
    /// (replace-gtk-with-wayland D7): the
    /// layer size, the margins, the viewport source and destination (only
    /// with a viewport), the reserve's anchor and zone, the buffer attach
    /// (the cached wide buffer with a viewport, a fresh copy of the crop
    /// without one), then the damage-and-commit. `None` on a frame nothing
    /// can present — a non-positive or oversized eased width, a zero
    /// height — which the caller skips; the next one or the watchdog ends
    /// the tween.
    #[must_use]
    pub(crate) fn commit_plan(&self, px: i32, viewporter: bool) -> Option<TweenFramePlan> {
        let frame = self.crop.frame(px, self.height, self.scale)?;
        let geometry = frame_geometry(
            self.layout,
            self.gap_side,
            self.gap_zone,
            self.gap_tweening,
            px,
        );
        let mut actions = Vec::with_capacity(7);
        actions.push(TweenAction::PanelSize(frame.layer_width()));
        actions.push(TweenAction::PanelMargins(geometry.margins()));
        if viewporter {
            actions.push(TweenAction::ViewportSource(frame.source()));
            let (width, height) = frame.destination();
            actions.push(TweenAction::ViewportDestination(width, height));
        }
        actions.push(TweenAction::Reserve {
            side: geometry.reserve_side(),
            zone: geometry.reserve_zone(),
        });
        actions.push(if viewporter {
            TweenAction::AttachWide
        } else {
            TweenAction::CopyFresh
        });
        actions.push(TweenAction::Present);
        let height_dev = u32::try_from(frame.source().height()).ok()?;
        let damage = if viewporter {
            (frame.wide_width_dev(), height_dev)
        } else {
            (u32::try_from(frame.source().width()).ok()?, height_dev)
        };
        Some(TweenFramePlan {
            actions,
            frame,
            damage,
        })
    }
}
