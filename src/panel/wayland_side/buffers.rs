//! The frame buffers and the output scale (replace-gtk-with-wayland D5): a
//! pool of two `wl_shm` buffers the painter draws the next frame into while
//! the compositor still reads the last one, and the preferred-scale sources
//! the buffer size and the snap rules draw at.
//!
//! The scale resolves from the `wp_fractional_scale_v1` preferred scale (in
//! 1/120 units) when that global exists, else from the integer preferred
//! buffer scale the compositor handler reports, else from 1. The pure
//! resolution order, the 1/120 conversion and the device-size rounding are
//! free functions here, tested without a display (`port-to-rust` D10). The
//! resolved scale reaches the renderer as the existing
//! [`crate::render::OutputScale`], not as a second scale type.
//!
//! `wp_fractional_scale_manager_v1` and `wp_viewporter` are optional
//! globals (D1): their absence degrades the scale to the integer fallback
//! and the buffers to their own size, and is never a start failure.
//!
//! The pool itself is queue-bound, so like the surfaces it is exercised
//! only in a live session; the row 4.3 pixel tests draw into the canvases
//! that this pool sizes.

use smithay_client_toolkit::globals::GlobalData;
use smithay_client_toolkit::shm::slot::{Buffer, Slot, SlotPool};
use smithay_client_toolkit::shm::{CreatePoolError, Shm};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::{wl_shm, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::{
    self, WpFractionalScaleManagerV1,
};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::client::wp_viewporter::{self, WpViewporter};

use crate::guard::guard;

use super::state::PanelState;
use super::surfaces::SurfaceId;

/// The preferred scale in 1/120 units (D5): the wayland fractional-scale
/// protocol carries the scale as a whole number of 1/120ths, so 180 is 1.5.
/// Private field, constructor-guarded: every value is a valid 1/120ths
/// count, so the invariant lives in the type's operations, not a check
/// (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FractionalScale {
    units_120: u32,
}

impl FractionalScale {
    /// The scale from a whole number of 1/120 units, as the
    /// `wp_fractional_scale_v1` `preferred_scale` event carries it.
    #[must_use]
    pub fn from_120ths(units_120: u32) -> Self {
        FractionalScale { units_120 }
    }

    /// The scale from an integer preferred buffer scale. A non-positive
    /// factor is no scale at all: `None`, so the resolution falls past it.
    #[must_use]
    pub fn from_integer(factor: i32) -> Option<Self> {
        if factor <= 0 {
            return None;
        }
        u32::try_from(factor)
            .ok()
            .and_then(|factor| factor.checked_mul(120))
            .map(|units_120| FractionalScale { units_120 })
    }

    /// The whole number of 1/120 units.
    #[must_use]
    pub fn units_120(self) -> u32 {
        self.units_120
    }

    /// The scale factor itself, in device pixels per logical pixel. The
    /// grid painter (row 4.3) reads the snap rules' scale from here, as
    /// `OutputScale::new(scale.as_f64())` — the existing renderer scale, so
    /// no second scale type exists beside it.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.units_120) / 120.0
    }

    /// One logical dimension scaled to device pixels, rounded half up (the
    /// rounding the fractional-scale protocol prescribes for surface
    /// sizes). `None` when the product does not fit `u32` — a dimension no
    /// buffer could hold.
    #[must_use]
    pub fn scale_dimension(self, logical: u32) -> Option<u32> {
        let scaled = u64::from(logical) * u64::from(self.units_120) + 60;
        u32::try_from(scaled / 120).ok()
    }
}

/// The scale resolution order (D5): the fractional preferred scale when it
/// has arrived, else the integer preferred buffer scale (a non-positive one
/// is no scale), else 1.
#[must_use]
pub fn resolve_scale(fractional: Option<FractionalScale>, integer: Option<i32>) -> FractionalScale {
    fractional
        .or_else(|| integer.and_then(FractionalScale::from_integer))
        .unwrap_or_else(|| FractionalScale::from_120ths(120))
}

/// The device size of a frame: the logical size times the scale, each
/// dimension rounded (`D5`). `None` when a scaled dimension does not fit
/// `u32` — a size no buffer could hold.
#[must_use]
pub fn device_size(logical: (u32, u32), scale: FractionalScale) -> Option<(u32, u32)> {
    Some((
        scale.scale_dimension(logical.0)?,
        scale.scale_dimension(logical.1)?,
    ))
}

/// The per-surface fractional-scale and viewport objects. Both are `None`
/// when their global is absent or failed to bind: optional (D1).
#[derive(Debug, Default)]
struct SurfaceScale {
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
}

