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
//! Face bytes are held as `Arc<[u8]>`: pure Rust (no memory-mapping crate, no
//! unsafe), cheap to clone between the four style faces, and exactly the
//! shared ownership swash's `FontRef` borrows want. The largest font on a
//! normal system is the colour-emoji face at roughly 10 MB, read once per
//! fallback family in use.

use std::collections::HashMap;
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ::fontconfig::{
    CharSet, FC_FAMILY, FC_SLANT, FC_SLANT_ITALIC, FC_SLANT_ROMAN, FC_WEIGHT, FC_WEIGHT_BOLD,
    FC_WEIGHT_REGULAR, Fontconfig, FontconfigError, Pattern, UnicodeCoverage,
};
use swash::FontRef;

use crate::fontconfig::{DEFAULT_FONT_FAMILY, FontConfig};

/// A font lookup failed. Returned as an error — never a panic — and the
/// `monospace` fallback is not one of these: it is normal operation.
#[derive(Debug)]
pub enum FontError {
    /// fontconfig could not be initialised (no usable fontconfig library).
    Unavailable,
    /// A fontconfig query failed.
    Lookup(FontconfigError),
    /// The face file fontconfig named could not be read.
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The face file is not a font swash can parse, or the face index is out
    /// of range for the file.
    NotAFont { path: PathBuf },
    /// fontconfig reported a negative face index.
    BadIndex(i32),
}

impl From<FontconfigError> for FontError {
    fn from(error: FontconfigError) -> Self {
        Self::Lookup(error)
    }
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("fontconfig could not be initialised"),
            Self::Lookup(error) => write!(f, "fontconfig lookup failed: {error}"),
            Self::Read { path, .. } => {
                write!(f, "cannot read font file {}", path.display())
            }
            Self::NotAFont { path } => {
                write!(f, "{} is not a font file swash can parse", path.display())
            }
            Self::BadIndex(index) => write!(f, "fontconfig reported face index {index}"),
        }
    }
}

impl std::error::Error for FontError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lookup(error) => Some(error),
            Self::Read { source, .. } => Some(source),
            Self::Unavailable | Self::NotAFont { .. } | Self::BadIndex(_) => None,
        }
    }
}

/// One of the four font variants a cell's style bits can select — the same
/// split the Pango path builds today (`Fonts` in `render/metrics.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Style {
    /// The fontconfig weight the style asks for.
    #[must_use]
    pub fn weight(self) -> i32 {
        match self {
            Self::Regular | Self::Italic => FC_WEIGHT_REGULAR,
            Self::Bold | Self::BoldItalic => FC_WEIGHT_BOLD,
        }
    }

    /// The fontconfig slant the style asks for.
    #[must_use]
    pub fn slant(self) -> i32 {
        match self {
            Self::Regular | Self::Bold => FC_SLANT_ROMAN,
            Self::Italic | Self::BoldItalic => FC_SLANT_ITALIC,
        }
    }
}

/// One loaded face: the shared file bytes plus the face index within the
/// file. Cloning is cheap (`Arc`), so the glyph caches of row 4.5 can hold a
/// face per cache entry without re-reading the file.
#[derive(Clone, Debug)]
pub struct Face {
    path: PathBuf,
    index: usize,
    bytes: Arc<[u8]>,
}

impl Face {
    /// The file the face was loaded from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The face index within the file (TTC collections hold several).
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// The shared face bytes, in the form swash parses.
    #[must_use]
    pub fn bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.bytes)
    }

    /// The face as swash sees it, or `None` when the bytes do not parse.
    #[must_use]
    pub fn parse(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.bytes, self.index)
    }

    /// Whether the face has a glyph for `codepoint` (swash charmap lookup).
    /// An unparseable face covers nothing.
    #[must_use]
    pub fn covers(&self, codepoint: char) -> bool {
        self.parse()
            .is_some_and(|font| font.charmap().map(u32::from(codepoint)) != 0)
    }
}

