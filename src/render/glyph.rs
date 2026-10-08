//! The glyph module (replace-gtk-with-wayland D6/D11):
//! rasterizing one glyph id of one face at one device size with swash, and
//! the cache over the results. Beside it,
//! `shape` shapes a cell's cluster
//! into glyph ids and ports the nerd-font constraints to placement
//! transforms, and `text_pass` places each origin on the device pixel
//! lattice and paints.
//!
//! The rasterizer runs swash's [`swash::scale::Render`] with hinting on and
//! grayscale antialiasing (`Format::Alpha`), the Ghostty defaults on Linux
//! (D6).
//! swash hints TrueType `glyf`, CFF and CFF2 outlines through skrifa's
//! hinter; embedded bitmaps are never hinted. Colour glyphs come from
//! layered colour outlines (COLR/CPAL) and embedded colour bitmaps
//! (CBDT/sbix — Noto Color Emoji), in that priority order, and arrive as
//! RGBA premultiplied for the canvas
//! ([`crate::render::canvas::image_pixmap`] built them). When
//! the face lacks a bold or italic style the request says so, and the
//! rasterizer emboldens or skews the outline with FreeType's own strengths
//! (see `raster`), as Pango synthesized those styles through FreeType
//! before the port. A face whose file is variable-only carries its named
//! instance in the identity, the instance's normalized coordinates go into
//! the scaler, and the instance is part of the cache key — otherwise a
//! family such as Cantarell would render its matched Bold as Regular.
//!
//! The cache keys on the request (face identity, glyph id, quantized ppem,
//! style, quantized transform), holds each glyph as an `Arc` so handed-out
//! results survive a clear, and bounds itself by both an entry cap and a
//! byte budget, clearing on overflow like the font module's fallback
//! cache. The swash `ScaleContext` lives in the cache, so the scaler's
//! font proxies and hinting instances are not rebuilt per glyph.
//!
//! GTK-free like the font module (D10): the tests run without a display.
//! The module is `pub` from `render` because its consumers — the shaper and
//! the text pass — live beside it (`pub(crate)` entries with no caller are
//! dead code under `-D warnings`, and no lint suppression is permitted).

mod cache;
#[cfg(test)]
mod cache_tests;
mod error;
#[cfg(test)]
mod harness;
mod identity;
mod raster;
mod request;
#[cfg(test)]
mod tests;

pub use cache::GlyphCache;
pub use error::GlyphError;
pub use identity::FaceIdentity;
pub use raster::{Glyph, GlyphImage, GlyphPlacement};
pub use request::{GlyphRequest, PlacementTransform, Ppem, Synthesis};
