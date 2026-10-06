//! The width tween's crop plan and wide buffer (replace-gtk-with-wayland
//! row 6.2, design decision 7): the pure half of the animation's drawing —
//! the wide buffer the grid is drawn into once at the tween's start, the
//! per-frame viewport crop that shows its docked-edge portion at the eased
//! width, and the no-viewporter copy. Pure and display-free (`port-to-rust`
//! D10): the decisions here are unit tested without a compositor, and the
//! Wayland calls themselves — `set_size`, the margins, `set_source`,
//! `set_destination`, the reserve's zone, the buffer attach and the commit —
//! stay with the frame handler a later row 6.2 unit wires. This module only
//! decides what those calls carry.
//!
//! The rounding rule, stated once: a device size is its logical size times
//! the scale, rounded half up in exact 1/120 rational arithmetic —
//! [`FractionalScale::scale_dimension`], the same function the buffer pool
//! sizes its buffers with (row 4.1). No `f64` product joins the decision:
//! at a scale like 1.45 the `f64` product of a tie width can round the
//! other way from the exact rational, and one pixel of disagreement between
//! the crop and the buffer it crops would show. The eased width itself is
//! the driver's integer logical width ([`super::tween::FrameStep::Frame`]);
//! nothing re-rounds it.
//!
//! The crop is the docked-edge portion of the wide buffer that is the eased
//! width wide: for a left-docked panel it starts at device x 0, for a
//! right-docked one at `wide - crop` device pixels. Both edges are whole
//! device pixels by construction, so the grid's offset from the docked edge
//! is a whole number of device pixels in every frame — the spec's "Width
//! animation" scenario. The viewport destination is the eased logical width
//! and the configure height: the crop mapped back through the scale, which
//! for every scale of at least 1 is exactly the eased width. The surface
//! therefore presents the crop one device pixel to one device pixel, with
//! no resampling blur, and the layer-shell width stays the width the layout
//! and the pty already know.
//!
//! `wp_viewport.set_source` carries its arguments as `wl_fixed` (1/256
//! units); wayland-client takes them as `f64` and encodes them as
//! `(value * 256.0) as i32`. A whole device pixel count encodes exactly;
//! [`CropRect::wl_fixed`] produces that number with a checked multiply, and
//! the tests assert it.
//!
//! The per-frame margins and the reserve's side and zone follow the
//! surfaces' own rules: [`panel_margins`] for the panel's insets and the
//! held-gap rule (overlay-expand D3) through
//! [`crate::surfaces::gap::reserve_gap_parts`] — the zone moves with the
//! panel only while a flagged gap tween runs on a pushing target.
//!
//! The wide draw ([`draw_wide`]) reuses the existing grid painter over a
//! canvas of the wide device size; nothing about the painter changes. The
//! cached wide canvas reaches its `wl_shm` buffer through [`upload_wide`]
//! once, at the tween's start; without the viewporter, [`copy_crop`] copies
//! each frame's crop into a fresh pool buffer — a memory copy, no glyph
//! work.

use crate::layout::{Layout, Side};
use crate::surfaces::gap::reserve_gap_parts;

use super::buffers::FractionalScale;
use super::surfaces::panel_margins;

mod draw;

pub use draw::{CropCopyError, copy_crop, draw_wide, upload_wide};

/// The tween's crop geometry, decided once at the tween's start: the docking
/// side and the wide buffer's logical width — the larger of the start and
/// end widths. Constructed only through [`TweenCrop::new`], so the width is
/// never negative (`port-to-rust` D6: the invariant lives in the
/// constructor, not a check).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TweenCrop {
    side: Side,
    wide_px: i32,
}

impl TweenCrop {
    /// The crop geometry of a tween from `start_px` to `end_px` logical
    /// pixels on `side`. `None` when a width is negative — no panel is
    /// narrower than nothing.
    #[must_use]
    pub fn new(side: Side, start_px: i32, end_px: i32) -> Option<Self> {
        if start_px < 0 || end_px < 0 {
            return None;
        }
        Some(TweenCrop {
            side,
            wide_px: start_px.max(end_px),
        })
    }