/// The four style faces of the terminal's family. A style the family does not
/// ship is `None`; row 4.5 synthesizes those glyphs (embolden / skew), as
/// Pango does today.
#[derive(Clone, Debug)]
pub struct FamilyFaces {
    /// The family the faces were resolved from, after the fallback rules:
    /// the configured family when fontconfig really matches it, else
    /// `monospace`.
    pub(crate) family: String,
    regular: Face,
    bold: Option<Face>,
    italic: Option<Face>,
    bold_italic: Option<Face>,
}

impl FamilyFaces {
    /// The family the faces were resolved from (see the field's doc).
    #[must_use]
    pub fn family(&self) -> &str {
        &self.family
    }

    /// The family's regular face; always present.
    #[must_use]
    pub fn regular(&self) -> &Face {
        &self.regular
    }

    /// The family's bold face, or `None` when the family has none.
    #[must_use]
    pub fn bold(&self) -> Option<&Face> {
        self.bold.as_ref()
    }

    /// The family's italic face, or `None` when the family has none.
    #[must_use]
    pub fn italic(&self) -> Option<&Face> {
        self.italic.as_ref()
    }

    /// The family's bold-italic face, or `None` when the family has none.
    #[must_use]
    pub fn bold_italic(&self) -> Option<&Face> {
        self.bold_italic.as_ref()
    }
}

/// What one fontconfig match came back with, before the bytes are loaded.
#[derive(Clone)]
struct MatchedFace {
    family: String,
    file: String,
    index: i32,
    weight: i32,
    slant: i32,
}

/// The fontconfig handle plus the per-code-point fallback cache. One per
/// panel; the cache lives as long as the panel's fonts do.
pub struct FontBook {
    fc: Fontconfig,
    /// One entry per code point looked up, found or not found, so a repeated
    /// lookup never re-queries fontconfig (D6: the painter caches each
    /// fallback per code point).
    fallbacks: HashMap<char, Option<Face>>,
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
            fallbacks: HashMap::new(),
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
        // whose variants are separate indexes); read each file once and
        // share the bytes.
        let mut loaded: HashMap<(String, usize), Face> = HashMap::new();
        let mut load = |file: &str, index: i32| -> Result<Face, FontError> {
            let face = load_face(file, index)?;
            let key = (file.to_owned(), face.index());
            if let Some(cached) = loaded.get(&key) {
                return Ok(cached.clone());
            }
            loaded.insert(key, face.clone());
            Ok(face)
        };

        let regular_match = self.best_match(&family, Style::Regular)?;
        let regular = load(&regular_match.file, regular_match.index)?;
        let bold = self.optional_style(&family, Style::Bold, &mut load)?;
        let italic = self.optional_style(&family, Style::Italic, &mut load)?;
        let bold_italic = self.optional_style(&family, Style::BoldItalic, &mut load)?;

