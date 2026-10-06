//! The kitty image pass of the grid painter (row 4.7, `replace-gtk-with-
//! wayland` D5): the open frame's kitty placements decoded, scaled and
//! blitted into the canvas, in [`crate::render::painter::paint_frame`]'s
//! layer order after the cursor and before the focus accent — exactly
//! where `DrawState::render_grid` calls `ImageCache::draw` on the GTK
//! path, so images draw above the cursor there too.
//!
//! The placements arrive through [`Terminal::image_next`] in the
//! iterator's order, and `z` is ignored exactly as the old path ignores
//! it: `ImageCache::draw` draws whatever order the iterator hands over,
//! and so does this pass (a comment, not an oversight — ghostty emits
//! placements in its own order and the GTK panel has always honoured it).
//!
//! Each placement's destination rectangle is the terminal's logical
//! rectangle snapped to the device grid through
//! [`PainterMetrics::logical_rect`]: the right and bottom edges snap on
//! their own (`snap(x + w)`, never `snap(x) + snap(w)`), so at a
//! fractional scale the device rectangle covers every device pixel the
//! true fractional span touches — at scale 1.5 an 8-logical-pixel-wide
//! image draws 12 device pixels wide, at 1.8 it draws 14 — and the
//! resampler maps the whole source rectangle onto the whole device
//! rectangle with the source's edge columns and rows carrying their full
//! weight in the destination's edge pixels. Nothing is cut off at the
//! right or bottom: "Images are not clipped" holds at every scale. The
//! tween's draw offset then shifts the rectangle along x, as it shifts
//! every grid layer, and the canvas clips what moves past the edges.
//!
//! The cache (design D5: "Cache each image scaled to its placement size
//! and output scale") holds only scaled results, keyed on the image id,
//! its generation, the source rectangle and the destination device size —
//! a scrolling crop (a placement whose source window moves every frame)
//! changes the key every frame and costs one resample per frame; the
//! budget clears bound the damage. The unscaled pixmap is built on a miss
//! only, from the ghostty pixels read synchronously through
//! [`image_pixmap`] (the one-time red/blue swap and premultiplication),
//! scaled, and dropped; the cache never holds it. Entries no placement
//! touched this frame are evicted, like the old `ImageCache::evict`, and
//! an image id that returns with a new generation drops its stale
//! entries. The cache is bounded by an entry cap and a byte budget with
//! clear-on-overflow, like [`crate::render::glyph::GlyphCache`]; handed
//!-out pixmaps are `Arc`s and survive a clear.
//!
//! GTK-free (`replace-gtk-with-wayland` D10): the tests run without a
//! display.

use std::collections::HashMap;
use std::slice;
use std::sync::Arc;

use tiny_skia::Pixmap;

use super::canvas::{Canvas, image_pixmap, resample};
use super::geom::{DeviceRect, PainterMetrics};
use super::png::MAX_DIMENSION;
use crate::term::Terminal;
use crate::term::cells::Image;

/// The entry cap: one placement size per image, and a media session shows
/// a few dozen images at a few sizes. 256 entries hold far more than a
/// frame draws; the byte budget is the binding cap.
const MAX_ENTRIES: usize = 256;
/// The byte budget over all cached scaled images: 64 MiB. It holds a
/// full-4K-width poster (3840 × 2160 premultiplied ≈ 33 MB) plus several
/// smaller ones, which covers every placement a media panel draws at
/// once; beyond it the cache clears and starts over, so a session that
/// churns placements (a scrolling crop re-keys every frame) costs
/// resamples, not memory.
const BYTE_BUDGET: usize = 64 * 1024 * 1024;

/// One cache key: which image, which transmission, which source window,
/// scaled to which destination device size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    image_id: u32,
    generation: i64,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    dst_w: u32,
    dst_h: u32,
}

/// One cached scaled image: the premultiplied, channel-swapped pixmap
/// plus the frame bookkeeping eviction and the byte accounting need.
#[derive(Debug)]
struct Entry {
    pixmap: Arc<Pixmap>,
    /// The entry's own byte cost, `dst_w * dst_h * 4`.
    bytes: usize,
    /// The last frame whose placements touched this entry.
    last_frame: u64,
}

