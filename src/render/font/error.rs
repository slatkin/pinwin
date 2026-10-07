//! A font lookup failed. Returned as an error — never a panic — and the
//! `monospace` fallback is not one of these: it is normal operation.

use std::fmt;
use std::path::PathBuf;

use ::fontconfig::FontconfigError;

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
    /// fontconfig reported a face index the loader cannot use (negative).
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
