//! The GSK node emitter for the terminal grid (gsk-render-nodes): a second
//! painter beside the cairo [`render_grid`](super::DrawState::render_grid),
//! which stays the fallback and the parity oracle. The emitter walks the
//! same cell iteration into a [`gtk4::Snapshot`] and emits rectangles as
//! colour nodes, in logical coordinates — GSK applies the device scale,
//! unlike the cairo path, which snaps with `Antialias::None` on
//! device-scaled surfaces.
//!
//! Cell text is emitted as Pango layout nodes ([`append_layout`](
//! gtk4::prelude::SnapshotExt::append_layout)): the same [`pango::Layout`]
//! calls [`text::draw_text`](super::text::draw_text) makes, with the same
//! baseline pin and the same nerd-font constraint maths, expressed as
//! `save`/`translate`/`scale`/`append_layout`/`restore` — `pango_cairo_show`
//! draws a layout with its top-left at the current point, and a layout node
//! draws it at the snapshot's current transform, so the two agree point for
//! point. Sprites are emitted by [`node_sprites`], the decorations and the
//! cursor by [`node_cursor`], from the same cell pass.
//!
//! The geometry the two painters share lives here ([`cell_background_rect`])
//! so they cannot drift; the cairo painter calls it from
//! [`paint_backgrounds`](super::paint_backgrounds).
//!
//! Like [`DrawState::snapshot_tween`](super::DrawState::snapshot_tween), the
//! emitter runs on the GTK thread inside a widget snapshot; the widget call
//! site catches panics with the shared D5 guard and falls back to the cairo
//! draw path.

use gtk4::gdk;
use gtk4::graphene;
use gtk4::prelude::SnapshotExt as _;

use super::DrawState;
use super::metrics::CellMetrics;
use super::node_cursor;
use super::node_sprites;
use super::text::{FontsRef, NerdGlyph, constrain};
use crate::nerd_font::constraint;
use crate::term::Terminal;
use crate::term::cells::{Cell, Rgb, StyleFlags, Wide, first_codepoint};

/// An [`Rgb`] as a GDK colour. GDK stores the channels as `f32`; the cairo
/// path divides in `f64`. Both quantize to the same 16-bit colour when
/// drawn, which is what the scale-1 parity test asserts.
pub(super) fn rgba(color: Rgb) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
        1.0,
    )
}

/// The background rectangle of `cell` in logical pixels, or `None` when the
/// cell has no explicit background. The frame's last row — `height / cell_h`
/// minus one, exactly what `render_grid` tests — fills down to `height`,
/// which need not be a multiple of the cell pitch. Each edge is snapped to
/// the device pixel grid (`snap-grid-edges` D1, D4): neighbouring cells
/// pass the same shared-edge value to the snap, so both compute the same
/// edge and no blended seam appears at a fractional scale.
pub(super) fn cell_background_rect(
    cell: &Cell,
    metrics: CellMetrics,
    height: i32,
) -> Option<(f64, f64, f64, f64)> {
    if !cell.has_bg {
        return None;
    }
    let row_height = if cell.y == height / metrics.cell_h - 1 {
        height - cell.y * metrics.cell_h
    } else {
        metrics.cell_h
    };
    Some(metrics.scale.snap_rect(
        f64::from(cell.x) * f64::from(metrics.cell_w),
        f64::from(cell.y) * f64::from(metrics.cell_h),
        f64::from(metrics.cell_w),
        f64::from(row_height),
    ))
}

impl DrawState {
    /// Emit the theme background as one colour node covering the whole
    /// widget (gsk-render-nodes task 1.2; `draw_inner`'s fill). `width` and
    /// `height` are the widget's logical size, the same coordinates
    /// [`DrawState::draw`] paints. Split from [`Self::emit_backgrounds`] so
    /// a frame can paint the background unshifted and still emit the grid
    /// itself translated to the docked edge (gsk-render-nodes design, Post-task decisions: C52).
    pub fn emit_theme_background(&self, snapshot: &gtk4::Snapshot, width: i32, height: i32) {
        snapshot.append_color(
            &rgba(self.theme_background),
            &graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
        );
    }