/// The output-scale state of one bound session: the optional fractional and
/// viewporter globals, their per-surface objects, and the two preferred-scale
/// sources the resolution order reads. Constructed only through
/// [`Scale::bind`] on the panel thread; the fields are private.
#[derive(Debug)]
pub struct Scale {
    fractional_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    panel: SurfaceScale,
    reserve: SurfaceScale,
    /// The `wp_fractional_scale_v1` preferred scale in 1/120 units, once it
    /// has arrived. Both surfaces sit on the panel's output (D3), so one
    /// value serves both.
    preferred_120: Option<u32>,
    /// The integer preferred buffer scale the compositor handler reports.
    integer: Option<i32>,
}

impl Scale {
    /// Bind the optional scale globals (D1): a missing or unbindable global
    /// is a degradation, never a start failure — the scale falls back to the
    /// integer preferred buffer scale, then to 1.
    pub(crate) fn bind(globals: &GlobalList, qh: &QueueHandle<PanelState>) -> Self {
        Scale {
            fractional_manager: globals.bind(qh, 1..=1, GlobalData).ok(),
            viewporter: globals.bind(qh, 1..=1, GlobalData).ok(),
            panel: SurfaceScale::default(),
            reserve: SurfaceScale::default(),
            preferred_120: None,
            integer: None,
        }
    }

    /// Create the per-surface fractional-scale and viewport objects. Without
    /// their globals the surface simply has none: the fractional events never
    /// arrive and the buffer is shown at its own size.
    pub(crate) fn attach(
        &mut self,
        id: SurfaceId,
        surface: &wl_surface::WlSurface,
        qh: &QueueHandle<PanelState>,
    ) {
        // Clones first: the managers are cheap proxies, and the per-surface
        // fields borrow `self` while the managers are read from it.
        let manager = self.fractional_manager.clone();
        let viewporter = self.viewporter.clone();
        let target = self.surface_mut(id);
        target.fractional = manager.map(|manager| manager.get_fractional_scale(surface, qh, id));
        target.viewport = viewporter.map(|viewporter| viewporter.get_viewport(surface, qh, id));
    }

    /// Set the viewporter destination of one surface to its logical size
    /// (D5): the compositor scales the device-size buffer down to this
    /// rectangle on screen. Without a viewport the call is a no-op.
    pub(crate) fn set_destination(&mut self, id: SurfaceId, width: i32, height: i32) {
        if let Some(viewport) = &self.surface_mut(id).viewport {
            viewport.set_destination(width, height);
        }
    }

    /// Note a `wp_fractional_scale_v1` preferred scale, in 1/120 units.
    pub(crate) fn note_preferred_scale(&mut self, units_120: u32) {
        self.preferred_120 = Some(units_120);
    }

    /// Note the integer preferred buffer scale the compositor handler
    /// reported.
    pub(crate) fn note_integer(&mut self, factor: i32) {
        self.integer = Some(factor);
    }

    /// The resolved scale (D5): fractional preferred, integer fallback, 1.
    /// The grid painter (row 4.3) reads it per frame through the session.
    #[must_use]
    pub fn resolved(&self) -> FractionalScale {
        resolve_scale(
            self.preferred_120.map(FractionalScale::from_120ths),
            self.integer,
        )
    }

    fn surface_mut(&mut self, id: SurfaceId) -> &mut SurfaceScale {
        match id {
            SurfaceId::Panel => &mut self.panel,
            SurfaceId::Reserve => &mut self.reserve,
        }
    }
}

/// Why the pool could not hand a buffer out. The callers degrade the same
/// way for every variant — the frame is skipped, the next one retries — so
/// the variants only carry the comment.
#[derive(Debug)]
pub(crate) enum BufferPoolError {
    /// A zero size has no buffer; a size that does not fit the buffer API's
    /// `i32` bounds cannot be allocated.
    InvalidSize,
    /// Both ping-pong buffers are still held by the compositor and a spare
    /// slot could not be grown.
    Busy,
    /// The shared memory could not be provided.
    Pool,
}

/// The pool of two `wl_shm` buffers (D5): the painter draws the next frame
/// into the free one while the compositor reads the other. Each buffer is
/// `ARGB8888` at a device size; the slot a released buffer leaves behind is
/// reused at the same size and reallocated at a new one. The compositor's
/// hold on a buffer is tracked through sctk's release handling
/// ([`Slot::has_active_buffers`]), so a busy buffer is never handed out.
pub(crate) struct BufferPool {
    pool: SlotPool,
    /// The two ping-pong slots, `None` until the first hand-out sizes them.
    /// A held handle keeps the slot's memory reserved for the pool even
    /// while the compositor still reads a buffer in it.
    slots: [Option<Slot>; 2],
    /// The buffer size each slot was last allocated at.
    sizes: [Option<(u32, u32)>; 2],
    /// The slot the next hand-out prefers, so the two buffers alternate.
    next: usize,
}

