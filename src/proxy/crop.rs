//! The panel-size tween's crop plan on one of the four edges
//! (replace-gtk-with-wayland D7, kept for the hosted app by
//! replace-terminal-with-app-host D8, four-edge geometry from D9): the
//! per-frame viewport crop that shows the docked-edge portion of the app's
//! wide buffer at the eased size across the edge, and the exact-rational
//! scale rounding that decides it in whole device pixels. Pure and
//! display-free (`port-to-rust` D10): the decisions here are unit tested
//! without a compositor, and the Wayland calls themselves — `set_size`, the
//! margins, `set_source`, `set_destination`, the reserve's zone and the
//! commit — stay with the proxy's frame step. This module only decides what
//! those calls carry.
//!
//! The app draws the wide buffer itself, so the terminal panel's grid
//! painter, its wide draw, the upload and the no-viewporter copy are gone
//! (D8).
//!
//! The rounding rule, stated once: a device size is its logical size times
//! the scale, rounded half up in exact 1/120 rational arithmetic —
//! [`FractionalScale::scale_dimension`]. No `f64` product joins the
//! decision: at a scale like 1.45 the `f64` product of a tie size can round
//! the other way from the exact rational, and one pixel of disagreement
//! between the crop and the buffer it crops would show. The eased size
//! itself is the tween's integer logical size; nothing re-rounds it.
//!
//! The crop is the docked-edge portion of the wide buffer that is the eased
//! size across the edge: a left- or right-docked panel crops the buffer's
//! width and spans its configure height, and a top- or bottom-docked panel
//! crops the height and spans the configure width (D9). A crop that starts
//! at the docked edge starts at device 0 on the left and top edges, and at
//! `wide - crop` device pixels on the right and bottom edges. Every crop
//! boundary is a whole device pixel by construction, so the app's content
//! keeps its offset from the docked edge in every frame — the spec's "Width
//! animation" scenario. The viewport destination is the eased logical size
//! and the configure's size along the edge: the crop mapped back through
//! the scale, which for every scale of at least 1 is exactly the eased
//! size. The surface therefore presents the crop one device pixel to one
//! device pixel, with no resampling blur, and the layer-shell size across
//! the edge stays the size the layout and the IPC already know.
//!
//! `wp_viewport.set_source` carries its arguments as `wl_fixed` (1/256
//! units); wayland-client takes them as `f64` and encodes them as
//! `(value * 256.0) as i32`. A whole device pixel count encodes exactly;
//! `CropRect::wl_fixed` produces that number with a checked multiply, and
//! the tests assert it.

use crate::geometry::Edge;

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
/// edge and the wide buffer's logical size across that edge — the larger of
/// the start and end sizes. Constructed only through [`TweenCrop::new`], so
/// the size is never negative (`port-to-rust` D6: the invariant lives in the
/// constructor, not a check).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TweenCrop {
    edge: Edge,
    wide_px: i32,
}

impl TweenCrop {
    /// The crop geometry of a tween from `start_px` to `end_px` logical
    /// pixels across `edge`. `None` when a size is negative — no panel is
    /// narrower than nothing.
    #[must_use]
    pub fn new(edge: Edge, start_px: i32, end_px: i32) -> Option<Self> {
        if start_px < 0 || end_px < 0 {
            return None;
        }
        Some(TweenCrop {
            edge,
            wide_px: start_px.max(end_px),
        })
    }

    /// The wide buffer's logical size across the edge: the larger of the
    /// start and end sizes. Test builds only: the crop tests read the wide
    /// size back; production reads it through [`Self::frame`].
    #[cfg(test)]
    #[must_use]
    pub const fn wide_px(self) -> i32 {
        self.wide_px
    }

    /// The wide buffer's device size across the edge — the extent of the
    /// buffer the crop rectangle lives in.
    fn wide_device(self, scale: FractionalScale) -> Option<u32> {
        scale.scale_dimension(u32::try_from(self.wide_px).ok()?)
    }