    /// Emit the cell backgrounds — including the last row's fill — as colour
    /// nodes into `snapshot` (gsk-render-nodes task 1.2). `height` is the
    /// grid-relative height the last row fills to.
    ///
    /// Reports whether nodes were emitted. `false` — no live frame — asks
    /// the caller to fall back to the cairo draw path, which paints the same
    /// thing; a draw before the first `cell_metrics_update` has no grid
    /// pass, so it reports `true` with no cells.
    pub fn emit_cell_backgrounds(
        &self,
        snapshot: &gtk4::Snapshot,
        terminal: &mut Terminal,
        height: i32,
    ) -> bool {
        if self.cell_metrics.cell_h <= 0 {
            return true;
        }
        if !terminal.frame_begin() {
            return false;
        }
        let metrics = self.cell_metrics;
        while let Some(cell) = terminal.cell_next() {
            if let Some((x, y, w, h)) = cell_background_rect(&cell, metrics, height) {
                snapshot.append_color(
                    &rgba(cell.bg),
                    &graphene::Rect::new(x as f32, y as f32, w as f32, h as f32),
                );
            }
        }
        terminal.frame_end();
        true
    }

    /// Emit the theme background and the cell backgrounds — including the
    /// last row's fill — as colour nodes into `snapshot` (gsk-render-nodes
    /// task 1.2). `width`/`height` are the widget's logical size, the same
    /// coordinates [`DrawState::draw`] paints: the theme background covers
    /// the whole widget and `height` is the grid-relative height the last
    /// row fills to.
    ///
    /// Reports whether nodes were emitted. `false` — no live frame — asks
    /// the caller to fall back to the cairo draw path, which paints the same
    /// thing; a draw before the first `cell_metrics_update` has no grid
    /// pass, so it emits the theme background only and still reports `true`.
    pub fn emit_backgrounds(
        &self,
        snapshot: &gtk4::Snapshot,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
    ) -> bool {
        self.emit_theme_background(snapshot, width, height);
        self.emit_cell_backgrounds(snapshot, terminal, height)
    }

    /// Emit the frame's cells as nodes (gsk-render-nodes tasks 2.1–2.3),
    /// mirroring [`render_grid`](super::DrawState::render_grid)'s cell pass:
    /// the same cell skips, the same foreground selection, sprites before
    /// text ([`super::node_sprites::emit_cell_sprite`], whose `false` is
    /// `draw_sprite`'s), the underline/strikethrough strokes and
    /// — after the loop, as `render_grid` draws it — the cursor. Needs the
    /// fonts and the pango context from [`Self::cell_metrics_update`]
    /// (gsk-render-nodes task 1.2's `emit_backgrounds` carries the same
    /// fall-back-to-cairo contract); `false` asks the caller to draw the
    /// cairo path instead.
    pub fn emit_cells(&self, snapshot: &gtk4::Snapshot, terminal: &mut Terminal) -> bool {
        let (Some(context), Some(fonts)) = (&self.pango_context, self.fonts.as_ref()) else {
            return false;
        };
        if self.cell_metrics.cell_h <= 0 {
            return false;
        }
        if !terminal.frame_begin() {
            return false;
        }
        let metrics = self.cell_metrics;
        let fonts = FontsRef {
            regular: &fonts.regular,
            bold: &fonts.bold,
            italic: &fonts.italic,
            bold_italic: &fonts.bold_italic,
        };
        let layout = pango::Layout::new(context);
        let mut cursor_text = [0u8; crate::term::cells::CELL_TEXT_CAP];
        let mut cursor_text_len = 0usize;
        let mut have_cursor_text = false;
        while let Some(cell) = terminal.cell_next() {
            if cell.wide == Wide::SpacerTail {
                continue; // do not render
            }
            if cell.len == 0 || cell.flags.contains(StyleFlags::INVISIBLE) {
                continue;
            }
            let fg = if cell.has_fg {
                cell.fg
            } else {
                self.theme_foreground
            };
            let colour = rgba(fg);
            let cp = first_codepoint(cell.text_bytes());
            if !node_sprites::emit_cell_sprite(snapshot, &cell, cp, &metrics, &colour) {
                emit_cell_text(snapshot, &layout, &cell, &fonts, &metrics, fg);
            }
            node_cursor::emit_decorations(snapshot, &cell, &metrics, &colour);

            // The block cursor redraws the glyph under it; the same cell
            // render_grid's cursor pass collects it for.
            if !have_cursor_text
                && let Some(cursor) = terminal.cursor()
                && cursor.x == cell.x
                && cursor.y == cell.y
            {
                cursor_text_len = cell.len.min(cursor_text.len() - 1);
                cursor_text[..cursor_text_len].copy_from_slice(&cell.text[..cursor_text_len]);
                have_cursor_text = true;
            }
        }

        if let Some(cursor) = terminal.cursor() {
            let text = if have_cursor_text {
                &cursor_text[..cursor_text_len]
            } else {
                &[]
            };
            node_cursor::emit_cursor(snapshot, self, terminal, &cursor, text);
        }
        terminal.frame_end();
        true
    }
}

