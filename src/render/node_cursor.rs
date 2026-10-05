//! The node emitter's decoration and cursor passes (gsk-render-nodes task
//! 2.3): the node forms of `render_grid`'s underline/strikethrough strokes
//! and [`super::DrawState::draw_cursor`]. Both shapes are 1–2 px bands, so
//! they go out as colour nodes; the geometry is shared with the cairo
//! painter ([`decoration_rects`], [`cursor_shape`]) so the two cannot drift.
//!
//! The hollow block cursor is the four 1 px bands of its outline on both
//! sides (`snap-grid-edges` D6): the cairo painter used to stroke a centre
//! rectangle, which is geometrically those bands; the shared function now
//! hands both painters the bands themselves, snapped like every other
//! rectangle.
use gtk4::prelude::SnapshotExt as _;

use super::DrawState;
use super::metrics::CellMetrics;
use super::nodes::{emit_cell_text, rgba};
use super::snap::OutputScale;
use crate::term::cells::{Cell, Cursor, CursorStyle, StyleFlags};

/// The underline band `cell`'s UNDERLINE flag asks for, as the rectangle
/// both painters fill, snapped to the device pixel grid (`snap-grid-edges`
/// D1, D4). `None` when the flag is unset; the split (instead of a shared
/// list) keeps the per-cell pass allocation-free.
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
/// the cell's mid-line, snapped like the underline. `None` when the flag is
/// unset.
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
/// cell's top, spanning the cell's width, snapped to the device pixel grid
/// (`snap-grid-edges` D1, D4).
fn underline_strikethrough_rect(
    cell: &Cell,
    cell_metrics: &CellMetrics,
    top: f64,
) -> (f64, f64, f64, f64) {
    cell_metrics.scale.snap_rect(
        f64::from(cell.x) * f64::from(cell_metrics.cell_w),
        f64::from(cell.y) * f64::from(cell_metrics.cell_h) + top,
        f64::from(cell_metrics.cell_w),
        1.0,
    )
}

/// The cursor's shape as the cairo painter and the node emitter draw it
/// (gsk-render-nodes task 2.3). `Fill` is one rectangle to fill; `Hollow`
/// is the hollow block cursor's outline as the four 1 px bands its former
/// 1 px stroke covered (`snap-grid-edges` D6) — both painters fill them.
pub(crate) enum CursorShape {
    Fill((f64, f64, f64, f64)),
    Hollow([(f64, f64, f64, f64); 4]),
}

/// The geometry [`super::DrawState::draw_cursor`] paints `cursor` with,
/// snapped to the device pixel grid (`snap-grid-edges` D1, D4).
pub(crate) fn cursor_shape(cursor: &Cursor, cell_metrics: &CellMetrics) -> CursorShape {
    let scale = cell_metrics.scale;
    let x = f64::from(cursor.x) * f64::from(cell_metrics.cell_w);
    let y = f64::from(cursor.y) * f64::from(cell_metrics.cell_h);
    let cw = f64::from(cell_metrics.cell_w);
    let ch = f64::from(cell_metrics.cell_h);
    match cursor.style {
        CursorStyle::Bar => CursorShape::Fill(scale.snap_rect(x, y, 2.0, ch)),
        CursorStyle::Underline => CursorShape::Fill(scale.snap_rect(x, y + ch - 2.0, cw, 2.0)),
        CursorStyle::BlockHollow => CursorShape::Hollow(hollow_bands(scale, x, y, cw, ch)),
        CursorStyle::Block => CursorShape::Fill(scale.snap_rect(x, y, cw, ch)),
    }
}

/// The hollow block cursor's outline as the four 1 px bands a 1 px stroke
/// over the rectangle inset by half a pixel covered: top, bottom, left,
/// right, each snapped on its own (`snap-grid-edges` D4, D6). Bands that
/// share an edge pass the same value to the snap — the vertical bands'
/// outer edges are the horizontal bands' ends — so they tile the ring
/// without a gap or an overlap.
fn hollow_bands(scale: OutputScale, x: f64, y: f64, cw: f64, ch: f64) -> [(f64, f64, f64, f64); 4] {
    let snap = |(x, y, w, h): (f64, f64, f64, f64)| scale.snap_rect(x, y, w, h);
    [
        snap((x, y, cw, 1.0)),
        snap((x, y + ch - 1.0, cw, 1.0)),
        snap((x, y + 1.0, 1.0, ch - 2.0)),
        snap((x + cw - 1.0, y + 1.0, 1.0, ch - 2.0)),
    ]
}

