//! The cell-background layer of the grid painter (row 4.3): the frame's
//! cell backgrounds, walked once and merged into horizontal runs of equal
//! colour. The canvas is already the theme background — the painter filled
//! it before the frame pass — so only cells with an explicit background
//! draw, exactly the cells [`crate::render::paint_backgrounds`] fills on
//! the GTK path.
//!
//! Each run fills as one rectangle through
//! [`PainterMetrics::run_rect`], so the cells inside it share one snapped
//! geometry and cannot develop seams, and two runs of different colour
//! that touch share the snapped edge — the same property the per-cell
//! [`crate::render::nodes::cell_background_rect`] rectangles get from the
//! shared edge values, now with fewer fills. The frame's last row fills
//! down to the frame's logical height, which need not land on a cell
//! boundary, as [`crate::render::nodes::cell_background_rect`] does.
//!
//! The frame's tween draw offset translates the grid: every run shifts by
//! the offset's device pixels, and the canvas clips what moves past the
//! edges, as the cairo path's translate does.
//!
//! GTK-free (`replace-gtk-with-wayland` D10); private to the painter, whose
//! frame pass calls it first, straight onto the theme background.

use super::canvas::{Canvas, CanvasColor};
use super::geom::{DeviceRect, FrameInput, PainterMetrics};
use crate::term::Terminal;
use crate::term::cells::Rgb;

/// One background run: `len` cells of one colour starting at `(col, row)`.
struct Run {
    row: i32,
    col: i32,
    len: i32,
    color: Rgb,
}

/// Walk the open frame's cells and fill the background runs, shifted by
/// `offset` device pixels along x. The frame must be open
/// ([`Terminal::frame_begin`]); the caller rewinds the frame afterwards.
pub(super) fn paint(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    terminal: &mut Terminal,
    offset: i32,
) {
    let mut run: Option<Run> = None;
    while let Some(cell) = terminal.cell_next() {
        // A run extends cell by cell while the colours match and the walk
        // stays contiguous; the frame's cells arrive in row-major order, so
        // contiguity is `col + len == cell.x`.
        let extends = cell.has_bg
            && run.as_ref().is_some_and(|active| {
                active.row == cell.y && active.col + active.len == cell.x && active.color == cell.bg
            });
        if extends {
            let active = run.as_mut().expect("extends checked the run exists");
            active.len += 1;
            continue;
        }
        flush(run.take(), canvas, metrics, frame, offset);
        if cell.has_bg {
            run = Some(Run {
                row: cell.y,
                col: cell.x,
                len: 1,
                color: cell.bg,
            });
        }
    }
    flush(run.take(), canvas, metrics, frame, offset);
}

/// Fill one finished run, shifted by `offset` device pixels along x.
fn flush(
    run: Option<Run>,
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    offset: i32,
) {
    let Some(run) = run else {
        return;
    };
    let rect = run_rect(metrics, frame, run.row, run.col, run.len);
    canvas.fill_rect(
        rect.x() + offset,
        rect.y(),
        rect.w(),
        rect.h(),
        CanvasColor::from_theme(run.color),
    );
}

/// The snapped device rectangle of `len` cells in one row: one rectangle
/// for the whole run. The frame's last row — the one row whose cell pitch
/// reaches the frame's logical height — fills down to that height, which
/// need not land on a cell boundary.
fn run_rect(
    metrics: &PainterMetrics,
    frame: &FrameInput,
    row: i32,
    col: i32,
    len: i32,
) -> DeviceRect {
    let cell_h = metrics.cell_h();
    let logical_height = f64::from(frame.logical_size().1);
    let row_top = f64::from(row) * cell_h;
    // `row` is the frame's last row when its pitch reaches the logical
    // height but the next one would overshoot it — the integer division
    // `logical_height / cell_h - 1` the GTK background pass compares.
    let is_last = f64::from(row) + 1.0 <= logical_height / cell_h
        && logical_height / cell_h < f64::from(row) + 2.0;
    if is_last {
        let height = (logical_height - row_top).max(0.0);
        metrics.cell_sub_rect(
            col,
            row,
            0.0,
            0.0,
            f64::from(len) * metrics.cell_w(),
            height,
        )
    } else {
        metrics.run_rect(col, row, len)
    }
}
