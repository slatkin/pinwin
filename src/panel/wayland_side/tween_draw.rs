//! The width tween's holder and its frame glue (replace-gtk-with-wayland
//! row 6.2, design decision 7): the state one running tween keeps on the
//! panel thread — the crop plan, the wide canvas drawn once at the tween's
//! start, the pool buffer it was uploaded into, the configure height and
//! scale they were decided at, and the gap state the frames commit the
//! reserve with — plus the two pieces that turn it into Wayland requests:
//! the pure per-frame commit plan and the thin executor
//! [`commit_tween_frame`] runs it.
//!
//! The decisions are pure and unit tested here without a compositor
//! (`port-to-rust` D10): [`TweenDraw::commit_plan`] maps an eased width to
//! the ordered commit actions — the layer size, the margins, the viewport
//! source and destination, the reserve's anchor and zone, the buffer attach
//! (the cached wide buffer with a viewport, a fresh copy of the crop
//! without one) and the damage-and-commit — and [`TweenDraw::keeps`]
//! decides whether a retarget keeps the cache or redraws it. The executor
//! and the render seam ([`TweenRender`], which row 8.1 fills with the
//! terminal and the painter passes) are the compile-only half: they run
//! only in a live session, and row 10.1 exercises them on niri.
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

use crate::fontconfig::ThemeColours;
use crate::layout::{Accent, Layout, Side};
use crate::render::canvas::Canvas;
use crate::render::geom::PainterMetrics;
use crate::render::image_pass::ImagePass;
use crate::render::text_pass::TextPass;

use super::buffers::{BufferPool, FractionalScale};
use super::crop::{
    CropFrame, CropRect, TweenCrop, copy_crop, draw_wide, frame_geometry, upload_wide,
};
use super::sizing::Grid;
use super::state::PanelState;
use super::surfaces::SurfaceId;
use super::tween::FrameStep;

/// The render state the tween's wide draw reads (row 6.2, D7): the terminal
/// the grid is drawn from and the painter passes it draws with, plus the
/// metrics, theme, accent and focus flag the frame carries. Row 8.1 moves
/// the terminal and the painter onto this thread and fills this; until then
/// it is `None` on the panel state and an animated apply snaps — a tween
/// without a drawn wide buffer has nothing to present, and nothing runs
/// this thread before row 8.1 switches `Panel::start` over. The fields are
/// `pub(crate)`: the bundle is row 8.1's to assemble, and the D6
/// field-privacy rule guards the crate's public API, not this internal one.
pub(crate) struct TweenRender {
    /// The terminal whose grid the wide draw paints. Shared, because the
    /// seat and the pty source that join this thread in row 8.1 hold the
    /// same terminal.
    pub(crate) terminal: Rc<RefCell<crate::term::Terminal>>,
    pub(crate) text: TextPass,
    pub(crate) images: ImagePass,
    /// One `PainterMetrics` serves the wide draw and every later frame
    /// draw: the cell pitch and ascent measured from the font (row 4.6) at
    /// the output scale.
    pub(crate) metrics: PainterMetrics,
    pub(crate) theme: ThemeColours,
    pub(crate) accent: Option<Accent>,
    /// Whether the panel holds keyboard focus; the wide draw paints the
    /// focus accent from it. The seat wiring row 8.1 assembles updates it
    /// on keyboard enter and leave.
    pub(crate) focused: bool,
}

/// One commit action of a tween frame, in the order the frame commits it
/// (row 6.2): the layer size, the margins, the viewport source and
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

/// The width tween's state on the panel thread (row 6.2): the crop plan,
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
    /// waits for the finish and the next configure — the same staleness the
    /// GTK path had.
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
        let canvas = draw_wide(
            draw,
            &render.metrics,
            render.focused,
            render.theme,
            render.accent,
            &mut render.terminal.borrow_mut(),
            &mut render.text,
            &mut render.images,
        )?;
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

    /// One tween frame's commit plan at the eased width `px` (row 6.2): the
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
            state.tween_finished(px, &mut |grid| super::state::apply_pty_size(fd, grid));
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
    /// session, so the apply's begin frame can request one too.
    pub(crate) fn request_tween_frame(&self) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(surfaces) = session.surfaces.as_ref() else {
            return;
        };
        surfaces.request_frame(&session.qh);
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
        self.tween_draw = None;
        self.sizing.defer_pushes(false);
        let cols = self.applied.cols();
        self.sizing.apply_columns(cols, push);
        let geometry = super::apply::SurfaceGeometry {
            panel_side: self.applied.side(),
            panel_margins: super::surfaces::panel_margins(self.applied),
            panel_width: Some(target_px),
            reserve_side: self.held.side(),
            reserve_zone: self.held.zone(),
        };
        if let Some(surfaces) = self
            .session
            .as_mut()
            .and_then(|session| session.surfaces.as_mut())
        {
            surfaces.apply_geometry(&geometry);
        }
    }
}

#[cfg(test)]
mod tests {
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
        }
    }

    /// The commit plan orders the actions the frame commits (row 6.2): the
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
    /// render state and plans its begin frame — the seam row 8.1 fills,
    /// driven here with the test rig's painter pieces (font-gated, like the
    /// crop module's draw tests).
    #[test]
    fn a_fresh_holder_draws_from_the_render_state() {
        use crate::render::text_pass::test_support::{self, Rig};

        let Some(rig) = Rig::new() else { return };
        let mut terminal = rig.terminal(rig.cell_h);
        terminal.push_pty_data(test_support::HIDE_CURSOR);
        let metrics = rig.metrics(1.5, rig.cell_h);
        let mut render = TweenRender {
            terminal: Rc::new(RefCell::new(terminal)),
            text: rig.test.pass,
            images: ImagePass::new(),
            metrics,
            theme: test_support::THEME,
            accent: None,
            focused: false,
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
}
