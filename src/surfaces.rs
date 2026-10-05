//! The layer-shell surfaces (port-to-rust D3): the visible panel plus its
//! transparent reservation, the layout application and publishing. Ported from
//! `src/glue.c`; the lifecycle half (the process-lifetime GTK thread, the
//! start handshake and the `Panel` handle) lives in [`crate::panel`], which
//! drives [`init`] and [`Surfaces::build`] per start.
//!
//! Layout types come from [`crate::layout`]; a monitor with degenerate metrics
//! maps to the invalid-layout verdict (port-to-rust D6), as `glue.c`
//! `PINWIN_GEOM_ERR_METRICS` did. The panel state lives on the GTK thread (D4);
//! mutable fields are `Cell`s so the glue hooks the closures drive never hold
//! conflicting borrows.
//!
//! The render- and pty-side pieces `glue.c` called into are hook slots
//! ([`SurfaceHooks`]) documented for rows 3.6 and 4.1 to fill: the drawing
//! area's draw function, the grid resize behind `apply_size`, the cell
//! measurement, the tween node-cache drop, the start handshake and the pty
//! tween-flag relay (`Pty::set_tween_active`).
//!
//! Panics must never cross back into GTK/glib (D5): every closure registered
//! here runs its body through the shared [`crate::guard`] helper, latching a
//! poisoned flag.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::guard::{Poisoned, guard};

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4_layer_shell as layer_shell;
use gtk4_layer_shell::LayerShell as _;
use gtk4_layer_shell::{Edge, KeyboardMode as LayerKeyboardMode, Layer};

use crate::anim::{Anim, AnimHooks};
use crate::input;
use crate::layout::{Accent, Keyboard, Layout, Side};
use crate::render::OutputScale;

mod area;
mod gap;

pub use area::GridArea;

use gap::{HeldGap, gap_tween_decision, reserve_gap, staged_publish, start_held_gap};

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
/// [`Surfaces`] methods synchronously, since those share the same glue state.
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
    /// applied [`Surfaces::cols`] — and the draw shift that keeps the grid
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

/// The outcome of publishing a layout (`glue_publish_layout`'s return codes).
/// The `panel` row (4.1) maps these onto `PinwinError`: `NotLive` is
/// `NotRunning`, `InvalidLayout` is `InvalidLayout` and `Terminal` is
/// `Internal` (`pinwin_api.c`'s `apply_on_gtk_thread`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    /// The layout is applied and the grid follows (`PINWIN_GEOM_OK`).
    Applied,
    /// The panel has no live metrics yet, or is already torn down
    /// (`GLUE_NOT_LIVE`).
    NotLive,
    /// The layout the live monitor refuses (`PINWIN_GEOM_ERR_METRICS`).
    InvalidLayout,
    /// The layout published but the terminal grid could not be allocated; the
    /// previous grid stays (`GLUE_ERR_TERMINAL`).
    Terminal,
}

/// Why [`init`] failed (`glue_init`'s zero return).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitFailure {
    /// GTK could not initialise (no display, or a second thread after a
    /// restart — the parked-thread model keeps that from happening, D4).
    GtkInit,
    /// The session's compositor lacks wlr-layer-shell (an X11 session, or
    /// GNOME): the spec's "Wayland layer-shell is required" case.
    LayerShell,
}

/// Whether a layout apply animates (`glue_publish_layout`'s decision): a
/// layout differing only in its column count and/or push/cover choice animates
/// — the side and the left and right gutters move the reservation's side, so a
/// layout touching them snaps. A covering-only change animates so a pushing
/// retarget at the same width can ease its gap back (overlay-expand D3). The
/// duration clamp to 1000 ms happens upstream (`pinwin_api.c`), as in the C.
fn should_animate(
    duration_ms: u32,
    animations_enabled: bool,
    applied: &Layout,
    requested: &Layout,
) -> bool {
    duration_ms > 0
        && animations_enabled
        && requested.side() == applied.side()
        && requested.left() == applied.left()
        && requested.right() == applied.right()
}

