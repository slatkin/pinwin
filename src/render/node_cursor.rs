//! The node emitter's decoration and cursor passes (gsk-render-nodes task
//! 2.3): the node forms of `render_grid`'s underline/strikethrough strokes
//! and [`super::DrawState::draw_cursor`]. Both shapes are 1–2 px bands, so
//! they go out as colour nodes; the geometry is shared with the cairo
//! painter ([`decoration_rects`], [`cursor_shape`]) so the two cannot drift.
//!
//! The one asymmetry: the cairo painter draws the hollow block cursor as a
//! stroked rectangle, which is geometrically the four 1 px bands the node
//! emitter fills — same pixels, different primitives, so [`cursor_shape`]
//! hands the cairo side the stroke rectangle and the node side decomposes it.
use gtk4::prelude::SnapshotExt as _;

use super::DrawState;
use super::metrics::CellMetrics;
use super::nodes::{emit_cell_text, rgba};
use crate::term::cells::{Cell, Cursor, CursorStyle, StyleFlags};

/// The underline band `cell`'s UNDERLINE flag asks for, as the rectangle
/// the node emitter fills and the cairo painter strokes along its
/// horizontal centre line — the stroke covers exactly the band, so the
/// two painters share these numbers. `None` when the flag is unset; the
/// split (instead of a shared list) keeps the per-cell pass
/// allocation-free.
pub(crate) fn underline_rect(
    cell: &Cell,
    cell_metrics: &CellMetrics,
) -> Option<(f64, f64, f64, f64)> {
    if !cell.flags.contains(StyleFlags::UNDERLINE) {
        return None;
    }
    Some(underline_strikethrough_rect(
        cell,
        cell_metrics,
        f64::from(cell_metrics.ascent) + 1.0,
    ))
}

/// The strikethrough band `cell`'s STRIKETHROUGH flag asks for, centred on
/// the cell's mid-line. `None` when the flag is unset.
pub(crate) fn strikethrough_rect(
    cell: &Cell,
    cell_metrics: &CellMetrics,
) -> Option<(f64, f64, f64, f64)> {
    if !cell.flags.contains(StyleFlags::STRIKETHROUGH) {
        return None;
    }
    Some(underline_strikethrough_rect(
        cell,
        cell_metrics,
        f64::from(cell_metrics.cell_h) / 2.0 - 0.5,
    ))
}

/// The 1 px band across `cell` whose top edge sits `top` px below the
/// cell's top, spanning the cell's width.
fn underline_strikethrough_rect(
    cell: &Cell,
    cell_metrics: &CellMetrics,
    top: f64,
) -> (f64, f64, f64, f64) {
    (
        f64::from(cell.x) * f64::from(cell_metrics.cell_w),
        f64::from(cell.y) * f64::from(cell_metrics.cell_h) + top,
        f64::from(cell_metrics.cell_w),
        1.0,
    )
}

/// The cursor's shape as the cairo painter and the node emitter draw it
/// (gsk-render-nodes task 2.3). `Fill` is one rectangle to fill; `Hollow` is
/// the outline rectangle the cairo painter strokes with a 1 px line — the
/// node emitter fills the four 1 px bands that stroke covers instead.
pub(crate) enum CursorShape {
    Fill((f64, f64, f64, f64)),
    Hollow((f64, f64, f64, f64)),
}

/// The geometry [`super::DrawState::draw_cursor`] paints `cursor` with.
pub(crate) fn cursor_shape(cursor: &Cursor, cell_metrics: &CellMetrics) -> CursorShape {
    let x = f64::from(cursor.x) * f64::from(cell_metrics.cell_w);
    let y = f64::from(cursor.y) * f64::from(cell_metrics.cell_h);
    let cw = f64::from(cell_metrics.cell_w);
    let ch = f64::from(cell_metrics.cell_h);
    match cursor.style {
        CursorStyle::Bar => CursorShape::Fill((x, y, 2.0, ch)),
        CursorStyle::Underline => CursorShape::Fill((x, y + ch - 2.0, cw, 2.0)),
        CursorStyle::BlockHollow => CursorShape::Hollow((x + 0.5, y + 0.5, cw - 1.0, ch - 1.0)),
        CursorStyle::Block => CursorShape::Fill((x, y, cw, ch)),
    }
}

/// Emit `cell`'s underline/strikethrough as colour nodes, the node form of
/// `render_grid`'s decoration strokes.
pub(crate) fn emit_decorations(
    snapshot: &gtk4::Snapshot,
    cell: &Cell,
    cell_metrics: &CellMetrics,
    colour: &gtk4::gdk::RGBA,
) {
    use gtk4::graphene;
    for (x, y, w, h) in underline_rect(cell, cell_metrics)
        .into_iter()
        .chain(strikethrough_rect(cell, cell_metrics))
    {
        snapshot.append_color(
            colour,
            &graphene::Rect::new(x as f32, y as f32, w as f32, h as f32),
        );
    }
}