    /// The docking side the crop aligns to.
    #[must_use]
    pub const fn side(&self) -> Side {
        self.side
    }

    /// The wide buffer's logical width: the larger of the start and end
    /// widths.
    #[must_use]
    pub const fn wide_px(&self) -> i32 {
        self.wide_px
    }

    /// The wide buffer's device width at `scale` — the size the pool buffer
    /// holding the cached wide canvas is allocated at.
    #[must_use]
    pub fn wide_device(&self, scale: FractionalScale) -> Option<u32> {
        scale.scale_dimension(u32::try_from(self.wide_px).ok()?)
    }

    /// The wide canvas' draw decision (D7): the wide buffer's logical and
    /// device sizes, and the draw offset that lands the live grid on the
    /// docked edge. `grid_px` is the live grid's width in logical pixels —
    /// the columns the tween defers the resize of, times the cell width.
    ///
    /// The offset is decided in device pixels and converted to the logical
    /// form [`FrameInput`] carries: for a right-docked panel the grid's
    /// right edge must land exactly on the buffer's last device pixel, so
    /// the offset is `wide - grid` device pixels expressed in logical
    /// pixels. A logical-integer offset instead could round one device
    /// pixel away from the edge, because `round(wide * s)` and
    /// `round(grid * s)` need not differ by `round((wide - grid) * s)`.
    /// The painter re-rounds the product (`device_px`), and the offset test
    /// pins the round trip. `None` when the height is zero — no buffer
    /// could hold it — or the grid width is negative.
    #[must_use]
    pub fn wide_draw(&self, height: u32, grid_px: i32, scale: FractionalScale) -> Option<WideDraw> {
        if height == 0 || grid_px < 0 {
            return None;
        }
        let wide_dev = self.wide_device(scale)?;
        let height_dev = scale.scale_dimension(height)?;
        // A zero device dimension is no canvas and no commit: the draw and
        // the frame plan both refuse it.
        if wide_dev == 0 || height_dev == 0 {
            return None;
        }
        let grid_dev = scale.scale_dimension(u32::try_from(grid_px).ok()?)?;
        let draw_offset = match self.side {
            // A left-docked grid is glued at device x 0 already.
            Side::Left => 0.0,
            Side::Right => (f64::from(wide_dev) - f64::from(grid_dev)) / scale.as_f64(),
        };
        Some(WideDraw {
            logical: (u32::try_from(self.wide_px).ok()?, height),
            device: (wide_dev, height_dev),
            draw_offset,
        })
    }

    /// One tween frame's crop plan at the eased width `current_px` (D7):
    /// the layer width to commit, the viewport source rectangle — the
    /// docked-edge crop of the wide buffer, in device pixels — and the
    /// viewport destination. `height` is the latest configure's height in
    /// logical pixels; the crop always spans it fully.
    ///
    /// `None` on a frame nothing can present: a non-positive eased width or
    /// height (the viewport protocol's zero-sized destination is fatal), an
    /// eased width past the wide buffer, or a size that does not fit the
    /// protocol's integers. The caller skips such a frame; the next one or
    /// the watchdog ends the tween.
    #[must_use]
    pub fn frame(&self, current_px: i32, height: u32, scale: FractionalScale) -> Option<CropFrame> {
        if current_px <= 0 || current_px > self.wide_px || height == 0 {
            return None;
        }
        let wide_dev = self.wide_device(scale)?;
        let crop_dev = scale.scale_dimension(u32::try_from(current_px).ok()?)?;
        let height_dev = scale.scale_dimension(height)?;
        // Defensive: the eased width never passes the wide one, and the
        // scale's rounding is monotone — but a crop past the buffer would
        // be a protocol error, not a clipped draw.
        if crop_dev > wide_dev {
            return None;
        }
        let Ok(wide_dev_i) = i32::try_from(wide_dev) else {
            return None;
        };
        let Ok(crop_dev) = i32::try_from(crop_dev) else {
            return None;
        };
        let Ok(height_dev) = i32::try_from(height_dev) else {
            return None;
        };
        let Ok(height_i) = i32::try_from(height) else {
            return None;
        };
        let x = match self.side {
            Side::Left => 0,
            Side::Right => wide_dev_i - crop_dev,
        };
        Some(CropFrame {
            wide_width_dev: wide_dev,
            layer_width: current_px,
            source: CropRect {
                x,
                y: 0,
                width: crop_dev,
                height: height_dev,
            },
            destination: (current_px, height_i),
        })
    }
}

