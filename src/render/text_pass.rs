//! The text pass of the grid painter (row 4.5, `replace-gtk-with-wayland`
//! D5/D6/D11): each cell's grapheme cluster shaped with the [`TextShaper`],
//! rasterized through the [`GlyphCache`] at the frame's device size, and
//! drawn onto the [`crate::render::canvas::Canvas`] at the device pixel
//! lattice — the same pixels
//! the GTK path's `text::draw_text` produces for the same cells.
//!
//! Placement on the lattice (the spec requirement "Cell text on the device
//! pixel lattice"): a glyph's origin is the cell's own snapped corner —
//! [`PainterMetrics::cell_rect`] — plus an offset inside the cell that is a
//! whole number of device pixels. The vertical offset is the device baseline
//! offset from the cell's top: the truncated logical ascent
//! [`PainterMetrics::ascent`] carries times the scale, rounded to a whole
//! device pixel with [`crate::render::geom::device_px`]. Neither the offset
//! nor the nerd-font
//! transform depends on the column or the row, so the same glyph in the same
//! style and colour renders to the same device pixels from every cell's
//! snapped top-left corner at any scale.
//!
//! Y-axis conventions: the shaper's `y_offset` is positive up (the font's
//! own axis), and a rasterized glyph's placement `top` is positive up from
//! the baseline. A glyph's top-left device corner is therefore drawn at
//! `(origin_x + pen_x + left, baseline_y - pen_y - top)`, where `baseline_y`
//! is the cell's snapped top plus the rounded ascent offset and the pen
//! position comes from the cluster's pen walk. The pen position and the
//! shaper's offsets are device pixels already (the shaper shapes at the
//! device ppem); each glyph's position is rounded to a whole device pixel
//! with [`crate::render::geom::device_px`].
//!
//! The pass owns exactly the cells `sprite::cell_sprite` declines:
//! the sprite pass and this pass own disjoint cells, and the frame walk runs
//! this pass after the sprites and before the bands, because GTK draws each
//! cell's decorations after its glyph.
//!
//! The walk also collects the text bytes of the cell at the cursor's
//! position — the same collection the GTK path's grid walk did — and
//! returns them as [`CursorText`] for the cursor layer's block-cursor glyph
//! redraw (see the seam in [`super::cursor`]).
//!
//! The state the pass needs (the shaper, the glyph cache, the cell metrics
//! the nerd-font constraints are expressed against, and the font size in
//! points) lives in [`crate::render::text_pass::TextPass`]; the panel
//! thread's renderer owns one and hands it to
//! [`super::painter::paint_frame`] each frame.
//!
//! GTK-free (`replace-gtk-with-wayland` D10): the pixel tests in
//! `text_pass/tests.rs` run without a display.

use crate::nerd_font::constraint;
use crate::render::cell_metrics::CellMetrics;
use crate::render::font::Style;
use crate::render::glyph::{Glyph, GlyphCache, GlyphImage, GlyphRequest, Ppem};
use crate::render::nerd::{InkBox, NerdMetrics};
use crate::render::shape::{ShapedCluster, TextShaper};
use crate::term::Terminal;
use crate::term::cells::{CELL_TEXT_CAP, Cell, StyleFlags, Wide, first_codepoint};

use super::canvas::{Canvas, CanvasColor};
use super::geom::{FrameInput, PainterMetrics, device_px};
use super::sprite;

/// The 96-dpi points-to-pixels conversion [`crate::render::cell_metrics::
/// measure`] applies inline (the font is placed at `points * 96 / 72`
/// pixels); there is no shared helper to reuse, so the constant carries the
/// same rule. The ppem itself is quantized by [`Ppem::from_px`], the same
/// 26.6 fixed point `measure`'s pixel arithmetic works in.
const PX_PER_POINT: f64 = 96.0 / 72.0;

/// The most text bytes the cursor redraw collects — the GTK path's
/// `cursor_text` buffer minus its terminating zero.
const CURSOR_TEXT_MAX: usize = CELL_TEXT_CAP - 1;

/// The text bytes of the cell at the cursor's position, collected during
/// the text pass's walk. Empty when no cell matched — the block cursor's
/// glyph redraw draws nothing then (see [`super::cursor::glyph_redraw`]).
#[derive(Clone, Copy, Debug)]
pub struct CursorText {
    bytes: [u8; CELL_TEXT_CAP],
    len: usize,
}

impl CursorText {
    /// No cell collected: the redraw's empty input.
    #[must_use]
    pub fn empty() -> Self {
        CursorText {
            bytes: [0; CELL_TEXT_CAP],
            len: 0,
        }
    }

    /// The collected bytes, at most `CURSOR_TEXT_MAX` of them.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// The text pass's state (row 4.5): the shaper, the glyph cache, the cell
/// metrics the nerd-font constraints are expressed against and the font
/// size in points. Not `Sync`: the painter runs on one thread, the panel's
/// render thread.
pub struct TextPass {
    shaper: TextShaper,
    glyphs: GlyphCache,
    cell_metrics: CellMetrics,
    size_pt: f64,
}

impl std::fmt::Debug for TextPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextPass")
            .field("size_pt", &self.size_pt)
            .field("cell_metrics", &self.cell_metrics)
            .field("shaper", &self.shaper)
            .field("glyphs", &self.glyphs)
            .finish()
    }
}