impl BufferPool {
    /// Create the pool. The two frame slots are allocated at the first
    /// hand-out, when the device size is known; the seed pool holds nothing.
    ///
    /// # Errors
    /// The seed shared memory could not be created; the caller reports the
    /// start as failed rather than run a panel whose buffers cannot exist.
    pub(crate) fn new(shm: &Shm) -> Result<Self, CreatePoolError> {
        Ok(BufferPool {
            pool: SlotPool::new(1, shm)?,
            slots: [None, None],
            sizes: [None, None],
            next: 0,
        })
    }

    /// Hand out a free `ARGB8888` buffer of `width` by `height` device
    /// pixels with its canvas: draw into the bytes, then attach the buffer
    /// to the surface and commit. The returned canvas is exactly
    /// `width * height * 4` bytes. Never per frame: a slot is reused once
    /// its buffer is released, and only a size change reallocates it.
    pub(crate) fn buffer(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<(Buffer, &mut [u8]), BufferPoolError> {
        if width == 0 || height == 0 {
            return Err(BufferPoolError::InvalidSize);
        }
        let Ok(width_i) = i32::try_from(width) else {
            return Err(BufferPoolError::InvalidSize);
        };
        let Ok(height_i) = i32::try_from(height) else {
            return Err(BufferPoolError::InvalidSize);
        };
        let Ok(stride) = i32::try_from(u64::from(width) * 4) else {
            return Err(BufferPoolError::InvalidSize);
        };
        let bytes = u64::from(width)
            .checked_mul(4)
            .and_then(|stride| stride.checked_mul(u64::from(height)))
            .and_then(|bytes| usize::try_from(bytes).ok());
        let Some(bytes) = bytes else {
            return Err(BufferPoolError::InvalidSize);
        };

        // A free slot already at this size: the steady-state hand-out.
        for i in 0..2 {
            let idx = (self.next + i) % 2;
            if self.sizes[idx] == Some((width, height))
                && self.slots[idx]
                    .as_ref()
                    .is_some_and(|slot| !slot.has_active_buffers())
            {
                self.next = (idx + 1) % 2;
                return self.buffer_in(idx, width_i, height_i, stride, bytes);
            }
        }
        // A free slot at a stale size: reallocate it. Dropping the old
        // handle is safe while the compositor still reads its buffers — the
        // slot's memory only returns to the pool's free list once the last
        // buffer referencing it is released.
        for i in 0..2 {
            let idx = (self.next + i) % 2;
            if self.slots[idx]
                .as_ref()
                .is_none_or(|slot| !slot.has_active_buffers())
            {
                self.slots[idx] = None;
                let slot = self
                    .pool
                    .new_slot(bytes)
                    .map_err(|_io| BufferPoolError::Pool)?;
                self.sizes[idx] = Some((width, height));
                self.slots[idx] = Some(slot.clone());
                self.next = (idx + 1) % 2;
                return self.buffer_in(idx, width_i, height_i, stride, bytes);
            }
        }
        // Both ping-pong buffers are still held: grow a spare slot so a
        // frame is skipped only when the compositor genuinely holds more
        // than two buffers. The spare's memory returns to the free list on
        // release, so the steady state stays at two slots. A spare the
        // pool could not grow leaves both buffers busy.
        let (buffer, canvas) = self
            .pool
            .create_buffer(width_i, height_i, stride, wl_shm::Format::Argb8888)
            .map_err(|_pool| BufferPoolError::Busy)?;
        Ok((buffer, &mut canvas[..bytes]))
    }

    /// Create the frame buffer in slot `idx` and return its exact-sized
    /// canvas.
    fn buffer_in(
        &mut self,
        idx: usize,
        width: i32,
        height: i32,
        stride: i32,
        bytes: usize,
    ) -> Result<(Buffer, &mut [u8]), BufferPoolError> {
        let slot = self.slots[idx].as_ref().ok_or(BufferPoolError::Pool)?;
        let buffer = self
            .pool
            .create_buffer_in(slot, width, height, stride, wl_shm::Format::Argb8888)
            .map_err(|_pool| BufferPoolError::Pool)?;
        let canvas = slot
            .clone()
            .canvas(&mut self.pool)
            .ok_or(BufferPoolError::Pool)?;
        Ok((buffer, &mut canvas[..bytes]))
    }
}

/// The dispatch of the scale protocol objects into [`PanelState`]: the two
/// manager interfaces carry no events, and the per-surface fractional scale
/// carries the preferred scale the resolution order reads.
impl Dispatch<WpFractionalScaleManagerV1, GlobalData> for PanelState {
    fn event(
        _state: &mut PanelState,
        _proxy: &WpFractionalScaleManagerV1,
        _event: wp_fractional_scale_manager_v1::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qh: &QueueHandle<PanelState>,
    ) {
        // The manager interface has no events.
    }
}

impl Dispatch<WpFractionalScaleV1, SurfaceId> for PanelState {
    fn event(
        state: &mut PanelState,
        _proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _data: &SurfaceId,
        _conn: &Connection,
        _qh: &QueueHandle<PanelState>,
    ) {
        // The preferred scale is a pure state note; it cannot panic, but it
        // runs inside the panel's dispatch, so it stays inside the guard
        // like every other handler body (D5).
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let poisoned = state.poisoned.clone();
            let _ = guard(&poisoned, || state.note_preferred_scale(scale));
        }
    }
}