/// The wide canvas' draw decision: the sizes to allocate the canvas and its
/// pool buffer at, and the docked-edge draw offset the grid layers shift by.
/// Constructed only by [`TweenCrop::wide_draw`] (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WideDraw {
    logical: (u32, u32),
    device: (u32, u32),
    draw_offset: f64,
}

impl WideDraw {
    /// The wide canvas' logical size: the wide width and the configure
    /// height.
    #[must_use]
    pub const fn logical(&self) -> (u32, u32) {
        self.logical
    }

    /// The wide canvas' device size — the canvas and pool buffer size.
    #[must_use]
    pub const fn device(&self) -> (u32, u32) {
        self.device
    }

    /// The docked-edge draw offset, in logical pixels, device-exact: the
    /// painter's `device_px(draw_offset * scale)` lands the grid on the
    /// docked edge.
    #[must_use]
    pub const fn draw_offset(&self) -> f64 {
        self.draw_offset
    }
}

/// One tween frame's crop plan (D7): everything the frame handler commits
/// for the width — the layer width, the viewport source and destination —
/// plus the wide buffer's device width the fallback copy reads. Constructed
/// only by [`TweenCrop::frame`] (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropFrame {
    wide_width_dev: u32,
    layer_width: i32,
    source: CropRect,
    destination: (i32, i32),
}

impl CropFrame {
    /// The wide buffer's device width the source rectangle lives in.
    #[must_use]
    pub const fn wide_width_dev(&self) -> u32 {
        self.wide_width_dev
    }

    /// The panel width to commit this frame: the eased logical width, for
    /// `set_size` (the height stays the compositor's, between the anchors).
    #[must_use]
    pub const fn layer_width(&self) -> i32 {
        self.layer_width
    }

    /// The viewport source rectangle: the docked-edge crop of the wide
    /// buffer, in buffer (device) pixels.
    #[must_use]
    pub const fn source(&self) -> CropRect {
        self.source
    }

    /// The viewport destination: the eased logical width and the configure
    /// height, in surface-local (logical) pixels.
    #[must_use]
    pub const fn destination(&self) -> (i32, i32) {
        self.destination
    }
}

/// A viewport source rectangle in buffer (device) pixels: the crop the
/// wide buffer shows this frame. Constructed only by [`TweenCrop::frame`]
/// (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl CropRect {
    /// The left edge, in device pixels.
    #[must_use]
    pub const fn x(&self) -> i32 {
        self.x
    }

    /// The top edge, in device pixels.
    #[must_use]
    pub const fn y(&self) -> i32 {
        self.y
    }

    /// The width, in device pixels.
    #[must_use]
    pub const fn width(&self) -> i32 {
        self.width
    }

    /// The height, in device pixels.
    #[must_use]
    pub const fn height(&self) -> i32 {
        self.height
    }

    /// The `wl_fixed` value the protocol carries for a whole number of
    /// device pixels: the count times 256, exactly, since wayland-client
    /// encodes `set_source`'s `f64` arguments as `(value * 256.0) as i32`.
    /// `None` past the fixed-point range.
    #[must_use]
    pub fn wl_fixed(pixels: i32) -> Option<i32> {
        pixels.checked_mul(256)
    }
}

