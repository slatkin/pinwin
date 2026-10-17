//! The panel viewport composition (replace-terminal-with-app-host D8): the
//! values the proxy's own `wp_viewport` carries on the panel surface — the
//! source rectangle in the app's buffer coordinates and the destination size
//! it is drawn at — composed from the visible fraction of the tween, the
//! docked edge, the app's own `wp_viewport` values and the buffer the app
//! attached.
//!
//! D8's one-viewport rule gives a surface at most one `wp_viewport`, and the
//! proxy owns the panel surface's. An app's viewport on that surface is
//! therefore kept local ([`super::viewport`]) and its mapping is folded into
//! the proxy's values here: the visible fraction of the destination length
//! maps to the same fraction of the source length, taken from the docked
//! edge, so the app's content never slides (spec "Content does not slide").
//!
//! The source stays in the viewporter's own coordinate space, the one after
//! the buffer transform and scale: the compositor multiplies by
//! `wl_surface.set_buffer_scale` itself, so a crop computed there would zoom
//! the panel (D8). The source is clamped to the attached buffer, because a
//! source past it is the fatal `out_of_buffer` error; a buffer the proxy
//! never saw created bounds nothing, and the whole buffer draws instead.
//!
//! Every decision is exact-rational or a whole pixel of the protocol: no
//! `f64` product joins it, so a tie at a fractional scale rounds the same way
//! in every frame. The module
//! is display-free (`port-to-rust` D10), so its matrix is unit tested
//! without a compositor.
//!
//! The visible fraction is [`Fraction::whole`] for a change that is not
//! animated; the tween driver (row 10.1) passes each frame's fraction.

use wl_proxy::fixed::Fixed;

use crate::geometry::Edge;

/// The `wl_fixed` denominator: a viewport source rectangle is carried in
/// 1/256ths of a surface-local pixel.
const FIXED_ONE: i32 = 256;

/// The same denominator as a source length, for the axes' arithmetic.
const FIXED_ONE_SOURCE: i64 = 256;

/// The fractional-scale protocol's denominator (D8): `preferred_scale` is a
/// whole number of 1/120ths.
const FRACTIONAL_SCALE_ONE: i64 = 120;

/// The value the viewporter reads as "unset" in every field of a source
/// rectangle: `-1.0` in `wl_fixed` units.
pub(super) const UNSET_FIXED: i32 = -256;

/// The visible fraction of the app's surface one composition shows (D8):
/// an exact rational, so the tween's eased width maps to a whole number of
/// source pixels with no rounding drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fraction {
    numerator: u32,
    denominator: u32,
}

impl Fraction {
    /// The whole of it: a change that is not animated shows all of the
    /// surface the app drew.
    #[must_use]
    pub const fn whole() -> Self {
        Self {
            numerator: 1,
            denominator: 1,
        }
    }

    /// `visible` of `whole` surface pixels, clamped to the whole — a frame
    /// never shows more than the surface it crops. `None` when `whole` is
    /// zero, which is no fraction of anything.
    #[must_use]
    pub fn of(visible: u32, whole: u32) -> Option<Self> {
        if whole == 0 {
            return None;
        }
        Some(Self {
            numerator: visible.min(whole),
            denominator: whole,
        })
    }

    /// The length this fraction covers of `length` surface pixels, rounded
    /// half away from zero in exact rational arithmetic.
    fn of_length(self, length: u32) -> u32 {
        let scaled = mul_div(
            i64::from(length),
            i64::from(self.numerator),
            i64::from(self.denominator),
        );
        u32::try_from(scaled).unwrap_or(u32::MAX)
    }
}

/// The app's own `wp_viewport` values on one surface (D8), kept local by
/// the proxy: the source rectangle in the app's buffer coordinates and the
/// destination size it maps onto, each unset until the app sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppViewport {
    source: Option<[Fixed; 4]>,
    destination: Option<(i32, i32)>,
}

