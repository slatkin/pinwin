//! The panel thread's layer surfaces (replace-gtk-with-wayland D3): the
//! visible panel on the `overlay` layer and the transparent reservation, the
//! wayland twins of [`crate::surfaces`] while the GTK surfaces stay until row
//! 8.1 removes them.
//!
//! The panel is created with no output, so the compositor places it on the
//! focused output; the first `wl_surface.enter` names that output and the
//! reserve is then created on it (row 3.2). The panel anchors to the top, the
//! bottom and the docked side with exclusive zone -1, `set_size` for the
//! width and margins for the gutters; the reserve carries the exclusive
//! zone, takes no input and shows a transparent buffer one pixel wide.
//!
//! The keyboard interactivity follows the map rule of decision 3: an
//! `on-demand` panel maps with no interactivity and switches to `on-demand`
//! in the commit after its first buffer, because niri grants a layer surface
//! keyboard focus only on the map itself; `exclusive` and `none` map
//! directly.
//!
//! The renderer arrived in group 4, so the panel's frames draw live from
//! dispatch D4c on: the configure path and the repaint service present the
//! renderer's frames through the pool at the device size, and only the
//! reserve still attaches a transparent placeholder buffer. The pure
//! geometry mapping (anchors, margins, zones, interactivity) is unit tested
//! here without a compositor (`port-to-rust` D10); the queue-dependent
//! surface creation lives in the state module beside the sctk handlers.

use std::num::NonZeroU16;

use smithay_client_toolkit::compositor::{FrameCallbackData, Region};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, LayerSurface};
use smithay_client_toolkit::shm::slot::Buffer;
use smithay_client_toolkit::shm::{CreatePoolError, Shm};
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_surface;

use crate::layout::{CellSize, Keyboard, Layout, Side};
use crate::surfaces::gap::start_held_gap;

use super::apply::SurfaceGeometry;
use super::buffers::{BufferPool, BufferPoolError};

/// Which of the panel thread's surfaces a `wl_surface` is: the user data the
/// surfaces are created with, so the compositor handlers can tell the panel's
/// events from the reserve's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceId {
    Panel,
    Reserve,
}

/// The layer-shell anchors of the panel for its docking side (D3): the top,
/// the bottom and the docked side.
#[must_use]
pub fn panel_anchor(side: Side) -> Anchor {
    match side {
        Side::Left => Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT,
        Side::Right => Anchor::TOP | Anchor::BOTTOM | Anchor::RIGHT,
    }
}

/// The panel surface's margins, in the `set_margin` order (top, right,
/// bottom, left): the vertical gutters on their edges and the docking edge's
/// gutter on its edge only — the opposite horizontal edge stays flush, the
/// way the GTK surfaces' `apply_layout_surfaces` sets them.
#[must_use]
pub fn panel_margins(layout: Layout) -> (i32, i32, i32, i32) {
    let (right, left) = match layout.side() {
        Side::Left => (0, layout.left()),
        Side::Right => (layout.right(), 0),
    };
    (layout.top(), right, layout.bottom(), left)
}

/// The panel's width in logical pixels: the applied columns times the cell
/// width. `None` when the product does not fit `i32` — a width no output
/// could hold, and one a validated layout excludes.
#[must_use]
pub fn grid_width_px(cols: NonZeroU16, cell: CellSize) -> Option<i32> {
    let width = i64::from(cols.get()) * i64::from(cell.width().get());
    i32::try_from(width).ok()
}

/// The exclusive zone the reserve holds at startup, the startup held gap
/// (overlay-expand D5): a pushing start reserves its own strip
/// `left + panel width + right`, a covering start reserves nothing until the
/// first pushing layout applies. The decision is `gap::start_held_gap`'s,
/// shared with the panel state's held gap (row 3.5).
#[must_use]
pub fn startup_reserve_zone(layout: Layout, cell: CellSize) -> i32 {
    start_held_gap(layout, cell.width().get()).zone()
}

/// The keyboard interactivity a mode maps with (D3): `on-demand` maps with
/// none — niri grants a layer surface focus only on the map itself, so the
/// launch must take no focus — while `exclusive` and `none` map directly.
#[must_use]
pub fn map_interactivity(keyboard: Keyboard) -> KeyboardInteractivity {
    match keyboard {
        Keyboard::None | Keyboard::OnDemand => KeyboardInteractivity::None,
        Keyboard::Exclusive => KeyboardInteractivity::Exclusive,
    }
}

