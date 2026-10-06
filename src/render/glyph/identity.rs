//! The hashable identity of one face: the file path, the face index within
//! the file and the named instance. The glyph cache keys on it, and it
//! derives the stable identifier swash's scaler caches key on.

use std::path::PathBuf;

use super::super::font::Face;

/// Which face a glyph request rasterizes, in a hashable form: the file's
/// path, the face index within the file (a TTC holds several) and the named
/// instance (`None` is the default instance). The bytes travel beside the
/// identity in the request, so the rasterizer never re-reads a file; the
/// identity is what the cache key and the swash cache id are built from.
///
/// The identity is the cache's promise that two equal identities name the
/// same glyph shapes: a file replaced on disk under the same path keeps its
/// old cached glyphs until the panel restarts, the same trade the font
/// module's byte map makes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FaceIdentity {
    path: PathBuf,
    index: usize,
    instance: Option<usize>,
}

impl FaceIdentity {
    /// The identity of a loaded [`Face`]: its path, file index and named
    /// instance, exactly as fontconfig reported them.
    #[must_use]
    pub fn of(face: &Face) -> Self {
        FaceIdentity {
            path: face.path().to_path_buf(),
            index: face.index(),
            instance: face.instance(),
        }
    }

    /// A test identity (D10): assembled directly, so the tests can build
    /// requests over identities no loaded `Face` would carry (an unknown
    /// named instance, bytes that are not a font).
    #[cfg(test)]
    pub(crate) fn for_test(path: &std::path::Path, index: usize, instance: Option<usize>) -> Self {
        FaceIdentity {
            path: path.to_path_buf(),
            index,
            instance,
        }
    }

    /// The file the face was loaded from.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The face index within the file.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// The named instance (`None` is the default instance).
    #[must_use]
    pub fn instance(&self) -> Option<usize> {
        self.instance
    }

    /// The identity with the named instance dropped: the same file and
    /// face index at the default instance. `GlyphRequest
    /// ::at_default_instance` forwards here, so the field stays private
    /// and the change cannot touch anything else.
    pub(crate) fn without_instance(&self) -> Self {
        let mut identity = self.clone();
        identity.instance = None;
        identity
    }

    /// A stable two-word identifier for swash's scaler caches. swash keys
    /// its per-font proxy data and its hinting instances on an id the
    /// caller supplies (`ScaleContext::builder_with_id`), because a
    /// `FontRef` rebuilt from bytes every call would otherwise carry a
    /// fresh cache key each time and evict its own caches. The id is two
    /// independent 64-bit hashes of the identity, so two faces of one
    /// panel collide only if both hashes collide — a 128-bit accident that
    /// will not happen within a process lifetime.
    pub(crate) fn swash_id(&self) -> [u64; 2] {
        use std::hash::{Hash, Hasher};
        let mut first = std::hash::DefaultHasher::new();
        // The salt keeps the two hashes independent despite the same input.
        0x7069_6e77_6964_3100u64.hash(&mut first);
        self.path.hash(&mut first);
        self.index.hash(&mut first);
        self.instance.hash(&mut first);
        let mut second = std::hash::DefaultHasher::new();
        0x7069_6e77_6964_3200u64.hash(&mut second);
        self.instance.hash(&mut second);
        self.index.hash(&mut second);
        self.path.hash(&mut second);
        [first.finish(), second.finish()]
    }
}