/// Validate a layout against live metrics (`pinwin_layout_validate` plus the
/// degenerate-metrics guard): non-positive cell or output metrics are the one
/// former `PINWIN_GEOM_ERR_METRICS` case the layout types cannot absorb (D6),
/// and any verdict maps to the invalid-layout outcome.
fn metrics_valid(layout: Layout, cell_w: i32, cell_h: i32, output_w: i32, output_h: i32) -> bool {
    match (
        crate::layout::CellSize::new(cell_w, cell_h),
        crate::layout::OutputSize::new(output_w, output_h),
    ) {
        (Some(cell), Some(output)) => layout.validate(cell, output).is_ok(),
        // Degenerate monitor metrics: never valid (D6).
        _ => false,
    }
}

/// Map the layout's keyboard mode onto layer-shell's.
fn keyboard_mode(keyboard: Keyboard) -> LayerKeyboardMode {
    match keyboard {
        Keyboard::None => LayerKeyboardMode::None,
        Keyboard::OnDemand => LayerKeyboardMode::OnDemand,
        Keyboard::Exclusive => LayerKeyboardMode::Exclusive,
    }
}

/// GTK and layer-shell initialisation (`glue_init` minus the state it stored
/// into globals: the startup layout, keyboard mode and accent go to
/// [`Surfaces::build`], and the theme colours are render state, row 3.6).
/// Run on the GTK thread before any surface exists. `app_id` is the per-start
/// application id the panel hands out (D4: every start runs its own
/// `GtkApplication` with a distinct id); `NON_UNIQUE` keeps the id off the
/// session bus so consecutive panels cannot collide.
pub fn init(app_id: Option<&str>) -> Result<gtk4::Application, InitFailure> {
    gtk4::init().map_err(|_| InitFailure::GtkInit)?;
    if !layer_shell::is_supported() {
        return Err(InitFailure::LayerShell);
    }
    Ok(gtk4::Application::new(
        app_id,
        gtk4::gio::ApplicationFlags::NON_UNIQUE,
    ))
}

/// The layer-shell surfaces and the applied layout state for one panel. Lives
/// on the GTK thread (D4); every method takes `&self`, mutating through the
/// `Cell` fields so the hook closures never fight a borrow.
pub struct Surfaces {
    /// The per-start application the surfaces belong to.
    pub app: gtk4::Application,
    /// The visible panel surface.
    pub win: gtk4::ApplicationWindow,
    /// The terminal drawing area, a subclass whose `snapshot` presents every
    /// frame as GSK nodes (poc-gsk-texture-grid 2.1, gsk-render-nodes 4.1).
    pub area: GridArea,
    /// The transparent reservation, created and presented only after the
    /// visible panel maps (`g_reserve`).
    reserve: RefCell<Option<gtk4::ApplicationWindow>>,
    /// The visible panel's original monitor, resolved on the first draw
    /// (`g_monitor`).
    monitor: RefCell<Option<gdk::Monitor>>,
    /// The applied layout (`g_layout`).
    layout: Cell<Layout>,
    /// The applied column count (`g_cols`).
    cols: Cell<u16>,
    /// The gap the reserve surface draws (overlay-expand D2): the last
    /// pushing layout's side and strip, updated only by validated publishes;
    /// a covering start holds a zero strip (D5).
    held_gap: Cell<HeldGap>,
    /// Set by a publish whose animated target pushes with a strip differing
    /// from the one the gap rests at (overlay-expand D3): with the tween
    /// live, [`reserve_gap`] moves the gap with the panel. Read only with
    /// [`Anim::active`], so a stopped tween falls back to the held strip.
    gap_tweening: Cell<bool>,
    /// The focus accent from the startup (`g_accent`); render consumes it.
    pub accent: Option<Accent>,
    /// The cell metrics from the last measurement (`g_cell_w`/`g_cell_h`).
    cell_w: Cell<i32>,
    cell_h: Cell<i32>,
    /// First-draw monitor resolution pending (`g_layout_latch`): render's
    /// first-draw path calls [`Surfaces::resolve_monitor`] while this is set.
    pub latch: Cell<bool>,
    /// Set when an animated apply left the terminal grid resize to the tween's
    /// end (`deferred_grid`).
    deferred_grid: Cell<bool>,
    /// The pixel width of the terminal grid whose content is still on screen
    /// after a column-widening resize, before the host's first output for the
    /// new width (gsk-render-nodes design, Post-task decisions: C52). libghostty-vt's resize does not
    /// rewrap the active screen, so right after the resize the terminal's own
    /// content sits in the leftmost columns of the new, wider grid — drawing
    /// it unshifted would park it at the widget's left edge, opposite the
    /// docked edge, with an empty band on the docked side until the host
    /// repaints. While this is non-zero the draw shift keys against it, so
    /// the stale content stays glued to the docked edge exactly as the
    /// tween's old grid was; the first terminal output clears it.
    stale_grid_px: Cell<i32>,
    /// Set by [`Surfaces::close`]; the stand-in for the C's `g_win`/`g_area`
    /// NULL checks after teardown.
    closed: Cell<bool>,
    /// The width tween.
    pub anim: Anim,
    /// The hooks rows 3.6 and 4.1 fill in.
    pub hooks: SurfaceHooks,
    /// Latched when a closure registered here panicked (D5).
    poisoned: Poisoned,
}