/// The keyboard interactivity after the first buffer (D3): the commit after
/// the first buffer switches an `on-demand` panel to `on-demand`; the other
/// modes keep theirs.
#[must_use]
pub fn post_buffer_interactivity(keyboard: Keyboard) -> KeyboardInteractivity {
    match keyboard {
        Keyboard::None => KeyboardInteractivity::None,
        Keyboard::OnDemand => KeyboardInteractivity::OnDemand,
        Keyboard::Exclusive => KeyboardInteractivity::Exclusive,
    }
}

/// The reservation surface and its empty input region. The region is kept
/// alive with the surface: destroying it after the commit is legal, but
/// keeping it costs nothing and removes the ordering question.
struct Reserve {
    surface: LayerSurface,
    _region: Region,
}

/// The panel thread's two layer surfaces plus the shared memory pool their
/// transparent placeholder buffers come from. Lives on the panel thread,
/// inside the bound session; the GTK surfaces stay in `crate::surfaces` until
/// row 8.1 switches `Panel` over.
pub(crate) struct PanelSurfaces {
    panel: LayerSurface,
    reserve: Option<Reserve>,
    /// The two-buffer pool the transparent placeholder buffers and, from
    /// row 4.3 on, the drawn frames come from (D5).
    pool: BufferPool,
    layout: Layout,
    keyboard: Keyboard,
    cell: CellSize,
    /// Whether the `on-demand` switch (D3) has run: it follows the first
    /// buffer, exactly once.
    switched_to_on_demand: bool,
    /// The size the reserve's transparent buffer was last attached at.
    reserve_buffer_size: Option<(u32, u32)>,
}

impl PanelSurfaces {
    /// Take the panel surface the state module created and apply the startup
    /// geometry (row 3.1): the top/bottom/docked-side anchors, exclusive
    /// zone -1 (other zones do not push the panel), the gutters as margins,
    /// the layout's pixel width through `set_size` and the mode's mapping
    /// interactivity. The initial commit with no buffer asks the compositor
    /// for the first configure.
    ///
    /// `cell` is the measured cell metrics the start command carried (row
    /// 4.6's font module will compute them): a required input, because a
    /// panel without them cannot ask for its width and would never map.
    ///
    /// # Errors
    /// The shared memory pool for the placeholder buffers could not be
    /// created; the caller reports the start as failed rather than map a
    /// panel whose buffers cannot exist.
    pub fn new(
        panel: LayerSurface,
        shm: &Shm,
        layout: Layout,
        keyboard: Keyboard,
        cell: CellSize,
    ) -> Result<Self, CreatePoolError> {
        panel.set_anchor(panel_anchor(layout.side()));
        panel.set_exclusive_zone(-1);
        let (top, right, bottom, left) = panel_margins(layout);
        panel.set_margin(top, right, bottom, left);
        panel.set_keyboard_interactivity(map_interactivity(keyboard));
        apply_panel_width(&panel, layout.cols(), cell);
        panel.commit();
        let pool = BufferPool::new(shm)?;
        Ok(PanelSurfaces {
            panel,
            reserve: None,
            pool,
            layout,
            keyboard,
            cell,
            switched_to_on_demand: false,
            reserve_buffer_size: None,
        })
    }

    /// Take the reserve surface and its empty input region the state module
    /// created on the panel's resolved output (rows 3.1 and 3.2) and apply
    /// the reservation: the layer-shell `Bottom` layer's surface, anchored
    /// like the panel, carrying the startup held gap's exclusive zone, one
    /// pixel wide and taking no input. The initial commit asks for the
    /// configure whose size the transparent buffer then fills.
    pub fn attach_reserve(&mut self, reserve: LayerSurface, region: Region) {
        reserve.set_anchor(panel_anchor(self.layout.side()));
        // The startup held gap (overlay-expand D5): the pushing strip is the
        // layout's side geometry around the panel's pixel width.
        reserve.set_exclusive_zone(startup_reserve_zone(self.layout, self.cell));
        reserve.set_size(1, 0);
        // The reserve takes no input (D3): an empty region covers no point.
        reserve.set_input_region(Some(region.wl_region()));
        reserve.commit();
        self.reserve = Some(Reserve {
            surface: reserve,
            _region: region,
        });
    }

    /// One configure of the panel surface, after the state drew the frame
    /// the configure maps the panel with (dispatch D4c): the buffer commit
    /// is the draw's, and this runs the `on-demand` switch once after the
    /// first buffer commit (D3, spike row 1.2). The caller invokes it only
    /// when a buffer was actually committed — a configure the pool could
    /// not serve leaves the panel unmapped and the switch unrun, and the
    /// next configure retries.
    pub fn panel_configured(&mut self) {
        self.switch_to_on_demand();
    }