/// Emit one cell's text as a Pango layout node, the node form of
/// [`text::draw_text`](super::text::draw_text): same font selection, same
/// baseline pin, same nerd-font constraint transform, same fallback when
/// the ink is empty. The layout's top-left lands where `pango_cairo_show`
/// would put it — a layout node draws at the snapshot's current transform,
/// so each `move_to` becomes a `translate`.
pub(super) fn emit_cell_text(
    snapshot: &gtk4::Snapshot,
    layout: &pango::Layout,
    cell: &Cell,
    fonts: &FontsRef<'_>,
    cell_metrics: &CellMetrics,
    fg: Rgb,
) {
    let desc = fonts.for_flags(
        cell.flags.contains(StyleFlags::BOLD),
        cell.flags.contains(StyleFlags::ITALIC),
    );
    layout.set_font_description(Some(desc));
    layout.set_text(cell.text_str());

    // The same baseline pin draw_text does: the layout's own ascent shifts
    // when Pango falls back to another font for a glyph, and the cell's
    // baseline is where the row's text belongs.
    let baseline = (layout.baseline() + pango::SCALE / 2) / pango::SCALE;

    if cell.len > 0
        && let Some(c) = constraint(first_codepoint(cell.text_bytes()))
        && c.does_anything()
    {
        let (ink, _) = layout.pixel_extents();
        if ink.width() > 0 && ink.height() > 0 {
            let m = cell_metrics.nerd;
            // The glyph's box relative to the cell's bottom-left corner.
            let glyph = NerdGlyph {
                width: f64::from(ink.width()),
                height: f64::from(ink.height()),
                x: f64::from(ink.x()),
                y: f64::from(cell_metrics.baseline)
                    - (f64::from(ink.y() + ink.height()) - f64::from(baseline)),
            };

            let constrained = constrain(
                &c,
                &m,
                glyph,
                if cell.cw >= 1 {
                    cell.cw.cast_unsigned()
                } else {
                    1
                },
            );

            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (f64::from(cell.x) * f64::from(cell_metrics.cell_w) + constrained.x) as f32,
                ((f64::from(cell.y) + 1.0) * f64::from(cell_metrics.cell_h)
                    - constrained.y
                    - constrained.height) as f32,
            ));
            snapshot.scale(
                (constrained.width / f64::from(ink.width())) as f32,
                (constrained.height / f64::from(ink.height())) as f32,
            );
            snapshot.translate(&graphene::Point::new(
                -f64::from(ink.x()) as f32,
                -f64::from(ink.y()) as f32,
            ));
            snapshot.append_layout(layout, &rgba(fg));
            snapshot.restore();
            return;
        }
    }

    snapshot.save();
    snapshot.translate(&graphene::Point::new(
        (f64::from(cell.x) * f64::from(cell_metrics.cell_w)) as f32,
        (f64::from(cell.y) * f64::from(cell_metrics.cell_h) + f64::from(cell_metrics.ascent)
            - f64::from(baseline)) as f32,
    ));
    snapshot.append_layout(layout, &rgba(fg));
    snapshot.restore();
}