/// Emit the cursor as colour nodes, the node form of
/// [`super::DrawState::draw_cursor`]: the shape in the terminal's default
/// foreground, and under a block cursor the cell's glyph redrawn in the
/// terminal's default background through the same text emitter. `text` is
/// the glyph the cell pass collected under the cursor (empty when the cursor
/// sits on an empty or skipped cell).
pub(crate) fn emit_cursor(
    snapshot: &gtk4::Snapshot,
    state: &DrawState,
    terminal: &crate::term::Terminal,
    cursor: &Cursor,
    text: &[u8],
) {
    use gtk4::graphene;
    let colors = terminal.colors();
    let colour = rgba(&colors.foreground);
    let metrics = state.cell_metrics;

    fn band(
        snapshot: &gtk4::Snapshot,
        colour: &gtk4::gdk::RGBA,
        (x, y, w, h): (f64, f64, f64, f64),
    ) {
        snapshot.append_color(
            colour,
            &graphene::Rect::new(x as f32, y as f32, w as f32, h as f32),
        );
    }

    match cursor_shape(cursor, &metrics) {
        CursorShape::Fill(rect) => band(snapshot, &colour, rect),
        // The stroke rectangle covers four 1 px bands around its outer edge
        // — the path sits 0.5 px inside the bands — so fill those bands from
        // the stroke's outer bounds instead of stroking.
        CursorShape::Hollow((rx, ry, rw, rh)) => {
            let (x, y, w, h) = (rx - 0.5, ry - 0.5, rw + 1.0, rh + 1.0);
            band(snapshot, &colour, (x, y, w, 1.0));
            band(snapshot, &colour, (x, y + h - 1.0, w, 1.0));
            band(snapshot, &colour, (x, y + 1.0, 1.0, h - 2.0));
            band(snapshot, &colour, (x + w - 1.0, y + 1.0, 1.0, h - 2.0));
        }
    }

    if cursor.style == CursorStyle::Block && !text.is_empty() && !cursor.wide_tail {
        let Some(context) = state.pango_context.clone() else {
            return;
        };
        // The same hand-built cell draw_cursor draws the glyph with: the
        // cursor's position and text, default style bits.
        let mut cell = Cell::default();
        let len = text.len().min(crate::term::cells::CELL_TEXT_CAP - 1);
        cell.x = cursor.x;
        cell.y = cursor.y;
        cell.text[..len].copy_from_slice(&text[..len]);
        cell.len = len;
        let layout = pango::Layout::new(&context);
        emit_cell_text(
            snapshot,
            &layout,
            &cell,
            &state.fonts_ref(),
            &metrics,
            &colors.background,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::parity;
    use crate::render::tests::state;

    /// The cairo oracle for one cursor: [`DrawState::draw_cursor`] into a
    /// backed surface at `scale`. The terminal's frame must be open so
    /// [`crate::term::Terminal::colors`] has the frame's colours.
    fn cairo_cursor(
        state: &mut DrawState,
        terminal: &crate::term::Terminal,
        cursor: &Cursor,
        text: &[u8],
        scale: f64,
    ) -> cairo::ImageSurface {
        let surface = parity::backed_surface(64, 64, scale, parity::BACKDROP);
        {
            let cr = cairo::Context::new(&surface).expect("context");
            state.draw_cursor(&cr, terminal, cursor, text);
        }
        surface
    }

    /// The node side for one cursor: [`emit_cursor`] into a snapshot, drawn
    /// to a backed surface at `scale`.
    fn node_cursor_surface(
        state: &DrawState,
        terminal: &crate::term::Terminal,
        cursor: &Cursor,
        text: &[u8],
        scale: f64,
    ) -> cairo::ImageSurface {
        let snapshot = parity::snapshot();
        emit_cursor(&snapshot, state, terminal, cursor, text);
        let node = snapshot.to_node().expect("snapshot produced a node");
        let surface = parity::backed_surface(64, 64, scale, parity::BACKDROP);
        parity::draw_node(&node, &surface);
        surface
    }

    /// The block cursor redraws the glyph under it, so the direct tests give
    /// it one: a terminal whose first cell holds `X`.
    fn terminal_with_x() -> crate::term::Terminal {
        let mut terminal = crate::render::tests::terminal();
        terminal.push_pty_data(b"X");
        assert!(terminal.frame_begin(), "frame began");
        terminal
    }

    /// A cursor of `style` over the `X` cell, wide-tail flagged on demand.
    fn cursor(style: CursorStyle, wide_tail: bool) -> Cursor {
        Cursor {
            has_value: true,
            x: 0,
            y: 0,
            style,
            wide_tail,
        }
    }

    /// All four styles (plus the wide-tail block, whose text draw_cursor
    /// suppresses) match the cairo painter exactly at scale 1. BlockHollow
    /// is not reachable through DECSCUSR — ghostty reports it only for its
    /// own unfocused rendering — so the direct test is how hollow gets its
    /// parity proof; the full-frame tests below cover the DECSCUSR styles.
    #[test]
    fn every_cursor_style_matches_the_cairo_painter_at_scale_1() {
        let _font = crate::render::font_lock::guard();
        let mut draw_state = state();
        let terminal = terminal_with_x();
        let cases: [(CursorStyle, bool); 5] = [
            (CursorStyle::Bar, false),
            (CursorStyle::Underline, false),
            (CursorStyle::BlockHollow, false),
            (CursorStyle::Block, false),
            (CursorStyle::Block, true),
        ];
        for (style, wide_tail) in cases {
            let cursor = cursor(style, wide_tail);
            let text: &[u8] = if style == CursorStyle::Block && !wide_tail {
                b"X"
            } else {
                &[]
            };
            let mut cairo_side = cairo_cursor(&mut draw_state, &terminal, &cursor, text, 1.0);
            let mut node_side = node_cursor_surface(&draw_state, &terminal, &cursor, text, 1.0);
            parity::assert_exact(
                &mut cairo_side,
                &mut node_side,
                &format!("cursor {style:?} wide_tail {wide_tail} at scale 1"),
            );
        }
    }

    /// At scale 1.5 the two primitives could blend their fractional edges
    /// differently; the stated tolerance is the same blend bound as
    /// everywhere else — half the channel distance between the drawn colour
    /// and the backdrop, rounded up.
    #[test]
    fn every_cursor_style_matches_the_cairo_painter_within_tolerance_at_scale_1_5() {
        let _font = crate::render::font_lock::guard();
        let mut draw_state = state();
        let terminal = terminal_with_x();
        for style in [
            CursorStyle::Bar,
            CursorStyle::Underline,
            CursorStyle::BlockHollow,
            CursorStyle::Block,
        ] {
            let cursor = cursor(style, false);
            let text: &[u8] = if style == CursorStyle::Block {
                b"X"
            } else {
                &[]
            };
            let mut cairo_side = cairo_cursor(&mut draw_state, &terminal, &cursor, text, 1.5);
            let mut node_side = node_cursor_surface(&draw_state, &terminal, &cursor, text, 1.5);
            parity::assert_within(
                &mut cairo_side,
                &mut node_side,
                parity::BLEND_TOLERANCE,
                &format!("cursor {style:?} at scale 1.5"),
            );
        }
    }

    /// Full-frame parity per DECSCUSR-reachable style: the cursor rides the
    /// cell pass in both painters, including the block cursor's redrawn
    /// glyph (the cursor is moved back over the `a` the cell pass already
    /// drew). `BlockHollow` has no DECSCUSR — the direct tests above are its
    /// parity proof.
    #[test]
    fn frame_cursor_styles_match_the_cairo_painter_exactly_at_scale_1() {
        let cases = [
            ("\x1b[2 q", CursorStyle::Block),
            ("\x1b[4 q", CursorStyle::Underline),
            ("\x1b[6 q", CursorStyle::Bar),
        ];
        let _font = crate::render::font_lock::guard();
        for (sequence, style) in cases {
            let data = format!("{sequence}ab\x1b[1;1H");
            let mut draw_state = state();
            let mut cairo_terminal = parity::terminal_raw(data.as_bytes());
            let mut node_terminal = parity::terminal_raw(data.as_bytes());
            assert!(cairo_terminal.frame_begin(), "frame began");
            assert_eq!(
                cairo_terminal.cursor().expect("a visible cursor").style,
                style,
                "DECSCUSR {sequence:?} must produce {style:?}"
            );
            cairo_terminal.frame_end();
            let mut cairo_side =
                parity::cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.0);
            let mut node_side =
                parity::node_frame(&draw_state, &mut node_terminal, 65, 71, 1.0, true);
            parity::assert_exact(
                &mut cairo_side,
                &mut node_side,
                &format!("frame cursor {style:?} at scale 1"),
            );
        }
    }

    /// Full-frame parity with an underline cell, a strikethrough cell and a
    /// cell with both: the decorations ride the cell pass in both painters.
    #[test]
    fn decorations_match_the_cairo_painter_exactly_at_scale_1() {
        let _font = crate::render::font_lock::guard();
        let data: &[u8] =
            b"\x1b[?25l\x1b[4munder\x1b[24m \x1b[9mstrike\x1b[29m \x1b[4;9mboth\x1b[0m";
        let mut draw_state = state();
        let mut cairo_terminal = parity::terminal_with(data);
        let mut node_terminal = parity::terminal_with(data);
        let mut cairo_side = parity::cairo_frame(&mut draw_state, &mut cairo_terminal, 65, 71, 1.0);
        let mut node_side = parity::node_frame(&draw_state, &mut node_terminal, 65, 71, 1.0, true);
        parity::assert_exact(&mut cairo_side, &mut node_side, "decorations at scale 1");
    }
}
