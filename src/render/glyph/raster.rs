//! The rasterizer: one glyph id of one face at one device size, in one
//! synthesized style, with an optional placement transform, rasterized
//! with swash into a coverage mask or a colour image.

use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source, StrikeWith, image::Content};

use super::error::GlyphError;
use super::request::{GlyphRequest, PlacementTransform};

/// One rasterized glyph: the image — a coverage mask for a plain glyph, an
/// RGBA pixmap for a colour glyph, or nothing for a blank glyph such as a
/// space — and where the image sits relative to the glyph origin.
#[derive(Debug)]
pub struct Glyph {
    image: GlyphImage,
    placement: GlyphPlacement,
}

impl Glyph {
    /// An empty glyph: nothing to draw, the origin untouched. Also what a
    /// swash render miss (a glyph id with no renderable image) becomes.
    #[must_use]
    pub fn empty() -> Self {
        Glyph {
            image: GlyphImage::Empty,
            placement: GlyphPlacement::ZERO,
        }
    }

    /// The glyph's image.
    #[must_use]
    pub fn image(&self) -> &GlyphImage {
        &self.image
    }

    /// Where the image's top-left corner sits relative to the glyph origin
    /// on the baseline.
    #[must_use]
    pub fn placement(&self) -> GlyphPlacement {
        self.placement
    }

    /// The image's byte cost, what the cache's byte budget counts: a mask
    /// is one byte per pixel, a colour image four.
    pub(crate) fn byte_cost(&self) -> usize {
        match &self.image {
            GlyphImage::Empty => 0,
            GlyphImage::Mask(mask) => mask.width() * mask.height(),
            GlyphImage::Color(pixmap) => {
                usize::try_from(pixmap.width() * pixmap.height() * 4).unwrap_or(0)
            }
        }
    }
}

/// What a glyph rasterized into. The variants are `pub` because the painter
/// (row 4.5's placement unit) matches on them to pick the draw call.
#[derive(Debug)]
pub enum GlyphImage {
    /// No image: the glyph has no coverage (a space, or a glyph id swash
    /// cannot render).
    Empty,
    /// An 8-bit coverage mask, row major from the top left, for the painter
    /// to tint with the cell's foreground ([`CanvasMask`]).
    ///
    /// [`CanvasMask`]: crate::render::canvas::CanvasMask
    Mask(crate::render::canvas::CanvasMask),
    /// A colour glyph as a premultiplied, channel-swapped tiny-skia pixmap,
    /// exactly what [`Canvas::draw_image`] blits ([`image_pixmap`] built
    /// it).
    ///
    /// [`Canvas::draw_image`]: crate::render::canvas::Canvas::draw_image
    /// [`image_pixmap`]: crate::render::canvas::image_pixmap
    Color(tiny_skia::Pixmap),
}

/// Where a glyph image's top-left corner sits relative to the glyph origin
/// on the baseline, in whole device pixels. `left` is positive to the
/// right; `top` is the distance from the baseline up to the image's top
/// edge, positive upward — swash's own report (`zeno`'s `Placement` with a
/// bottom-left origin, and the CBDT bitmap metrics' y-up convention), kept
/// as it is so the placement unit (U18) does one conversion, at the draw
/// call: the image's top-left corner lands at `(origin_x + left,
/// origin_y - top)` in the canvas' y-down coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphPlacement {
    left: i32,
    top: i32,
}

impl GlyphPlacement {
    /// The zero placement of an empty glyph.
    pub(crate) const ZERO: Self = GlyphPlacement { left: 0, top: 0 };

    /// A placement from its parts (the tests use it).
    #[must_use]
    pub fn new(left: i32, top: i32) -> Self {
        GlyphPlacement { left, top }
    }

    /// The horizontal offset of the image's left edge from the origin,
    /// positive to the right.
    #[must_use]
    pub fn left(self) -> i32 {
        self.left
    }

    /// The distance from the baseline up to the image's top edge, positive
    /// upward (see the type's doc for the draw conversion).
    #[must_use]
    pub fn top(self) -> i32 {
        self.top
    }
}