    /// One configure of the reserve surface: fill the surface with a
    /// transparent buffer of the configured size, so the reservation maps.
    /// The configure size matches the one-pixel `set_size` on the horizontal
    /// axis; the vertical one is the compositor's height between the anchors.
    pub fn reserve_configured(&mut self, width: u32, height: u32) {
        let Some(reserve) = &self.reserve else {
            return;
        };
        if self.reserve_buffer_size == Some((width, height)) {
            return;
        }
        if attach_transparent(&mut self.pool, &reserve.surface, width, height).is_err() {
            // Same degradation as the panel's: the next configure retries.
            return;
        }
        self.reserve_buffer_size = Some((width, height));
    }

    /// The compositor closed the reserve surface: drop it, taking the
    /// exclusive zone with it. The panel itself stays.
    pub fn reserve_closed(&mut self) {
        self.reserve = None;
        self.reserve_buffer_size = None;
    }

    /// Whether `layer` is the panel surface.
    #[must_use]
    pub fn is_panel(&self, layer: &LayerSurface) -> bool {
        self.panel == *layer
    }

    /// Whether `surface` is the panel's `wl_surface`.
    #[must_use]
    pub fn is_panel_surface(&self, surface: &wl_surface::WlSurface) -> bool {
        self.panel.wl_surface() == surface
    }

    /// Whether `layer` is the reserve surface.
    #[must_use]
    pub fn is_reserve(&self, layer: &LayerSurface) -> bool {
        self.reserve
            .as_ref()
            .is_some_and(|reserve| reserve.surface == *layer)
    }

    /// Apply one staged layout's geometry to both surfaces (row 3.5): the
    /// panel re-anchors to the layout's docking side with its margins and
    /// pixel width, the reserve re-anchors to the held gap's side with its
    /// zone. Each commit asks the compositor for the configure that follows;
    /// the grid and the pty size were already pushed through the sizing path
    /// (row 3.3), and a configure at the same height pushes nothing more.
    pub fn apply_geometry(&mut self, geometry: &SurfaceGeometry) {
        self.panel.set_anchor(panel_anchor(geometry.panel_side));
        let (top, right, bottom, left) = geometry.panel_margins;
        self.panel.set_margin(top, right, bottom, left);
        // A validated layout's width always fits; a skipped `set_size` is
        // the same skip the startup width applies.
        if let Some(width) = geometry.panel_width
            && let Ok(width) = u32::try_from(width)
        {
            self.panel.set_size(width, 0);
        }
        self.panel.commit();
        if let Some(reserve) = &self.reserve {
            reserve
                .surface
                .set_anchor(panel_anchor(geometry.reserve_side));
            reserve.surface.set_exclusive_zone(geometry.reserve_zone);
            reserve.surface.commit();
        }
    }

    /// The tween frame's panel writes (row 6.2): `set_size` at the eased
    /// width — the height stays the compositor's, between the anchors.
    /// The anchors are not written: a tween never changes side.
    pub(crate) fn tween_panel_size(&mut self, width: i32) {
        if let Ok(width) = u32::try_from(width) {
            self.panel.set_size(width, 0);
        }
    }

    /// The tween frame's margin write (row 6.2): the tweening layout's
    /// margins, whose top and bottom may change while the horizontal ones
    /// cannot (the animate rule pins side and gutters).
    pub(crate) fn tween_panel_margins(&mut self, margins: (i32, i32, i32, i32)) {
        let (top, right, bottom, left) = margins;
        self.panel.set_margin(top, right, bottom, left);
    }

    /// The tween frame's reserve writes (row 6.2): the held-gap rule's side
    /// and exclusive zone, committed at once, so the zone moves with the
    /// panel in the same frames (D7). The anchor is written with the zone
    /// because the rule's side and the committed one must not drift, even
    /// though a tween never changes side.
    pub(crate) fn tween_reserve(&mut self, side: Side, zone: i32) {
        let Some(reserve) = &self.reserve else {
            return;
        };
        reserve.surface.set_anchor(panel_anchor(side));
        reserve.surface.set_exclusive_zone(zone);
        reserve.surface.commit();
    }