        Ok(FamilyFaces {
            family,
            regular,
            bold,
            italic,
            bold_italic,
        })
    }

    /// The fallback face for a code point the primary face lacks (D6): the
    /// first font in fontconfig's ranking that really covers the code point.
    /// Emoji resolve to the colour or symbol face the system ships. The
    /// answer — found or not found — is cached per code point, so a second
    /// lookup for the same code point never touches fontconfig again.
    pub fn fallback_face(&mut self, codepoint: char) -> Result<Option<Face>, FontError> {
        if let Some(cached) = self.fallbacks.get(&codepoint) {
            return Ok(cached.clone());
        }
        self.fallback_lookups += 1;
        let face = self.lookup_fallback(codepoint)?;
        self.fallbacks.insert(codepoint, face.clone());
        Ok(face)
    }

    /// How many real fontconfig lookups the fallback cache has performed.
    #[must_use]
    pub fn fallback_lookups(&self) -> usize {
        self.fallback_lookups
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
    fn best_match(&self, family: &str, style: Style) -> Result<MatchedFace, FontconfigError> {
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
    fn optional_style(
        &self,
        family: &str,
        style: Style,
        load: &mut dyn FnMut(&str, i32) -> Result<Face, FontError>,
    ) -> Result<Option<Face>, FontError> {
        let matched = self.best_match(family, style)?;
        if !style_matches(style, &matched) {
            return Ok(None);
        }
        load(&matched.file, matched.index).map(Some)
    }

    /// One uncached fallback lookup: fontconfig's ranked list for a charset
    /// pattern holding just the code point, walked until a candidate really
    /// covers it. Candidates that cannot be read or parsed are skipped — a
    /// broken font file elsewhere on the system must not fail the panel.
    fn lookup_fallback(&self, codepoint: char) -> Result<Option<Face>, FontError> {
        let mut charset = CharSet::new(&self.fc)?;
        charset.add_char(codepoint)?;
        let mut pattern = Pattern::new(&self.fc)?;
        pattern.add_charset(charset)?;
        let ranked = pattern.sort_fonts(UnicodeCoverage::Trim)?;
        for candidate in ranked.iter() {
            let (Ok(file), Ok(index)) = (candidate.filename(), candidate.face_index()) else {
                continue;
            };
            match load_face(file, index) {
                Ok(face) if face.covers(codepoint) => return Ok(Some(face)),
                // Unreadable, unparseable or non-covering candidates fall
                // through to the next one; anything else (fontconfig itself)
                // is an error.
                Ok(_) | Err(FontError::Read { .. } | FontError::NotAFont { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }
}

/// The fontconfig generic aliases: they legitimately resolve to a family
/// with a different name, so the unknown-family name check does not apply.
const GENERIC_FAMILIES: [&str; 6] = [
    "monospace",
    "sans-serif",
    "serif",
    "system-ui",
    "cursive",
    "fantasy",
];

fn is_generic_family(family: &str) -> bool {
    GENERIC_FAMILIES.contains(&family)
}

/// Whether a matched face plausibly realises the requested family. Names
/// compare case-insensitively with separators dropped, which keeps the vendor
/// aliases (`JetBrains Mono`, `JetBrainsMono NF`) matching while an unknown
/// request matched to an unrelated face is rejected.
fn family_name_matches(requested: &str, matched: &str) -> bool {
    let key = |value: &str| -> String {
        value
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let requested = key(requested);
    let matched = key(matched);
    !requested.is_empty()
        && !matched.is_empty()
        && (matched.contains(&requested) || requested.contains(&matched))
}

/// Whether fontconfig's match really realises the requested style. The match
/// always succeeds, so a family without the style comes back with the closest
/// face instead (weight or slant off) — that is the "missing" row 4.5
/// synthesizes.
fn style_matches(style: Style, matched: &MatchedFace) -> bool {
    matched.weight == style.weight() && matched.slant == style.slant()
}

/// Read a fontconfig-named face file and check swash can parse it at `index`.
fn load_face(file: &str, index: i32) -> Result<Face, FontError> {
    let Ok(index) = usize::try_from(index) else {
        return Err(FontError::BadIndex(index));
    };
    let path = PathBuf::from(file);
    let bytes = fs::read(&path).map_err(|source| FontError::Read {
        path: path.clone(),
        source,
    })?;
    if FontRef::from_index(&bytes, index).is_none() {
        return Err(FontError::NotAFont { path });
    }
    Ok(Face {
        path,
        index,
        bytes: bytes.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fontconfig::DEFAULT_FONT_SIZE;

    fn book() -> Option<FontBook> {
        match FontBook::new() {
            Ok(book) => Some(book),
            Err(error) => {
                eprintln!("skipping: fontconfig unavailable: {error}");
                None
            }
        }
    }

    fn skip_unless_family(book: &FontBook, family: &str) -> bool {
        if book.has_family(family) {
            return true;
        }
        eprintln!("skipping: {family} is not installed");
        false
    }

    /// The family this machine's tests pin. Font-dependent tests skip when it
    /// is not installed (CI installs it — design D10).
    const TEST_FAMILY: &str = "JetBrains Mono";

    #[test]
    fn configured_family_resolves_to_a_matching_file() {
        let Some(book) = book() else { return };
        if !skip_unless_family(&book, TEST_FAMILY) {
            return;
        }

        let config = FontConfig {
            family: Some(TEST_FAMILY.to_owned()),
            size: 11.0,
        };
        let faces = book.family_faces(&config).unwrap();
        assert_eq!(faces.family(), TEST_FAMILY);
        let regular = faces.regular();
        assert!(regular.path().is_file(), "{}", regular.path().display());
        assert_eq!(faces.bold().map(|f| f.path().is_file()), Some(true));
        assert_eq!(faces.italic().map(|f| f.path().is_file()), Some(true));
        assert_eq!(faces.bold_italic().map(|f| f.path().is_file()), Some(true));
        // The name fontconfig reports for the file is the requested family.
        let name = book.best_match(TEST_FAMILY, Style::Regular).unwrap();
        assert!(family_name_matches(TEST_FAMILY, &name.family));
    }

    #[test]
    fn missing_config_resolves_to_monospace_11() {
        let Some(book) = book() else { return };

        let config = FontConfig::default();
        assert_eq!(config.effective_family(), "monospace");
        assert_eq!(config.size, DEFAULT_FONT_SIZE);

        let faces = book.family_faces(&config).unwrap();
        assert_eq!(faces.family(), "monospace");
        assert!(faces.regular().path().is_file());
        assert!(faces.regular().parse().is_some(), "swash parses it");
    }

    #[test]
    fn unknown_family_falls_back_to_monospace() {
        let Some(book) = book() else { return };

        let config = FontConfig {
            family: Some("NoSuchFamilyPinwinZzz".to_owned()),
            size: 11.0,
        };
        let faces = book.family_faces(&config).unwrap();
        // Not the unknown name, not the unrelated face fontconfig best-matched
        // for it: the monospace fallback, and still a real file.
        assert_eq!(faces.family(), "monospace");
        assert!(faces.regular().path().is_file());
    }

    #[test]
    fn the_four_style_faces_resolve_for_the_test_family() {
        let Some(book) = book() else { return };
        if !skip_unless_family(&book, TEST_FAMILY) {
            return;
        }

        let config = FontConfig {
            family: Some(TEST_FAMILY.to_owned()),
            size: 11.0,
        };
        let faces = book.family_faces(&config).unwrap();
        // JetBrains Mono ships all four styles; each is really the requested
        // style, since `style_matches` demands the exact weight and slant.
        assert!(faces.bold().is_some(), "bold resolved");
        assert!(faces.italic().is_some(), "italic resolved");
        assert!(faces.bold_italic().is_some(), "bold-italic resolved");
    }

    #[test]
    fn style_matches_rejects_off_weight_and_slant_matches() {
        // The pure seam: fontconfig's best match for a style the family does
        // not ship comes back with the closest weight or slant, and that is
        // "missing" (row 4.5 synthesizes).
        let regular_only = MatchedFace {
            family: "Only Regular".to_owned(),
            file: "/fonts/only-regular.ttf".to_owned(),
            index: 0,
            weight: FC_WEIGHT_REGULAR,
            slant: FC_SLANT_ROMAN,
        };
        assert!(style_matches(Style::Regular, &regular_only));
        assert!(!style_matches(Style::Bold, &regular_only));
        assert!(!style_matches(Style::Italic, &regular_only));
        assert!(!style_matches(Style::BoldItalic, &regular_only));

        let mut bold_no_italic = regular_only.clone();
        bold_no_italic.weight = FC_WEIGHT_BOLD;
        assert!(style_matches(Style::Bold, &bold_no_italic));
        assert!(!style_matches(Style::BoldItalic, &bold_no_italic));

        let mut italic_no_bold = regular_only.clone();
        italic_no_bold.slant = FC_SLANT_ITALIC;
        assert!(style_matches(Style::Italic, &italic_no_bold));
        assert!(!style_matches(Style::BoldItalic, &italic_no_bold));
    }

    #[test]
    fn family_name_matches_keep_aliases_and_reject_unrelated_faces() {
        assert!(family_name_matches("JetBrains Mono", "JetBrains Mono"));
        // A Nerd Font alias with dropped spaces still matches.
        assert!(family_name_matches("JetBrainsMono NF", "JetBrainsMono NF"));
        assert!(family_name_matches("Foo Bar", "foobar"));
        // An unknown request best-matched to an unrelated family is rejected.
        assert!(!family_name_matches("NoSuchFamilyPinwinZzz", "Noto Sans"));
        assert!(!family_name_matches("Anything", ""));
        assert!(!family_name_matches("", "Noto Sans"));
    }

    #[test]
    fn a_fallback_face_is_found_for_an_emoji() {
        let Some(mut book) = book() else { return };

        let fire = '\u{1F525}';
        let Some(face) = book.fallback_face(fire).unwrap() else {
            eprintln!("skipping: no emoji font is installed");
            return;
        };
        // The fallback really covers the code point, and it is not the
        // primary monospace face.
        assert!(face.covers(fire), "the fallback covers U+1F525");
        assert!(face.path().is_file());
        assert_ne!(face.path(), book_primary_path(&book));
        // Emoji render in colour or as symbols through a dedicated face;
        // assert the resolved face is one of those families.
        let family = face.path().to_string_lossy().to_lowercase();
        assert!(
            family.contains("emoji") || family.contains("symbol"),
            "the emoji fallback is a colour or symbol face, got {family}"
        );
    }

    fn book_primary_path(book: &FontBook) -> PathBuf {
        book.family_faces(&FontConfig::default())
            .unwrap()
            .regular()
            .path()
            .to_path_buf()
    }

    #[test]
    fn a_second_fallback_lookup_hits_the_cache() {
        let Some(mut book) = book() else { return };
        if !skip_unless_family(&book, TEST_FAMILY) {
            return;
        }

        let fire = '\u{1F525}';
        let Some(first) = book.fallback_face(fire).unwrap() else {
            eprintln!("skipping: no emoji font is installed");
            return;
        };
        assert_eq!(book.fallback_lookups(), 1);

        let second = book.fallback_face(fire).unwrap().unwrap();
        assert_eq!(second.path(), first.path());
        assert_eq!(second.index(), first.index());
        // Still one real lookup: the cache answered the second call.
        assert_eq!(book.fallback_lookups(), 1);
    }

    #[test]
    fn a_codepoint_no_font_covers_is_cached_as_not_found() {
        let Some(mut book) = book() else { return };

        // U+0378 is unassigned in Unicode; no installed font claims it.
        let unassigned = '\u{0378}';
        assert!(book.fallback_face(unassigned).unwrap().is_none());
        assert_eq!(book.fallback_lookups(), 1);

        assert!(book.fallback_face(unassigned).unwrap().is_none());
        assert_eq!(book.fallback_lookups(), 1, "the not-found answer is cached");
    }

    #[test]
    fn loaded_faces_parse_with_swash_and_cover_their_letters() {
        let Some(book) = book() else { return };
        if !skip_unless_family(&book, TEST_FAMILY) {
            return;
        }

        let config = FontConfig {
            family: Some(TEST_FAMILY.to_owned()),
            size: 11.0,
        };
        let faces = book.family_faces(&config).unwrap();
        let regular = faces.regular();
        let font = regular.parse().expect("swash parses the regular face");
        assert_ne!(font.charmap().map(u32::from('A')), 0, "covers 'A'");
        assert_ne!(font.charmap().map(u32::from('0')), 0, "covers '0'");
        assert!(regular.covers('A'), "Face::covers agrees with the charmap");
        // An obviously absent code point is not covered.
        assert!(!regular.covers('\u{1F525}'), "no emoji in JetBrains Mono");
    }

    #[test]
    fn font_error_displays_and_implements_error() {
        let errors = [
            FontError::Unavailable,
            FontError::Lookup(FontconfigError::NoMatch),
            FontError::NotAFont {
                path: PathBuf::from("/fonts/nope.ttf"),
            },
            FontError::BadIndex(-1),
        ];
        for error in errors {
            assert_ne!(error.to_string(), "");
            let boxed: Box<dyn std::error::Error> = Box::new(error);
            assert_ne!(boxed.to_string(), "");
        }
        let read = FontError::Read {
            path: PathBuf::from("/fonts/nope.ttf"),
            source: std::io::Error::other("gone"),
        };
        assert!(std::error::Error::source(&read).is_some());
    }
}