impl std::fmt::Debug for Surfaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The GTK handles, hooks, tween and image state have no `Debug`
        // impl; the applied layout and its flags are what a test failure
        // needs to identify the panel.
        f.debug_struct("Surfaces")
            .field("layout", &self.layout.get())
            .field("cols", &self.cols.get())
            .field("held_gap", &self.held_gap.get())
            .field("gap_tweening", &self.gap_tweening.get())
            .field("accent", &self.accent)
            .field("cell_w", &self.cell_w.get())
            .field("cell_h", &self.cell_h.get())
            .field("latch", &self.latch.get())
            .field("deferred_grid", &self.deferred_grid.get())
            .field("stale_grid_px", &self.stale_grid_px.get())
            .field("closed", &self.closed.get())
            .field("poisoned", &self.poisoned)
            .finish_non_exhaustive()
    }
}

/// The tween stop's deferred-grid gate ([`Surfaces::fire_deferred_grid_resize`]):
/// apply when an animated apply left the resize pending and the panel is
/// still open. Split out so the gate is unit testable without a display.
/// There is no waiting-for-the-tween case any more: the gate runs at the
/// stop itself, synchronously, so the resized grid is on screen with the
/// tween's final frame (gsk-render-nodes design, Post-task decisions: C52).
fn deferred_grid_at_stop(pending: bool, closed: bool) -> bool {
    pending && !closed
}

/// The width of the grid the next draw shows: the stale pre-resize grid
/// while its content is still on screen after a widening resize, else the
/// terminal's live grid. The draw shift keys against this so the stale
/// content stays glued to the docked edge instead of parking at the widget's
/// left edge opposite the docked side (gsk-render-nodes design, Post-task decisions: C52). Split out so
/// the selection is unit testable without a display.
fn drawn_grid_px(live_grid_px: i32, stale_grid_px: i32) -> i32 {
    if stale_grid_px > 0 {
        stale_grid_px
    } else {
        live_grid_px
    }
}

