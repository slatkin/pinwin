//! The shaper: the family's faces, the font book, the swash shaping context
//! and the bounded shape cache, behind one `shape` call per cell.

use std::collections::HashMap;
use std::sync::Arc;

use swash::FontRef;
use swash::shape::{Direction, ShapeContext};
use swash::text::Codepoint as _;
use swash::text::Script;
use swash::text::cluster::{CharCluster, CharInfo, Parser, Token};

use super::super::font::{Face, FamilyFaces, FontBook, Style};
use super::cluster::{ShapedCluster, ShapedGlyph};
use super::error::ShapeError;
use crate::render::glyph::{FaceIdentity, Ppem, Synthesis};

/// The entry cap: every distinct (cluster, style, size) one panel shapes,
/// cached. A cell's cluster repeats heavily — the same letters, over and
/// over — so the working set is small; 8192 entries hold it with room for
/// every fallback sequence a session draws. See the shape module's doc for
/// the memory ceiling.
const DEFAULT_ENTRY_CAP: usize = 8192;

/// The cache key: the request's inputs — the cluster text, the quantized
/// ppem and the style as its two bits (`Style` itself does not implement
/// `Hash`, and the (bold, italic) bits are exactly what the face choice and
/// the synthesis derive from). The key is the choice's input, not its
/// outcome: for one `TextShaper` the face choice is a pure function of
/// these three (the shaper owns one [`FamilyFaces`] and one [`FontBook`],
/// and the book's fallback answer per code point is itself cached), so the
/// cached cluster — which carries its own face — stays valid without the
/// face in the key, and a hit never re-runs the choice. The ppem arrives
/// quantized ([`Ppem`]), so float dust cannot split one size into two keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ShapeKey {
    text: Box<str>,
    ppem: Ppem,
    style: (bool, bool),
}

/// The shaper (row 4.5): shapes one cell's cluster into
/// [`ShapedCluster`]s. Owns the [`FamilyFaces`] the cells draw with, the
/// [`FontBook`] the per-code-point fallback resolves through, the swash
/// [`ShapeContext`] (whose font caches make repeated shaping cheap, the
/// same scratch-state trade the glyph module's `ScaleContext` makes) and
/// the bounded shape cache.
///
/// Not `Sync`: the painter runs on one thread, the panel's render thread.
pub struct TextShaper {
    faces: FamilyFaces,
    book: FontBook,
    context: ShapeContext,
    shaped: HashMap<ShapeKey, Arc<ShapedCluster>>,
    max_entries: usize,
    hits: usize,
    misses: usize,
    /// How many requests ran the face choice: every cache miss and every
    /// request that errors before it can be cached. The tests assert
    /// through it that a hit skips the choice.
    face_choices: usize,
    clears: usize,
}

impl std::fmt::Debug for TextShaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The swash context and the font book have no useful `Debug`; the
        // cache shape is what identifies a shaper in test failures.
        f.debug_struct("TextShaper")
            .field("family", &self.faces.family())
            .field("shaped", &self.shaped.len())
            .field("max_entries", &self.max_entries)
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .field("face_choices", &self.face_choices)
            .field("clears", &self.clears)
            .finish_non_exhaustive()
    }
}

impl TextShaper {
    /// A shaper over the family's faces and the book that resolves the
    /// fallbacks, at the default entry cap (see [`DEFAULT_ENTRY_CAP`]).
    #[must_use]
    pub fn new(faces: FamilyFaces, book: FontBook) -> Self {
        Self::with_cap(faces, book, DEFAULT_ENTRY_CAP)
    }

    /// A shaper at an explicit entry cap; the smaller caps exist for the
    /// tests.
    #[must_use]
    pub fn with_cap(faces: FamilyFaces, book: FontBook, max_entries: usize) -> Self {
        TextShaper {
            faces,
            book,
            context: ShapeContext::new(),
            shaped: HashMap::new(),
            max_entries,
            hits: 0,
            misses: 0,
            face_choices: 0,
            clears: 0,
        }
    }