/// The synthesized-italic skew: `FreeType`'s `FT_GlyphSlot_Oblique` shears
/// the outline by `transform.xy = 0x0366A` in 16.16 fixed point —
/// 0.2126, the tangent of 12 degrees — and Pango's synthesized italic goes
/// through `FreeType`'s oblique, so 12 degrees is the angle the old Pango
/// path drew with too. The shear is `x' = x + tan(12°) * y` in the
/// outline's y-up coordinates, so the top leans right.
const SKEW_TAN_12_DEGREES: f64 = 0.212_556_561_670_022_14;

/// The synthesized-bold strength: `FreeType`'s `FT_GlyphSlot_Embolden` uses
/// `FT_MulFix(units_per_EM, y_scale) / 24` on both axes — the ppem over
/// 24, in device pixels — and Pango's synthesized bold goes through it,
/// so the old Pango path emboldened with the same strength. zeno's
/// `embolden` takes that outward offset in pixels, once per axis.
const EMBOLDEN_DIVISOR: f64 = 24.0;

/// The source priority the renderer walks: layered colour outlines first
/// (a COLR face), then colour bitmaps at the best-fitting strike (a CBDT
/// or sbix face — Noto Color Emoji), then the plain outline. A face with
/// none of the first two falls through to the outline. A `static`, because
/// `Render::new` borrows the slice for the render's lifetime and a `const`
/// would hand it a temporary.
static SOURCES: [Source; 3] = [
    Source::ColorOutline(0),
    Source::ColorBitmap(StrikeWith::BestFit),
    Source::Outline,
];

/// Rasterize `request` through `context` (the shared swash scaler scratch
/// state). The result is a fresh [`Glyph`] every call; the cache owns the
/// sharing. Errors are typed, never panics; a glyph swash cannot render is
/// an empty result, not an error.
pub(crate) fn rasterize(
    request: &GlyphRequest,
    context: &mut ScaleContext,
) -> Result<Glyph, GlyphError> {
    let font = FontRef::from_index(request.bytes().as_ref(), request.face_identity().index())
        .ok_or(GlyphError::UnparsableFace)?;
    let coords = instance_coords(&font, request)?;
    let ppem = request.ppem();

    // A scaler carries no state of its own beyond the build; swash's caches
    // (the font proxy, the hinting instances, the scratch buffers) live in
    // the context, keyed on the stable face id, so repeated glyphs of one
    // face never rebuild them.
    let mut scaler = context
        .builder_with_id(font, request.face_identity().swash_id())
        .size(ppem.value())
        .hint(true)
        .normalized_coords(coords)
        .build();

    // The builder methods return `&mut Self`, so the `Render` value needs
    // its own binding before the chained options borrow it.
    let mut builder = Render::new(&SOURCES);
    let render = builder.format(swash::zeno::Format::Alpha);
    if request.synthesis().bold() {
        render.embolden(crate::render::geom::device_f32(
            f64::from(ppem.units()) / 64.0 / EMBOLDEN_DIVISOR,
        ));
    }
    render.transform(outline_transform(request));

    let Some(image) = render.render(&mut scaler, request.glyph_id()) else {
        return Ok(Glyph::empty());
    };
    let placement = image.placement;
    let glyph_image = match image.content {
        Content::Mask => {
            if placement.width == 0 || placement.height == 0 {
                return Ok(Glyph::empty());
            }
            let mask = crate::render::canvas::CanvasMask::new(
                placement.width,
                placement.height,
                &image.data,
            )
            .ok_or(GlyphError::BadImage("mask size does not match its data"))?;
            GlyphImage::Mask(mask)
        }
        // swash's `Format::Alpha` never produces a subpixel mask.
        Content::SubpixelMask => return Err(GlyphError::BadImage("unexpected subpixel mask")),
        Content::Color => {
            if placement.width == 0 || placement.height == 0 {
                return Ok(Glyph::empty());
            }
            let rgba = colour_rgba(&image)?;
            let pixmap =
                crate::render::canvas::image_pixmap(&rgba, placement.width, placement.height)
                    .ok_or(GlyphError::BadImage(
                        "colour image size does not match its data",
                    ))?;
            GlyphImage::Color(pixmap)
        }
    };
    Ok(Glyph {
        image: glyph_image,
        placement: GlyphPlacement {
            left: placement.left,
            top: placement.top,
        },
    })
}