impl Surfaces {
    /// Build the surfaces for one panel (`on_activate`): create the window and
    /// drawing area, initialise layer-shell, measure the cells, apply the
    /// startup anchors and present the panel. The input controllers attach
    /// separately through [`Surfaces::attach_input`], once the caller has
    /// composed their links; the draw function comes from
    /// [`SurfaceHooks::draw`].
    #[must_use]
    pub fn build(
        app: &gtk4::Application,
        layout: Layout,
        keyboard: Keyboard,
        accent: Option<Accent>,
        hooks: SurfaceHooks,
        poisoned: Poisoned,
    ) -> Rc<Surfaces> {
        let win = gtk4::ApplicationWindow::new(app);
        win.init_layer_shell();
        win.set_namespace(Some("pinwin"));
        win.set_layer(Layer::Overlay);
        win.set_keyboard_mode(keyboard_mode(keyboard));

        let area = GridArea::new(poisoned.clone(), Rc::clone(&hooks.grid_snapshot));
        let draw = Rc::clone(&hooks.draw);
        let draw_poisoned = poisoned.clone();
        area.set_draw_func(move |_area, cr, width, height| {
            let _ = guard(&draw_poisoned, || draw(cr, width, height));
        });
        // pty.c's on_area_resize: every allocation change runs apply_size.
        let resize_hook = Rc::clone(&hooks.apply_size);
        let resize_poisoned = poisoned.clone();
        area.connect_resize(move |_area, _width, _height| {
            let _ = guard(&resize_poisoned, || {
                (resize_hook)();
            });
        });
        area.set_focusable(true);
        win.set_child(Some(&area));

        // cell_metrics_update(win): the cell metrics the default width needs.
        let (cell_w, cell_h) = (hooks.measure)(win.upcast_ref());

        // Width is fixed by COLS; the top/bottom anchors give the height.
        win.set_default_size(i32::from(layout.cols().get()) * cell_w, -1);
        // Ignore Noctalia's top zone.
        win.set_exclusive_zone(-1);

        // The held gap starts at the startup layout's own choice: a pushing
        // start reserves its own strip, a covering start reserves nothing
        // (overlay-expand D5).
        let held_gap = start_held_gap(layout, cell_w);

        let surfaces = Rc::new_cyclic(|weak: &Weak<Surfaces>| {
            let anim = Anim::new(poisoned.clone(), Self::anim_hooks(weak));
            Surfaces {
                app: app.clone(),
                win: win.clone(),
                area,
                reserve: RefCell::new(None),
                monitor: RefCell::new(None),
                layout: Cell::new(layout),
                cols: Cell::new(layout.cols().get()),
                held_gap: Cell::new(held_gap),
                gap_tweening: Cell::new(false),
                accent,
                cell_w: Cell::new(cell_w),
                cell_h: Cell::new(cell_h),
                latch: Cell::new(false),
                deferred_grid: Cell::new(false),
                stale_grid_px: Cell::new(0),
                closed: Cell::new(false),
                anim,
                hooks,
                poisoned,
            }
        });

        // Initial anchors/margins from the startup layout; the map callback
        // resolves the original monitor and presents the reservation once
        // those are known.
        surfaces.apply_layout_surfaces();
        let map_weak = Rc::downgrade(&surfaces);
        let map_poisoned = surfaces.poisoned.clone();
        surfaces.win.connect_map(move |_win| {
            if let Some(surfaces) = map_weak.upgrade() {
                let _ = guard(&map_poisoned, || surfaces.on_map());
            }
        });
        surfaces.win.present();
        (surfaces.hooks.apply_size)();
        surfaces
    }

    /// The tween hooks, wired from a weak self reference so the surfaces and
    /// the anim do not keep each other alive.
    fn anim_hooks(weak: &Weak<Surfaces>) -> AnimHooks {
        let frame = Weak::clone(weak);
        let stop = Weak::clone(weak);
        let finish = Weak::clone(weak);
        AnimHooks {
            on_frame: Box::new(move |px| {
                if let Some(surfaces) = frame.upgrade() {
                    surfaces.apply_frame(px);
                }
            }),
            on_stop: Box::new(move || {
                if let Some(surfaces) = stop.upgrade() {
                    surfaces.on_tween_stopped();
                }
            }),
            on_finish: Box::new(move || {
                if let Some(surfaces) = finish.upgrade() {
                    surfaces.apply_geometry();
                }
            }),
        }
    }

    /// Attach the input controllers (`attach_controllers`). Call once after
    /// [`Surfaces::build`], with the links the panel composed.
    pub fn attach_input(&self, links: &input::InputLinks) {
        input::attach(self.area.upcast_ref(), links);
    }