/// The kitty image pass: the cache and the placement drawing, owned by
/// the caller across frames beside the [`crate::render::text_pass::
/// TextPass`]. Not `Sync`: the painter runs on one thread, the panel's
/// render thread.
#[derive(Debug)]
pub struct ImagePass {
    entries: HashMap<Key, Entry>,
    /// The summed [`Entry::bytes`] of the cached entries.
    bytes: usize,
    /// The entry cap and the byte budget the cache clears at.
    max_entries: usize,
    byte_budget: usize,
    frame: u64,
    hits: usize,
    misses: usize,
    clears: usize,
}

impl Default for ImagePass {
    fn default() -> Self {
        Self::new()
    }
}

impl ImagePass {
    /// A pass at the default limits (see the constants above).
    #[must_use]
    pub fn new() -> Self {
        ImagePass::with_limits(MAX_ENTRIES, BYTE_BUDGET)
    }

    /// A pass at explicit limits: at most `max_entries` cached scaled
    /// images and at most `byte_budget` bytes of them. `new` uses the
    /// defaults; the smaller limits exist for the tests and for a caller
    /// that wants to size the cache to its panel.
    #[must_use]
    pub fn with_limits(max_entries: usize, byte_budget: usize) -> Self {
        ImagePass {
            entries: HashMap::new(),
            bytes: 0,
            max_entries,
            byte_budget,
            frame: 0,
            hits: 0,
            misses: 0,
            clears: 0,
        }
    }

    /// Draw the open frame's kitty placements into `canvas`, shifted by
    /// `offset` device pixels along x. The frame must be open
    /// ([`Terminal::frame_begin`]) and its cells walked (a virtual
    /// placement resolves against the placeholder origins the cell walk
    /// recorded); the caller ends the frame afterwards.
    pub fn paint(
        &mut self,
        canvas: &mut Canvas,
        metrics: &PainterMetrics,
        terminal: &mut Terminal,
        offset: i32,
    ) {
        self.frame += 1;
        let this_frame = self.frame;
        while let Some(img) = terminal.image_next() {
            // The old `draw` skips these and so does this pass: a
            // placement with no extent draws nothing.
            if img.sw <= 0 || img.sh <= 0 || img.w <= 0 || img.h <= 0 {
                continue;
            }
            // The destination is the logical rectangle snapped to the
            // device grid, each edge on its own, so the whole image stays
            // visible at a fractional scale (see the module docs).
            let rect = metrics.logical_rect(
                f64::from(img.x),
                f64::from(img.y),
                f64::from(img.w),
                f64::from(img.h),
            );
            if rect.is_empty() {
                continue;
            }
            self.draw_placement(canvas, &rect, &img, offset);
        }
        self.evict(this_frame);
    }

    /// Draw one placement: the cached scaled image on a hit, a resample
    /// on a miss, blitted at the rectangle's position. A placement that
    /// cannot be read (no pixels, an unusable size) draws nothing.
    fn draw_placement(&mut self, canvas: &mut Canvas, rect: &DeviceRect, img: &Image, offset: i32) {
        let (Ok(dst_w), Ok(dst_h)) = (u32::try_from(rect.w()), u32::try_from(rect.h())) else {
            return;
        };
        let Some(pixmap) = self.scaled(img, dst_w, dst_h) else {
            return;
        };
        canvas.draw_image(&pixmap, rect.x() + offset, rect.y());
    }

    /// The scaled pixmap for this placement, from the cache on a hit and
    /// a fresh resample on a miss.
    fn scaled(&mut self, img: &Image, dst_w: u32, dst_h: u32) -> Option<Arc<Pixmap>> {
        let key = Key {
            image_id: img.image_id,
            generation: img.generation,
            sx: img.sx,
            sy: img.sy,
            sw: img.sw,
            sh: img.sh,
            dst_w,
            dst_h,
        };
        if let Some(entry) = self.entries.get_mut(&key) {
            self.hits += 1;
            entry.last_frame = self.frame;
            return Some(Arc::clone(&entry.pixmap));
        }
        self.misses += 1;

        let src = unscaled(img)?;
        // The whole image at its own size needs no resample: the pixmap
        // is already the result.
        let one_to_one = img.sx == 0
            && img.sy == 0
            && img.sw == img.image_w
            && img.sh == img.image_h
            && dst_w == u32::try_from(img.sw).ok()?
            && dst_h == u32::try_from(img.sh).ok()?;
        let pixmap = if one_to_one {
            src
        } else {
            resample(&src, img.sx, img.sy, img.sw, img.sh, dst_w, dst_h)?
        };

        let cost = usize::try_from(dst_w)
            .ok()?
            .checked_mul(usize::try_from(dst_h).ok()?)?
            .checked_mul(4)?;
        self.insert(key, pixmap, cost);
        self.entries
            .get(&key)
            .map(|entry| Arc::clone(&entry.pixmap))
    }

