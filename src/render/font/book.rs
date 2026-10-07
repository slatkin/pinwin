//! The font book: the fontconfig handle plus the caches over it. One per
//! panel; the caches live as long as the panel's fonts do.

use std::ffi::CString;
use std::fmt;

use ::fontconfig::{FC_FAMILY, FC_SLANT, FC_WEIGHT, Fontconfig, FontconfigError, Pattern};

use super::error::FontError;
use super::face::{ByteMap, Face};
use super::fallback::{self, Cached, FallbackCache};
use super::family::{
    FamilyFaces, MatchedFace, Style, family_name_matches, is_generic_family, style_matches,
};
use crate::fontconfig::{DEFAULT_FONT_FAMILY, FontConfig};

/// The fontconfig handle plus the per-code-point fallback cache. One per
/// panel; the cache lives as long as the panel's fonts do.
pub struct FontBook {
    fc: Fontconfig,
    /// The per-file byte map: every font file read once, its bytes shared
    /// by every face over it.
    files: ByteMap,
    /// One entry per code point looked up, found or not found, so a repeated
    /// lookup never re-queries fontconfig (D6: the painter caches each
    /// fallback per code point).
    fallbacks: FallbackCache,
    /// How many real fontconfig lookups the fallback cache performed; the
    /// tests assert caching through it.
    fallback_lookups: usize,
}

impl fmt::Debug for FontBook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The fontconfig handle has no `Debug` impl; the cache shape is what
        // identifies a book in test failures.
        f.debug_struct("FontBook")
            .field("cached_fallbacks", &self.fallbacks.len())
            .field("fallback_lookups", &self.fallback_lookups)
            .finish_non_exhaustive()
    }
}

impl FontBook {
    /// Initialise fontconfig. `Err(FontError::Unavailable)` when fontconfig
    /// cannot be initialised at all.
    pub fn new() -> Result<Self, FontError> {
        let fc = Fontconfig::new().ok_or(FontError::Unavailable)?;
        Ok(Self {
            fc,
            files: ByteMap::new(),
            fallbacks: FallbackCache::new(),
            fallback_lookups: 0,
        })
    }

    /// Resolve the four style faces for `config`'s family. The config's size
    /// travels with the [`FontConfig`] to rows 4.5/4.6, which rasterize and
    /// measure at it.
    ///
    /// The regular face is fontconfig's best match for the family — the
    /// family's default face, whatever weight it actually ships. The other
    /// three styles must really match the requested weight and slant and are
    /// reported missing otherwise, so row 4.5 knows when to synthesize.
    pub fn family_faces(&self, config: &FontConfig) -> Result<FamilyFaces, FontError> {
        let family = self.accept_family(config.effective_family())?;

        // Style faces of one family often share a file (a TTC, or a family
        // whose variants are separate indexes); the byte map reads each file
        // once and the faces share its allocation.
        let regular_match = self.best_match(&family, Style::Regular)?;
        let regular = self.files.face(&regular_match.file, regular_match.index)?;
        let bold = self.optional_style(&family, Style::Bold)?;
        let italic = self.optional_style(&family, Style::Italic)?;
        let bold_italic = self.optional_style(&family, Style::BoldItalic)?;

        Ok(FamilyFaces::new(family, regular, bold, italic, bold_italic))
    }

    /// The fallback face for a code point the primary face lacks (D6): the
    /// first font in fontconfig's ranking that really covers the code point.
    /// Emoji resolve to the colour or symbol face the system ships. The
    /// answer — found or not found, including a walk whose covering
    /// candidates all failed to load — is cached per code point, so a second
    /// lookup for the same code point never touches fontconfig again. Only a
    /// fontconfig-level failure (no charset, no sort) propagates as an
    /// error, and such a failure costs nothing to retry.
    pub fn fallback_face(&mut self, codepoint: char) -> Result<Option<Face>, FontError> {
        match self.fallbacks.get(codepoint) {
            Some(Cached::Found(face)) => return Ok(Some(face)),
            Some(Cached::NotFound) => return Ok(None),
            None => {}
        }
        self.fallback_lookups += 1;
        let face = fallback::lookup(&self.fc, codepoint, &self.files)?;
        // An overflowed cache clears the byte map with it, so the fallback's
        // footprint stays bounded; faces already handed out keep their bytes
        // through their own `Arc`.
        if self.fallbacks.insert(codepoint, face.clone()) {
            self.files.clear();
        }
        Ok(face)
    }

    /// How many real fontconfig lookups the fallback cache has performed.
    #[must_use]
    pub fn fallback_lookups(&self) -> usize {
        self.fallback_lookups
    }

    /// How many font files the book has read from disk; the tests assert
    /// the read-once sharing through it.
    #[cfg(test)]
    pub(crate) fn byte_reads(&self) -> usize {
        self.files.reads()
    }

    /// Test seam (D10): make every face load fail until the book is
    /// dropped, so the fallback walk's failure caching can be exercised
    /// without a broken system font.
    #[cfg(test)]
    pub(crate) fn fail_all_loads(&mut self) {
        self.files.fail_all_loads();
    }

    /// Whether fontconfig really matches `family` (a known family, not a
    /// best-effort match of an unknown name). Used by the tests to skip
    /// cleanly when a font is not installed.
    #[must_use]
    pub fn has_family(&self, family: &str) -> bool {
        self.best_match(family, Style::Regular)
            .is_ok_and(|matched| family_name_matches(family, &matched.family))
    }

    /// The family to load faces for: a generic alias or a family fontconfig
    /// really matches stays as asked; anything else falls back to
    /// `monospace` (D6). A `NoMatch` or an unrelated match is normal
    /// operation; other lookup failures are errors.
    fn accept_family(&self, requested: &str) -> Result<String, FontError> {
        if is_generic_family(requested) {
            return Ok(requested.to_owned());
        }
        match self.best_match(requested, Style::Regular) {
            Ok(matched) if family_name_matches(requested, &matched.family) => {
                Ok(requested.to_owned())
            }
            Ok(_) | Err(FontconfigError::NoMatch) => Ok(DEFAULT_FONT_FAMILY.to_owned()),
            Err(error) => Err(FontError::Lookup(error)),
        }
    }

    /// fontconfig's best match for a family at a style, with the matched
    /// pattern's own weight and slant. The match always returns something;
    /// [`style_matches`] decides whether it really is the requested style.
    pub(crate) fn best_match(
        &self,
        family: &str,
        style: Style,
    ) -> Result<MatchedFace, FontconfigError> {
        let mut pattern = Pattern::new(&self.fc)?;
        pattern.add_string(FC_FAMILY, &CString::new(family)?)?;
        pattern.add_integer(FC_WEIGHT, style.weight())?;
        pattern.add_integer(FC_SLANT, style.slant())?;
        let matched = pattern.font_match()?;
        Ok(MatchedFace {
            family: matched.get_string(FC_FAMILY)?.to_owned(),
            file: matched.filename()?.to_owned(),
            index: matched.face_index()?,
            weight: matched.weight()?,
            slant: matched.slant()?,
        })
    }

    /// Resolve one of the three optional styles, or `None` when the family's
    /// best match is not really that style (row 4.5 synthesizes it).
    fn optional_style(&self, family: &str, style: Style) -> Result<Option<Face>, FontError> {
        let matched = self.best_match(family, style)?;
        if !style_matches(style, &matched) {
            return Ok(None);
        }
        self.files.face(&matched.file, matched.index).map(Some)
    }
}