    /// The applied column count (`g_cols`).
    pub fn cols(&self) -> u16 {
        self.cols.get()
    }

    /// The horizontal cell pitch in pixels from the last measurement
    /// (`g_cell_w`).
    pub fn cell_w(&self) -> i32 {
        self.cell_w.get()
    }

    /// The vertical cell pitch in pixels from the last measurement
    /// (`g_cell_h`).
    pub fn cell_h(&self) -> i32 {
        self.cell_h.get()
    }

    /// The applied width in pixels (`g_cols * g_cell_w`).
    fn grid_px(&self) -> i32 {
        i32::from(self.cols.get()) * self.cell_w.get()
    }

    /// The panel's current pixel width: the animated width while a tween runs,
    /// else the applied width (`panel_px`).
    pub fn panel_px(&self) -> i32 {
        self.anim.current_px(self.grid_px())
    }

    /// The output scale the panel's surface is drawn at (`snap-grid-edges`
    /// D3): `gdk::Surface::scale()`, read fresh at each draw. A missing
    /// surface — before map, or after close — and a value that is not
    /// positive and finite both give 1.
    pub(crate) fn scale(&self) -> OutputScale {
        OutputScale::new(self.win.surface().map_or(1.0, |surface| surface.scale()))
    }

    /// The drawing shift input x coordinates subtract (`glue_anim_draw_offset`).
    /// Computed against the width of the grid that is actually on screen
    /// ([`Self::drawn_grid_px`]): the terminal's live grid, or — while a
    /// resize's stale content is still there — the pre-resize grid the
    /// terminal still shows in its leftmost columns. The shift keeps that
    /// grid glued to the docked edge whenever it does not fill the widget,
    /// tween or not (gsk-render-nodes design, Post-task decisions: C52).
    ///
    /// The integer logical offset snaps once, here, to a whole device pixel
    /// at the surface's current scale (`snap-grid-edges` D7): the draw hooks,
    /// `DrawState::draw`, `DrawState::snapshot_grid` and the input
    /// controllers all use this one snapped value, so the drawn cell and the
    /// cell under the pointer agree and a moving frame cannot put a cell
    /// edge between two device pixels.
    #[must_use]
    pub fn draw_offset(&self) -> f64 {
        self.draw_offset_at(self.scale())
    }

    /// [`Self::draw_offset`] at a scale the caller already read, so a draw
    /// hook resolves the surface scale once.
    pub(crate) fn draw_offset_at(&self, scale: OutputScale) -> f64 {
        let width = if self.closed.get() {
            None
        } else {
            Some(self.area.width())
        };
        scale.snap_edge(f64::from(self.anim.draw_offset(
            self.layout.get().side(),
            width,
            self.drawn_grid_px(),
        )))
    }

    /// The width of the grid the next draw will show: the terminal's live
    /// grid, or — while the stale pre-resize content from a widening resize
    /// is still on screen ([`Self::note_grid_widened`], cleared by
    /// [`Self::note_terminal_output`]) — that narrower grid, whose content
    /// the terminal still holds in its leftmost columns.
    fn drawn_grid_px(&self) -> i32 {
        drawn_grid_px((self.hooks.live_grid_px)(), self.stale_grid_px.get())
    }

    /// Record that a resize widened the terminal grid from `previous_cols_px`:
    /// the content the host drew for the old width is still on screen (the
    /// vt does not rewrap), so draws keep it glued to the docked edge until
    /// the host produces output for the new width (gsk-render-nodes design, Post-task decisions: C52).
    /// The content occupies the narrowest grid since the last host output —
    /// a widening after a widening without any output in between keeps the
    /// narrower of the two stale widths.
    pub(crate) fn note_grid_widened(&self, previous_cols_px: i32) {
        if previous_cols_px <= 0 {
            return;
        }
        let current = self.stale_grid_px.get();
        self.stale_grid_px.set(if current > 0 {
            current.min(previous_cols_px)
        } else {
            previous_cols_px
        });
    }