impl AppViewport {
    /// An app that has set no viewport values yet.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            source: None,
            destination: None,
        }
    }

    /// The values with the app's `set_source` applied; `None` for the
    /// `-1.0` form the viewporter reads as unset.
    #[must_use]
    pub const fn with_source(self, source: Option<[Fixed; 4]>) -> Self {
        Self {
            source,
            destination: self.destination,
        }
    }

    /// The values with the app's `set_destination` applied; `None` for the
    /// `-1` form the viewporter reads as unset.
    #[must_use]
    pub const fn with_destination(self, destination: Option<(i32, i32)>) -> Self {
        Self {
            source: self.source,
            destination,
        }
    }

    /// The source rectangle the app set, in the coordinates the viewporter
    /// reads.
    #[must_use]
    pub const fn source(self) -> Option<[Fixed; 4]> {
        self.source
    }

    /// The destination size the app set.
    #[must_use]
    pub const fn destination(self) -> Option<(i32, i32)> {
        self.destination
    }

    /// The app's own source with the destination it is drawn at (D8), when
    /// the source wins. A source with no destination crops without scaling,
    /// so the surface size becomes the source's own size, which the
    /// compositor only accepts in whole pixels (`bad_size` otherwise).
    fn mapping(self) -> Option<([Fixed; 4], (i32, i32))> {
        let [x, y, width, height] = self.source?;
        if x.to_wire() < 0 || y.to_wire() < 0 || width.to_wire() <= 0 || height.to_wire() <= 0 {
            return None;
        }
        if let Some(destination) = self.usable_destination() {
            return Some(([x, y, width, height], destination));
        }
        let destination_width = whole_source_units(width)?;
        let destination_height = whole_source_units(height)?;
        Some((
            [x, y, width, height],
            (destination_width, destination_height),
        ))
    }

    /// The app's destination when it is a size the protocol accepts: a zero
    /// or negative side is the `bad_value` error, so it is no destination.
    fn usable_destination(self) -> Option<(i32, i32)> {
        let (width, height) = self.destination?;
        (width > 0 && height > 0).then_some((width, height))
    }
}

/// The whole-pixel value of a source length in `wl_fixed` units: `None` for
/// a fractional length, which the viewporter refuses as a destination of its
/// own (`bad_size`).
fn whole_source_units(length: Fixed) -> Option<i32> {
    let pixel = length.to_wire() / FIXED_ONE;
    (pixel * FIXED_ONE == length.to_wire()).then_some(pixel)
}

/// One composition's inputs (D8).
#[derive(Clone, Copy, Debug)]
pub struct Compose {
    /// The edge the panel is docked at: the edge the crop is taken from.
    pub edge: Edge,
    /// The visible fraction of the app's surface along the tween axis.
    pub fraction: Fraction,
    /// The app surface's extent in surface-local pixels: the size its window
    /// geometry reports, or the panel's configured size when it reports
    /// none.
    pub surface: (u32, u32),
    /// The app surface rect's origin in surface-local pixels: the window
    /// geometry's origin, or the surface's own when it reports none.
    pub origin: (i32, i32),
    /// The app's own `wp_viewport` values on this surface (D8).
    pub app: AppViewport,
    /// The compositor's `wp_fractional_scale_v1.preferred_scale` for the
    /// surface, in 1/120ths.
    pub preferred_scale: Option<u32>,
    /// The attached buffer's pixel size, when the proxy saw it created.
    pub buffer: Option<(u32, u32)>,
    /// The surface's `wl_surface.set_buffer_scale`, at least 1.
    pub buffer_scale: u32,
}

/// The viewport values one composition decides (D8): the source rectangle in
/// the app's buffer coordinates — `None` draws the whole buffer — and the
/// destination size it is drawn at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Composition {
    source: Option<[i32; 4]>,
    destination: (i32, i32),
}

impl Composition {
    /// The source rectangle, in the viewporter's coordinates.
    #[must_use]
    pub const fn source(self) -> Option<[i32; 4]> {
        self.source
    }

    /// The destination size, in surface-local pixels.
    #[must_use]
    pub const fn destination(self) -> (i32, i32) {
        self.destination
    }
}