/// The named instance's normalized coordinates, or none for the default
/// instance. The obligation from row 4.4's review lands here:
/// `Face::parse` always opens the default instance, so a variable-only
/// family's matched Bold face must carry its named instance's coordinates
/// into the scaler, and the instance is part of the cache key (the face
/// identity carries it).
fn instance_coords(
    font: &FontRef<'_>,
    request: &GlyphRequest,
) -> Result<Vec<swash::NormalizedCoord>, GlyphError> {
    let Some(instance) = request.face_identity().instance() else {
        return Ok(Vec::new());
    };
    let mut instances = font.instances();
    let available = instances.len();
    let instance = instances.nth(instance).ok_or(GlyphError::UnknownInstance {
        index: instance,
        available,
    })?;
    Ok(instance.normalized_coords().collect())
}

/// The transform swash applies to the scaled, y-up outline before
/// rasterizing: the synthesized-italic skew first (`FreeType` shears the
/// finished outline), then the placement transform's scale about the
/// origin and offset from it. Both are rasterized at the final geometry —
/// a scaled glyph is never a resized bitmap. swash applies transforms to
/// outlines only; a bitmap glyph (a CBDT emoji) ignores them, and so does
/// its placement, which swash scales with the strike's own ppem ratio.
fn outline_transform(request: &GlyphRequest) -> Option<swash::zeno::Transform> {
    let placement = request.transform().map(PlacementTransform::zeno);
    if !request.synthesis().italic() {
        return placement;
    }
    let skew = swash::zeno::Transform::new(
        1.0,
        0.0,
        crate::render::geom::device_f32(SKEW_TAN_12_DEGREES),
        1.0,
        0.0,
        0.0,
    );
    // `then` composes left-to-right: skew first, then the placement.
    Some(match placement {
        Some(placement) => skew.then(&placement),
        None => skew,
    })
}

/// The colour image's straight (non-premultiplied) RGBA, the form
/// [`image_pixmap`] takes for its one-time premultiplication and
/// channel swap. The two swash colour paths disagree on the alpha
/// convention: the layered colour outline composites its layers with the
/// premultiplied-over formula and returns premultiplied RGBA, while the
/// embedded colour bitmaps (CBDT/sbix, PNG-decoded) come back straight —
/// so the outline path is un-premultiplied here and the bitmap path passes
/// through.
///
/// [`image_pixmap`]: crate::render::canvas::image_pixmap
fn colour_rgba(image: &swash::scale::image::Image) -> Result<Vec<u8>, GlyphError> {
    let height = usize::try_from(image.placement.height)
        .ok()
        .ok_or(GlyphError::BadImage("colour image size overflows usize"))?;
    let expected = usize::try_from(image.placement.width)
        .ok()
        .ok_or(GlyphError::BadImage("colour image size overflows usize"))?
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(GlyphError::BadImage("colour image size overflows usize"))?;
    if image.data.len() != expected {
        return Err(GlyphError::BadImage(
            "colour image size does not match its data",
        ));
    }
    match image.source {
        Source::ColorOutline(_) => Ok(premultiplied_to_straight(&image.data)),
        _ => Ok(image.data.clone()),
    }
}

/// Un-premultiply one premultiplied RGBA buffer into straight RGBA: each
/// channel is `c * 255 / a`, rounded. A zero alpha keeps the channels at
/// zero (the only straight colour a transparent pixel could carry).
/// Round-to-nearest through `u16` arithmetic: the product of a channel and
/// 255 cannot overflow, so the division is the only rounding.
pub(crate) fn premultiplied_to_straight(data: &[u8]) -> Vec<u8> {
    data.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            if a == 0 {
                [0, 0, 0, 0]
            } else {
                let scale = |c: u8| {
                    u8::try_from((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a))
                        .unwrap_or(255)
                };
                [scale(r), scale(g), scale(b), 255]
            }
        })
        .collect()
}
