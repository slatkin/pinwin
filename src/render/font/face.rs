//! One loaded face, and the shared per-file byte map behind every face
//! load.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use swash::FontRef;

use super::error::FontError;

/// One loaded face: the shared file bytes plus the face index within the
/// file. Cloning is cheap (`Arc`), so the glyph cache can hold a
/// face per cache entry without re-reading the file.
#[derive(Clone, Debug)]
pub struct Face {
    path: PathBuf,
    index: usize,
    instance: Option<usize>,
    bytes: Arc<[u8]>,
}

impl Face {
    /// The file the face was loaded from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The face index within the file (TTC collections hold several): the
    /// low 16 bits of fontconfig's combined `FC_INDEX`, which is the index
    /// swash's [`FontRef::from_index`] wants.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// The named instance fontconfig matched, when the face is a variable
    /// font's named instance: the zero-based index into the file's `fvar`
    /// named styles (the high 16 bits of fontconfig's `FC_INDEX`, minus
    /// one). `None` is the default instance. Row 4.5 may apply it as a
    /// variation; loading itself always opens the default instance.
    #[must_use]
    pub fn instance(&self) -> Option<usize> {
        self.instance
    }

    /// The shared face bytes, in the form swash parses.
    #[must_use]
    pub fn bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.bytes)
    }

    /// The face as swash sees it, or `None` when the bytes do not parse. That
    /// is always the file face's default instance; a matched named instance
    /// travels separately (see [`Face::instance`]).
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

/// The shared per-path byte map. Every fontconfig-named file is read from
/// disk once per map lifetime, so the four style faces of one family and all
/// the fallback code points on one font share a single allocation.
///
/// Memory ceiling: the map holds at most one byte allocation per distinct
/// font file loaded since its last clear — dominated by the colour-emoji
/// face at about 10 MB — and it clears when the code-point fallback cache
/// overflows (see `fallback::FallbackCache`, capped at
/// `fallback::CACHE_CAP` entries). Faces already handed out keep their
/// bytes alive through their own `Arc` after a clear.
/// Split fontconfig's combined face index into what swash needs. The
/// `FC_INDEX` property packs the face index within the file into the low 16
/// bits and, for a variable font, the matched named instance into the high
/// 16 bits plus one (0 there means the default instance) — fontconfig's
/// `fcfreetype.c` builds it as `id = (instance_num << 16) + face_num` and
/// decodes it as `namedstyle[(id >> 16) - 1]`. swash's `FontRef` only takes
/// the file index, so the instance must travel beside it. A negative index
/// is a fontconfig error.
pub(crate) fn split_index(index: i32) -> Result<(usize, Option<usize>), FontError> {
    if index < 0 {
        return Err(FontError::BadIndex(index));
    }
    let Ok(file_index) = usize::try_from(index & 0xFFFF) else {
        return Err(FontError::BadIndex(index));
    };
    let instance = match (index >> 16) & 0xFFFF {
        0 => None,
        packed => {
            let Ok(instance) = usize::try_from(packed - 1) else {
                return Err(FontError::BadIndex(index));
            };
            Some(instance)
        }
    };
    Ok((file_index, instance))
}

pub(crate) struct ByteMap {
    files: RefCell<HashMap<PathBuf, Arc<[u8]>>>,
    reads: Cell<usize>,
    /// Test seam (D10): when set, every load fails, so the fallback walk's
    /// failure caching can be exercised without a broken system font.
    #[cfg(test)]
    fail_loads: Cell<bool>,
}

impl ByteMap {
    /// An empty map.
    pub(crate) fn new() -> Self {
        Self {
            files: RefCell::new(HashMap::new()),
            reads: Cell::new(0),
            #[cfg(test)]
            fail_loads: Cell::new(false),
        }
    }

    /// A [`Face`] over `file` at fontconfig's combined `FC_INDEX`, reading
    /// the file's bytes once and sharing them with every other face of the
    /// same file.
    pub(crate) fn face(&self, file: &str, index: i32) -> Result<Face, FontError> {
        let (index, instance) = split_index(index)?;
        #[cfg(test)]
        if self.fail_loads.get() {
            return Err(FontError::NotAFont {
                path: PathBuf::from(file),
            });
        }
        let path = PathBuf::from(file);
        let bytes = self.bytes(&path)?;
        if FontRef::from_index(&bytes, index).is_none() {
            return Err(FontError::NotAFont { path });
        }
        Ok(Face {
            path,
            index,
            instance,
            bytes,
        })
    }

    /// The shared bytes of `path`, read from disk on first use.
    fn bytes(&self, path: &Path) -> Result<Arc<[u8]>, FontError> {
        if let Some(shared) = self.files.borrow().get(path) {
            return Ok(Arc::clone(shared));
        }
        let bytes: Arc<[u8]> = fs::read(path)
            .map_err(|source| FontError::Read {
                path: path.to_path_buf(),
                source,
            })?
            .into();
        self.reads.set(self.reads.get() + 1);
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), Arc::clone(&bytes));
        Ok(bytes)
    }

    /// Drop every cached file. The fallback cache calls this when it
    /// overflows, so the fallback's footprint stays bounded.
    pub(crate) fn clear(&self) {
        self.files.borrow_mut().clear();
    }

    /// How many files the map has read from disk; the tests assert the
    /// read-once sharing through it.
    #[cfg(test)]
    pub(crate) fn reads(&self) -> usize {
        self.reads.get()
    }

    /// Test seam (D10): make every load fail until the map is dropped.
    #[cfg(test)]
    pub(crate) fn fail_all_loads(&self) {
        self.fail_loads.set(true);
    }
}
