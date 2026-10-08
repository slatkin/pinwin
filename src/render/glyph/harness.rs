//! The glyph tests' shared harness (display-free, replace-gtk-with-wayland
//! D10): the pinned families and the read-back helpers both test files use.
//!
//! The font-dependent tests skip with a printed message only when the
//! family is not installed; a broken `FontBook` or a failed lookup is an
//! `expect`, so a glyph-rasterizer regression cannot hide behind "not
//! installed".
//!
//! `CanvasMask` exposes its size but not its bytes, so the coverage tests
//! read the mask back through the public draw path: blitted over a cleared
//! canvas with an opaque colour, a pixel's alpha is exactly the coverage
//! byte (the
//! `draw_mask` blend writes `alpha = coverage * 255 / 255`).

use std::sync::Arc;

use super::cache::GlyphCache;
use super::raster::{Glyph, GlyphImage};
use super::request::{GlyphRequest, Ppem, Synthesis};
use crate::fontconfig::FontConfig;
use crate::render::canvas::{Canvas, CanvasColor, CanvasMask};
use crate::render::font::{Face, FontBook};

/// The family the plain-glyph tests pin; CI installs it (design D10).
pub(super) const FAMILY: &str = "JetBrainsMono Nerd Font";
/// The size in points the tests run at: the Ghostty default 11, which is
/// 14.6667 device px per em at the 96 dpi convention.
pub(super) const SIZE: f64 = 11.0;
/// The variable-only family the named-instance test pins: Cantarell ships
/// only as a variable font, and fontconfig reports its named Bold instance
/// as a packed `FC_INDEX` the `Face` carries.
pub(super) const VARIABLE_FAMILY: &str = "Cantarell";

/// The ppem of `size` points, the cell metrics' conversion.
pub(super) fn ppem_at(size: f64) -> Ppem {
    Ppem::from_px(size * 96.0 / 72.0).expect("the test ppem is usable")
}

/// The regular face of `family`, or `None` (printed) when the machine
/// lacks it.
pub(super) fn regular_face(family: &str) -> Option<Face> {
    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(family) {
        return None;
    }
    let config = FontConfig {
        family: Some(family.to_owned()),
        size: SIZE,
    };
    let faces = book.family_faces(&config).expect("the family's faces load");
    Some(faces.regular().clone())
}

/// The glyph id of `ch` in `face`.
pub(super) fn glyph_of(face: &Face, ch: char) -> u16 {
    face.parse()
        .expect("the face parses")
        .charmap()
        .map(u32::from(ch))
}

/// A plain request for `ch` of `face` at `ppem`.
pub(super) fn request(face: &Face, ch: char, ppem: Ppem) -> GlyphRequest {
    GlyphRequest::new(
        face,
        glyph_of(face, ch),
        ppem,
        Synthesis::new(false, false),
        None,
    )
}

/// `f64` to `usize`, the tests' one float-to-int seam (no `as`).
pub(super) fn to_usize(value: f64) -> usize {
    num_traits::cast(value).expect("the test sizes are small")
}

/// `f64` to `i32`, the tests' other float-to-int seam (no `as`).
pub(super) fn to_i32(value: f64) -> i32 {
    num_traits::cast(value).expect("the test sizes are small")
}

/// The mask's coverage bytes, row major from the top left, read back
/// through a canvas blit (see the module comment).
pub(super) fn mask_bytes(mask: &CanvasMask) -> Vec<u8> {
    let width = u32::try_from(mask.width()).expect("mask width fits u32");
    let height = u32::try_from(mask.height()).expect("mask height fits u32");
    let mut canvas = Canvas::new(width, height).expect("the mask-sized canvas opens");
    canvas.draw_mask(mask, 0, 0, CanvasColor::from_rgba(255, 255, 255, 255));
    let mut bytes = Vec::with_capacity(mask.width() * mask.height());
    for y in 0..height {
        for x in 0..width {
            bytes.push(canvas.pixel(x, y).expect("inside the canvas")[3]);
        }
    }
    bytes
}

/// The total coverage of a mask: the sum of its coverage bytes.
pub(super) fn coverage(mask: &CanvasMask) -> u64 {
    mask_bytes(mask).iter().map(|byte| u64::from(*byte)).sum()
}

/// Whether two masks carry the same coverage.
pub(super) fn masks_equal(left: &CanvasMask, right: &CanvasMask) -> bool {
    left.width() == right.width()
        && left.height() == right.height()
        && mask_bytes(left) == mask_bytes(right)
}

/// The columns of `rows` that carry any coverage, as contiguous
/// `(start, len)` runs.
pub(super) fn covered_runs(mask: &CanvasMask, rows: std::ops::Range<usize>) -> Vec<(usize, usize)> {
    let bytes = mask_bytes(mask);
    let width = mask.width();
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for column in 0..width {
        let covered = rows.clone().any(|row| bytes[row * width + column] > 0);
        match (start, covered) {
            (None, true) => start = Some(column),
            (Some(open), false) => {
                runs.push((open, column - open));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(open) = start {
        runs.push((open, width - open));
    }
    runs
}

/// The leftmost column of `rows` that carries any coverage.
pub(super) fn leftmost_column(mask: &CanvasMask, rows: std::ops::Range<usize>) -> usize {
    let bytes = mask_bytes(mask);
    let width = mask.width();
    for column in 0..width {
        if rows.clone().any(|row| bytes[row * width + column] > 0) {
            return column;
        }
    }
    panic!("no covered column in rows {rows:?}");
}

/// The mask of one rasterized glyph (the tests only rasterize masks here).
pub(super) fn mask_of(glyph: &Glyph) -> &CanvasMask {
    match glyph.image() {
        GlyphImage::Mask(mask) => mask,
        other => panic!("expected a mask, got {other:?}"),
    }
}

/// 'A' of the test family at the test ppem, through a fresh cache.
pub(super) fn rasterized_a() -> (Arc<Glyph>, GlyphRequest) {
    let face = regular_face(FAMILY).expect("the test family resolves");
    let request = request(&face, 'A', ppem_at(SIZE));
    let mut cache = GlyphCache::new();
    let glyph = cache.rasterize(&request).expect("the glyph rasterizes");
    (glyph, request)
}
