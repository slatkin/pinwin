//! A glyph request failed. Returned as an error — never a panic — and a
//! glyph swash cannot render is not one of these: it is an empty result.

use std::fmt;

/// A glyph request failed. Returned as an error — never a panic — and a
/// glyph swash cannot render is not one of these: it is an empty result
/// (the blank the painter draws for a space).
#[derive(Debug)]
pub enum GlyphError {
    /// The face bytes do not parse (swash rejected the file the request
    /// carries).
    UnparsableFace,
    /// The device pixels-per-em is not finite and positive, or leaves the
    /// 26.6 fixed-point range the cache key quantizes it into.
    BadPpem(f64),
    /// A placement-transform value is not finite, or its 26.6 fixed-point
    /// quantization leaves the `i32` range.
    BadTransform {
        /// Which value failed: `"scale x"`, `"scale y"`, `"offset x"` or
        /// `"offset y"`.
        what: &'static str,
        /// The rejected value.
        value: f64,
    },
    /// The request names a named instance the face does not have (the
    /// zero-based index is past the file's `fvar` instance count).
    UnknownInstance {
        /// The instance index the request carried.
        index: usize,
        /// How many named instances the face actually has.
        available: usize,
    },
    /// swash produced an image whose data does not match its placement —
    /// a swash bug, reported instead of trusted.
    BadImage(&'static str),
}

impl fmt::Display for GlyphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnparsableFace => f.write_str("the face does not parse"),
            Self::BadPpem(ppem) => {
                write!(f, "the pixels-per-em {ppem} is not usable as a device size")
            }
            Self::BadTransform { what, value } => {
                write!(f, "the placement transform {what} {value} is not usable")
            }
            Self::UnknownInstance { index, available } => write!(
                f,
                "the face has {available} named instances, the request names {index}"
            ),
            Self::BadImage(what) => write!(f, "swash produced a bad glyph image: {what}"),
        }
    }
}

impl std::error::Error for GlyphError {}