/// Emit `cell`'s underline/strikethrough as colour nodes, the node form of
/// `render_grid`'s decoration fills (`snap-grid-edges` D6).
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
        // Approved per-instance (#13): GSK/graphene take f32;
        // screen-bounded pixels.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "approved #13: GSK/graphene take f32, screen-bounded pixels"
        )]
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

    fn band(
        snapshot: &gtk4::Snapshot,
        colour: &gtk4::gdk::RGBA,
        (x, y, w, h): (f64, f64, f64, f64),
    ) {
        // Approved per-instance (#13): GSK/graphene take f32;
        // screen-bounded pixels.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "approved #13: GSK/graphene take f32, screen-bounded pixels"
        )]
        snapshot.append_color(
            colour,
            &graphene::Rect::new(x as f32, y as f32, w as f32, h as f32),
        );
    }

    let colors = terminal.colors();
    let colour = rgba(colors.foreground);
    let metrics = state.cell_metrics;

    match cursor_shape(cursor, &metrics) {
        CursorShape::Fill(rect) => band(snapshot, &colour, rect),
        CursorShape::Hollow(bands) => {
            for rect in bands {
                band(snapshot, &colour, rect);
            }
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
            colors.background,
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
    /// [`crate::term::Terminal::colors`] has the frame's colours. The
    /// drawing scale is recorded on the draw state first (`set_scale`,
    /// `snap-grid-edges` D2), so both painters snap at `scale`.
    fn cairo_cursor(
        state: &mut DrawState,
        terminal: &crate::term::Terminal,
        cursor: &Cursor,
        text: &[u8],
        scale: f64,
    ) -> cairo::ImageSurface {
        state.set_scale(super::super::OutputScale::new(scale));
        let surface = parity::backed_surface(64, 64, scale, parity::BACKDROP);
        let cr = cairo::Context::new(&surface).expect("context");
        state.draw_cursor(&cr, terminal, cursor, text);
        surface
    }

    /// The node side for one cursor: [`emit_cursor`] into a snapshot, drawn
    /// to a backed surface at `scale`. The drawing scale is recorded on the
    /// draw state first (`set_scale`, `snap-grid-edges` D2), so both
    /// painters snap at `scale`.
    fn node_cursor_surface(
        state: &mut DrawState,
        terminal: &crate::term::Terminal,
        cursor: &Cursor,
        text: &[u8],
        scale: f64,
    ) -> cairo::ImageSurface {
        state.set_scale(super::super::OutputScale::new(scale));
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

    /// All four styles (plus the wide-tail block, whose text `draw_cursor`
    /// suppresses) match the cairo painter exactly at scale 1. `BlockHollow`
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
            let mut node_side = node_cursor_surface(&mut draw_state, &terminal, &cursor, text, 1.0);
            parity::assert_exact(
                &mut cairo_side,
                &mut node_side,
                &format!("cursor {style:?} wide_tail {wide_tail} at scale 1"),
            );
        }
    }

    /// At scale 1.5 the two painters fill the same snapped geometry: the
    /// shared cursor and band rectangles snap each edge to the device pixel
    /// grid (`snap-grid-edges` D1, D4), and the cairo painter fills them
    /// unantialiased (D6), so the shapes match pixel for pixel. The block
    /// cursor also redraws the glyph under it — that is text, which keeps
    /// its stated blend tolerance (the text parity tests).
    #[test]
    fn every_cursor_style_matches_the_cairo_painter_exactly_at_scale_1_5() {
        let _font = crate::render::font_lock::guard();
        let mut draw_state = state();
        let terminal = terminal_with_x();
        for style in [
            CursorStyle::Bar,
            CursorStyle::Underline,
            CursorStyle::BlockHollow,
        ] {
            let cursor = cursor(style, false);
            let mut cairo_side = cairo_cursor(&mut draw_state, &terminal, &cursor, &[], 1.5);
            let mut node_side = node_cursor_surface(&mut draw_state, &terminal, &cursor, &[], 1.5);
            parity::assert_exact(
                &mut cairo_side,
                &mut node_side,
                &format!("cursor {style:?} at scale 1.5"),
            );
        }
    }

    /// The block cursor at scale 1.5: the snapped shape and the redrawn
    /// glyph both match exactly (the glyph's measured text delta on this
    /// suite is 0, like the text parity tests').
    #[test]
    fn the_block_cursor_matches_exactly_at_scale_1_5() {
        let _font = crate::render::font_lock::guard();
        let mut draw_state = state();
        let terminal = terminal_with_x();
        let cursor = cursor(CursorStyle::Block, false);
        let mut cairo_shape = cairo_cursor(&mut draw_state, &terminal, &cursor, &[], 1.5);
        let mut node_shape = node_cursor_surface(&mut draw_state, &terminal, &cursor, &[], 1.5);
        parity::assert_exact(
            &mut cairo_shape,
            &mut node_shape,
            "block cursor shape at scale 1.5",
        );

        let mut cairo_glyph = cairo_cursor(&mut draw_state, &terminal, &cursor, b"X", 1.5);
        let mut node_glyph = node_cursor_surface(&mut draw_state, &terminal, &cursor, b"X", 1.5);
        parity::assert_exact(
            &mut cairo_glyph,
            &mut node_glyph,
            "block cursor glyph at scale 1.5",
        );
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
                parity::node_frame(&mut draw_state, &mut node_terminal, 65, 71, 1.0, true);
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
        let mut node_side =
            parity::node_frame(&mut draw_state, &mut node_terminal, 65, 71, 1.0, true);
        parity::assert_exact(&mut cairo_side, &mut node_side, "decorations at scale 1");
    }
}
