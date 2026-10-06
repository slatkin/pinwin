//! The glyph request: which face, which glyph, at which device size, in
//! which synthesized style, with which placement transform. Every field is
//! quantized into a hashable form, because the request is the cache key.

use std::sync::Arc;

use super::super::font::Face;
use super::error::GlyphError;
use super::identity::FaceIdentity;

/// The device pixels-per-em a glyph rasterizes at, quantized to `FreeType`'s
/// 26.6 fixed point (whole 1/64-px units, the same quantization the cell
/// metrics use). The ppem arrives as `points * 96 / 72` times the output
/// scale, so at the fractional scales niri reports it is a value like
/// 14.6667 by 1.5 = 22.0 or 26.4 by 1.8; float dust from that arithmetic
/// must not split one size into two cache keys or two rasterizations, and
/// 1/64 px is far below anything the rasterizer could resolve anyway. The
/// same quantization bounds the size swash accepts: its scaler takes the
/// ppem as `f32`, whose 24-bit mantissa cannot represent a 1/64-px
/// distinction beyond 2^17 px either.
///
/// The range is the 26.6 range (0 to 65535 px), which the cell metrics
/// already refuse to exceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ppem {
    units: i32,
}

impl Ppem {
    /// Quantize a device pixels-per-em into 26.6 fixed point. An error when
    /// the value is not finite and positive, or leaves the 26.6 range.
    pub fn from_px(px: f64) -> Result<Self, GlyphError> {
        if !px.is_finite() || px <= 0.0 {
            return Err(GlyphError::BadPpem(px));
        }
        let units = px * 64.0;
        if units > 65535.0 * 64.0 {
            return Err(GlyphError::BadPpem(px));
        }
        let units = num_traits::cast(units.round()).ok_or(GlyphError::BadPpem(px))?;
        Ok(Ppem { units })
    }

    /// The quantized 1/64-px units, the cache-key form.
    #[must_use]
    pub fn units(self) -> i32 {
        self.units
    }

    /// The ppem as swash takes it (`f32`): the exact quantized value, so
    /// the rasterization always matches the key it is cached under.
    #[must_use]
    pub fn value(self) -> f32 {
        crate::render::geom::device_f32(f64::from(self.units) / 64.0)
    }
}

/// Which styles to synthesize when the face does not really carry them
/// (`FamilyFaces` reports a missing style as `None`, and row 4.5
/// synthesizes those glyphs, as Pango does today). Both flags default to
/// off, and a face that really has the style never asks for synthesis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Synthesis {
    bold: bool,
    italic: bool,
}

impl Synthesis {
    /// A synthesis request from the two flags.
    #[must_use]
    pub fn new(bold: bool, italic: bool) -> Self {
        Synthesis { bold, italic }
    }

    /// Whether to synthesize bold (swash `embolden`).
    #[must_use]
    pub fn bold(self) -> bool {
        self.bold
    }

    /// Whether to synthesize italic (a skew transform).
    #[must_use]
    pub fn italic(self) -> bool {
        self.italic
    }
}

/// The placement transform a nerd-font constraint turns into (U17): a
/// scale about the glyph origin — the point on the baseline the glyph sits
/// on — followed by an offset from it, in device pixels. swash applies the
/// transform to the scaled outline before rasterizing, so a scaled glyph
/// is rasterized at its final size, not resized as a bitmap.
///
/// The values are quantized to 26.6 fixed point like the ppem, for the
/// same reason: the transform is part of the cache key, and the nerd-font
/// arithmetic produces fractional values whose float dust must not split
/// one constraint into two cache entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlacementTransform {
    scale_x: i32,
    scale_y: i32,
    offset_x: i32,
    offset_y: i32,
}

impl PlacementTransform {
    /// Quantize a scale-and-offset transform into 26.6 fixed point. An
    /// error when a value is not finite or its quantization leaves the
    /// `i32` range — a transform that large cannot come from the
    /// nerd-font constraints, which scale a glyph into one cell.
    pub fn new(
        scale_x: f64,
        scale_y: f64,
        offset_x: f64,
        offset_y: f64,
    ) -> Result<Self, GlyphError> {
        let quantize = |what: &'static str, value: f64| -> Result<i32, GlyphError> {
            if !value.is_finite() {
                return Err(GlyphError::BadTransform { what, value });
            }
            num_traits::cast((value * 64.0).round()).ok_or(GlyphError::BadTransform { what, value })
        };
        Ok(PlacementTransform {
            scale_x: quantize("scale x", scale_x)?,
            scale_y: quantize("scale y", scale_y)?,
            offset_x: quantize("offset x", offset_x)?,
            offset_y: quantize("offset y", offset_y)?,
        })
    }

    /// The identity transform: no scale, no offset.
    #[must_use]
    pub fn identity() -> Self {
        PlacementTransform {
            scale_x: 64,
            scale_y: 64,
            offset_x: 0,
            offset_y: 0,
        }
    }

    /// The horizontal scale.
    #[must_use]
    pub fn scale_x(self) -> f64 {
        f64::from(self.scale_x) / 64.0
    }

    /// The vertical scale.
    #[must_use]
    pub fn scale_y(self) -> f64 {
        f64::from(self.scale_y) / 64.0
    }

    /// The horizontal offset, in device pixels.
    #[must_use]
    pub fn offset_x(self) -> f64 {
        f64::from(self.offset_x) / 64.0
    }

    /// The vertical offset, in device pixels (positive up, the swash
    /// outline's axis).
    #[must_use]
    pub fn offset_y(self) -> f64 {
        f64::from(self.offset_y) / 64.0
    }

    /// The zeno transform swash applies to the scaled outline: the scale
    /// about the origin followed by the offset (`(x, y) -> (sx*x + ox,
    /// sy*y + oy)`, in zeno's `transform_point` order).
    pub(crate) fn zeno(self) -> swash::zeno::Transform {
        swash::zeno::Transform::new(
            crate::render::geom::device_f32(self.scale_x()),
            0.0,
            0.0,
            crate::render::geom::device_f32(self.scale_y()),
            crate::render::geom::device_f32(self.offset_x()),
            crate::render::geom::device_f32(self.offset_y()),
        )
    }
}

