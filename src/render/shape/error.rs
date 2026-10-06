//! A shaping request failed. Returned as an error — never a panic — and a
//! cluster whose glyphs the face lacks is not one of these: it shapes into
//! the face's notdef box, the same box the old Pango path drew.

use std::fmt;

use crate::render::font::FontError;

/// A shaping request failed. Returned as an error — never a panic.
#[derive(Debug)]
pub enum ShapeError {
    /// The chosen face's bytes do not parse (swash rejected the file).
    UnparsableFace,
    /// The face names a named instance it does not have (the zero-based
    /// index is past the file's `fvar` instance count).
    UnknownInstance {
        /// The instance index the face carried.
        index: usize,
        /// How many named instances the face actually has.
        available: usize,
    },
    /// The per-code-point fallback lookup failed at the fontconfig level.
    /// A code point no font covers is not this: it is a `None` answer, and
    /// the cluster shapes on the primary face.
    Fallback(FontError),
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnparsableFace => f.write_str("the face does not parse"),
            Self::UnknownInstance { index, available } => write!(
                f,
                "the face has {available} named instances, the face names {index}"
            ),
            Self::Fallback(error) => write!(f, "the fallback lookup failed: {error}"),
        }
    }
}

impl std::error::Error for ShapeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fallback(error) => Some(error),
            Self::UnparsableFace | Self::UnknownInstance { .. } => None,
        }
    }
}