    /// Insert a freshly scaled pixmap: drop the image's stale-generation
    /// entries, clear the cache when it is over a limit, then insert.
    fn insert(&mut self, key: Key, pixmap: Pixmap, cost: usize) {
        // An image id that returns with a new generation invalidates its
        // old entries: their pixels are stale.
        let mut stale = 0usize;
        self.entries.retain(|existing, entry| {
            let keep =
                !(existing.image_id == key.image_id && existing.generation != key.generation);
            if !keep {
                stale += entry.bytes;
            }
            keep
        });
        self.bytes -= stale;

        // A full cache clears, then the new entry starts the fresh
        // generation — even one bigger than the whole budget is kept
        // alone, so a pathological placement costs a clear per frame
        // instead of failing the frame.
        if self.entries.len() >= self.max_entries
            || self.bytes.checked_add(cost) > Some(self.byte_budget)
        {
            self.entries.clear();
            self.bytes = 0;
            self.clears += 1;
        }
        self.bytes += cost;
        self.entries.insert(
            key,
            Entry {
                pixmap: Arc::new(pixmap),
                bytes: cost,
                last_frame: self.frame,
            },
        );
    }

    /// Drop every entry no placement touched this frame (`image_cache_evict`).
    fn evict(&mut self, this_frame: u64) {
        let mut dropped = 0usize;
        self.entries.retain(|_, entry| {
            let keep = entry.last_frame == this_frame;
            if !keep {
                dropped += entry.bytes;
            }
            keep
        });
        self.bytes -= dropped;
    }

    /// How many placement draws the cache answered from its map.
    #[must_use]
    pub fn hits(&self) -> usize {
        self.hits
    }

    /// How many placement draws had to resample.
    #[must_use]
    pub fn misses(&self) -> usize {
        self.misses
    }

    /// How many times the cache overflowed a limit and cleared.
    #[must_use]
    pub fn clears(&self) -> usize {
        self.clears
    }

    /// How many entries the cache holds (the tests assert the eviction
    /// and budget contracts through it).
    #[cfg(test)]
    pub(crate) fn cached(&self) -> usize {
        self.entries.len()
    }

    /// The cached pixmaps as handed out, for the Arc-validity test.
    #[cfg(test)]
    pub(crate) fn pixmaps(&self) -> Vec<Arc<Pixmap>> {
        self.entries
            .values()
            .map(|entry| Arc::clone(&entry.pixmap))
            .collect()
    }
}

#[cfg(test)]
mod tests;

/// The whole image as a premultiplied, channel-swapped pixmap, read
/// synchronously from the ghostty pixels. `None` when the image has no
/// usable pixels or its size is beyond what the decoder would have
/// produced.
fn unscaled(img: &Image) -> Option<Pixmap> {
    if img.image_w <= 0 || img.image_h <= 0 || img.pixels.is_null() {
        return None;
    }
    let width = u32::try_from(img.image_w).ok()?;
    let height = u32::try_from(img.image_h).ok()?;
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }
    let len = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)?;

    // SAFETY: `img.pixels` is ghostty-owned storage of
    // `image_w * image_h` RGBA8 pixels, valid for the synchronous read
    // between `image_next` handing the placement out and the next
    // iterator call or `frame_end` — the same lifetime rule the old
    // `image_surface` read under. `len` is checked above and the pointer
    // is not null; this is the image pass's one unsafe block.
    let bytes = unsafe { slice::from_raw_parts(img.pixels, len) };

    image_pixmap(bytes, width, height)
}