    /// One tween frame's crop plan at the eased size `current_px` (D7): the
    /// layer size to commit, the viewport source rectangle — the docked-edge
    /// crop of the wide buffer, in device pixels — and the viewport
    /// destination. `length` is the latest configure's size along the edge in
    /// logical pixels: the height of a left- or right-docked panel, the width
    /// of a top- or bottom-docked one. The crop spans it fully.
    ///
    /// `None` on a frame nothing can present: a non-positive eased size or a
    /// zero length (the viewport protocol's zero-sized destination is fatal),
    /// an eased size past the wide buffer, or a size that does not fit the
    /// protocol's integers. The caller skips such a frame; the next one or
    /// the tween's end rule closes it.
    #[must_use]
    pub fn frame(self, current_px: i32, length: u32, scale: FractionalScale) -> Option<CropFrame> {
        if current_px <= 0 || current_px > self.wide_px || length == 0 {
            return None;
        }
        let wide_dev = self.wide_device(scale)?;
        let crop_dev = scale.scale_dimension(u32::try_from(current_px).ok()?)?;
        let length_dev = scale.scale_dimension(length)?;
        // Defensive: the eased size never passes the wide one, and the
        // scale's rounding is monotone — but a crop past the buffer would be
        // a protocol error, not a clipped draw.
        if crop_dev > wide_dev {
            return None;
        }
        let wide_dev = i32::try_from(wide_dev).ok()?;
        let crop_dev = i32::try_from(crop_dev).ok()?;
        let length_dev = i32::try_from(length_dev).ok()?;
        let length = i32::try_from(length).ok()?;
        // A right- or bottom-docked crop starts at the far end of the buffer,
        // so its offset moves as the crop shrinks; the other two edges start
        // at the buffer's own origin.
        let offset = match self.edge {
            Edge::Left | Edge::Top => 0,
            Edge::Right | Edge::Bottom => wide_dev - crop_dev,
        };
        let (source, destination) = if self.edge.across_is_width() {
            (
                CropRect {
                    x: offset,
                    y: 0,
                    width: crop_dev,
                    height: length_dev,
                },
                (current_px, length),
            )
        } else {
            (
                CropRect {
                    x: 0,
                    y: offset,
                    width: length_dev,
                    height: crop_dev,
                },
                (length, current_px),
            )
        };
        Some(CropFrame {
            layer_across: current_px,
            source,
            destination,
        })
    }
}

/// One tween frame's crop plan (D7): everything the frame step commits for
/// the tween — the layer size across the edge, the viewport source and
/// destination. Constructed only by [`TweenCrop::frame`] (`port-to-rust`
/// D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropFrame {
    layer_across: i32,
    source: CropRect,
    destination: (i32, i32),
}

impl CropFrame {
    /// The panel's eased size across the docked edge, in logical pixels, for
    /// `set_size` (the size along the edge stays the compositor's, between
    /// the anchors).
    #[must_use]
    pub const fn layer_across(&self) -> i32 {
        self.layer_across
    }

    /// The viewport source rectangle: the docked-edge crop of the wide
    /// buffer, in buffer (device) pixels.
    #[must_use]
    pub const fn source(&self) -> CropRect {
        self.source
    }

    /// The viewport destination: the eased logical size and the configure's
    /// size along the edge, in surface-local (logical) pixels.
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

    /// The four docking edges, for the sweeps.
    const EDGES: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    fn scale(units: u32) -> FractionalScale {
        FractionalScale::from_120ths(units)
    }

    /// A `u32` device size as `i32`, for the tests' arithmetic.
    fn dev(value: u32) -> i32 {
        i32::try_from(value).expect("test device size fits i32")
    }

    /// The crop's offset, extent and spanned size, taken along the tween axis
    /// on `edge`: a side edge crops the width and spans the height, a top or
    /// bottom edge crops the height and spans the width.
    fn axis(source: CropRect, edge: Edge) -> (i32, i32, i32) {
        if edge.across_is_width() {
            (source.x(), source.width(), source.height())
        } else {
            (source.y(), source.height(), source.width())
        }
    }

    /// The viewport destination of an eased size `current` on `edge`, with
    /// the configure's `length` along the edge.
    fn destination(edge: Edge, current: i32, length: i32) -> (i32, i32) {
        if edge.across_is_width() {
            (current, length)
        } else {
            (length, current)
        }
    }

    /// One scale's sweep over one edge's eased sizes: the crop's docked edge,
    /// its extent, the spanned size, the `wl_fixed` encodings and the eased
    /// destination.
    fn sweep(s: FractionalScale, units: u32, edge: Edge) {
        let wide_dev = dev(s.scale_dimension(128).expect("wide fits"));
        let length_dev = dev(s.scale_dimension(720).expect("length fits"));
        let crop = TweenCrop::new(edge, 40, 128).expect("a valid tween");
        assert_eq!(crop.wide_px(), 128, "the wide size is the larger one");
        let mut previous = 0;
        for current in 1..=128 {
            let frame = crop
                .frame(current, 720, s)
                .unwrap_or_else(|| panic!("frame {current} at {units} on {edge:?}"));
            let (offset, extent, spanned) = axis(frame.source(), edge);
            assert!(extent > 0, "the crop is non-empty on {edge:?}");
            assert!(
                extent <= wide_dev,
                "the crop stays inside the buffer on {edge:?}"
            );
            assert_eq!(
                spanned, length_dev,
                "the crop spans the configure length on {edge:?}"
            );
            let docked = if matches!(edge, Edge::Left | Edge::Top) {
                0
            } else {
                wide_dev - extent
            };
            assert_eq!(offset, docked, "{edge:?} crops from its docked edge");
            assert_eq!(
                CropRect::wl_fixed(offset),
                Some(offset.checked_mul(256).expect("test range")),
                "the source offset on {edge:?} encodes exactly"
            );
            assert_eq!(
                CropRect::wl_fixed(extent),
                Some(extent.checked_mul(256).expect("test range")),
                "the source extent on {edge:?} encodes exactly"
            );
            assert!(extent >= previous, "the crop is monotone on {edge:?}");
            previous = extent;
            assert_eq!(
                frame.layer_across(),
                current,
                "the layer size eases along {edge:?}"
            );
            assert_eq!(
                frame.destination(),
                destination(edge, current, 720),
                "the destination is the eased logical size on {edge:?}"
            );
            assert_eq!(
                extent,
                dev(s
                    .scale_dimension(u32::try_from(current).expect("positive"))
                    .expect("crop fits")),
                "the crop is the eased size in device pixels on {edge:?}"
            );
        }
    }