    /// Shape `cluster` (one cell's grapheme) in `style` at `ppem` device
    /// pixels per em. The face choice follows the rules of the shape
    /// module's doc: the style's face, else the nearest face with
    /// synthesis, else — when a code point that needs a glyph is not
    /// covered — the fallback face for the first uncovered code point,
    /// taken only when it covers the whole cluster, else the primary face
    /// whose notdef draws. The whole cluster shapes on the one chosen
    /// face.
    ///
    /// An empty cluster shapes to an empty result, not an error. A shaping
    /// error (an unparsable face, an unknown named instance, a failed
    /// fallback lookup) is returned but never cached: the next request
    /// retries, and a broken font stays broken visibly rather than freezing
    /// into one error.
    ///
    /// The cache answers before the face choice runs: the choice is a pure
    /// function of the key (see [`ShapeKey`]), so a hit is one map lookup
    /// and the choice's cost — a charmap walk over the cluster, a swash
    /// parser pass — is paid once per distinct cluster, style and size.
    pub fn shape(
        &mut self,
        cluster: &str,
        style: Style,
        ppem: Ppem,
    ) -> Result<Arc<ShapedCluster>, ShapeError> {
        let key = ShapeKey {
            text: cluster.into(),
            ppem,
            style: style_bits(style),
        };
        if let Some(shaped) = self.shaped.get(&key) {
            self.hits += 1;
            return Ok(Arc::clone(shaped));
        }
        self.misses += 1;
        let (face, synthesis) = self.choose_face(cluster, style)?;
        let shaped = Arc::new(self.shape_miss(&face, &face.bytes(), cluster, synthesis, ppem)?);
        // A full cache clears, then the new cluster starts the fresh
        // generation — like [`GlyphCache`]: every hit stays O(1), no
        // per-entry bookkeeping, and handed-out clusters stay valid through
        // their own `Arc`.
        if self.shaped.len() >= self.max_entries {
            self.shaped.clear();
            self.clears += 1;
        }
        self.shaped.insert(key, Arc::clone(&shaped));
        Ok(shaped)
    }

    /// How many requests the cache answered from its map.
    #[must_use]
    pub fn hits(&self) -> usize {
        self.hits
    }

    /// How many requests the cache had to shape.
    #[must_use]
    pub fn misses(&self) -> usize {
        self.misses
    }

    /// How many requests ran the face choice — every cache miss, plus every
    /// request that errored before it could be cached. A hit never runs it.
    #[must_use]
    pub fn face_choices(&self) -> usize {
        self.face_choices
    }

    /// How many times the cache overflowed its cap and cleared.
    #[must_use]
    pub fn clears(&self) -> usize {
        self.clears
    }

    /// How many clusters the cache holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shaped.len()
    }

    /// Whether the cache holds no clusters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shaped.is_empty()
    }

    /// Choose the face the whole cluster draws on, and the synthesis to
    /// apply. See the shape method's doc for the rules. Runs only on a
    /// cache miss (see [`ShapeKey`]).
    fn choose_face(
        &mut self,
        cluster: &str,
        style: Style,
    ) -> Result<(Face, Synthesis), ShapeError> {
        self.face_choices += 1;
        let (face, synthesis) = match style {
            Style::Regular => (self.faces.regular().clone(), Synthesis::new(false, false)),
            Style::Bold => match self.faces.bold() {
                Some(bold) => (bold.clone(), Synthesis::new(false, false)),
                None => (self.faces.regular().clone(), Synthesis::new(true, false)),
            },
            Style::Italic => match self.faces.italic() {
                Some(italic) => (italic.clone(), Synthesis::new(false, false)),
                None => (self.faces.regular().clone(), Synthesis::new(false, true)),
            },
            Style::BoldItalic => {
                // The nearest face that carries half the style: the
                // bold-italic face, else the italic face (synthesize bold),
                // else the bold face (synthesize italic), else regular
                // (synthesize both). Never synthesize a style the chosen
                // face really has.
                if let Some(bold_italic) = self.faces.bold_italic() {
                    (bold_italic.clone(), Synthesis::new(false, false))
                } else if let Some(italic) = self.faces.italic() {
                    (italic.clone(), Synthesis::new(true, false))
                } else if let Some(bold) = self.faces.bold() {
                    (bold.clone(), Synthesis::new(false, true))
                } else {
                    (self.faces.regular().clone(), Synthesis::new(true, true))
                }
            }
        };

        if let Some(codepoint) = first_uncovered(&face, cluster)
            && let Some(fallback) = self
                .book
                .fallback_face(codepoint)
                .map_err(ShapeError::Fallback)?
                // The whole cluster shapes on one face, so the fallback is
                // taken only when it covers every code point that needs a
                // glyph: a fallback for an uncovered mark that lacks the
                // base would draw the base as notdef, worse than the
                // primary's missing mark.
            && cluster
                .chars()
                .filter(|ch| !coverage_ignorable(*ch))
                .all(|ch| fallback.covers(ch))
        {
            // The fallback face has no style faces of its own: it is
            // fontconfig's best cover for the code point, always a regular
            // roman face, so the cell's own bold and italic synthesize on
            // it. When no fallback covers the whole cluster, the cluster
            // shapes on the primary face and its notdef box draws.
            let (bold, italic) = style_bits(style);
            return Ok((fallback, Synthesis::new(bold, italic)));
        }
        Ok((face, synthesis))
    }

    /// The cache-miss path: shape `text` on `face` and insert the result.
    /// `bytes` is the face's own bytes in production; the tests pass
    /// different bytes under a real face's identity to reach the error
    /// paths, the same seam `GlyphRequest::from_parts` gives the
    /// rasterizer's tests.
    /// pub(super): the tests reach it from the shape module's tests child.
    pub(super) fn shape_miss(
        &mut self,
        face: &Face,
        bytes: &[u8],
        text: &str,
        synthesis: Synthesis,
        ppem: Ppem,
    ) -> Result<ShapedCluster, ShapeError> {
        let font = FontRef::from_index(bytes, face.index()).ok_or(ShapeError::UnparsableFace)?;
        // The named-instance obligation from the row 4.4 review, honoured
        // the same way the rasterizer honours it: the instance's normalized
        // coordinates go into the shaper, so shaping and rasterizing see
        // the same variable-font instance.
        let mut instances = font.instances();
        let coords = match face.instance() {
            Some(index) => {
                let available = instances.len();
                let instance = instances
                    .nth(index)
                    .ok_or(ShapeError::UnknownInstance { index, available })?;
                instance.normalized_coords().collect()
            }
            None => Vec::new(),
        };
        let identity = FaceIdentity::of(face);
        let mut shaper = self
            .context
            .builder_with_id(font, identity.swash_id())
            .script(script_of(text))
            .direction(Direction::LeftToRight)
            .size(ppem.value())
            .normalized_coords(coords)
            .build();
        shaper.add_str(text);
        let mut glyphs = Vec::new();
        let mut advance = 0.0f32;
        shaper.shape_with(|cluster| {
            for glyph in cluster.glyphs {
                glyphs.push(ShapedGlyph::new(glyph.id, glyph.x, glyph.y, glyph.advance));
            }
            advance += cluster.advance();
        });
        Ok(ShapedCluster::new(face.clone(), synthesis, glyphs, advance))
    }
}

