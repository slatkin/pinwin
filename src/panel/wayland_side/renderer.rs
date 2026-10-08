//! The panel thread's renderer (row 8.1, `replace-gtk-with-wayland` D5):
//! the one owner of everything a frame draws with — one
//! [`crate::render::text_pass::TextPass`], one
//! [`crate::render::image_pass::ImagePass`], one
//! [`crate::render::frame_gate::FrameGate`] and a persistent
//! [`crate::render::canvas::Canvas`] at the
//! current frame's device size — plus the scale, theme, accent and focus
//! state the frames and the tween's wide draw read. There is exactly one
//! renderer per panel thread, exactly as there is one text pass and one
//! image pass; the tween path borrows it through the
//! `TweenRender` bundle (`super::tween_draw`) and the live
//! frames draw through [`Renderer::draw`].
//!
//! The renderer holds no Wayland object and no display: it is testable
//! like the painter (`replace-gtk-with-wayland` D10), with the terminal
//! and the frame as parameters. The pool buffers, the present step and
//! the frame callback wiring are the later row 8.1 dispatches; this
//! module is `pub` so that dispatch can switch the panel to it (a
//! `pub(crate)` entry with no caller is dead code under `-D warnings`,
//! and no lint suppression is permitted).
//!
//! Panics never cross back into calloop or the compositor (D5): the
//! renderer's bodies are plain field operations and painter calls whose
//! failure paths return rather than panic, and the thread runs them
//! under the shared guard.

mod font_setup;
#[cfg(test)]
mod tests;

pub use font_setup::{FontSetup, FontSetupError};

use crate::fontconfig::ThemeColours;
use crate::layout::Accent;
#[cfg(test)]
use crate::layout::CellSize;
use crate::render::canvas::Canvas;
use crate::render::cell_metrics::CellMetrics;
use crate::render::frame_gate::{FrameGate, FrameOutcome, paint_frame_gated};
use crate::render::geom::{FrameInput, PainterMetrics};
use crate::render::image_pass::ImagePass;
use crate::render::text_pass::TextPass;
use crate::term::Terminal;

use super::crop::WideDraw;

/// The panel thread's renderer (D5): the draw state one panel owns across
/// its frames. Built from the [`FontSetup`] the thread measured at start,
/// the output scale, the Ghostty theme and the startup accent.
pub struct Renderer {
    /// The one text pass of the thread (the shaper, the glyph cache and
    /// the cell metrics), owned here; the tween's wide draw borrows it
    /// through the render bundle's renderer handle.
    text: TextPass,
    /// The one kitty image pass of the thread, beside the text pass.
    images: ImagePass,
    /// The frame gate (row 4.8): what the next frame must redraw.
    gate: FrameGate,
    /// The canvas at the current frame's device size, persistent across
    /// frames so a partial repaint keeps its pixels; `None` before the
    /// first frame and after a resize the allocator refused. A resize
    /// invalidates the gate: the old canvas' pixels do not survive it.
    canvas: Option<Canvas>,
    /// The measured cell metrics the metrics rebuild draws from.
    cell_metrics: CellMetrics,
    /// The painter metrics at the output scale (the cell pitch and the
    /// truncated ascent), rebuilt on a scale change.
    metrics: PainterMetrics,
    /// The Ghostty theme colours, fixed at start.
    theme: ThemeColours,
    /// The startup accent, fixed at start.
    accent: Option<Accent>,
    /// Whether the panel holds keyboard focus; the seat wiring of row 8.1
    /// updates it on keyboard enter and leave.
    focused: bool,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The canvas' size and the gate's state are what identifies a
        // renderer in test failures; the passes debug as their own types.
        f.debug_struct("Renderer")
            .field("text", &self.text)
            .field("images", &self.images)
            .field("gate", &self.gate)
            .field("canvas", &self.canvas)
            .field("cell_metrics", &self.cell_metrics)
            .field("metrics", &self.metrics)
            .field("theme", &self.theme)
            .field("accent", &self.accent)
            .field("focused", &self.focused)
            .finish()
    }
}

impl Renderer {
    /// A renderer over the font setup the thread measured, at the output
    /// scale, with the Ghostty theme and the startup accent. `None` when
    /// the scale yields no painter metrics — which the whole positive
    /// pitch a [`CellMetrics`] carries never is.
    #[must_use]
    pub fn new(
        setup: FontSetup,
        scale: f64,
        theme: ThemeColours,
        accent: Option<Accent>,
    ) -> Option<Self> {
        let (book, faces, cell_metrics, size_pt) = setup.into_parts();
        let metrics = cell_metrics.painter_metrics(scale)?;
        Some(Renderer {
            text: TextPass::new(
                crate::render::shape::TextShaper::new(faces, book),
                crate::render::glyph::GlyphCache::new(),
                cell_metrics,
                size_pt,
            ),
            images: ImagePass::new(),
            gate: FrameGate::new(),
            canvas: None,
            cell_metrics,
            metrics,
            theme,
            accent,
            focused: false,
        })
    }