    /// The crop stays whole device pixels across the scales, all four edges
    /// and a sweep of eased sizes including the odd ones: the source
    /// rectangle sits inside the wide buffer with its docked edge at 0 (left,
    /// top) or at `wide - crop` (right, bottom), the destination is the eased
    /// logical size, and the `wl_fixed` encodings are exact.
    #[test]
    fn the_crop_stays_whole_device_pixels_across_scales_edges_and_sizes() {
        for units in [120, 150, 180, 216] {
            for edge in EDGES {
                sweep(scale(units), units, edge);
            }
        }
    }

    /// A shrinking tween stays at the docked edge on all four edges at scale
    /// 1.5, with a whole-device-pixel offset — asserted on the number the
    /// protocol would carry, the `wl_fixed` source coordinate.
    #[test]
    fn a_shrink_stays_at_the_docked_edge_at_1_5_on_every_edge() {
        let s = scale(180);
        // A shrinking tween from 72 to 45 logical px, mid-tween at 41:
        // 41 * 1.5 = 61.5 device px, rounded half up to 62.
        let frame = |edge| {
            TweenCrop::new(edge, 72, 45)
                .expect("a valid tween")
                .frame(41, 720, s)
                .expect("a presentable frame")
        };

        let left = frame(Edge::Left);
        assert_eq!(left.source().x(), 0);
        assert_eq!(left.source().width(), 62);
        assert_eq!(left.destination(), (41, 720));
        assert_eq!(CropRect::wl_fixed(left.source().x()), Some(0));

        let right = frame(Edge::Right);
        // The wide buffer is 108 device px wide; the crop starts 46 device
        // pixels from its left edge — a whole device pixel.
        assert_eq!(right.source().x(), 46);
        assert_eq!(right.source().width(), 62);
        assert_eq!(CropRect::wl_fixed(right.source().x()), Some(46 * 256));
        assert_eq!(right.destination(), (41, 720));

        let top = frame(Edge::Top);
        assert_eq!(top.source().y(), 0);
        assert_eq!(top.source().height(), 62);
        assert_eq!(top.destination(), (720, 41));
        assert_eq!(CropRect::wl_fixed(top.source().y()), Some(0));

        let bottom = frame(Edge::Bottom);
        // The wide buffer is 108 device px tall; the crop starts 46 device
        // pixels from its top edge, so its content stays docked at the
        // bottom.
        assert_eq!(bottom.source().y(), 46);
        assert_eq!(bottom.source().height(), 62);
        assert_eq!(CropRect::wl_fixed(bottom.source().y()), Some(46 * 256));
        assert_eq!(bottom.destination(), (720, 41));
    }

    /// Degenerate sizes never panic: a non-positive eased size, a zero
    /// length and an eased size past the wide buffer refuse the frame on
    /// every edge; equal start and end crop the whole buffer; a start larger
    /// than the end takes the start as the wide size.
    #[test]
    fn degenerate_sizes_refuse_instead_of_panicking() {
        let s = scale(180);

        for edge in EDGES {
            // Negative sizes never build a crop.
            assert!(
                TweenCrop::new(edge, -1, 100).is_none(),
                "a negative start on {edge:?}"
            );
            assert!(
                TweenCrop::new(edge, 100, -1).is_none(),
                "a negative end on {edge:?}"
            );

            // A zero wide buffer builds, but presents nothing.
            let zero = TweenCrop::new(edge, 0, 0).expect("a valid tween");
            assert!(
                zero.frame(0, 720, s).is_none(),
                "a zero size has no crop on {edge:?}"
            );

            // A zero length is no viewport destination.
            let crop = TweenCrop::new(edge, 40, 100).expect("a valid tween");
            assert!(
                crop.frame(60, 0, s).is_none(),
                "a zero length is no destination on {edge:?}"
            );

            // An eased size past the wide buffer is refused.
            assert!(
                crop.frame(200, 720, s).is_none(),
                "a size past the buffer on {edge:?}"
            );

            // Equal start and end: the crop covers the whole buffer.
            let equal = TweenCrop::new(edge, 100, 100).expect("a valid tween");
            let frame = equal.frame(100, 720, s).expect("a presentable frame");
            assert_eq!(equal.wide_px(), 100, "the wide size on {edge:?}");
            let (offset, extent, _) = axis(frame.source(), edge);
            assert_eq!(offset, 0, "the crop covers the whole buffer on {edge:?}");
            assert_eq!(extent, dev(s.scale_dimension(100).expect("fits")));

            // A start larger than the end takes the start as the wide size.
            let shrinking = TweenCrop::new(edge, 100, 40).expect("a valid tween");
            assert_eq!(shrinking.wide_px(), 100);
            assert!(
                shrinking.frame(60, 720, s).is_some(),
                "a mid-tween frame on {edge:?}"
            );
        }
    }
}
