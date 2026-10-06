//! The glyph module (replace-gtk-with-wayland row 4.5, design D6/D11):
//! rasterizing one glyph id of one face at one device size with swash, and
//! the cache over the results. One unit of the row at a time: this module
//! rasterizes and caches; a later unit of the row shapes a cell's cluster
//! into glyph ids and ports the nerd-font constraints to placement
//! transforms, and the one after places each origin on the device pixel
//! lattice and paints.
//!
//! The rasterizer runs swash's [`Render`] with hinting on and grayscale
//! antialiasing (`Format::Alpha`), the Ghostty defaults on Linux (D6).
//! swash hints TrueType `glyf`, CFF and CFF2 outlines through skrifa's
//! hinter; embedded bitmaps are never hinted. Colour glyphs come from
//! layered colour outlines (COLR/CPAL) and embedded colour bitmaps
//! (CBDT/sbix — Noto Color Emoji), in that priority order, and arrive as
//! RGBA premultiplied for the canvas ([`image_pixmap`] built them). When
//! the face lacks a bold or italic style the request says so, and the
//! rasterizer emboldens or skews the outline with FreeType's own strengths
//! (see `raster`), as Pango synthesized those styles through FreeType
//! before the port. The named-instance obligation from the row 4.4 review
//! is honoured here: a face whose file is variable-only carries its named
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
//! The module is `pub` from `render` so the row's later units can use it
//! before their own rows land (`pub(crate)` entries with no caller are
//! dead code under `-D warnings`, and no lint suppression is permitted).

mod cache;
mod error;
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