#[cfg(test)]
mod tests {
    use super::super::tests::{state, terminal};
    use super::*;
    use crate::render::parity;
    use crate::render::parity::{cairo_frame, node_frame, terminal_with};
    use crate::render::set_rgb;

    use crate::render::font_lock;

    /// The opaque backing both parity surfaces get (from the parity harness;
    /// the cairo background oracle below blends against the same colour).
    use crate::render::parity::BACKDROP;

    /// The stated text tolerance (gsk-render-nodes task 2.1): an antialiased
    /// glyph edge pixel can differ by up to half the channel distance between
    /// the glyph colour and the backdrop, rounded up — the same blend bound
    /// the background tests use. Measured on this suite the delta is 0 (both
    /// painters rasterise through pangocairo on the same surface), so any
    /// failure at this bound is a real rendering difference, not noise.
    const TEXT_TOLERANCE: u8 = 128;

    /// The cairo oracle for the background parity tests: `draw_inner`'s theme
    /// fill plus `render_grid`'s background pass, into a backed surface at
    /// `scale`. The drawing scale is recorded on the draw state first
    /// (`set_scale`, `snap-grid-edges` D2), so both painters snap at `scale`.
    fn cairo_backgrounds(
        draw_state: &mut DrawState,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        scale: f64,
    ) -> cairo::ImageSurface {
        draw_state.set_scale(super::super::OutputScale::new(scale));
        let surface = parity::backed_surface(width, height, scale, BACKDROP);
        let cr = cairo::Context::new(&surface).expect("context");
        set_rgb(&cr, draw_state.theme_background);
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _ = cr.fill();
        assert!(terminal.frame_begin(), "frame began");
        super::super::paint_backgrounds(&cr, terminal, draw_state.cell_metrics, height);
        terminal.frame_end();
        surface
    }

    /// A terminal whose cells carry three different explicit backgrounds:
    /// red on row 0, green on row 1, blue on rows 2 and 3 (the frame's last
    /// row against a 71px-high area, which it fills past its cell boundary).
    fn terminal_with_backgrounds() -> crate::term::Terminal {
        let mut terminal = terminal();
        terminal.push_pty_data(b"\x1b[41m........\x1b[42m........\x1b[44m................\x1b[0m");
        terminal
    }

    /// ASCII, bold, italic, bold-italic and an explicit foreground, wrapped
    /// across two rows.
    fn terminal_with_styled_text() -> crate::term::Terminal {
        terminal_with(
            b"Hi \x1b[1mbo\x1b[0m \x1b[3mit\x1b[0m \x1b[1;3mbi\x1b[0m \x1b[38:5:203mred\x1b[0m",
        )
    }

    /// Two wide cells (CJK; the head cell's `cw` is 2).
    fn terminal_with_wide_text() -> crate::term::Terminal {
        terminal_with("漢字".as_bytes())
    }

    /// A nerd-font glyph the constraint table scales and centres (U+2630,
    /// the trigram the `nerd_font` test names as constrained).
    fn terminal_with_constrained_text() -> crate::term::Terminal {
        terminal_with("☰".as_bytes())
    }

