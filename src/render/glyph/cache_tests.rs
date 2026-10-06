//! The glyph cache's bound tests (display-free, replace-gtk-with-wayland
//! D10): filling past the entry cap or the byte budget clears the cache,
//! the handed-out glyphs survive through their `Arc`, and the counters
//! show the clear.

use std::sync::Arc;

use super::cache::GlyphCache;
use super::harness::{
    FAMILY, SIZE, glyph_of, mask_of, masks_equal, ppem_at, regular_face, request,
};
use super::raster::Glyph;
use super::request::{GlyphRequest, Ppem, Synthesis};

/// Filling past the entry cap clears the cache, the earlier handed-out
/// glyphs stay valid through their `Arc`, and the counters show the clear.
#[test]
fn the_cache_clears_at_the_entry_cap() {
    let face = regular_face(FAMILY).expect("the test family resolves");
    let mut cache = GlyphCache::with_limits(4, usize::MAX);
    let mut first: Option<Arc<Glyph>> = None;
    for ch in ['A', 'B', 'C', 'D', 'E', 'F'] {
        let glyph = cache
            .rasterize(&request(&face, ch, ppem_at(SIZE)))
            .expect("the glyph rasterizes");
        first.get_or_insert(glyph);
    }
    assert_eq!(cache.misses(), 6, "six distinct glyphs, no hits");
    assert_eq!(cache.hits(), 0);
    assert_eq!(cache.clears(), 1, "the fifth insert cleared the full cache");
    assert!(cache.len() <= 4, "the fresh generation stays capped");

    // The glyph handed out before the clear still carries its bytes.
    let first = first.expect("the first glyph was rasterized");
    let again = cache
        .rasterize(&request(&face, 'A', ppem_at(SIZE)))
        .expect("the glyph rasterizes");
    assert_eq!(cache.misses(), 7, "'A' was cleared, so it rasterizes again");
    assert!(masks_equal(mask_of(&again), mask_of(&first)));
    // And it is cached again now.
    cache
        .rasterize(&request(&face, 'A', ppem_at(SIZE)))
        .expect("the glyph rasterizes");
    assert_eq!(cache.hits(), 1);
}

/// Filling past the byte budget clears the cache, and the byte accounting
/// starts over.
#[test]
fn the_cache_clears_at_the_byte_budget() {
    let face = regular_face(FAMILY).expect("the test family resolves");
    // A large ppem gives each mask a measurable byte cost; the budget is
    // built from the measured cost, so the test does not guess the hinted
    // size.
    let mut probe = GlyphCache::new();
    let request = request(
        &face,
        'A',
        Ppem::from_px(100.0).expect("the ppem is usable"),
    );
    let cost = probe
        .rasterize(&request)
        .expect("the glyph rasterizes")
        .byte_cost();
    assert!(cost > 0, "the large glyph costs bytes");

    let mut cache = GlyphCache::with_limits(1000, cost * 2 + 1);
    let first = cache.rasterize(&request).expect("the glyph rasterizes");
    assert_eq!(cache.clears(), 0, "the first glyph fits");
    let second = GlyphRequest::new(
        &face,
        glyph_of(&face, 'B'),
        Ppem::from_px(100.0).expect("the ppem is usable"),
        Synthesis::new(false, false),
        None,
    );
    cache.rasterize(&second).expect("the glyph rasterizes");
    assert_eq!(cache.clears(), 0, "two glyphs fit the budget");
    let third = GlyphRequest::new(
        &face,
        glyph_of(&face, 'C'),
        Ppem::from_px(100.0).expect("the ppem is usable"),
        Synthesis::new(false, false),
        None,
    );
    let _ = cache.rasterize(&third).expect("the glyph rasterizes");
    assert_eq!(cache.clears(), 1, "the third insert overflowed the budget");
    assert_eq!(cache.len(), 1, "the byte accounting starts over");
    assert_eq!(cache.misses(), 3, "three distinct glyphs were rasterized");
    // The glyph handed out before the clear still carries its bytes, the
    // same bytes an independent rasterization of the request produces.
    let fresh = probe.rasterize(&request).expect("the glyph rasterizes");
    assert!(masks_equal(mask_of(&first), mask_of(&fresh)), "same bytes");
}
