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
//! The renderer arrives in group 4, so the buffers here are transparent
//! placeholders of the right size — enough to map the surfaces. The pure
//! geometry mapping (anchors, margins, zones, interactivity) is unit tested
//! here without a compositor (`port-to-rust` D10); the queue-dependent
//! surface creation lives in the state module beside the sctk handlers.

use std::num::NonZeroU16;

use smithay_client_toolkit::compositor::Region;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, LayerSurface};
use smithay_client_toolkit::shm::{CreatePoolError, Shm, slot::SlotPool};
use wayland_client::protocol::{wl_shm, wl_surface};

use crate::layout::{CellSize, Coverage, Keyboard, Layout, Side};

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
/// first pushing layout applies. Row 3.5 ports the apply path onto
/// `crate::surfaces::gap`; until then this mirrors its `start_held_gap`. A
/// strip the geometry rejects (the checked-arithmetic overflow a validated
/// layout excludes) reserves nothing.
#[must_use]
pub fn startup_reserve_zone(layout: Layout, cell: CellSize) -> i32 {
    match layout.coverage() {
        Coverage::Push => {
            let Some(width) = grid_width_px(layout.cols(), cell) else {
                return 0;
            };
            layout
                .side_geometry(i64::from(width))
                .map_or(0, |geometry| geometry.reservation())
        }
        Coverage::Cover => 0,
    }
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
    pool: SlotPool,
    layout: Layout,
    keyboard: Keyboard,
    cell: Option<CellSize>,
    /// The size the panel's transparent buffer was last attached at, so a
    /// repeated configure at the same size does not re-attach it.
    panel_buffer_size: Option<(u32, u32)>,
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
    /// `cell` is the measured cell metrics; when they are still unknown the
    /// `set_size` is skipped (no width to ask for) until the font module
    /// supplies them (replace-gtk-with-wayland row 4.6).
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
        cell: Option<CellSize>,
    ) -> Result<Self, CreatePoolError> {
        panel.set_anchor(panel_anchor(layout.side()));
        panel.set_exclusive_zone(-1);
        let (top, right, bottom, left) = panel_margins(layout);
        panel.set_margin(top, right, bottom, left);
        panel.set_keyboard_interactivity(map_interactivity(keyboard));
        if let Some(cell) = cell {
            apply_panel_width(&panel, layout.cols(), cell);
        }
        panel.commit();
        let pool = SlotPool::new(1, shm)?;
        Ok(PanelSurfaces {
            panel,
            reserve: None,
            pool,
            layout,
            keyboard,
            cell,
            panel_buffer_size: None,
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
        // No metrics yet: the panel width is unknown, so the pushing strip is
        // unknown too and reserves nothing until the font module supplies the
        // metrics (replace-gtk-with-wayland row 4.6).
        let zone = self
            .cell
            .map_or(0, |cell| startup_reserve_zone(self.layout, cell));
        reserve.set_exclusive_zone(zone);
        reserve.set_size(1, 0);
        // The reserve takes no input (D3): an empty region covers no point.
        reserve.set_input_region(Some(region.wl_region()));
        reserve.commit();
        self.reserve = Some(Reserve {
            surface: reserve,
            _region: region,
        });
    }

    /// One configure of the panel surface: map the panel with a transparent
    /// buffer of the configured size when the size changed, and switch an
    /// `on-demand` panel to `on-demand` in the commit after its first buffer
    /// (D3). Later configures at a new size re-commit a transparent buffer so
    /// the surface stays coherent until the renderer takes the frames over
    /// (group 4).
    pub fn panel_configured(&mut self, width: u32, height: u32) {
        if self.panel_buffer_size == Some((width, height)) {
            return;
        }
        if attach_transparent(&mut self.pool, &self.panel, width, height).is_err() {
            // A buffer the pool could not provide (or a zero-sized
            // configure) leaves the panel unmapped; the next configure
            // retries. The library never exits over it (port-to-rust D3).
            return;
        }
        self.panel_buffer_size = Some((width, height));

        // The first buffer mapped the panel: the `on-demand` switch runs in
        // the commit after it, exactly once (D3, spike row 1.2).
        if !self.switched_to_on_demand {
            self.switched_to_on_demand = true;
            let target = post_buffer_interactivity(self.keyboard);
            if target != map_interactivity(self.keyboard) {
                self.panel.set_keyboard_interactivity(target);
                self.panel.commit();
            }
        }
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
}

/// Attach a fully transparent `ARGB8888` buffer of `width` by `height` to
/// `surface` and commit, so a layer surface maps without drawing anything
/// (the renderer arrives in group 4). Fails on a zero-sized or oversized
/// configure, a pool the size does not fit, or an attach the compositor
/// refused; the caller keeps its previous state in every failure case.
fn attach_transparent(
    pool: &mut SlotPool,
    surface: &LayerSurface,
    width: u32,
    height: u32,
) -> Result<(), AttachFailure> {
    if width == 0 || height == 0 {
        return Err(AttachFailure::ZeroSized);
    }
    let Ok(logical_width) = i32::try_from(width) else {
        return Err(AttachFailure::Oversized);
    };
    let Ok(logical_height) = i32::try_from(height) else {
        return Err(AttachFailure::Oversized);
    };
    let Ok(stride) = i32::try_from(i64::from(logical_width) * 4) else {
        return Err(AttachFailure::Oversized);
    };
    let (buffer, canvas) = pool
        .create_buffer(
            logical_width,
            logical_height,
            stride,
            wl_shm::Format::Argb8888,
        )
        .map_err(|_pool| AttachFailure::Pool)?;
    // A fresh mmap is already zero-filled; the fill keeps the transparency
    // true also for a slot reused from the free list.
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
    /// A zero-sized configure cannot have a buffer.
    ZeroSized,
    /// The size does not fit the buffer API's bounds.
    Oversized,
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