/// One glyph rasterization request, and the glyph cache's key: the face
/// (identity plus the shared bytes, so the rasterizer never re-reads a
/// file), the glyph id, the quantized ppem, the synthesized style and the
/// quantized placement transform. The bytes take no part in the key's
/// equality and hash — the identity names them (see [`FaceIdentity`]), and
/// hashing a colour-emoji face's 10 MB per cache lookup would defeat the
/// cache.
#[derive(Clone)]
pub struct GlyphRequest {
    identity: FaceIdentity,
    bytes: Arc<[u8]>,
    glyph_id: u16,
    ppem: Ppem,
    synthesis: Synthesis,
    transform: Option<PlacementTransform>,
}

impl GlyphRequest {
    /// A request to rasterize `glyph_id` of `face` at `ppem` device pixels
    /// per em, with `synthesis` and an optional placement transform.
    #[must_use]
    pub fn new(
        face: &Face,
        glyph_id: u16,
        ppem: Ppem,
        synthesis: Synthesis,
        transform: Option<PlacementTransform>,
    ) -> Self {
        Self::from_parts(
            FaceIdentity::of(face),
            face.bytes(),
            glyph_id,
            ppem,
            synthesis,
            transform,
        )
    }

    /// The request's parts, assembled directly. `new` forwards here; the
    /// tests use it to build requests over bytes no `Face` would carry
    /// (the unparsable-face and unknown-instance error paths).
    pub(crate) fn from_parts(
        identity: FaceIdentity,
        bytes: Arc<[u8]>,
        glyph_id: u16,
        ppem: Ppem,
        synthesis: Synthesis,
        transform: Option<PlacementTransform>,
    ) -> Self {
        GlyphRequest {
            identity,
            bytes,
            glyph_id,
            ppem,
            synthesis,
            transform,
        }
    }

    /// The same request over the face's default instance: the same file
    /// bytes, but no named instance in the identity. Row 4.5's tests use it
    /// to show a named-instance Bold of a variable font draws heavier than
    /// the default instance of the very same file.
    #[must_use]
    pub fn at_default_instance(&self) -> Self {
        let mut request = self.clone();
        request.identity = self.identity.without_instance();
        request
    }

    /// The face identity.
    #[must_use]
    pub fn face_identity(&self) -> &FaceIdentity {
        &self.identity
    }

    /// The glyph id.
    #[must_use]
    pub fn glyph_id(&self) -> u16 {
        self.glyph_id
    }

    /// The quantized ppem.
    #[must_use]
    pub fn ppem(&self) -> Ppem {
        self.ppem
    }

    /// The synthesized style.
    #[must_use]
    pub fn synthesis(&self) -> Synthesis {
        self.synthesis
    }

    /// The placement transform, when the request carries one.
    #[must_use]
    pub fn transform(&self) -> Option<PlacementTransform> {
        self.transform
    }

    /// The shared face bytes, in the form swash parses.
    pub(crate) fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }
}

impl PartialEq for GlyphRequest {
    fn eq(&self, other: &Self) -> bool {
        // The bytes are excluded: the identity names them (see the type's
        // doc), and comparing two 10 MB emoji faces per lookup would
        // defeat the cache.
        self.identity == other.identity
            && self.glyph_id == other.glyph_id
            && self.ppem == other.ppem
            && self.synthesis == other.synthesis
            && self.transform == other.transform
    }
}

impl Eq for GlyphRequest {}

impl std::hash::Hash for GlyphRequest {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.identity.hash(state);
        self.glyph_id.hash(state);
        self.ppem.hash(state);
        self.synthesis.hash(state);
        self.transform.hash(state);
    }
}

impl std::fmt::Debug for GlyphRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The bytes are megabytes of font data; the identity is what
        // identifies a request in test failures.
        f.debug_struct("GlyphRequest")
            .field("identity", &self.identity)
            .field("glyph_id", &self.glyph_id)
            .field("ppem", &self.ppem)
            .field("synthesis", &self.synthesis)
            .field("transform", &self.transform)
            .finish_non_exhaustive()
    }
}
