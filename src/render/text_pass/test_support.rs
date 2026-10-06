//! The shared test helper for the text pass (replace-gtk-with-wayland D10):
//! one place builds a [`TextPass`] over the real test family, so every
//! `paint_frame` pixel test — the painter's, the sprite's, the cursor's and
//! the text pass's own — draws with the same font. The font-dependent tests
//! skip with a printed message only when the family is not installed; a
//! broken `FontBook` or a failed lookup is an `expect`, so a regression
//! cannot hide behind "not installed".

use crate::fontconfig::FontConfig;
use crate::render::cell_metrics::CellMetrics;
use crate::render::font::{FamilyFaces, FontBook};
use crate::render::glyph::GlyphCache;
use crate::render::shape::TextShaper;

use super::TextPass;

/// The family the tests pin; CI installs it (design D10). The same family
/// the shape, glyph and nerd tests pin.
pub(crate) const FAMILY: &str = "JetBrainsMono Nerd Font";

/// The size in points the tests run at: the Ghostty default 11.
pub(crate) const SIZE: f64 = 11.0;

/// A [`TextPass`] plus the cell metrics it was built with, so a test can
/// reason about the font's own numbers (the nerd scenario, the logical
/// sizes scenario).
pub(crate) struct TestPass {
    /// The pass the frame paints with.
    pub pass: TextPass,
    /// The cell metrics measured from the family's regular face at
    /// [`SIZE`].
    pub cell_metrics: CellMetrics,
}

/// A text pass over the test family's faces, or `None` (printed) when the
/// machine lacks the family.
pub(crate) fn text_pass() -> Option<TestPass> {
    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(FAMILY) {
        println!("skipped: {FAMILY} is not installed");
        return None;
    }
    let config = FontConfig {
        family: Some(FAMILY.to_owned()),
        size: SIZE,
    };
    let faces = book.family_faces(&config).expect("the family's faces load");
    Some(from_faces(&faces, book))
}

/// A text pass over faces a test built itself — the emoji scenario builds
/// the same family and lets the shaper's fallback find the emoji face.
pub(crate) fn from_faces(faces: &FamilyFaces, book: FontBook) -> TestPass {
    let cell_metrics = crate::render::cell_metrics::measure(faces.regular(), SIZE)
        .expect("the cell metrics compute");
    let shaper = TextShaper::new(faces.clone(), book);
    TestPass {
        pass: TextPass::new(shaper, GlyphCache::new(), cell_metrics, SIZE),
        cell_metrics,
    }
}