/// The viewport values of the panel surface at `input`'s visible fraction
/// (D8).
///
/// Without an app viewport the source is the surface's own rect, one source
/// unit per surface pixel. A destination-only app viewport maps the whole
/// buffer onto its destination, so the app draws `preferred_scale / 120`
/// buffer units per surface pixel; an app source wins over both and is
/// scaled by the ratio between it and its destination. Whatever the mapping,
/// the crop is taken from the docked edge and clamped to the attached
/// buffer, and its source stays in the viewporter's coordinates.
///
/// `None` when the surface has no visible rect at all — a collapsed window
/// geometry or a destination the window lies entirely outside of.
#[must_use]
pub fn compose(input: &Compose) -> Option<Composition> {
    let surface_width = i64::from(input.surface.0.max(1));
    let surface_height = i64::from(input.surface.1.max(1));
    let mut left = i64::from(input.origin.0);
    let mut top = i64::from(input.origin.1);
    let mut right = left + surface_width;
    let mut bottom = top + surface_height;

    let (x_axis, y_axis) = match input.app.mapping() {
        Some(([x, y, width, height], destination)) => {
            // The app's destination is its surface's size (viewporter), so
            // the visible rect stops there whatever the window reports.
            let (destination_width, destination_height) = destination;
            left = left.max(0);
            top = top.max(0);
            right = right.min(i64::from(destination_width));
            bottom = bottom.min(i64::from(destination_height));
            (
                Axis::mapped(x, i64::from(width.to_wire()), destination_width),
                Axis::mapped(y, i64::from(height.to_wire()), destination_height),
            )
        }
        None => match input.app.usable_destination() {
            Some((destination_width, destination_height)) => {
                left = left.max(0);
                top = top.max(0);
                right = right.min(i64::from(destination_width));
                bottom = bottom.min(i64::from(destination_height));
                let axis = Axis::of_preferred_scale(input.preferred_scale);
                (axis, axis)
            }
            None => (Axis::IDENTITY, Axis::IDENTITY),
        },
    };
    if right <= left || bottom <= top {
        return None;
    }

    let (across_length, along_length) = if input.edge.across_is_width() {
        (right - left, bottom - top)
    } else {
        (bottom - top, right - left)
    };
    let visible = i64::from(
        input
            .fraction
            .of_length(u32::try_from(across_length).ok()?)
            .max(1),
    );
    let crop = match input.edge {
        Edge::Left => [left, top, visible, along_length],
        Edge::Right => [right - visible, top, visible, along_length],
        Edge::Top => [left, top, along_length, visible],
        Edge::Bottom => [left, bottom - visible, along_length, visible],
    };

    // The buffer bounds the source: past it the compositor raises
    // `out_of_buffer`, which ends the whole panel (D8).
    let bounds = input.buffer.map(|(width, height)| {
        let scale = i64::from(input.buffer_scale.max(1));
        (i64::from(width) / scale, i64::from(height) / scale)
    });
    let source = bounds.and_then(|(bound_width, bound_height)| {
        let left = whole_pixels(x_axis.to_source(crop[0]).max(0));
        let top = whole_pixels(y_axis.to_source(crop[1]).max(0));
        let right = whole_pixels(x_axis.to_source(crop[0] + crop[2])).min(bound_width);
        let bottom = whole_pixels(y_axis.to_source(crop[1] + crop[3])).min(bound_height);
        (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
    });
    let destination = match source {
        // The destination is the crop mapped back through the app's mapping,
        // so the source draws one source unit to one surface pixel and a
        // clamp inside the buffer never stretches what remains.
        Some([_, _, width, height]) => (
            x_axis.to_surface(width * i64::from(FIXED_ONE)),
            y_axis.to_surface(height * i64::from(FIXED_ONE)),
        ),
        None => match input.edge {
            Edge::Left | Edge::Right => (visible, along_length),
            Edge::Top | Edge::Bottom => (along_length, visible),
        },
    };
    Some(Composition {
        source: source.map(|[x, y, width, height]| {
            [protocol(x), protocol(y), protocol(width), protocol(height)]
        }),
        destination: (
            protocol(destination.0.max(1)),
            protocol(destination.1.max(1)),
        ),
    })
}

/// One axis of the app's own mapping (D8): a surface-local pixel to the
/// app's source rectangle, held as an exact rational in `wl_fixed` units so
/// no `f64` product joins the decision.
#[derive(Clone, Copy, Debug)]
struct Axis {
    /// The source coordinate the surface origin maps to, in 1/256ths.
    origin: i64,
    /// Source units (1/256ths of a pixel) per surface pixel.
    source_units: i64,
    /// Surface pixels per source unit.
    surface_units: i64,
}

impl Axis {
    /// The axis without an app mapping: one source unit per surface pixel.
    const IDENTITY: Self = Self {
        origin: 0,
        source_units: FIXED_ONE_SOURCE,
        surface_units: 1,
    };

    /// The axis of an app source of `source_units` (in 1/256ths) drawn at
    /// `destination` pixels.
    fn mapped(origin: Fixed, source_units: i64, destination: i32) -> Self {
        Self {
            origin: i64::from(origin.to_wire()),
            source_units,
            surface_units: i64::from(destination),
        }
    }

    /// The axis of a destination-only app viewport (D8): the app maps its
    /// whole buffer onto the destination, which is `preferred_scale / 120`
    /// source units per surface pixel. No preferred scale means no scaling.
    fn of_preferred_scale(scale: Option<u32>) -> Self {
        match scale {
            Some(scale) if scale > 0 => Self {
                origin: 0,
                source_units: i64::from(scale) * i64::from(FIXED_ONE),
                surface_units: FRACTIONAL_SCALE_ONE,
            },
            _ => Self::IDENTITY,
        }
    }

    /// The source coordinate of a surface pixel.
    fn to_source(self, pixel: i64) -> i64 {
        self.origin + mul_div(pixel, self.source_units, self.surface_units)
    }

    /// The surface length of a source length.
    fn to_surface(self, source_units: i64) -> i64 {
        mul_div(source_units, self.surface_units, self.source_units)
    }
}

/// A source length in `wl_fixed` units as a whole number of source pixels,
/// rounded half away from zero.
fn whole_pixels(source_units: i64) -> i64 {
    mul_div(source_units, 1, i64::from(FIXED_ONE))
}

/// `value * numerator / denominator`, rounded half away from zero in exact
/// rational arithmetic. `denominator` is positive and `numerator` is not
/// negative for every call here.
fn mul_div(value: i64, numerator: i64, denominator: i64) -> i64 {
    let product = i128::from(value) * i128::from(numerator);
    let denominator = i128::from(denominator);
    let doubled = denominator * 2;
    let rounded = if product < 0 {
        (product * 2 - denominator) / doubled
    } else {
        (product * 2 + denominator) / doubled
    };
    i64::try_from(rounded).unwrap_or(i64::MAX)
}

/// A whole-pixel source value, saturated at the protocol's `i32` range. Every
/// value that reaches this is a non-negative pixel count of a surface the
/// protocol carries as `i32`.
fn protocol(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{AppViewport, Compose, Fraction, compose};
    use crate::geometry::Edge;
    use wl_proxy::fixed::Fixed;

    /// The four docking edges, for the sweeps.
    const EDGES: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    /// A source rectangle in `wl_fixed` units, from whole pixels.
    fn source(x: i32, y: i32, width: i32, height: i32) -> [Fixed; 4] {
        [
            Fixed::from_wire(x * 256),
            Fixed::from_wire(y * 256),
            Fixed::from_wire(width * 256),
            Fixed::from_wire(height * 256),
        ]
    }

    /// A 640x480 surface of a 640x480 buffer with no app viewport, on
    /// `edge` at `fraction`.
    fn input(edge: Edge, fraction: Fraction) -> Compose {
        Compose {
            edge,
            fraction,
            surface: (640, 480),
            origin: (0, 0),
            app: AppViewport::none(),
            preferred_scale: None,
            buffer: Some((640, 480)),
            buffer_scale: 1,
        }
    }

    /// The whole surface at fraction 1: the source is the surface rect and
    /// the destination its own size, on every edge (D8, no app viewport).
    #[test]
    fn a_static_change_shows_the_whole_surface_on_every_edge() {
        for edge in EDGES {
            let composition = compose(&input(edge, Fraction::whole())).expect("a visible surface");
            assert_eq!(composition.source(), Some([0, 0, 640, 480]), "{edge:?}");
            assert_eq!(composition.destination(), (640, 480), "{edge:?}");
        }
    }

    /// The visible fraction of the destination length is the same fraction of
    /// the source length, taken from the docked edge, so the app's content
    /// stays against that edge (spec "Content does not slide").
    #[test]
    fn the_visible_part_is_taken_from_the_docked_edge() {
        let half = Fraction::of(1, 2).expect("half");
        let cases = [
            (Edge::Left, Some([0, 0, 320, 480]), (320, 480)),
            (Edge::Right, Some([320, 0, 320, 480]), (320, 480)),
            (Edge::Top, Some([0, 0, 640, 240]), (640, 240)),
            (Edge::Bottom, Some([0, 240, 640, 240]), (640, 240)),
        ];
        for (edge, source, destination) in cases {
            let composition = compose(&input(edge, half)).expect("a visible surface");
            assert_eq!(composition.source(), source, "{edge:?}");
            assert_eq!(composition.destination(), destination, "{edge:?}");
        }
    }

    /// The window geometry is the surface the crop works in (D5): its origin
    /// moves the source, so a client-side title bar and shadow inside the
    /// buffer stay outside the panel.
    #[test]
    fn the_window_geometry_origin_moves_the_source() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.origin = (5, 7);
        compose_input.buffer = Some((700, 520));
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([5, 7, 640, 480]));
        assert_eq!(composition.destination(), (640, 480));
    }

    /// A negative geometry origin, which a client-side drop shadow reports,
    /// clamps to the buffer's origin and shortens the destination with it,
    /// so the window keeps drawing 1:1 (D5).
    #[test]
    fn a_negative_origin_clamps_to_the_buffer_origin() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.origin = (-20, -30);
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([0, 0, 620, 450]));
        assert_eq!(composition.destination(), (620, 450));
    }

    /// An app source plus its destination wins over the surface rect (D8),
    /// so a fractional-scale app draws its whole buffer at its own mapping.
    #[test]
    fn an_app_source_is_scaled_onto_its_destination() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.buffer = Some((960, 720));
        compose_input.app = AppViewport::none()
            .with_source(Some(source(0, 0, 960, 720)))
            .with_destination(Some((640, 480)));
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([0, 0, 960, 720]));
        assert_eq!(composition.destination(), (640, 480));

        compose_input.fraction = Fraction::of(1, 2).expect("half");
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([0, 0, 480, 720]));
        assert_eq!(composition.destination(), (320, 480));
    }

    /// A source with no destination crops without scaling, so its own size
    /// becomes the surface size and the source is drawn one to one.
    #[test]
    fn a_source_without_a_destination_crops_without_scaling() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.surface = (320, 240);
        compose_input.app = AppViewport::none().with_source(Some(source(10, 20, 320, 240)));
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([10, 20, 320, 240]));
        assert_eq!(composition.destination(), (320, 240));
    }

    /// A destination-only app viewport maps the whole buffer onto the
    /// destination, so the ratio is `preferred_scale / 120` (D8): a 1.5x
    /// output draws 1.5 buffer pixels per surface pixel.
    #[test]
    fn a_destination_only_viewport_maps_the_whole_buffer() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.preferred_scale = Some(180);
        compose_input.buffer = Some((960, 720));
        compose_input.app = AppViewport::none().with_destination(Some((640, 480)));
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([0, 0, 960, 720]));
        assert_eq!(composition.destination(), (640, 480));

        compose_input.fraction = Fraction::of(1, 2).expect("half");
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([0, 0, 480, 720]));
        assert_eq!(composition.destination(), (320, 480));
    }

    /// A destination-only viewport without a preferred scale is no scaling at
    /// all (D8): the whole buffer is one surface pixel per source pixel.
    #[test]
    fn a_destination_only_viewport_without_a_scale_is_identity() {
        let mut compose_input = input(Edge::Top, Fraction::whole());
        compose_input.app = AppViewport::none().with_destination(Some((640, 480)));
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([0, 0, 640, 480]));
        assert_eq!(composition.destination(), (640, 480));
    }

    /// The source is clamped to the attached buffer, and the destination
    /// shrinks with it, so no frame asks for a source past the buffer — the
    /// fatal `out_of_buffer` (D8).
    #[test]
    fn the_source_is_clamped_to_the_buffer() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.app = AppViewport::none()
            .with_source(Some(source(0, 0, 2000, 720)))
            .with_destination(Some((640, 480)));
        compose_input.buffer = Some((900, 720));
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([0, 0, 900, 720]));
        assert_eq!(composition.destination(), (288, 480));
    }

    /// The buffer's scale is the compositor's, not the crop's: the source
    /// stays in the viewporter's coordinates and the bound is the buffer
    /// divided by the scale (D8).
    #[test]
    fn the_buffer_is_bounded_in_source_coordinates() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.buffer = Some((1280, 960));
        compose_input.buffer_scale = 2;
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), Some([0, 0, 640, 480]));
        assert_eq!(composition.destination(), (640, 480));
    }

    /// A buffer the proxy never saw created bounds nothing: the whole buffer
    /// draws (no source), at the visible size (D8).
    #[test]
    fn an_unknown_buffer_draws_the_whole_buffer() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.buffer = None;
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), None);
        assert_eq!(composition.destination(), (640, 480));

        compose_input.fraction = Fraction::of(1, 2).expect("half");
        let composition = compose(&compose_input).expect("a visible surface");
        assert_eq!(composition.source(), None);
        assert_eq!(composition.destination(), (320, 480));
    }

    /// A fraction of nothing still shows the one pixel the viewporter
    /// accepts as a destination, so no frame is a protocol error.
    #[test]
    fn a_fraction_of_nothing_keeps_one_pixel() {
        let composition = compose(&input(Edge::Left, Fraction::of(0, 100).expect("zero")));
        assert_eq!(
            composition.expect("a visible surface").source(),
            Some([0, 0, 1, 480])
        );
        assert_eq!(
            composition.expect("a visible surface").destination(),
            (1, 480)
        );
    }

    /// A collapsed window geometry leaves nothing to draw.
    #[test]
    fn a_collapsed_surface_has_no_composition() {
        let mut compose_input = input(Edge::Left, Fraction::whole());
        compose_input.surface = (0, 0);
        compose_input.origin = (-20, -30);
        compose_input.app = AppViewport::none().with_destination(Some((10, 10)));
        assert_eq!(compose(&compose_input), None);
    }
}
