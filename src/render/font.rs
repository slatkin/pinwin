//! The font module (replace-gtk-with-wayland D11): fontconfig resolves the
//! Ghostty `font-family` to font files and finds a fallback face per code
//! point. Rows 4.5 (glyph shaping and rasterizing) and 4.6 (cell metrics)
//! shape and measure through the faces this module loads; the module itself
//! only finds fonts and loads their bytes.
//!
//! The Ghostty font settings arrive as [`crate::fontconfig::FontConfig`] (the
//! existing reader). A config with no family or size falls back to
//! `monospace` at size 11 as part of normal operation, not as an error
//! (replace-gtk-with-wayland D6). An unknown family is rejected too: fontconfig
//! best-matches *any* request, so an unconfigured name would otherwise draw
//! with a proportional face, and the port keeps the `monospace` fallback the
//! "No Ghostty config" scenario promises (D6).
//!
//! Face bytes are held as `Arc<[u8]>`: pure Rust (no memory-mapping crate),
//! cheap to clone between the four style faces, and exactly the shared
//! ownership swash's `FontRef` borrows want. The largest font on a normal
//! system is the colour-emoji face at roughly 10 MB, read once per fallback
//! family in use. The one `unsafe` seam is the charset probe behind the
//! fallback lookup (the `fontconfig` wrapper exposes no charset accessor);
//! it only reads fontconfig's own data.
//!
//! The fallback lookup must not run per frame: the text pass resolves a
//! cell's code points through
//! [`crate::render::font::FontBook::fallback_face`], which
//! caches the answer — found or not found — per code point, so a repeated
//! code point costs a map hit and a new one at most one fontconfig sort
//! (milliseconds) plus one file read.

mod book;
mod error;
mod face;
mod fallback;
mod family;
#[cfg(test)]
mod tests;

pub use book::FontBook;
pub use error::FontError;
pub use face::Face;
pub use family::{FamilyFaces, Style};