impl Dispatch<WpViewporter, GlobalData> for PanelState {
    fn event(
        _state: &mut PanelState,
        _proxy: &WpViewporter,
        _event: wp_viewporter::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qh: &QueueHandle<PanelState>,
    ) {
        // The viewporter interface has no events.
    }
}

impl Dispatch<WpViewport, SurfaceId> for PanelState {
    fn event(
        _state: &mut PanelState,
        _proxy: &WpViewport,
        _event: wp_viewport::Event,
        _data: &SurfaceId,
        _conn: &Connection,
        _qh: &QueueHandle<PanelState>,
    ) {
        // The viewport interface has no events.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 1/120 conversion: the units the protocol carries read as their
    /// scale factor, and the factor feeds the renderer's snap rules' scale
    /// as the existing `OutputScale` (D5) — no second scale type.
    #[test]
    fn the_120ths_conversion_matches_the_factor_and_the_output_scale() {
        use crate::render::OutputScale;

        assert_eq!(FractionalScale::from_120ths(120).as_f64(), 1.0);
        assert_eq!(FractionalScale::from_120ths(150).as_f64(), 1.25);
        assert_eq!(FractionalScale::from_120ths(180).as_f64(), 1.5);
        assert_eq!(FractionalScale::from_120ths(180).units_120(), 180);
        assert_eq!(
            OutputScale::new(FractionalScale::from_120ths(180).as_f64()).get(),
            1.5
        );
        assert_eq!(
            OutputScale::new(FractionalScale::from_120ths(150).as_f64()).get(),
            1.25
        );
        // The snap rules' own default is the 1 scale.
        assert_eq!(
            OutputScale::new(FractionalScale::from_120ths(120).as_f64()),
            OutputScale::default()
        );
    }

    /// An integer preferred buffer scale converts into 1/120 units; a
    /// non-positive one is no scale at all.
    #[test]
    fn an_integer_scale_converts_and_a_non_positive_one_is_none() {
        assert_eq!(
            FractionalScale::from_integer(1),
            Some(FractionalScale::from_120ths(120))
        );
        assert_eq!(
            FractionalScale::from_integer(2),
            Some(FractionalScale::from_120ths(240))
        );
        assert_eq!(FractionalScale::from_integer(0), None);
        assert_eq!(FractionalScale::from_integer(-1), None);
    }

    /// The device size is the logical size times the scale, each dimension
    /// rounded half up; an exact product stays exact.
    #[test]
    fn the_device_size_is_the_logical_size_times_the_scale_rounded() {
        let scale_150 = FractionalScale::from_120ths(180);
        assert_eq!(device_size((1080, 720), scale_150), Some((1620, 1080)));
        // 3 logical pixels at 1.5 land on 4.5 device pixels: rounded up.
        assert_eq!(device_size((3, 3), scale_150), Some((5, 5)));
        let scale_125 = FractionalScale::from_120ths(150);
        assert_eq!(device_size((1000, 640), scale_125), Some((1250, 800)));
        // A dimension that does not fit u32 after scaling is refused rather
        // than truncated.
        assert_eq!(device_size((u32::MAX, 1), scale_125), None);
    }

    /// The resolution order (D5): the fractional preferred scale wins, then
    /// the integer preferred buffer scale, then 1. A non-positive integer
    /// falls through to 1.
    #[test]
    fn the_scale_resolution_order_is_fractional_then_integer_then_1() {
        let fractional = Some(FractionalScale::from_120ths(180));
        assert_eq!(
            resolve_scale(fractional, Some(2)),
            FractionalScale::from_120ths(180)
        );
        assert_eq!(
            resolve_scale(None, Some(2)),
            FractionalScale::from_120ths(240)
        );
        assert_eq!(
            resolve_scale(None, Some(0)),
            FractionalScale::from_120ths(120)
        );
        assert_eq!(
            resolve_scale(None, Some(-1)),
            FractionalScale::from_120ths(120)
        );
        assert_eq!(resolve_scale(None, None), FractionalScale::from_120ths(120));
    }
}