/// The panel geometry one tween frame commits beside the crop: the panel's
/// margins and the reserve surface's side and exclusive zone. The margins
/// are the tweening layout's own — a tween only runs between layouts that
/// match in side and gutters — and the zone follows the held-gap rule
/// (overlay-expand D3): it moves with the panel only while the apply
/// flagged the gap as tweening on a pushing target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameGeometry {
    margins: (i32, i32, i32, i32),
    reserve_side: Side,
    reserve_zone: i32,
}

impl FrameGeometry {
    /// The panel margins, in the `set_margin` order (top, right, bottom,
    /// left).
    #[must_use]
    pub const fn margins(&self) -> (i32, i32, i32, i32) {
        self.margins
    }

    /// The reserve surface's docking side.
    #[must_use]
    pub const fn reserve_side(&self) -> Side {
        self.reserve_side
    }

    /// The reserve surface's exclusive zone.
    #[must_use]
    pub const fn reserve_zone(&self) -> i32 {
        self.reserve_zone
    }
}

/// The per-frame panel geometry of a tween frame at `panel_px` (D7): the
/// tweening layout's margins and the held gap's rule for the reserve. The
/// same rules the plain apply writes ([`panel_margins`], the held-gap rule
/// of overlay-expand D3), read once here so the tween frames cannot drift
/// from them.
#[must_use]
pub fn frame_geometry(
    layout: Layout,
    held_side: Side,
    held_zone: i32,
    gap_tweening: bool,
    panel_px: i32,
) -> FrameGeometry {
    let (reserve_side, reserve_zone) =
        reserve_gap_parts(held_side, held_zone, gap_tweening, layout, panel_px);
    FrameGeometry {
        margins: panel_margins(layout),
        reserve_side,
        reserve_zone,
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use crate::render::geom::device_px;

    use super::*;

    fn scale(units: u32) -> FractionalScale {
        FractionalScale::from_120ths(units)
    }

    /// A `u32` device size as `i32`, for the tests' arithmetic.
    fn dev(value: u32) -> i32 {
        i32::try_from(value).expect("test device size fits i32")
    }

    /// The crop stays whole device pixels across the scales, both sides and
    /// a sweep of eased widths including the odd ones: the source rectangle
    /// sits inside the wide buffer with its docked edge at 0 (left) or at
    /// `wide - crop` (right), the destination is the eased logical width
    /// and the configure height, and the `wl_fixed` encodings are exact.
    #[test]
    fn the_crop_stays_whole_device_pixels_across_scales_sides_and_widths() {
        for units in [120, 150, 180, 216] {
            let s = scale(units);
            let wide_dev = dev(s.scale_dimension(128).expect("wide fits"));
            for &side in &[Side::Left, Side::Right] {
                let crop = TweenCrop::new(side, 40, 128).expect("a valid tween");
                assert_eq!(crop.wide_px(), 128, "the wide width is the larger one");
                let mut previous_width = 0;
                for current in 1..=128 {
                    let frame = crop
                        .frame(current, 720, s)
                        .unwrap_or_else(|| panic!("frame {current} at {units}"));
                    let source = frame.source();
                    assert!(source.x() >= 0, "the offset is non-negative");
                    assert!(source.width() > 0, "the crop is non-empty");
                    assert!(
                        i64::from(source.x()) + i64::from(source.width()) <= i64::from(wide_dev),
                        "the crop stays inside the buffer"
                    );
                    match side {
                        Side::Left => assert_eq!(source.x(), 0, "left crops from x 0"),
                        Side::Right => assert_eq!(
                            source.x(),
                            wide_dev - source.width(),
                            "right crops from the docked edge"
                        ),
                    }
                    assert_eq!(
                        CropRect::wl_fixed(source.x()),
                        Some(source.x().checked_mul(256).expect("test range")),
                        "the source x encodes exactly"
                    );
                    assert_eq!(
                        CropRect::wl_fixed(source.width()),
                        Some(source.width().checked_mul(256).expect("test range")),
                        "the source width encodes exactly"
                    );
                    assert!(source.width() >= previous_width, "the crop is monotone");
                    previous_width = source.width();
                    assert_eq!(frame.layer_width(), current, "the layer width eases");
                    assert_eq!(
                        frame.destination(),
                        (current, 720),
                        "the destination is the eased logical size"
                    );
                    assert_eq!(source.y(), 0, "the crop spans the full height");
                    assert_eq!(
                        source.height(),
                        dev(s.scale_dimension(720).expect("height fits")),
                        "the crop height is the buffer height"
                    );
                }
            }
        }
    }

    /// The row's own verify: at scale 1.5 the crop offset is a whole device
    /// pixel for both sides — asserted on the number the protocol would
    /// carry, the `wl_fixed` source x.
    #[test]
    fn the_crop_offset_is_a_whole_device_pixel_at_1_5_for_both_sides() {
        let s = scale(180);
        // A shrinking tween from 72 to 45 logical px, mid-tween at 41:
        // 41 * 1.5 = 61.5 device px, rounded half up to 62.
        let left = TweenCrop::new(Side::Left, 72, 45)
            .expect("a valid tween")
            .frame(41, 720, s)
            .expect("a presentable frame");
        assert_eq!(left.source().x(), 0);
        assert_eq!(left.source().width(), 62);
        assert_eq!(CropRect::wl_fixed(left.source().x()), Some(0));

        let right = TweenCrop::new(Side::Right, 72, 45)
            .expect("a valid tween")
            .frame(41, 720, s)
            .expect("a presentable frame");
        // The wide buffer is 108 device px wide; the crop starts 46 device
        // pixels from its left edge — a whole device pixel.
        assert_eq!(right.source().x(), 46);
        assert_eq!(CropRect::wl_fixed(right.source().x()), Some(46 * 256));
        assert_eq!(right.destination(), (41, 720));
    }

    /// The wide draw offset lands the live grid on the docked edge in
    /// device pixels: the painter's round trip through `device_px` gives
    /// `wide - grid` device pixels for a right-docked grid (negative when a
    /// retarget left the grid wider than the wide buffer), and 0 for a
    /// left-docked one.
    #[test]
    fn the_wide_draw_offset_lands_the_grid_on_the_docked_edge() {
        for units in [120, 150, 180, 216] {
            let s = scale(units);
            let wide_dev = dev(s.scale_dimension(128).expect("wide fits"));
            let grid_dev = dev(s.scale_dimension(64).expect("grid fits"));
            let right = TweenCrop::new(Side::Right, 64, 128)
                .expect("a valid tween")
                .wide_draw(720, 64, s)
                .expect("a drawable size");
            assert_eq!(
                device_px(right.draw_offset() * s.as_f64()),
                wide_dev - grid_dev,
                "the grid's right edge lands on the buffer's last device pixel"
            );
            let left = TweenCrop::new(Side::Left, 64, 128)
                .expect("a valid tween")
                .wide_draw(720, 64, s)
                .expect("a drawable size");
            assert_eq!(left.draw_offset(), 0.0, "a left-docked grid needs no shift");
        }

        // A grid wider than the wide buffer (a shrinking retarget: the
        // tween eased to 700 of its 1080 run, then retargeted to 400 while
        // the deferred grid is still 1080): the offset is negative and
        // still exact — the grid stays glued to the docked edge and clips
        // at the free one.
        let s = scale(180);
        let wide_dev = dev(s.scale_dimension(700).expect("wide fits"));
        let grid_dev = dev(s.scale_dimension(1080).expect("grid fits"));
        let clipped = TweenCrop::new(Side::Right, 700, 400)
            .expect("a valid tween")
            .wide_draw(720, 1080, s)
            .expect("a drawable size");
        assert_eq!(
            device_px(clipped.draw_offset() * s.as_f64()),
            wide_dev - grid_dev
        );
    }

    /// The wide draw's sizes are the wide width and the configure height at
    /// the scale, logical and device.
    #[test]
    fn the_wide_draw_sizes_follow_the_scale() {
        let s = scale(180);
        let draw = TweenCrop::new(Side::Left, 64, 128)
            .expect("a valid tween")
            .wide_draw(720, 64, s)
            .expect("a drawable size");
        assert_eq!(draw.logical(), (128, 720));
        assert_eq!(draw.device(), (192, 1080));
    }

    /// The per-frame panel geometry follows the surfaces' own rules: the
    /// tweening layout's margins, and the reserve zone from the held-gap
    /// rule (overlay-expand D3) — the zone moves with the panel only while
    /// the gap tween runs on a pushing target.
    #[test]
    fn the_frame_geometry_follows_the_margin_and_gap_rules() {
        let layout = crate::layout::Layout::new(
            Side::Left,
            NonZeroU16::new(40).expect("test columns"),
            0,
            0,
            0,
            12,
        );
        let geometry = frame_geometry(layout, Side::Left, 372, false, 700);
        assert_eq!(geometry.margins(), (0, 0, 0, 0), "the layout's own margins");
        assert_eq!(geometry.reserve_side(), Side::Left);
        assert_eq!(geometry.reserve_zone(), 372, "the held strip holds still");

        let geometry = frame_geometry(layout, Side::Left, 372, true, 700);
        assert_eq!(
            geometry.reserve_zone(),
            712,
            "0 + 700 + 12: the strip moves"
        );

        // A covering target never moves the zone, flag or not.
        let covering = layout.covering();
        let geometry = frame_geometry(covering, Side::Left, 372, true, 700);
        assert_eq!(geometry.reserve_zone(), 372);

        // The margins follow the docking side.
        let right = crate::layout::Layout::new(
            Side::Right,
            NonZeroU16::new(40).expect("test columns"),
            5,
            6,
            0,
            12,
        );
        let geometry = frame_geometry(right, Side::Right, 0, false, 700);
        assert_eq!(geometry.margins(), (5, 12, 6, 0));
    }

    /// Degenerate widths never panic: a non-positive eased width, a zero
    /// height, an eased width past the wide buffer and a negative start or
    /// end refuse the frame; equal start and end crop the whole buffer; a
    /// start wider than the end takes the start as the wide width.
    #[test]
    fn degenerate_widths_refuse_instead_of_panicking() {
        let s = scale(180);

        // Negative widths never build a crop.
        assert!(TweenCrop::new(Side::Left, -1, 100).is_none());
        assert!(TweenCrop::new(Side::Right, 100, -1).is_none());

        // A zero wide buffer builds, but presents nothing.
        let zero = TweenCrop::new(Side::Left, 0, 0).expect("a valid tween");
        assert!(zero.frame(0, 720, s).is_none(), "a zero width has no crop");
        assert!(zero.wide_draw(720, 0, s).is_none(), "no canvas either");

        // A zero height is no viewport destination.
        let crop = TweenCrop::new(Side::Left, 40, 100).expect("a valid tween");
        assert!(crop.frame(60, 0, s).is_none());
        assert!(crop.wide_draw(0, 40, s).is_none());

        // An eased width past the wide buffer is refused.
        assert!(crop.frame(200, 720, s).is_none());

        // Equal start and end: the crop is the whole buffer, on both sides.
        let equal = TweenCrop::new(Side::Right, 100, 100).expect("a valid tween");
        let frame = equal.frame(100, 720, s).expect("a presentable frame");
        assert_eq!(frame.source().x(), 0, "the crop covers the whole buffer");
        assert_eq!(
            frame.source().width(),
            dev(s.scale_dimension(100).expect("fits"))
        );

        // A start wider than the end takes the start as the wide width.
        let shrinking = TweenCrop::new(Side::Right, 100, 40).expect("a valid tween");
        assert_eq!(shrinking.wide_px(), 100);
        assert!(shrinking.frame(60, 720, s).is_some());
    }
}