/// The style's two bits, the (bold, italic) form the cache key and the
/// fallback synthesis use.
fn style_bits(style: Style) -> (bool, bool) {
    match style {
        Style::Regular => (false, false),
        Style::Bold => (true, false),
        Style::Italic => (false, true),
        Style::BoldItalic => (true, true),
    }
}

/// The cluster's script: the first character whose script is neither
/// Common nor Inherited, else Common — the base character's script for any
/// real cluster, and the right neutral answer for punctuation and emoji.
fn script_of(text: &str) -> Script {
    use swash::text::Codepoint as _;
    text.chars()
        .map(|ch| ch.properties().script())
        .find(|script| !matches!(script, Script::Common | Script::Inherited))
        .unwrap_or(Script::Common)
}

/// The cluster text as swash's parser tokens. The offsets are code-unit
/// offsets in `text`, which a cell's cluster bounds far below `u32`; a
/// `char`'s UTF-8 length is at most 4, far below `u8`.
fn tokens(text: &str) -> impl Iterator<Item = Token> + '_ {
    text.char_indices().map(|(offset, ch)| Token {
        ch,
        offset: u32::try_from(offset).unwrap_or(u32::MAX),
        len: u8::try_from(ch.len_utf8()).unwrap_or(1),
        info: CharInfo::from(ch),
        data: 0,
    })
}

/// Characters the coverage check ignores: variation selectors, the zero
/// width joiner and swash's own default-ignorables never need a glyph of
/// their own — they select or join glyphs the base code points already
/// cover. Everything else does need a glyph, combining and enclosing marks
/// included: the base only positions a mark, so an uncovered mark must
/// drive the fallback exactly like an uncovered base.
fn coverage_ignorable(ch: char) -> bool {
    matches!(ch, '\u{FE00}'..='\u{FE0F}' | '\u{200D}')
        || ch.properties().category() == swash::text::Category::Format
}

/// The first code point of `cluster` that needs a glyph and has none in
/// `face`: the code point the fallback lookup starts from. `None` when the
/// face covers the cluster (or the cluster needs no glyph at all). An
/// unparsable face covers nothing, so the first non-ignorable code point
/// comes back and the fallback gets a chance; if none exists, the shaping
/// itself reports the unparsable face.
fn first_uncovered(face: &Face, cluster: &str) -> Option<char> {
    let Some(font) = face.parse() else {
        return cluster.chars().find(|ch| !coverage_ignorable(*ch));
    };
    let charmap = font.charmap();
    let script = script_of(cluster);
    // The parser needs a cloneable token iterator; the collected vec is
    // one cheap allocation per lookup, not per frame.
    let toks: Vec<Token> = tokens(cluster).collect();
    let mut parser = Parser::new(script, toks.into_iter());
    let mut shaped = CharCluster::new();
    while parser.next(&mut shaped) {
        shaped.map(|ch| charmap.map(u32::from(ch)));
        let chars = shaped.mapped_chars();
        for c in chars {
            if c.glyph_id != 0 || !c.contributes_to_shaping {
                continue;
            }
            if c.ignorable || coverage_ignorable(c.ch) {
                continue;
            }
            return Some(c.ch);
        }
    }
    None
}