    /// Draw one frame of `terminal` into the renderer's canvas under the
    /// frame gate, returning the [`FrameOutcome`] the caller presents by:
    /// [`FrameOutcome::Clean`] says nothing was drawn and nothing may be
    /// committed, [`FrameOutcome::Damage`] carries the device rectangles
    /// that changed. The canvas is resized to the frame's device size
    /// first, and a resize tells the gate — the old pixels do not survive
    /// it.
    pub fn draw(&mut self, terminal: &mut Terminal, frame: &FrameInput) -> FrameOutcome {
        let device = frame.device_size();
        if self
            .canvas
            .as_ref()
            .is_none_or(|canvas| canvas.size() != device)
        {
            self.canvas = Canvas::new(device.0, device.1);
            self.gate.invalidate();
        }
        // The painter call takes the canvas, metrics, passes and gate as
        // disjoint borrows, which a method on `&mut self` cannot spell in
        // one call: the fields are split here instead.
        let Renderer {
            text,
            images,
            gate,
            canvas,
            metrics,
            ..
        } = self;
        let Some(canvas) = canvas.as_mut() else {
            // A canvas no allocator produced (a zero or oversized frame —
            // `Canvas::new` refuses both): draw nothing, commit nothing.
            // The pool presentation refuses the same sizes, so nothing
            // could have been attached that this would leave stale.
            return FrameOutcome::Clean;
        };
        paint_frame_gated(
            canvas, metrics, frame, terminal, text, images, gate,
            // The text pass is never rebuilt on this thread: the font is
            // fixed at start and the scale lives in the metrics, which
            // the gate's fingerprint compares. `font_changed` is the
            // input of a rebuilt pass; there is none.
            false,
        )
    }

    /// Draw the tween's wide cache (row 6.2, D7) with this renderer's
    /// passes and state: the grid drawn once into a wide canvas through
    /// the existing painter. The one place the renderer's fields are
    /// split across a call — the metrics are shared with the text and
    /// image passes' mutable borrows.
    pub(crate) fn paint_wide(&mut self, draw: WideDraw, terminal: &mut Terminal) -> Option<Canvas> {
        super::crop::draw_wide(
            draw,
            &self.metrics,
            self.focused,
            self.theme,
            self.accent,
            terminal,
            &mut self.text,
            &mut self.images,
        )
    }

    /// The canvas of the current frame, for the caller's copy into a pool
    /// slot (row 4.1). `None` before the first frame and after a resize
    /// the allocator refused — nothing to present then.
    #[must_use]
    pub fn canvas(&self) -> Option<&Canvas> {
        self.canvas.as_ref()
    }

    /// The measured cell, the `CellSize` the panel thread's sizing needs
    /// (D3); `None` is as unreachable as a zero cell (see
    /// [`FontSetup::cell`]). Test builds only: the glue test reads the
    /// renderer's cell back; production reads it through the sizing and
    /// the painter metrics.
    #[cfg(test)]
    #[must_use]
    pub fn cell(&self) -> Option<CellSize> {
        CellSize::new(self.cell_metrics.cell_w(), self.cell_metrics.cell_h())
    }

    /// The Ghostty theme colours the frames and the wide draw fill with,
    /// the frame input's source.
    #[must_use]
    pub fn theme(&self) -> ThemeColours {
        self.theme
    }

    /// The startup accent the focus accent draws with, the frame input's
    /// source; `None` is a disabled accent.
    #[must_use]
    pub fn accent(&self) -> Option<Accent> {
        self.accent
    }

    /// Move the renderer to a new output scale: the painter metrics are
    /// rebuilt, and the next frame's fingerprint carries the new metrics,
    /// so the gate repaints everything without an explicit invalidate.
    /// The canvas catches up at the next draw, whose device size differs.
    pub fn set_scale(&mut self, scale: f64) {
        // The whole positive pitch a `CellMetrics` carries always yields
        // metrics (`PainterMetrics::new` refuses only a non-finite or
        // non-positive pitch), so this always replaces; the `if let`
        // keeps the unreachable case at the old metrics instead of a
        // panic.
        if let Some(metrics) = self.cell_metrics.painter_metrics(scale) {
            self.metrics = metrics;
        }
    }

    /// Record whether the panel holds keyboard focus. The next frame's
    /// fingerprint carries the flag, so the gate repaints everything —
    /// the accent draws over the whole surface edge.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Whether the panel holds keyboard focus, the read a frame's input
    /// is built from. Test builds only: the renderer tests read the flag
    /// back; production reads it through the frame input.
    #[cfg(test)]
    #[must_use]
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// Force the next frame to repaint everything: the canvas was
    /// invalidated outside the gate's view (a fresh `wl_shm` buffer from
    /// the pool, a lost frame).
    pub fn invalidate(&mut self) {
        self.gate.invalidate();
    }
}