    /// The terminal produced output — the host's post-resize repaint, or any
    /// other bytes: the drawn grid is the terminal's live one again. A
    /// retained tween node built from the stale grid must not outlive it.
    pub(crate) fn note_terminal_output(&self) {
        if self.stale_grid_px.replace(0) != 0 {
            (self.hooks.tween_cache_drop)();
        }
        self.queue_draw();
    }

    /// Queue a redraw of the drawing area (`glue_queue_draw`).
    pub fn queue_draw(&self) {
        if !self.closed.get() {
            self.area.queue_draw();
        }
    }

    /// Apply the panel width to the drawing area and the window default size
    /// (`apply_panel_width`). GTK4 has no `gtk_window_resize` and
    /// `gtk_window_set_default_size` does not move a mapped window, so the
    /// drawing area's natural width is the mechanism; the default size is a
    /// size floor, so leaving the launch value there would pin the panel at
    /// its launch width once it shrinks.
    fn apply_panel_width(&self, px: i32) {
        if self.closed.get() {
            return;
        }
        self.area.set_size_request(px, -1);
        self.win.set_default_size(px, -1);
    }

    /// The gap the reserve surface draws right now (overlay-expand D2, D3):
    /// the held strip, except while a flagged gap tween runs and the applied
    /// layout pushes, when the strip moves with the panel. One rule for both
    /// the per-frame apply and the tween decision, so they cannot disagree.
    fn drawn_gap(&self, layout: Layout, panel_px: i32) -> (Side, i32) {
        reserve_gap(
            self.held_gap.get(),
            self.gap_tweening.get() && self.anim.active(),
            layout,
            panel_px,
        )
    }

    /// Push the applied layout onto both surfaces in one main-loop turn
    /// (`apply_layout_surfaces`): the visible panel gets its side anchor and
    /// margins from the applied layout, the reservation its side and strip
    /// from the held gap (overlay-expand D2), except while a flagged gap
    /// tween moves the strip with the panel (D3). Never leaves both
    /// horizontal anchors set.
    fn apply_layout_surfaces(&self) {
        if self.closed.get() {
            return;
        }
        let layout = self.layout.get();
        // Callers validate first; this is belt and braces.
        let Ok(geometry) = layout.side_geometry(i64::from(self.panel_px())) else {
            return;
        };
        let left = layout.side() == Side::Left;
        let win = &self.win;
        win.set_anchor(Edge::Left, left);
        win.set_anchor(Edge::Right, !left);
        win.set_anchor(Edge::Top, true);
        win.set_anchor(Edge::Bottom, true);
        win.set_margin(Edge::Left, if left { geometry.edge_margin() } else { 0 });
        win.set_margin(Edge::Right, if left { 0 } else { geometry.edge_margin() });
        win.set_margin(Edge::Top, layout.top());
        win.set_margin(Edge::Bottom, layout.bottom());

        // The reservation follows the drawn gap, not the applied layout (D2):
        // a covering layout cannot move it.
        let (reserve_side, reserve_zone) = self.drawn_gap(layout, self.panel_px());
        let reserve_left = reserve_side == Side::Left;
        if let Some(reserve) = self.reserve.borrow().as_ref() {
            reserve.set_anchor(Edge::Left, reserve_left);
            reserve.set_anchor(Edge::Right, !reserve_left);
            reserve.set_anchor(Edge::Top, true);
            reserve.set_anchor(Edge::Bottom, true);
            reserve.set_margin(Edge::Left, 0);
            reserve.set_margin(Edge::Right, 0);
            reserve.set_margin(Edge::Top, 0);
            reserve.set_margin(Edge::Bottom, 0);
            reserve.set_exclusive_zone(reserve_zone);
            if let Some(monitor) = self.monitor.borrow().as_ref() {
                reserve.set_monitor(Some(monitor));
            }
        }
        self.area.queue_draw();
    }

    /// Push the current panel width onto both surfaces and queue a draw
    /// (`glue_apply_geometry`).
    pub fn apply_geometry(&self) {
        self.apply_panel_width(self.panel_px());
        self.apply_layout_surfaces();
    }

