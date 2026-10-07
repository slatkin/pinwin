//! The shaped result: one cell's cluster shaped into glyph ids and pen
//! positions on one face, ready for the placement unit to put on the
//! device pixel lattice and the glyph cache to rasterize.

use crate::render::font::Face;
use crate::render::glyph::Synthesis;

/// One shaped glyph of a cluster: its glyph id in the cluster's face, its
/// pen position and its advance, in device pixels.
///
/// The pen position is relative to the pen position where the glyph itself
/// begins — the pen that has already advanced over the preceding glyphs of
/// the cluster. `x_offset` is positive to the right; `y_offset` is positive
/// up, the font's own axis, the same y-up convention the glyph module's
/// placement and transform use — a mark above its base carries a positive
/// `y_offset`. A shaper positions a mark against its base by giving the
/// mark an offset (often negative in `x_offset`) and no advance, so the
/// pen does not move for it.
///
/// The values are swash's `f32` reports, unquantized: the cluster is cached
/// under its text, so no quantization is needed for the cache key, and the
/// placement unit (row 4.5's next unit) quantizes the device origins it
/// derives from them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    id: u16,
    x_offset: f32,
    y_offset: f32,
    x_advance: f32,
}

impl ShapedGlyph {
    /// A glyph from its parts (the shaper builds these; the tests read
    /// them back through the accessors).
    pub(crate) fn new(id: u16, x_offset: f32, y_offset: f32, x_advance: f32) -> Self {
        ShapedGlyph {
            id,
            x_offset,
            y_offset,
            x_advance,
        }
    }

    /// The glyph id in the cluster's face.
    #[must_use]
    pub fn id(self) -> u16 {
        self.id
    }

    /// The horizontal pen offset, in device pixels, positive to the right
    /// (see the type's doc for the pen-relative convention).
    #[must_use]
    pub fn x_offset(self) -> f32 {
        self.x_offset
    }

    /// The vertical pen offset, in device pixels, positive up.
    #[must_use]
    pub fn y_offset(self) -> f32 {
        self.y_offset
    }

    /// The advance the pen moves by after this glyph, in device pixels. A
    /// mark carries a zero advance.
    #[must_use]
    pub fn x_advance(self) -> f32 {
        self.x_advance
    }
}

/// One cell's cluster, shaped: the face it was shaped on, the synthesis to
/// rasterize with, the glyphs in draw order and the cluster's advance.
///
/// One cluster is drawn with one face (see the shape module's doc): the
/// face here is the one every glyph id belongs to, and [`Self::synthesis`]
/// carries the styles the face does not really have and must synthesize.
#[derive(Debug)]
pub struct ShapedCluster {
    face: Face,
    synthesis: Synthesis,
    glyphs: Box<[ShapedGlyph]>,
    advance: f32,
}

impl ShapedCluster {
    /// A shaped cluster from its parts (the shaper builds it).
    pub(crate) fn new(
        face: Face,
        synthesis: Synthesis,
        glyphs: Vec<ShapedGlyph>,
        advance: f32,
    ) -> Self {
        ShapedCluster {
            face,
            synthesis,
            glyphs: glyphs.into(),
            advance,
        }
    }

    /// The face the cluster was shaped on.
    #[must_use]
    pub fn face(&self) -> &Face {
        &self.face
    }

    /// The styles to synthesize when rasterizing the cluster's glyphs.
    #[must_use]
    pub fn synthesis(&self) -> Synthesis {
        self.synthesis
    }

    /// The cluster's glyphs, in draw order.
    #[must_use]
    pub fn glyphs(&self) -> &[ShapedGlyph] {
        &self.glyphs
    }

    /// The cluster's full advance, in device pixels: the distance the pen
    /// moves over the whole cluster. An empty cluster advances zero.
    #[must_use]
    pub fn advance(&self) -> f32 {
        self.advance
    }
}
