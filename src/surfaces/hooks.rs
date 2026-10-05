//! The surfaces' hook slots (port-to-rust D3): the render- and pty-side pieces
//! `glue.c` called into, filled by rows 3.6 and 4.1. Split from
//! [`super::Surfaces`] so the surfaces module stays under the file-line cap;
//! the hooks travel with the surfaces handle, which owns them.

use std::rc::Rc;

/// A drawing-area draw function body (`render.c`'s `on_draw`).
pub type DrawFn = dyn Fn(&cairo::Context, i32, i32);

/// The grid frame's GSK snapshot emission (poc-gsk-texture-grid task 2.1,
/// gsk-render-nodes row 4.1): emit the frame — the theme background, the
/// grid (the retained node translated while a tween runs, rebuilt on every
/// non-tween draw) and the focus accent — into `snapshot` and report `true`;
/// `false` falls back to the ordinary cairo draw path. The extents are the
/// drawing area's width and height. The hook body reads the tween state off
/// the surfaces handle, builds or reuses the grid node on the renderer, and
/// draws nothing when the panel's D5 latch is set.
pub type GridSnapshotFn = dyn Fn(&gtk4::Snapshot, i32, i32) -> bool;

/// A cell measurement against a widget (`render.c`'s
/// `cell_metrics_update`), reporting the cell size in pixels.
pub type MeasureFn = dyn Fn(&gtk4::Widget) -> (i32, i32);

/// The hooks rows 3.6 and 4.1 fill in. Every hook is an `Rc` closure so the
/// surfaces can call it from `&self`; hook bodies must not call back into
/// [`super::Surfaces`] methods synchronously, since those share the same glue state.
pub struct SurfaceHooks {
    /// `render.c`'s `on_draw` (row 3.6): the drawing area's draw function,
    /// receiving the cairo context and the pixel extents.
    pub draw: Rc<DrawFn>,
    /// `pty.c`'s `apply_size` (rows 3.6/4.1): recompute the grid from the
    /// drawing area's allocation and push it through the terminal and the pty.
    /// `false` reports a terminal that could not be allocated; the previous
    /// grid stays (`glue_publish_layout`'s `GLUE_ERR_TERMINAL` path).
    pub apply_size: Rc<dyn Fn() -> bool>,
    /// `render.c`'s `cell_metrics_update` (rows 3.6/4.1): measure the font on
    /// the window and report the cell size in pixels.
    pub measure: Rc<MeasureFn>,
    /// `render.c`'s `render_grid_cache_drop` (row 3.6): drop the tween's
    /// retained grid node (gsk-render-nodes row 4.1) so the next tween
    /// rebuilds it.
    pub tween_cache_drop: Rc<dyn Fn()>,
    /// The grid frame's GSK snapshot emission (poc-gsk-texture-grid task
    /// 2.1, gsk-render-nodes row 4.1), called by the drawing area subclass's
    /// `snapshot` override before it chains to the cairo draw path.
    pub grid_snapshot: Rc<GridSnapshotFn>,
    /// `pinwin_api.c`'s start handshake (row 4.1): `true` once the panel is on
    /// screen with live metrics, `false` when the GTK side failed. Only the
    /// first result counts.
    pub start_result: Rc<dyn Fn(bool)>,
    /// The pty tween-flag relay (row 4.1): `Pty::set_tween_active`, called on
    /// every tween start and stop so the pty read drain bounds itself.
    pub set_tween_active: Rc<dyn Fn(bool)>,
    /// The terminal's live grid width in pixels: its current cols times the
    /// cell width. While a width tween runs the grid resize is deferred to the
    /// tween's end (`publish`'s `deferred_grid`), so the live grid lags the
    /// applied [`super::Surfaces::cols`] — and the draw shift that keeps the grid
    /// against the docked edge must be computed against the grid that is
    /// actually drawn. A shift computed from the applied cols walks the old
    /// grid off the docked edge and draws nothing but background for the whole
    /// tween.
    pub live_grid_px: Rc<dyn Fn() -> i32>,
}

impl std::fmt::Debug for SurfaceHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The slots are hook closures with no `Debug` impl; the shape (one
        // slot per row) is the stable contract, not the bodies.
        f.debug_struct("SurfaceHooks").finish_non_exhaustive()
    }
}