    /// Attach `buffer` to the panel surface (row 6.2): the cached wide
    /// buffer, re-attached every tween frame. A refused activate leaves the
    /// previous attach in place; the commit below still presents it.
    pub(crate) fn tween_attach(&mut self, buffer: &Buffer) {
        let _ = buffer.attach_to(self.panel.wl_surface());
    }

    /// A fresh pool buffer at the crop size for the no-viewporter fallback
    /// (row 6.2): the caller copies the frame's crop into the returned
    /// bytes and attaches the buffer. The pool is queue-bound, so this runs
    /// only in the live session.
    ///
    /// # Errors
    /// The pool could not provide the buffer: the caller skips the frame,
    /// and the next one or the watchdog retries.
    pub(crate) fn tween_fresh_buffer(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<(Buffer, &mut [u8]), BufferPoolError> {
        self.pool.buffer(width, height)
    }

    /// The pool, for the wide cache's one upload at the tween's start
    /// (row 6.2): the cache keeps the buffer it is uploaded into.
    pub(crate) fn pool_mut(&mut self) -> &mut BufferPool {
        &mut self.pool
    }

    /// Damage the panel's buffer rectangle and commit the frame (row 6.2):
    /// the whole presented buffer is damaged, because every frame moves the
    /// crop. A size past `i32` damages nothing — the commit still presents.
    pub(crate) fn tween_present(&mut self, width: u32, height: u32) {
        if let (Ok(width), Ok(height)) = (i32::try_from(width), i32::try_from(height)) {
            self.panel.wl_surface().damage_buffer(0, 0, width, height);
        }
        self.panel.commit();
    }

    /// Request the panel surface's next `wl_surface.frame` callback (row
    /// 6.2): the compositor completes it when the next frame is due, and
    /// the callback drives the tween's next eased width.
    pub(crate) fn request_frame(&self, qh: &QueueHandle<super::state::PanelState>) {
        let surface = self.panel.wl_surface();
        surface.frame(qh, FrameCallbackData(surface.clone()));
    }

    /// The panel's `wl_surface`, for the committed tween frame's
    /// presentation feedback request (row 6.3).
    pub(crate) fn panel_wl_surface(&self) -> &wl_surface::WlSurface {
        self.panel.wl_surface()
    }

    /// One panel configure while a tween runs (row 6.2): the tween frames
    /// own the panel surface's size, viewport and buffer commits until the
    /// tween finishes, so this configure runs the `on-demand` switch only —
    /// the buffer it would otherwise have followed may have been a tween
    /// frame's. The grid sizing the caller drives records the height; the
    /// defer mode holds its push until the finish.
    pub(crate) fn panel_configured_under_tween(&mut self) {
        self.switch_to_on_demand();
    }

    /// The `on-demand` switch (D3), run once after the first buffer commit —
    /// whichever path committed that buffer: the placeholder path on a plain
    /// configure, or a tween frame's buffer on a configure that arrives while
    /// a tween owns the commits.
    fn switch_to_on_demand(&mut self) {
        if self.switched_to_on_demand {
            return;
        }
        self.switched_to_on_demand = true;
        let target = post_buffer_interactivity(self.keyboard);
        if target != map_interactivity(self.keyboard) {
            self.panel.set_keyboard_interactivity(target);
            self.panel.commit();
        }
    }
}

/// Attach a fully transparent `ARGB8888` buffer of `width` by `height` to
/// `surface` and commit, so a layer surface maps without drawing anything
/// (the reserve still maps this way; the panel draws live from dispatch
/// D4c on). Fails on a zero-sized or oversized configure, a pool the size
/// does not fit, or an attach the compositor refused; the caller keeps its
/// previous state in every failure case.
fn attach_transparent(
    pool: &mut BufferPool,
    surface: &LayerSurface,
    width: u32,
    height: u32,
) -> Result<(), AttachFailure> {
    let (buffer, canvas) = pool
        .buffer(width, height)
        .map_err(|_pool| AttachFailure::Pool)?;
    // A fresh mmap is already zero-filled; the fill keeps the transparency
    // true also for a slot reused from the pool.
    canvas.fill(0);
    buffer
        .attach_to(surface.wl_surface())
        .map_err(|_attach| AttachFailure::Attach)?;
    surface.commit();
    Ok(())
}

/// Why a transparent placeholder buffer could not be attached. The caller
/// degrades the same way for every variant, so the variants only carry the
/// comment.
#[derive(Debug)]
enum AttachFailure {
    /// The pool could not provide the buffer.
    Pool,
    /// The attach was refused.
    Attach,
}

/// Apply the layout's pixel width to the panel surface (`set_size`): the
/// width is the columns times the cell width and the height is the
/// compositor's, between the top and bottom anchors. A width that does not
/// fit `u32` is skipped — a validated layout cannot produce one.
fn apply_panel_width(panel: &LayerSurface, cols: NonZeroU16, cell: CellSize) {
    if let Some(width) = grid_width_px(cols, cell) {
        let Ok(width) = u32::try_from(width) else {
            return;
        };
        panel.set_size(width, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(side: Side, cols: u16, top: i32, bottom: i32, left: i32, right: i32) -> Layout {
        Layout::new(
            side,
            NonZeroU16::new(cols).expect("test column count is non-zero"),
            top,
            bottom,
            left,
            right,
        )
    }

    fn cell(width: i32, height: i32) -> CellSize {
        CellSize::new(width, height).expect("test cell size is non-zero")
    }

    /// The panel anchors to the top, the bottom and the docked side (D3):
    /// never both horizontal edges at once.
    #[test]
    fn the_panel_anchors_to_the_docked_side() {
        let left = panel_anchor(Side::Left);
        assert!(left.contains(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT));
        assert!(!left.contains(Anchor::RIGHT));

        let right = panel_anchor(Side::Right);
        assert!(right.contains(Anchor::TOP | Anchor::BOTTOM | Anchor::RIGHT));
        assert!(!right.contains(Anchor::LEFT));
    }

    /// The margins follow the docking side: the edge gutter on its edge, the
    /// opposite edge flush, the vertical gutters on their edges.
    #[test]
    fn the_panel_margins_follow_the_docking_side() {
        let left = layout(Side::Left, 40, 5, 6, 8, 12);
        assert_eq!(panel_margins(left), (5, 0, 6, 8));

        let right = layout(Side::Right, 40, 5, 6, 8, 12);
        assert_eq!(panel_margins(right), (5, 12, 6, 0));
    }

    /// The panel width is the columns times the cell width, and a product
    /// that does not fit `i32` is refused rather than truncated.
    #[test]
    fn the_grid_width_is_columns_times_cell_width() {
        assert_eq!(
            grid_width_px(NonZeroU16::new(40).expect("40"), cell(9, 16)),
            Some(360)
        );
        assert_eq!(
            grid_width_px(NonZeroU16::new(65535).expect("65535"), cell(9, 16)),
            Some(589_815)
        );
        // 65535 columns of a 65536-px cell do not fit i32.
        assert_eq!(
            grid_width_px(NonZeroU16::new(65535).expect("65535"), cell(65_536, 16)),
            None
        );
    }

    /// The startup reservation: a pushing start reserves its own strip, a
    /// covering start reserves nothing (overlay-expand D5).
    #[test]
    fn the_startup_reserve_zone_follows_the_coverage_choice() {
        let pushing = layout(Side::Left, 40, 0, 0, 0, 12);
        assert_eq!(startup_reserve_zone(pushing, cell(9, 16)), 372);

        let covering = layout(Side::Left, 120, 0, 0, 0, 12).covering();
        assert_eq!(startup_reserve_zone(covering, cell(9, 16)), 0);
    }

    /// A negative gutter shrinks the pushing strip (the spec's "Negative
    /// gutter" scenario), the way `side_geometry` computes it.
    #[test]
    fn a_negative_gutter_shrinks_the_startup_strip() {
        let negative = layout(Side::Left, 40, 0, 0, -40, 12);
        assert_eq!(startup_reserve_zone(negative, cell(9, 16)), 332);
    }

    /// The keyboard interactivity mapping (D3): an `on-demand` panel maps
    /// with none and switches to `on-demand` after its first buffer;
    /// `exclusive` and `none` map directly and stay.
    #[test]
    fn the_interactivity_maps_by_mode() {
        assert_eq!(
            map_interactivity(Keyboard::None),
            KeyboardInteractivity::None
        );
        assert_eq!(
            map_interactivity(Keyboard::OnDemand),
            KeyboardInteractivity::None
        );
        assert_eq!(
            map_interactivity(Keyboard::Exclusive),
            KeyboardInteractivity::Exclusive
        );

        assert_eq!(
            post_buffer_interactivity(Keyboard::None),
            KeyboardInteractivity::None
        );
        assert_eq!(
            post_buffer_interactivity(Keyboard::OnDemand),
            KeyboardInteractivity::OnDemand
        );
        assert_eq!(
            post_buffer_interactivity(Keyboard::Exclusive),
            KeyboardInteractivity::Exclusive
        );
    }
}