impl TextPass {
    /// A text pass over the shaper and glyph cache the panel thread owns,
    /// with the cell metrics measured from the terminal face at `size_pt`
    /// points (the Ghostty `font-size`) — the same measurement row 4.6
    /// built the cell pitch from.
    #[must_use]
    pub fn new(
        shaper: TextShaper,
        glyphs: GlyphCache,
        cell_metrics: CellMetrics,
        size_pt: f64,
    ) -> Self {
        TextPass {
            shaper,
            glyphs,
            cell_metrics,
            size_pt,
        }
    }

    /// Walk the open frame's cells and draw the text, shifted by `offset`
    /// device pixels along x; return the cursor cell's text for the block
    /// cursor's glyph redraw. The frame must be open
    /// ([`Terminal::frame_begin`]); the caller rewinds the frame afterwards.
    pub fn paint(
        &mut self,
        canvas: &mut Canvas,
        metrics: &PainterMetrics,
        frame: &FrameInput,
        terminal: &mut Terminal,
        offset: i32,
    ) -> CursorText {
        let mut cursor_text = CursorText::empty();
        let scale = metrics.scale();
        // A size no rasterizer accepts draws no text: the frame degrades to
        // its other layers rather than failing.
        let Ok(ppem) = Ppem::from_px(self.size_pt * PX_PER_POINT * scale) else {
            return cursor_text;
        };
        // The device baseline offset from each cell's snapped top: the
        // truncated logical ascent at this frame's scale, rounded once — a
        // per-frame constant, so no cell's placement can drift from
        // another's.
        let baseline_offset = device_px(metrics.ascent() * scale);
        let cursor = terminal.cursor();
        let mut collected = false;
        while let Some(cell) = terminal.cell_next() {
            if cell.wide == Wide::SpacerTail {
                continue; // do not render
            }
            if cell.len == 0 || cell.flags.contains(StyleFlags::INVISIBLE) {
                continue;
            }
            // The cursor cell's text, collected once — the first cell with
            // a glyph at the cursor's position, exactly the GTK path's
            // grid-walk collection. A cell the sprite pass owns still
            // collects: the old path redraws it through the text path too.
            if !collected
                && let Some(at) = cursor
                && at.x == cell.x
                && at.y == cell.y
            {
                let take = cell.len.min(CURSOR_TEXT_MAX);
                cursor_text.bytes[..take].copy_from_slice(&cell.text[..take]);
                cursor_text.len = take;
                collected = true;
            }
            if !text_owns(metrics, frame, &cell) {
                continue; // the sprite pass drew this cell
            }
            let color = if cell.has_fg {
                CanvasColor::from_theme(cell.fg)
            } else {
                frame.foreground()
            };
            self.draw_cluster(canvas, metrics, &cell, color, offset, ppem, baseline_offset);
        }
        cursor_text
    }

    /// Draw one cell's text in `color` — the block cursor's glyph redraw
    /// enters here, with the terminal's default background and a cell built
    /// from the collected [`CursorText`], exactly the synthetic cell the
    /// GTK path's `draw_cursor` hands `draw_text`. Shaping is cached, so a
    /// redraw costs at most the rasterization the walk already paid.
    pub fn draw_cell(
        &mut self,
        canvas: &mut Canvas,
        metrics: &PainterMetrics,
        cell: &Cell,
        color: CanvasColor,
        offset: i32,
    ) {
        let scale = metrics.scale();
        let Ok(ppem) = Ppem::from_px(self.size_pt * PX_PER_POINT * scale) else {
            return;
        };
        let baseline_offset = device_px(metrics.ascent() * scale);
        self.draw_cluster(canvas, metrics, cell, color, offset, ppem, baseline_offset);
    }

    /// Shape and draw one cell's cluster. A shaping or rasterization error
    /// (a broken font) draws nothing rather than failing the frame.
    fn draw_cluster(
        &mut self,
        canvas: &mut Canvas,
        metrics: &PainterMetrics,
        cell: &Cell,
        color: CanvasColor,
        offset: i32,
        ppem: Ppem,
        baseline_offset: i32,
    ) {
        let Ok(shaped) = self.shaper.shape(cell.text_str(), style_of(cell), ppem) else {
            return;
        };
        let rect = metrics.cell_rect(cell.x, cell.y);
        let origin_x = rect.x();
        let baseline_y = rect.y() + baseline_offset;

        // The nerd-font constraint applies to single-glyph clusters — the
        // nerd icons are single code points; a multi-glyph cluster (a base
        // and a mark, an emoji sequence) draws unconstrained.
        let glyphs = shaped.glyphs();
        if let (Some(first), Some(constrained)) = (
            glyphs.first().copied(),
            self.constrained_glyph(&shaped, cell, metrics.scale(), ppem),
        ) {
            draw_glyph(
                canvas,
                origin_x,
                baseline_y,
                &constrained,
                first.x_offset(),
                first.y_offset(),
                color,
                offset,
            );
            return;
        }
        let mut pen = 0.0f32;
        for glyph in glyphs {
            if let Ok(rasterized) = self.glyphs.rasterize(&GlyphRequest::new(
                shaped.face(),
                glyph.id(),
                ppem,
                shaped.synthesis(),
                None,
            )) {
                draw_glyph(
                    canvas,
                    origin_x,
                    baseline_y,
                    &rasterized,
                    pen + glyph.x_offset(),
                    glyph.y_offset(),
                    color,
                    offset,
                );
            }
            pen += glyph.x_advance();
        }
    }

