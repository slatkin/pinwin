//! The shaper (row 4.5, design D6): one cell's grapheme cluster, shaped
//! with swash into glyph ids and pen positions, with the face chosen from
//! the family's four style faces and the per-code-point fallback, and the
//! whole result cached.
//!
//! One cluster is drawn with one face. The old Pango path itemized each
//! cell's text into runs and could split a cell across faces; a cell holds
//! one grapheme cluster, whose code points form one glyph sequence — a base
//! and its marks, an emoji sequence, a ligature — and a cluster that mixed
//! faces would position its parts against each other's metrics for no
//! visual gain. So the shaper picks one face for the whole cluster: the
//! style's face when it covers every code point that needs a glyph, else
//! the fallback face for the first uncovered code point — taken only when
//! it covers the whole cluster, so a fallback for an uncovered mark never
//! trades a drawn base for a notdef box — else the primary face whose
//! notdef box draws, the same box Pango's missing-glyph path drew. A mark
//! needs a glyph of its own like any other code point: the base it attaches
//! to only positions it, so an uncovered combining or enclosing mark (the
//! keycap U+20E3, an accent in a face without it) drives the fallback the
//! same way an uncovered base does. This matches the design's "swash shapes
//! each cell's grapheme cluster" (D6).
//!
//! The named instance travels with the face: the shaper applies the face's
//! named instance to the shaper (`normalized_coords`) the same way the
//! glyph module applies it to the scaler, so shaping and rasterizing see
//! the same variable-font instance.
//!
//! Script and direction come from swash's own analysis: the cluster's
//! script is its first character's script (the first that is neither
//! Common nor Inherited, else Common), and the direction is left to right —
//! the terminal draws cells left to right; right-to-left reordering is the
//! terminal emulator's job, done before the cells reach the renderer.
//! Default features only (kerning, the font's default ligatures), which
//! within one cluster is what the old Pango path applied.
//!
//! The shape cache keys on the request's inputs — the cluster text, the
//! quantized ppem and the style — not on the face those inputs pick: the
//! face choice is a pure function of the three for one shaper, so a hit
//! skips the choice entirely (a charmap walk over the cluster and a swash
//! parser pass) and returns the cached cluster, which carries its own face.
//! Each result is held as an `Arc`, and the cache bounds itself by an entry
//! cap, clearing on overflow like [`GlyphCache`] and the font module's
//! fallback cache. Errors are not cached. Memory ceiling: 8192 entries, each
//! a key (a cluster text of at most a few dozen bytes, a ppem and two style
//! bits) and a [`ShapedCluster`] of at most a few dozen 16-byte glyphs plus
//! a shared `Face` — under 8 MiB in the worst case, a few hundred KiB
//! typically.
//!
//! GTK-free like the font module (replace-gtk-with-wayland D10): the tests
//! run without a display. The module is `pub` from `render` so the row's
//! later units can use it before their own rows land (`pub(crate)` entries
//! with no caller are dead code under `-D warnings`, and no lint
//! suppression is permitted).
//!
//! [`GlyphCache`]: crate::render::glyph::GlyphCache

mod cluster;
mod error;
mod shaper;
#[cfg(test)]
mod tests;

pub use cluster::{ShapedCluster, ShapedGlyph};
pub use error::ShapeError;
pub use shaper::TextShaper;