    /// One eased tween frame at `px` (`anim_tick`'s `glue_apply_geometry`).
    fn apply_frame(&self, px: i32) {
        self.apply_panel_width(px);
        self.apply_layout_surfaces();
    }

    /// The tween stopped for any reason: drop the tween's retained grid
    /// node, clear the pty's tween flag and apply the deferred grid resize,
    /// if an animated apply left one pending (`anim_stop`'s glue half).
    ///
    /// The resize runs here, synchronously, not behind an idle: every draw
    /// after this stop — including the frame that presents the tween's final
    /// width, whose paint follows this stop in the same frame cycle — must
    /// see the resized grid, or a right-docked panel paints its old, narrower
    /// grid at offset 0 against the wider widget and the gap on the docked
    /// side stays on screen until the next damage-driven frame
    /// (gsk-render-nodes design, Post-task decisions: C52). The vt reflow cost lands inside the tween's
    /// last frame instead of after it; the tween is over, so nothing animates
    /// behind the block.
    fn on_tween_stopped(&self) {
        (self.hooks.tween_cache_drop)();
        (self.hooks.set_tween_active)(false);
        self.fire_deferred_grid_resize();
    }

    /// Apply the deferred terminal grid resize at the tween's stop
    /// (`glue_grid_resize_deferred_fire` + `deferred_grid_resize`). Publish
    /// resets the flag before a retarget, so a tween superseded by another
    /// one leaves nothing pending here; only a stop of the tween that deferred
    /// the resize applies it, exactly once.
    fn fire_deferred_grid_resize(&self) {
        if !deferred_grid_at_stop(self.deferred_grid.get(), self.closed.get()) {
            return;
        }
        self.deferred_grid.set(false);
        // The hook guards its own body (D5); a latched panel skips the push
        // exactly as the old idle's guarded step did.
        (self.hooks.apply_size)();
    }

    /// Validate the layout against the live metrics and publish it
    /// (`glue_publish_layout`).
    pub fn publish(&self, layout: Layout, duration_ms: u32) -> PublishOutcome {
        if self.closed.get() {
            return PublishOutcome::NotLive;
        }
        // A panel without live metrics is a lifecycle state, not a layout
        // verdict (`GLUE_NOT_LIVE`).
        let Some(geometry) = self.monitor.borrow().as_ref().map(gdk::Monitor::geometry) else {
            return PublishOutcome::NotLive;
        };
        let applied = self.layout.get();
        // Validate before staging (overlay-expand D4): a rejected apply
        // leaves the applied layout and the held gap untouched.
        let Some((layout, cols, held_gap)) = staged_publish(
            geometry.width(),
            geometry.height(),
            self.cell_w.get(),
            self.cell_h.get(),
            self.held_gap.get(),
            layout,
        ) else {
            return PublishOutcome::InvalidLayout;
        };

        // A layout differing only in its column count and/or push/cover
        // choice animates (overlay-expand D3); the side and gutters must
        // match. A tween already heading for these columns keeps going.
        let animate = should_animate(duration_ms, Anim::allowed(), &applied, &layout);
        let from_px = self.panel_px();
        let cols_changed = layout.cols().get() != self.cols.get();
        let coverage_changed = layout.coverage() != applied.coverage();

        // The strip the gap rests at right now, before this apply mutates (D3).
        let (_, gap_zone) = self.drawn_gap(applied, from_px);
        let target_px = i32::from(layout.cols().get()) * self.cell_w.get();
        let gap_tweens = gap_tween_decision(gap_zone, animate, layout, target_px);

        // The staged layout is validated, so it may now mutate the applied
        // layout and held gap as one operation (overlay-expand D4).
        self.layout.set(layout);
        self.cols.set(cols);
        self.held_gap.set(held_gap);
        self.deferred_grid.set(false);
        if !animate {
            self.gap_tweening.set(false);
            self.anim.cancel();
        } else if cols_changed || coverage_changed {
            // A coverage-only apply still tweens: the panel eases in place
            // while a pushing target's strip tweens back (overlay-expand D3).
            self.gap_tweening.set(gap_tweens);
            self.anim
                .begin(&self.win, from_px, self.grid_px(), duration_ms);
            (self.hooks.set_tween_active)(true);
            // The terminal grid resize does not run at t0: the vt reflow costs
            // tens to hundreds of ms on a real grid, which would block the
            // caller for that long and delay the tween's first frame. The
            // tween draws the old grid from the cache; the grid catches up
            // when the tween ends.
            self.deferred_grid.set(true);
        }
        self.apply_geometry();
        if self.deferred_grid.get() {
            return PublishOutcome::Applied;
        }
        // A terminal allocation failure keeps the previous grid and is an
        // internal failure of this apply, not a layout verdict; the panel
        // snaps to the requested layout rather than stay mid-animation.
        if !(self.hooks.apply_size)() {
            self.anim.cancel();
            self.apply_geometry();
            return PublishOutcome::Terminal;
        }
        PublishOutcome::Applied
    }

