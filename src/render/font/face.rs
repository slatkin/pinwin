//! One loaded face and the loader for fontconfig-named files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use swash::FontRef;

use super::error::FontError;

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

/// Read a fontconfig-named face file and check swash can parse it at `index`.
pub(crate) fn load_face(file: &str, index: i32) -> Result<Face, FontError> {
    let Ok(index) = usize::try_from(index) else {
        return Err(FontError::BadIndex(index));
    };
    let path = PathBuf::from(file);
    let bytes: Arc<[u8]> = std::fs::read(&path)
        .map_err(|source| FontError::Read {
            path: path.clone(),
            source,
        })?
        .into();
    if FontRef::from_index(&bytes, index).is_none() {
        return Err(FontError::NotAFont { path });
    }
    Ok(Face { path, index, bytes })
}
