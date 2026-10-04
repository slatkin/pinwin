//! The GSK node emitter for the terminal grid (gsk-render-nodes): a second
//! painter beside the cairo [`render_grid`](super::DrawState::render_grid),
//! which stays the fallback and the parity oracle. The emitter walks the
//! same cell iteration into a [`gtk4::Snapshot`] and emits rectangles as
//! colour nodes, in logical coordinates — GSK applies the device scale,
//! unlike the cairo path, which snaps with `Antialias::None` on
//! device-scaled surfaces.
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
use crate::term::Terminal;
use crate::term::cells::{Cell, Rgb};

/// An [`Rgb`] as a GDK colour. GDK stores the channels as `f32`; the cairo
/// path divides in `f64`. Both quantize to the same 16-bit colour when
/// drawn, which is what the scale-1 parity test asserts.
pub(super) fn rgba(color: &Rgb) -> gdk::RGBA {
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
/// which need not be a multiple of the cell pitch.
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
    Some((
        f64::from(cell.x) * f64::from(metrics.cell_w),
        f64::from(cell.y) * f64::from(metrics.cell_h),
        f64::from(metrics.cell_w),
        f64::from(row_height),
    ))
}

impl DrawState {
    /// Emit the theme background and the cell backgrounds — including the
    /// last row's fill — as colour nodes into `snapshot` (gsk-render-nodes
    /// task 1.2). `width`/`height` are the widget's logical size, the same
    /// coordinates [`DrawState::draw`] paints: the theme background covers
    /// the whole widget and `height` is the grid-relative height the last
    /// row fills to.
    ///
    /// Reports whether nodes were emitted. `false` — no live frame — asks
    /// the caller to fall back to the cairo draw path, which paints the same
    /// thing; a draw before the first `cell_metrics_update` has no grid pass
    /// (render P2 in `draw_inner`), so it emits the theme background only
    /// and still reports `true`.
    pub fn emit_backgrounds(
        &self,
        snapshot: &gtk4::Snapshot,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
    ) -> bool {
        snapshot.append_color(
            &rgba(&self.theme_background),
            &graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
        );

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
                    &rgba(&cell.bg),
                    &graphene::Rect::new(x as f32, y as f32, w as f32, h as f32),
                );
            }
        }
        terminal.frame_end();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::{set_rgb, tests::state, tests::terminal};
    use super::*;

    use crate::render::font_lock;
    use crate::render::parity;

    /// The opaque backing both parity surfaces get, so a fractional edge
    /// blends against the same colour on both painters (the real panel draws
    /// over an opaque window).
    const BACKDROP: [u8; 3] = [40, 40, 40];

    /// The cairo oracle: `draw_inner`'s theme fill plus `render_grid`'s
    /// background pass, into a backed surface at `scale`.
    fn cairo_backgrounds(
        draw_state: &DrawState,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        scale: f64,
    ) -> cairo::ImageSurface {
        let surface = parity::backed_surface(width, height, scale, BACKDROP);
        {
            let cr = cairo::Context::new(&surface).expect("context");
            set_rgb(&cr, &draw_state.theme_background);
            cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
            let _ = cr.fill();
            assert!(terminal.frame_begin(), "frame began");
            super::super::paint_backgrounds(&cr, terminal, draw_state.cell_metrics, height);
            terminal.frame_end();
        }
        surface
    }

    /// The node side: [`DrawState::emit_backgrounds`] into a snapshot, drawn
    /// to a backed surface at `scale`.
    fn node_backgrounds(
        draw_state: &DrawState,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        scale: f64,
    ) -> cairo::ImageSurface {
        let snapshot = parity::snapshot();
        assert!(
            draw_state.emit_backgrounds(&snapshot, terminal, width, height),
            "nodes were emitted"
        );
        let node = snapshot.to_node().expect("snapshot produced a node");
        let surface = parity::backed_surface(width, height, scale, BACKDROP);
        parity::draw_node(&node, &surface);
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

    /// The theme background differs from every cell background by more than
    /// half a channel, so a missing cell cannot hide inside the scale-1.5
    /// tolerance below.
    #[test]
    fn backgrounds_match_the_cairo_painter_exactly_at_scale_1() {
        let _font = font_lock::guard();
        let draw_state = state();
        let mut terminal = terminal_with_backgrounds();
        let (width, height) = (65, 71);
        let mut cairo_side = cairo_backgrounds(&draw_state, &mut terminal, width, height, 1.0);
        let mut node_side = node_backgrounds(&draw_state, &mut terminal, width, height, 1.0);
        parity::assert_exact(&mut cairo_side, &mut node_side, "backgrounds at scale 1");
    }

    /// At scale 1.5 fractional cell edges land between device pixels: the
    /// cairo path snaps them (`Antialias::None`), the node path blends. The
    /// stated tolerance is the blend bound — half the channel distance
    /// between the colours an edge separates, rounded up.
    #[test]
    fn backgrounds_match_the_cairo_painter_within_tolerance_at_scale_1_5() {
        let _font = font_lock::guard();
        let draw_state = state();
        let mut terminal = terminal_with_backgrounds();
        let (width, height) = (65, 71);
        let mut cairo_side = cairo_backgrounds(&draw_state, &mut terminal, width, height, 1.5);
        let mut node_side = node_backgrounds(&draw_state, &mut terminal, width, height, 1.5);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            128,
            "backgrounds at scale 1.5",
        );
    }
}