    /// The visible panel mapped (`on_win_map`): mark the first-draw monitor
    /// resolution pending, create and present the reservation, then size the
    /// terminal. Also connects `notify::scale` (snap-grid-edges D3): when
    /// the panel moves to an output with a different scale, the grid must
    /// redraw at the new scale. The handler only queues a redraw — the draw
    /// hooks read the scale fresh at each draw, so a stored copy could go
    /// stale between the notify and the draw.
    fn on_map(&self) {
        self.latch.set(true);

        if let Some(surface) = self.win.surface() {
            let area = self.area.clone();
            let scale_poisoned = self.poisoned.clone();
            // `_local`: the closure holds GTK-thread handles (Rc), like every
            // other closure registered here; it runs on the GTK thread only.
            surface.connect_notify_local(Some("scale"), move |_, _| {
                let _ = guard(&scale_poisoned, || area.queue_draw());
            });
        }

        // The reservation is created and presented only after the visible
        // panel is mapped; the first draw then pins it to the resolved monitor.
        let reserve = gtk4::ApplicationWindow::new(&self.app);
        reserve.init_layer_shell();
        reserve.set_namespace(Some("pinwin-reserve"));
        reserve.set_layer(Layer::Bottom);
        reserve.set_default_size(1, -1);
        reserve.set_opacity(0.0);
        *self.reserve.borrow_mut() = Some(reserve.clone());

        self.apply_layout_surfaces();
        reserve.present();
        (self.hooks.apply_size)();
    }

    /// One-shot, from the first draw after map (`resolve_layout_monitor`): by
    /// then the surface has entered its output, so the monitor reported here
    /// is the panel's real monitor, and the live metrics exist, so this
    /// completes the start handshake. Success requires a resolved monitor:
    /// with no output the panel has no metrics to validate or publish against,
    /// so the handshake fails and the loop quits (the surfaces are left alone
    /// here; the caller closes them after the loop returns).
    pub fn resolve_monitor(&self) {
        self.latch.set(false);
        let monitor = gdk::Display::default().and_then(|display| {
            self.win
                .surface()
                .and_then(|surface| display.monitor_at_surface(&surface))
        });
        let Some(monitor) = monitor else {
            (self.hooks.start_result)(false);
            self.app.quit();
            return;
        };
        *self.monitor.borrow_mut() = Some(monitor);
        self.apply_layout_surfaces();
        (self.hooks.apply_size)();
        (self.hooks.start_result)(true);
    }

    /// Close both layer-shell surfaces (`glue_close_surfaces`). Shared by the
    /// stop teardown and the failed start after a NULL monitor resolution.
    pub fn close(&self) {
        self.anim.cancel();
        if let Some(reserve) = self.reserve.borrow_mut().take() {
            reserve.destroy();
        }
        self.win.destroy();
        self.closed.set(true);
    }
}

#[cfg(test)]
mod tests;