    /// The theme background differs from every cell background by more than
    /// half a channel, so a missing cell cannot hide inside the scale-1.5
    /// tolerance below.
    #[test]
    fn backgrounds_match_the_cairo_painter_exactly_at_scale_1() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut terminal = terminal_with_backgrounds();
        let (width, height) = (65, 71);
        let mut cairo_side = cairo_backgrounds(&mut draw_state, &mut terminal, width, height, 1.0);
        let mut node_side = node_frame(&mut draw_state, &mut terminal, width, height, 1.0, false);
        parity::assert_exact(&mut cairo_side, &mut node_side, "backgrounds at scale 1");
    }

    /// At scale 1.5 fractional cell edges land between device pixels. Both
    /// painters snap them the same way now — the shared geometry snaps each
    /// edge to the device pixel grid (`snap-grid-edges` D1, D4) and the
    /// cairo path's `Antialias::None` then has nothing left to round — so
    /// the two agree pixel for pixel.
    #[test]
    fn backgrounds_match_the_cairo_painter_exactly_at_scale_1_5() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut terminal = terminal_with_backgrounds();
        let (width, height) = (65, 71);
        let mut cairo_side = cairo_backgrounds(&mut draw_state, &mut terminal, width, height, 1.5);
        let mut node_side = node_frame(&mut draw_state, &mut terminal, width, height, 1.5, false);
        parity::assert_exact(&mut cairo_side, &mut node_side, "backgrounds at scale 1.5");
    }

    #[test]
    fn styled_ascii_text_matches_the_cairo_painter_exactly_at_scale_1() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut cairo_terminal = terminal_with_styled_text();
        let mut node_terminal = terminal_with_styled_text();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.0);
        let mut node_side = node_frame(&mut draw_state, &mut node_terminal, 65, 71, 1.0, true);
        parity::assert_exact(&mut cairo_side, &mut node_side, "styled ASCII at scale 1");
    }

    #[test]
    fn wide_text_matches_the_cairo_painter_exactly_at_scale_1() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut cairo_terminal = terminal_with_wide_text();
        let mut node_terminal = terminal_with_wide_text();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.0);
        let mut node_side = node_frame(&mut draw_state, &mut node_terminal, 65, 71, 1.0, true);
        parity::assert_exact(&mut cairo_side, &mut node_side, "wide text at scale 1");
    }

    /// The nerd-font constraint path scales the layout with `f32` modelview
    /// factors where the cairo path scales in `f64`, so glyph rasterisation
    /// could differ by a subpixel step; the stated tolerance is the same
    /// blend bound as everywhere else ([`TEXT_TOLERANCE`]), and the measured
    /// delta on this suite is 0.
    #[test]
    fn constrained_text_matches_the_cairo_painter_within_tolerance_at_scale_1() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        // The parity only means anything if the constraint branch actually
        // runs: the trigram is constrained and renders with ink, so the
        // emitter takes the scaled path, not the fallback.
        let fonts = draw_state.fonts.as_ref().expect("fonts loaded");
        let context = draw_state.pango_context.as_ref().expect("pango context");
        let probe = pango::Layout::new(context);
        probe.set_font_description(Some(&fonts.regular));
        probe.set_text("☰");
        let (ink, _) = probe.pixel_extents();
        assert!(
            ink.width() > 0 && ink.height() > 0,
            "the trigram must render with ink"
        );
        assert!(
            crate::nerd_font::constraint(0x2630).is_some_and(|c| c.does_anything()),
            "the trigram must be constrained"
        );

        let mut cairo_terminal = terminal_with_constrained_text();
        let mut node_terminal = terminal_with_constrained_text();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.0);
        let mut node_side = node_frame(&mut draw_state, &mut node_terminal, 65, 71, 1.0, true);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            TEXT_TOLERANCE,
            "constrained at scale 1",
        );
    }

    /// At scale 1.5 the layout nodes' fractional modelview could blend
    /// antialiased glyph edges differently from the cairo path's
    /// device-scaled rasterisation; the tolerance is the same blend bound as
    /// everywhere else ([`TEXT_TOLERANCE`]), and the measured delta on this
    /// suite is 0.
    #[test]
    fn all_text_matches_the_cairo_painter_within_tolerance_at_scale_1_5() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut cairo_terminal = terminal_with_styled_text();
        let mut node_terminal = terminal_with_styled_text();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.5);
        let mut node_side = node_frame(&mut draw_state, &mut node_terminal, 65, 71, 1.5, true);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            TEXT_TOLERANCE,
            "styled text at scale 1.5",
        );

        let mut cairo_terminal = terminal_with_wide_text();
        let mut node_terminal = terminal_with_wide_text();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.5);
        let mut node_side = node_frame(&mut draw_state, &mut node_terminal, 65, 71, 1.5, true);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            TEXT_TOLERANCE,
            "wide text at scale 1.5",
        );

        let mut cairo_terminal = terminal_with_constrained_text();
        let mut node_terminal = terminal_with_constrained_text();
        let mut cairo_side = cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.5);
        let mut node_side = node_frame(&mut draw_state, &mut node_terminal, 65, 71, 1.5, true);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            TEXT_TOLERANCE,
            "constrained at scale 1.5",
        );
    }
}