    /// The constrained rasterization of a cell's cluster, or `None` when it
    /// draws unconstrained: no constraint for the first code point, more
    /// than one shaped glyph, or a constraint that places nothing. The
    /// glyph is rasterized once unconstrained to take its ink box, the
    /// transform is built from the device-scaled cell metrics and the
    /// device baseline (both unsnapped — the spec requires the transform to
    /// be identical in every cell at one scale), and the glyph is
    /// rasterized again with the transform applied to the outline.
    fn constrained_glyph(
        &mut self,
        shaped: &ShapedCluster,
        cell: &Cell,
        scale: f64,
        ppem: Ppem,
    ) -> Option<std::sync::Arc<Glyph>> {
        let glyphs = shaped.glyphs();
        if glyphs.len() != 1 {
            return None;
        }
        let constraint = constraint(first_codepoint(cell.text_bytes()))?;
        if !constraint.does_anything() {
            return None;
        }
        let request = |transform| {
            GlyphRequest::new(
                shaped.face(),
                glyphs[0].id(),
                ppem,
                shaped.synthesis(),
                transform,
            )
        };
        let unconstrained = self.glyphs.rasterize(&request(None)).ok()?;
        let ink = InkBox::from_glyph(&unconstrained);
        let nerd_metrics = NerdMetrics::from_cell_metrics(&self.cell_metrics, scale)?;
        // The old `draw_text` used the cell baseline for the constraint
        // frame and the ascent for the draw origin; both carry over — the
        // baseline scaled to device pixels as an unsnapped f64 here, the
        // ascent in the caller's rounded offset.
        let baseline = f64::from(self.cell_metrics.baseline()) * scale;
        // The constraint width is the cell's column span, at least 1.
        let constraint_width = u32::try_from(cell.cw.max(1)).unwrap_or(1);
        let transform = crate::render::nerd::placement(
            &constraint,
            &nerd_metrics,
            baseline,
            constraint_width,
            &ink,
        )?;
        self.glyphs.rasterize(&request(Some(transform))).ok()
    }
}

/// The cell style bits as the font module's [`Style`] — the same four-way
/// selection the GTK path's `FontsRef::for_flags` makes.
fn style_of(cell: &Cell) -> Style {
    let bold = cell.flags.contains(StyleFlags::BOLD);
    let italic = cell.flags.contains(StyleFlags::ITALIC);
    match (bold, italic) {
        (true, true) => Style::BoldItalic,
        (true, false) => Style::Bold,
        (false, true) => Style::Italic,
        (false, false) => Style::Regular,
    }
}

/// Whether the text pass draws `cell`: the skips are the GTK cell pass's —
/// a wide glyph's spacer tail, a cell with no glyph, the INVISIBLE flag —
/// and the cell must not be one the sprite pass owns, so the two passes
/// stay disjoint.
pub(super) fn text_owns(metrics: &PainterMetrics, frame: &FrameInput, cell: &Cell) -> bool {
    if cell.wide == Wide::SpacerTail {
        return false; // never rendered
    }
    if cell.len == 0 || cell.flags.contains(StyleFlags::INVISIBLE) {
        return false;
    }
    sprite::cell_sprite(metrics, frame, cell).is_none()
}

/// Draw one rasterized glyph at the device pixel lattice: its top-left
/// device corner sits at
/// `(origin_x + round(pen_x) + left + offset, baseline_y - round(pen_y) - top)`
/// — the pen position and offsets rounded as one whole device pixel each,
/// the placement `left`/`top` whole pixels already. A mask glyph is tinted
/// with `color`; a colour glyph (emoji) is blitted untinted; an empty glyph
/// draws nothing.
fn draw_glyph(
    canvas: &mut Canvas,
    origin_x: i32,
    baseline_y: i32,
    glyph: &Glyph,
    pen_x: f32,
    pen_y: f32,
    color: CanvasColor,
    offset: i32,
) {
    let placement = glyph.placement();
    let x = origin_x + device_px(f64::from(pen_x)) + placement.left() + offset;
    let y = baseline_y - device_px(f64::from(pen_y)) - placement.top();
    match glyph.image() {
        GlyphImage::Mask(mask) => canvas.draw_mask(mask, x, y, color),
        GlyphImage::Color(pixmap) => canvas.draw_image(pixmap, x, y),
        GlyphImage::Empty => {}
    }
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
