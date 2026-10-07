//! Family and style resolution: the four style variants a cell's style bits
//! select, the fontconfig match of one style, and the checks that decide
//! whether a match really realises the requested family and style.

use ::fontconfig::{
    FC_SLANT_ITALIC, FC_SLANT_ROMAN, FC_WEIGHT_BLACK, FC_WEIGHT_BOLD, FC_WEIGHT_DEMIBOLD,
    FC_WEIGHT_MEDIUM, FC_WEIGHT_REGULAR,
};

use super::face::Face;

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

    /// Assemble the resolved faces (the `FontBook` resolves them; the struct
    /// only holds the result).
    pub(crate) fn new(
        family: String,
        regular: Face,
        bold: Option<Face>,
        italic: Option<Face>,
        bold_italic: Option<Face>,
    ) -> Self {
        Self {
            family,
            regular,
            bold,
            italic,
            bold_italic,
        }
    }
}

/// What one fontconfig match came back with, before the bytes are loaded.
#[derive(Clone)]
pub(crate) struct MatchedFace {
    pub(crate) family: String,
    pub(crate) file: String,
    pub(crate) index: i32,
    pub(crate) weight: i32,
    pub(crate) slant: i32,
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

pub(crate) fn is_generic_family(family: &str) -> bool {
    GENERIC_FAMILIES.contains(&family)
}

/// Whether a matched face plausibly realises the requested family. Names
/// compare case-insensitively with separators dropped, which keeps the vendor
/// aliases (`JetBrains Mono`, `JetBrainsMono NF`) matching while an unknown
/// request matched to an unrelated face is rejected.
pub(crate) fn family_name_matches(requested: &str, matched: &str) -> bool {
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
/// synthesizes. Bold accepts the weights from demibold (180) through black
/// (210), so a family whose boldest face is extrabold or black still gets a
/// real bold face; italic accepts oblique, the slant fontconfig reports for
/// slanted roman faces, so an oblique-only family is not synthesized on top
/// of a real face.
pub(crate) fn style_matches(style: Style, matched: &MatchedFace) -> bool {
    let weight_ok = match style {
        Style::Regular | Style::Italic => matched.weight <= FC_WEIGHT_MEDIUM,
        Style::Bold | Style::BoldItalic => {
            (FC_WEIGHT_DEMIBOLD..=FC_WEIGHT_BLACK).contains(&matched.weight)
        }
    };
    let slant_ok = match style {
        Style::Regular | Style::Bold => matched.slant == FC_SLANT_ROMAN,
        Style::Italic | Style::BoldItalic => matched.slant >= FC_SLANT_ITALIC,
    };
    weight_ok && slant_ok
}
