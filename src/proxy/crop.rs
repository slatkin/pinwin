//! The width tween's crop plan (replace-gtk-with-wayland D7, kept for the
//! hosted app by replace-terminal-with-app-host D8): the per-frame viewport
//! crop that shows the docked-edge portion of the app's wide buffer at the
//! eased width, and the exact-rational scale rounding that decides it in
//! whole device pixels. Pure and display-free (`port-to-rust` D10): the
//! decisions here are unit tested without a compositor, and the Wayland
//! calls themselves — `set_size`, the margins, `set_source`,
//! `set_destination`, the reserve's zone and the commit — stay with the
//! proxy's frame step. This module only decides what those calls carry.
//!
//! The app draws the wide buffer itself, so the terminal panel's grid
//! painter, its wide draw, the upload and the no-viewporter copy are gone
//! (D8).
//!
//! The rounding rule, stated once: a device size is its logical size times
//! the scale, rounded half up in exact 1/120 rational arithmetic —
//! [`FractionalScale::scale_dimension`]. No `f64` product joins the
//! decision: at a scale like 1.45 the `f64` product of a tie width can
//! round the other way from the exact rational, and one pixel of
//! disagreement between the crop and the buffer it crops would show. The
//! eased width itself is the tween's integer logical width; nothing
//! re-rounds it.
//!
//! The crop is the docked-edge portion of the wide buffer that is the eased
//! width wide: for a left-docked panel it starts at device x 0, for a
//! right-docked one at `wide - crop` device pixels. Both edges are whole
//! device pixels by construction, so the app's content keeps its offset
//! from the docked edge in every frame — the spec's "Width animation"
//! scenario. The viewport destination is the eased logical width and the
//! configure height: the crop mapped back through the scale, which for
//! every scale of at least 1 is exactly the eased width. The surface
//! therefore presents the crop one device pixel to one device pixel, with
//! no resampling blur, and the layer-shell width stays the width the layout
//! and the IPC already know.
//!
//! `wp_viewport.set_source` carries its arguments as `wl_fixed` (1/256
//! units); wayland-client takes them as `f64` and encodes them as
//! `(value * 256.0) as i32`. A whole device pixel count encodes exactly;
//! `CropRect::wl_fixed` produces that number with a checked multiply, and
//! the tests assert it.

/// The edge the panel docks against: the crop reads from this edge of the
/// wide buffer. A panel docks left or right today; the four-edge geometry
/// of replace-terminal-with-app-host D9 replaces this type, and the crop
/// then spans the height as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// The preferred scale in 1/120 units (replace-gtk-with-wayland D5): the
/// wayland fractional-scale protocol carries the scale as a whole number of
/// 1/120ths, so 180 is 1.5. Private field, constructor-guarded: every value
/// is a valid 1/120ths count, so the invariant lives in the type's
/// operations, not a check (`port-to-rust` D6).
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

    /// The wide buffer's logical width: the larger of the start and end
    /// widths. Test builds only: the crop tests read the wide width back;
    /// production reads it through [`Self::frame`].
    #[cfg(test)]
    #[must_use]
    pub const fn wide_px(self) -> i32 {
        self.wide_px
    }

    /// The wide buffer's device width at `scale` — the width of the buffer
    /// the crop rectangle lives in.
    fn wide_device(self, scale: FractionalScale) -> Option<u32> {
        scale.scale_dimension(u32::try_from(self.wide_px).ok()?)
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
    /// the tween's end rule closes it.
    #[must_use]
    pub fn frame(self, current_px: i32, height: u32, scale: FractionalScale) -> Option<CropFrame> {
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

/// One tween frame's crop plan (D7): everything the frame step commits for
/// the width — the layer width, the viewport source and destination.
/// Constructed only by [`TweenCrop::frame`] (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropFrame {
    layer_width: i32,
    source: CropRect,
    destination: (i32, i32),
}

impl CropFrame {
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
    /// `None` past the fixed-point range. Test builds only: the crop tests
    /// pin the exact encoding; production hands `f64`s to wayland-client,
    /// which encodes them itself.
    #[cfg(test)]
    #[must_use]
    pub fn wl_fixed(pixels: i32) -> Option<i32> {
        pixels.checked_mul(256)
    }
}

#[cfg(test)]
mod tests {
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

    /// At scale 1.5 the crop offset is a whole device pixel for both sides —
    /// asserted on the number the protocol would carry, the `wl_fixed`
    /// source x.
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

    /// Degenerate widths never panic: a non-positive eased width, a zero
    /// height and an eased width past the wide buffer refuse the frame;
    /// equal start and end crop the whole buffer; a start wider than the end
    /// takes the start as the wide width.
    #[test]
    fn degenerate_widths_refuse_instead_of_panicking() {
        let s = scale(180);

        // Negative widths never build a crop.
        assert!(TweenCrop::new(Side::Left, -1, 100).is_none());
        assert!(TweenCrop::new(Side::Right, 100, -1).is_none());

        // A zero wide buffer builds, but presents nothing.
        let zero = TweenCrop::new(Side::Left, 0, 0).expect("a valid tween");
        assert!(zero.frame(0, 720, s).is_none(), "a zero width has no crop");

        // A zero height is no viewport destination.
        let crop = TweenCrop::new(Side::Left, 40, 100).expect("a valid tween");
        assert!(crop.frame(60, 0, s).is_none());

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
