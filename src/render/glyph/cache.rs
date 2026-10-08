//! The glyph cache: request in, shared glyph out, bounded both by an entry
//! cap and a byte budget, with swash's scaler context as its scratch
//! state.

use std::collections::HashMap;
use std::sync::Arc;

use swash::scale::ScaleContext;

use super::error::GlyphError;
use super::raster::{self, Glyph};
use super::request::GlyphRequest;

/// The entry cap: every distinct glyph one panel renders, cached. A
/// monospace terminal face holds a few thousand glyphs; the fallback emoji
/// face adds the code points the session actually draws. 8192 entries hold
/// all of that with headroom.
const DEFAULT_ENTRY_CAP: usize = 8192;

/// The byte budget over all cached images: 16 MiB. It is the binding cap
/// on the worst case the entry cap alone would allow — a 4K panel at
/// scale 2 rasterizes glyphs up to about 20 by 40 device pixels, 800 bytes
/// as a mask or 3200 as a colour image, and 8192 of the colour ones would
/// want 26 MiB. 16 MiB holds every distinct glyph of a full 4K grid
/// several times over (a grid draws far fewer distinct glyphs than it has
/// cells), plus the colour emoji at their strike sizes, and it is small
/// next to the font bytes themselves. An insert onto a full cache clears
/// it and starts over, like the font module's fallback cache: every hit
/// stays O(1), no per-entry bookkeeping, and handed-out glyphs stay valid
/// through their own `Arc`.
const DEFAULT_BYTE_BUDGET: usize = 16 * 1024 * 1024;

/// The glyph cache: keyed on the [`GlyphRequest`] — face
/// identity, glyph id, quantized ppem, synthesized style, quantized
/// placement transform — holding each result as an `Arc`, so the glyphs a
/// frame hands out survive a cache clear. It also owns the swash
/// [`ScaleContext`], the scaler scratch state the rasterizer would
/// otherwise rebuild per glyph: the font proxies, the hinting instances
/// and the scratch buffers live there, keyed on the face's stable id.
///
/// Not `Sync`: the painter runs on one thread, the panel's render thread.
pub struct GlyphCache {
    context: ScaleContext,
    glyphs: HashMap<GlyphRequest, Arc<Glyph>>,
    /// The summed [`Glyph::byte_cost`] of the cached glyphs.
    bytes: usize,
    max_entries: usize,
    byte_budget: usize,
    hits: usize,
    misses: usize,
    clears: usize,
}

impl std::fmt::Debug for GlyphCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The swash context has no `Debug` impl; the cache shape is what
        // identifies a cache in test failures.
        f.debug_struct("GlyphCache")
            .field("glyphs", &self.glyphs.len())
            .field("bytes", &self.bytes)
            .field("max_entries", &self.max_entries)
            .field("byte_budget", &self.byte_budget)
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .field("clears", &self.clears)
            .finish_non_exhaustive()
    }
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GlyphCache {
    /// A cache at the default limits (see the constants above).
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_ENTRY_CAP, DEFAULT_BYTE_BUDGET)
    }

    /// A cache at explicit limits: at most `max_entries` glyphs and at
    /// most `byte_budget` bytes of glyph images. `new` uses the defaults;
    /// the smaller limits exist for the tests and for a caller that wants
    /// to size the cache to its grid.
    #[must_use]
    pub fn with_limits(max_entries: usize, byte_budget: usize) -> Self {
        GlyphCache {
            context: ScaleContext::new(),
            glyphs: HashMap::new(),
            bytes: 0,
            max_entries,
            byte_budget,
            hits: 0,
            misses: 0,
            clears: 0,
        }
    }

    /// The glyph for `request`, rasterizing and caching it on a miss. An
    /// error (an unparsable face, an unknown named instance) is not cached:
    /// the next request retries, and a broken font stays broken visibly
    /// rather than freezing into one error.
    pub fn rasterize(&mut self, request: &GlyphRequest) -> Result<Arc<Glyph>, GlyphError> {
        if let Some(glyph) = self.glyphs.get(request) {
            self.hits += 1;
            return Ok(Arc::clone(glyph));
        }
        self.misses += 1;
        let glyph = Arc::new(raster::rasterize(request, &mut self.context)?);
        let cost = glyph.byte_cost();
        // A full cache clears, then the new glyph starts the fresh
        // generation — even one bigger than the whole budget is kept
        // alone, so a pathological glyph costs a clear per insert instead
        // of failing the frame.
        if self.glyphs.len() >= self.max_entries || self.bytes + cost > self.byte_budget {
            self.glyphs.clear();
            self.bytes = 0;
            self.clears += 1;
        }
        self.bytes += cost;
        self.glyphs.insert(request.clone(), Arc::clone(&glyph));
        Ok(glyph)
    }

    /// How many requests the cache answered from its map.
    #[must_use]
    pub fn hits(&self) -> usize {
        self.hits
    }

    /// How many requests the cache had to rasterize.
    #[must_use]
    pub fn misses(&self) -> usize {
        self.misses
    }

    /// How many times the cache overflowed a limit and cleared.
    #[must_use]
    pub fn clears(&self) -> usize {
        self.clears
    }

    /// How many glyphs the cache holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Whether the cache holds no glyphs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}
