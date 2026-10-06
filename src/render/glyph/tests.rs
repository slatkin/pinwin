//! Display-free tests for the glyph rasterizer and its cache
//! (replace-gtk-with-wayland D10): real system fonts, real pixel masks, no
//! display. The font-dependent tests skip with a printed message only when
//! the family is not installed; a broken `FontBook` or a failed lookup is
//! an `expect`, so a row 4.4 or 4.5 regression cannot hide behind "not
//! installed".
//!
//! `CanvasMask` exposes its size but not its bytes, and the canvas module
//! was frozen for its own rows, so the coverage tests read the mask back
//! through the public draw path: blitted over a cleared canvas with an
//! opaque colour, a pixel's alpha is exactly the coverage byte (the
//! `draw_mask` blend writes `alpha = coverage * 255 / 255`).

use std::sync::Arc;

use super::cache::GlyphCache;
use super::error::GlyphError;
use super::identity::FaceIdentity;
use super::raster::{Glyph, GlyphImage, GlyphPlacement};
use super::request::{GlyphRequest, PlacementTransform, Ppem, Synthesis};
use crate::fontconfig::FontConfig;
use crate::render::canvas::{Canvas, CanvasColor, CanvasMask, image_pixmap};
use crate::render::font::{Face, FontBook};
use crate::term::cells::Rgb;

/// The family the plain-glyph tests pin; CI installs it (design D10).
const FAMILY: &str = "JetBrainsMono Nerd Font";
/// The size in points the tests run at: the Ghostty default 11, which is
/// 14.6667 device px per em at the 96 dpi convention.
const SIZE: f64 = 11.0;
/// The variable-only family the named-instance test pins: Cantarell ships
/// only as a variable font, and fontconfig reports its named Bold instance
/// as a packed `FC_INDEX` the `Face` carries.
const VARIABLE_FAMILY: &str = "Cantarell";

/// The ppem of `size` points, the cell metrics' conversion.
fn ppem_at(size: f64) -> Ppem {
    Ppem::from_px(size * 96.0 / 72.0).expect("the test ppem is usable")
}

/// The regular face of `family`, or `None` (printed) when the machine
/// lacks it.
fn regular_face(family: &str) -> Option<Face> {
    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(family) {
        return None;
    }
    let config = FontConfig {
        family: Some(family.to_owned()),
        size: SIZE,
    };
    let faces = book.family_faces(&config).expect("the family's faces load");
    Some(faces.regular().clone())
}

/// The glyph id of `ch` in `face`.
fn glyph_of(face: &Face, ch: char) -> u16 {
    face.parse()
        .expect("the face parses")
        .charmap()
        .map(u32::from(ch))
}

/// A plain request for `ch` of `face` at `ppem`.
fn request(face: &Face, ch: char, ppem: Ppem) -> GlyphRequest {
    GlyphRequest::new(
        face,
        glyph_of(face, ch),
        ppem,
        Synthesis::new(false, false),
        None,
    )
}

/// `f64` to `usize`, the tests' one float-to-int seam (no `as`).
fn to_usize(value: f64) -> usize {
    num_traits::cast(value).expect("the test sizes are small")
}

/// `f64` to `i32`, the tests' other float-to-int seam (no `as`).
fn to_i32(value: f64) -> i32 {
    num_traits::cast(value).expect("the test sizes are small")
}

/// The mask's coverage bytes, row major from the top left, read back
/// through a canvas blit (see the module comment).
fn mask_bytes(mask: &CanvasMask) -> Vec<u8> {
    let width = u32::try_from(mask.width()).expect("mask width fits u32");
    let height = u32::try_from(mask.height()).expect("mask height fits u32");
    let mut canvas = Canvas::new(width, height).expect("the mask-sized canvas opens");
    canvas.draw_mask(mask, 0, 0, CanvasColor::from_rgba(255, 255, 255, 255));
    let mut bytes = Vec::with_capacity(mask.width() * mask.height());
    for y in 0..height {
        for x in 0..width {
            bytes.push(canvas.pixel(x, y).expect("inside the canvas")[3]);
        }
    }
    bytes
}

/// The total coverage of a mask: the sum of its coverage bytes.
fn coverage(mask: &CanvasMask) -> u64 {
    mask_bytes(mask).iter().map(|byte| u64::from(*byte)).sum()
}

/// Whether two masks carry the same coverage.
fn masks_equal(left: &CanvasMask, right: &CanvasMask) -> bool {
    left.width() == right.width()
        && left.height() == right.height()
        && mask_bytes(left) == mask_bytes(right)
}

/// The columns of `rows` that carry any coverage, as contiguous
/// `(start, len)` runs.
fn covered_runs(mask: &CanvasMask, rows: std::ops::Range<usize>) -> Vec<(usize, usize)> {
    let bytes = mask_bytes(mask);
    let width = mask.width();
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for column in 0..width {
        let covered = rows.clone().any(|row| bytes[row * width + column] > 0);
        match (start, covered) {
            (None, true) => start = Some(column),
            (Some(open), false) => {
                runs.push((open, column - open));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(open) = start {
        runs.push((open, width - open));
    }
    runs
}

/// The leftmost column of `rows` that carries any coverage.
fn leftmost_column(mask: &CanvasMask, rows: std::ops::Range<usize>) -> usize {
    let bytes = mask_bytes(mask);
    let width = mask.width();
    for column in 0..width {
        if rows.clone().any(|row| bytes[row * width + column] > 0) {
            return column;
        }
    }
    panic!("no covered column in rows {rows:?}");
}

/// The mask of one rasterized glyph (the tests only rasterize masks here).
fn mask_of(glyph: &Glyph) -> &CanvasMask {
    match glyph.image() {
        GlyphImage::Mask(mask) => mask,
        other => panic!("expected a mask, got {other:?}"),
    }
}

/// 'A' of the test family at the test ppem, through a fresh cache.
fn rasterized_a() -> (Arc<Glyph>, GlyphRequest) {
    let face = regular_face(FAMILY).expect("the test family resolves");
    let request = request(&face, 'A', ppem_at(SIZE));
    let mut cache = GlyphCache::new();
    let glyph = cache.rasterize(&request).expect("the glyph rasterizes");
    (glyph, request)
}

/// 'A' of the test family rasterizes to a plausible mask above the
/// baseline, and a second request is a cache hit with the same bytes.
#[test]
fn a_plain_glyph_rasterizes_plausibly_and_caches() {
    let (glyph, request) = rasterized_a();
    let mask = mask_of(&glyph);
    let ppem = f64::from(ppem_at(SIZE).value());
    assert!(
        mask.width() >= 4 && mask.width() <= to_usize(ppem.ceil()) + 1,
        "the mask width {} is bounded by the ppem {ppem}",
        mask.width()
    );
    assert!(
        mask.height() >= 8 && mask.height() <= to_usize(ppem.ceil()) + 2,
        "the mask height {} is bounded by the ppem {ppem}",
        mask.height()
    );
    assert!(coverage(mask) > 0, "the mask carries coverage");
    let placement = glyph.placement();
    assert!(
        placement.top() > 0 && placement.top() <= to_i32(ppem.ceil()) + 2,
        "the glyph sits above the baseline: {placement:?}"
    );

    // The same request is a cache hit, and the handed-out mask is byte for
    // byte the same.
    let mut cache = GlyphCache::new();
    cache.rasterize(&request).expect("the glyph rasterizes");
    let again = cache.rasterize(&request).expect("the glyph rasterizes");
    assert_eq!(cache.misses(), 1);
    assert_eq!(cache.hits(), 1, "the second request is a cache hit");
    assert!(
        masks_equal(mask_of(&again), mask),
        "the cache hit matches the rasterized bytes"
    );
}

/// 'H' of the test family puts its coverage on two vertical stems: the
/// covered columns of the top quarter and of the bottom quarter are two
/// narrow runs each, at the same columns — the shape says so, and hinting
/// keeps the stems on the pixel grid.
#[test]
fn a_stemmed_glyphs_coverage_sits_on_two_vertical_stems() {
    let face = regular_face(FAMILY).expect("the test family resolves");
    let mut cache = GlyphCache::new();
    let glyph = cache
        .rasterize(&request(&face, 'H', ppem_at(SIZE)))
        .expect("the glyph rasterizes");
    let mask = mask_of(&glyph);
    let height = mask.height();
    assert!(height >= 8, "'H' is most of the ppem tall: {height}");

    let top_quarter = 0..height / 4;
    let bottom_quarter = height * 3 / 4..height;
    let top_runs = covered_runs(mask, top_quarter);
    let bottom_runs = covered_runs(mask, bottom_quarter);
    assert_eq!(top_runs.len(), 2, "the two stems at the top: {top_runs:?}");
    assert_eq!(
        bottom_runs.len(),
        2,
        "the two stems at the bottom: {bottom_runs:?}"
    );
    for (top, bottom) in top_runs.iter().zip(&bottom_runs) {
        assert!(
            top.0.abs_diff(bottom.0) <= 1,
            "the stems are vertical: top {top:?} vs bottom {bottom:?}"
        );
        assert!(
            top.1 <= 3 && bottom.1 <= 3,
            "a hinted stem is at most three pixels wide: {top:?} {bottom:?}"
        );
    }
}

/// The ppem quantizes to 26.6 fixed point, so float dust is one cache key,
/// and two sizes are two keys that rasterize differently.
#[test]
fn the_ppem_quantizes_and_two_sizes_rasterize_differently() {
    let px = SIZE * 96.0 / 72.0;
    let base = Ppem::from_px(px).expect("the ppem is usable");
    assert_eq!(
        base,
        Ppem::from_px(px + 1e-9).expect("the ppem is usable"),
        "dust below 1/64 px is the same key"
    );
    assert_ne!(
        base,
        Ppem::from_px(px * 1.5).expect("the ppem is usable"),
        "the fractional scale 1.5 is another key"
    );

    let face = regular_face(FAMILY).expect("the test family resolves");
    let mut cache = GlyphCache::new();
    let small = cache
        .rasterize(&request(&face, 'A', base))
        .expect("the glyph rasterizes");
    let large = cache
        .rasterize(&request(
            &face,
            'A',
            Ppem::from_px(px * 1.5).expect("the ppem is usable"),
        ))
        .expect("the glyph rasterizes");
    assert_eq!(cache.misses(), 2, "two ppems are two cache keys");
    let small_mask = mask_of(&small);
    let large_mask = mask_of(&large);
    assert!(
        large_mask.width() > small_mask.width() && large_mask.height() > small_mask.height(),
        "the larger ppem rasterizes larger: {}x{} vs {}x{}",
        large_mask.width(),
        large_mask.height(),
        small_mask.width(),
        small_mask.height()
    );
}

/// Synthesized bold draws heavier than the regular glyph of the same face.
#[test]
fn synthesized_bold_gains_coverage() {
    let face = regular_face(FAMILY).expect("the test family resolves");
    let ppem = ppem_at(SIZE);
    let glyph_id = glyph_of(&face, 'A');
    let mut cache = GlyphCache::new();
    let plain = cache
        .rasterize(&GlyphRequest::new(
            &face,
            glyph_id,
            ppem,
            Synthesis::new(false, false),
            None,
        ))
        .expect("the glyph rasterizes");
    let bold = cache
        .rasterize(&GlyphRequest::new(
            &face,
            glyph_id,
            ppem,
            Synthesis::new(true, false),
            None,
        ))
        .expect("the glyph rasterizes");
    assert_eq!(cache.misses(), 2, "the style is part of the key");
    assert!(
        coverage(mask_of(&bold)) > coverage(mask_of(&plain)),
        "synthesized bold covers more than regular"
    );
}

/// Synthesized italic moves the top rows of an upright glyph right
/// relative to its bottom rows, while the plain glyph stays vertical.
#[test]
fn synthesized_italic_leans_right() {
    let face = regular_face(FAMILY).expect("the test family resolves");
    let ppem = ppem_at(SIZE);
    let glyph_id = glyph_of(&face, 'H');
    let mut cache = GlyphCache::new();
    let plain = cache
        .rasterize(&GlyphRequest::new(
            &face,
            glyph_id,
            ppem,
            Synthesis::new(false, false),
            None,
        ))
        .expect("the glyph rasterizes");
    let italic = cache
        .rasterize(&GlyphRequest::new(
            &face,
            glyph_id,
            ppem,
            Synthesis::new(false, true),
            None,
        ))
        .expect("the glyph rasterizes");
    let plain_mask = mask_of(&plain);
    let italic_mask = mask_of(&italic);
    assert_eq!(
        plain_mask.height(),
        italic_mask.height(),
        "the skew keeps the height"
    );
    let height = plain_mask.height();
    let top = 0..height / 3;
    let bottom = height * 2 / 3..height;
    let plain_top = leftmost_column(plain_mask, top.clone());
    let plain_bottom = leftmost_column(plain_mask, bottom.clone());
    assert!(
        plain_top.abs_diff(plain_bottom) <= 1,
        "the plain glyph is upright: top {plain_top} vs bottom {plain_bottom}"
    );
    let italic_top = leftmost_column(italic_mask, top);
    let italic_bottom = leftmost_column(italic_mask, bottom);
    assert!(
        italic_top > italic_bottom,
        "the italic top leans right: top {italic_top} vs bottom {italic_bottom}"
    );
}

/// A space rasterizes to the empty result, not an error, and the empty
/// result is cached like any other.
#[test]
fn a_space_is_empty_and_cached() {
    let face = regular_face(FAMILY).expect("the test family resolves");
    let mut cache = GlyphCache::new();
    let request = request(&face, ' ', ppem_at(SIZE));
    let glyph = cache.rasterize(&request).expect("the space rasterizes");
    assert!(
        matches!(glyph.image(), GlyphImage::Empty),
        "a space is empty"
    );
    assert_eq!(glyph.placement(), GlyphPlacement::new(0, 0));
    cache.rasterize(&request).expect("the space rasterizes");
    assert_eq!(cache.misses(), 1, "the empty result is cached");
    assert_eq!(cache.hits(), 1);
}

/// The fallback face's fire emoji rasterizes to a colour image with real
/// colour, and is cached. Skipped only when no colour emoji font covers
/// the code point.
#[test]
fn a_colour_emoji_rasterizes_in_colour() {
    let mut book = FontBook::new().expect("the font book opens");
    let face = match book.fallback_face('🔥') {
        Ok(Some(face)) => face,
        Ok(None) => {
            println!("skipped: no font covers the fire emoji");
            return;
        }
        Err(error) => panic!("the fallback lookup failed: {error}"),
    };
    let glyph_id = glyph_of(&face, '🔥');
    assert_ne!(glyph_id, 0, "the emoji face covers the code point");
    let mut cache = GlyphCache::new();
    let request = request(&face, '🔥', ppem_at(SIZE));
    let glyph = cache.rasterize(&request).expect("the emoji rasterizes");
    let pixmap = match glyph.image() {
        GlyphImage::Color(pixmap) => pixmap,
        other => panic!("the emoji rasterizes to a colour image, got {other:?}"),
    };
    assert!(pixmap.width() > 0 && pixmap.height() > 0);
    // The pixmap is premultiplied and channel-swapped (canvas order: blue,
    // green, red, alpha). Some pixel must carry alpha and colour.
    let pixels = pixmap.data().as_chunks::<4>().0;
    assert!(pixels.iter().any(|px| px[3] > 0), "some pixel has alpha");
    assert!(
        pixels
            .iter()
            .any(|px| px[3] == 255 && !(px[0] == px[1] && px[1] == px[2])),
        "some pixel is coloured, not grey"
    );
    cache.rasterize(&request).expect("the emoji rasterizes");
    assert_eq!(cache.misses(), 1, "the emoji is cached");
    assert_eq!(cache.hits(), 1);
}

/// The named-instance obligation from the row 4.4 review: a named-instance
/// Bold of a variable-only font draws heavier than the default instance of
/// the same file, and the two are distinct cache keys. Skipped when
/// fontconfig matches no variable face for the family.
#[test]
fn a_named_instance_bold_draws_heavier_than_the_default_instance() {
    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(VARIABLE_FAMILY) {
        println!("skipped: {VARIABLE_FAMILY} is not installed");
        return;
    }
    let config = FontConfig {
        family: Some(VARIABLE_FAMILY.to_owned()),
        size: SIZE,
    };
    let faces = book.family_faces(&config).expect("the family's faces load");
    let Some(bold) = faces.bold() else {
        println!("skipped: {VARIABLE_FAMILY} matched no real bold face");
        return;
    };
    let Some(instance) = bold.instance() else {
        println!("skipped: fontconfig matched no named instance for {VARIABLE_FAMILY} bold");
        return;
    };
    let font = bold.parse().expect("the face parses");
    assert!(
        instance < font.instances().len(),
        "the instance {instance} exists in the font's {} instances",
        font.instances().len()
    );

    let mut cache = GlyphCache::new();
    let with_instance = request(bold, 'A', ppem_at(SIZE));
    let default = with_instance.at_default_instance();
    let bold_glyph = cache
        .rasterize(&with_instance)
        .expect("the glyph rasterizes");
    let default_glyph = cache.rasterize(&default).expect("the glyph rasterizes");
    assert_eq!(cache.misses(), 2, "the instance is part of the key");
    cache
        .rasterize(&with_instance)
        .expect("the glyph rasterizes");
    assert_eq!(cache.hits(), 1, "the instance request hits its own key");
    assert!(
        coverage(mask_of(&bold_glyph)) > coverage(mask_of(&default_glyph)),
        "the named-instance Bold draws heavier than the default instance"
    );
}

/// The placement transform scales the rasterized geometry (a scale of 0.5
/// halves the mask) and shifts the placement, and dust in the transform
/// values is the same cache key.
#[test]
fn a_placement_transform_scales_and_shifts() {
    let face = regular_face(FAMILY).expect("the test family resolves");
    // The larger test ppem leaves the half-size glyph several pixels wide.
    let ppem = ppem_at(SIZE * 1.5);
    let mut cache = GlyphCache::new();
    let base = cache
        .rasterize(&request(&face, 'A', ppem))
        .expect("the glyph rasterizes");
    let base_mask = mask_of(&base);

    let half = PlacementTransform::new(0.5, 0.5, 0.0, 0.0).expect("the transform is usable");
    assert_eq!(
        half,
        PlacementTransform::new(0.5 + 1e-12, 0.5, 0.0, 0.0).expect("the transform is usable"),
        "transform dust quantizes to one key"
    );
    let scaled = cache
        .rasterize(&GlyphRequest::new(
            &face,
            glyph_of(&face, 'A'),
            ppem,
            Synthesis::new(false, false),
            Some(half),
        ))
        .expect("the glyph rasterizes");
    let scaled_mask = mask_of(&scaled);
    assert!(scaled_mask.width() > 0 && scaled_mask.height() > 0);
    assert!(
        scaled_mask.width().abs_diff(base_mask.width() / 2) <= 1
            && scaled_mask.height().abs_diff(base_mask.height() / 2) <= 1,
        "a scale of 0.5 halves the mask: {}x{} vs {}x{}",
        scaled_mask.width(),
        scaled_mask.height(),
        base_mask.width(),
        base_mask.height()
    );

    let shifted_request = GlyphRequest::new(
        &face,
        glyph_of(&face, 'A'),
        ppem,
        Synthesis::new(false, false),
        Some(PlacementTransform::new(1.0, 1.0, 2.0, 3.0).expect("the transform is usable")),
    );
    let shifted = cache
        .rasterize(&shifted_request)
        .expect("the glyph rasterizes");
    assert_eq!(
        shifted.placement().left(),
        base.placement().left() + 2,
        "the offset moves the placement right"
    );
    assert_eq!(
        shifted.placement().top(),
        base.placement().top() + 3,
        "the offset moves the placement up"
    );
    assert!(
        masks_equal(mask_of(&shifted), base_mask),
        "an identity scale keeps the bytes"
    );
}

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

/// A mask result draws into the canvas through `draw_mask`, and a colour
/// result draws through `draw_image`.
#[test]
fn the_results_feed_the_canvas() {
    let (glyph, _) = rasterized_a();
    let mask = mask_of(&glyph);
    let bytes = mask_bytes(mask);
    // The densest coverage byte of the mask, and where it sits.
    let densest = bytes.iter().copied().max().expect("the mask is not empty");
    let at = bytes
        .iter()
        .position(|byte| *byte == densest)
        .expect("the densest byte exists");
    let mut canvas = Canvas::new(
        u32::try_from(mask.width()).expect("fits u32") + 2,
        u32::try_from(mask.height()).expect("fits u32") + 2,
    )
    .expect("the canvas opens");
    canvas.draw_mask(
        mask,
        1,
        1,
        CanvasColor::from_theme(Rgb { r: 255, g: 0, b: 0 }),
    );
    let x = u32::try_from(at % mask.width()).expect("fits u32") + 1;
    let y = u32::try_from(at / mask.width()).expect("fits u32") + 1;
    // The tinted pixel carries the coverage in red and alpha, canvas order
    // (blue, green, red, alpha).
    assert_eq!(
        canvas.pixel(x, y).expect("inside the canvas")[3],
        densest,
        "the alpha is the coverage byte"
    );
    assert_eq!(
        canvas.pixel(x, y).expect("inside the canvas")[2],
        densest,
        "the red channel carries the coverage"
    );
    assert_eq!(
        canvas.pixel(0, 0).expect("inside the canvas"),
        [0, 0, 0, 0],
        "outside the mask stays clear"
    );

    // A colour result, built the way the rasterizer builds one, blits
    // through `draw_image`.
    let image = image_pixmap(&[10, 20, 30, 255], 1, 1).expect("the image is valid");
    let mut canvas = Canvas::new(4, 4).expect("the canvas opens");
    canvas.draw_image(&image, 1, 1);
    assert_eq!(canvas.pixel(1, 1), Some([30, 20, 10, 255]));
}

/// The emoji's colour pixmap draws into the canvas through `draw_image`.
/// Skipped when no colour emoji font covers the code point.
#[test]
fn a_colour_glyph_draws_into_the_canvas() {
    let mut book = FontBook::new().expect("the font book opens");
    let face = match book.fallback_face('🔥') {
        Ok(Some(face)) => face,
        Ok(None) => {
            println!("skipped: no font covers the fire emoji");
            return;
        }
        Err(error) => panic!("the fallback lookup failed: {error}"),
    };
    let mut cache = GlyphCache::new();
    let glyph = cache
        .rasterize(&request(&face, '🔥', ppem_at(SIZE)))
        .expect("the emoji rasterizes");
    let pixmap = match glyph.image() {
        GlyphImage::Color(pixmap) => pixmap,
        other => panic!("the emoji rasterizes to a colour image, got {other:?}"),
    };
    let mut canvas =
        Canvas::new(pixmap.width() + 2, pixmap.height() + 2).expect("the canvas opens");
    canvas.draw_image(pixmap, 1, 1);
    // Find an opaque pixel of the emoji and read the same pixel back from
    // the canvas at the blit position.
    let pixels = pixmap.data().as_chunks::<4>().0;
    let at = pixels
        .iter()
        .position(|px| px[3] == 255)
        .expect("the emoji has an opaque pixel");
    let x = u32::try_from(at % usize::try_from(pixmap.width()).expect("fits usize"))
        .expect("fits u32")
        + 1;
    let y = u32::try_from(at / usize::try_from(pixmap.width()).expect("fits usize"))
        .expect("fits u32")
        + 1;
    assert_eq!(
        canvas.pixel(x, y).expect("inside the canvas"),
        pixels[at],
        "the blit copies the emoji's bytes"
    );
}

/// Bad requests give typed errors instead of panics: an unusable ppem, an
/// unusable transform, an unparsable face and an unknown named instance.
#[test]
fn bad_requests_error_instead_of_panic() {
    for px in [0.0, -1.0, f64::NAN, f64::INFINITY, 70_000.0] {
        assert!(
            matches!(Ppem::from_px(px), Err(GlyphError::BadPpem(_))),
            "{px}"
        );
    }
    assert!(
        matches!(
            PlacementTransform::new(f64::NAN, 1.0, 0.0, 0.0),
            Err(GlyphError::BadTransform {
                what: "scale x",
                ..
            })
        ),
        "a non-finite scale is refused"
    );
    assert!(
        matches!(
            PlacementTransform::new(1.0, 1.0, 0.0, f64::INFINITY),
            Err(GlyphError::BadTransform {
                what: "offset y",
                ..
            })
        ),
        "a non-finite offset is refused"
    );

    let Some(face) = regular_face(FAMILY) else {
        println!("skipped: {FAMILY} is not installed");
        return;
    };
    let mut cache = GlyphCache::new();

    // Bytes that are not a font, under the identity of a real face.
    let garbage = Arc::from(&b"not a font file"[..]);
    let unparsable = GlyphRequest::from_parts(
        FaceIdentity::of(&face),
        garbage,
        glyph_of(&face, 'A'),
        ppem_at(SIZE),
        Synthesis::new(false, false),
        None,
    );
    assert!(matches!(
        cache.rasterize(&unparsable),
        Err(GlyphError::UnparsableFace)
    ));

    // A named instance the face does not have.
    let unknown = GlyphRequest::from_parts(
        FaceIdentity::for_test(face.path(), face.index(), Some(9999)),
        face.bytes(),
        glyph_of(&face, 'A'),
        ppem_at(SIZE),
        Synthesis::new(false, false),
        None,
    );
    assert!(matches!(
        cache.rasterize(&unknown),
        Err(GlyphError::UnknownInstance { index: 9999, .. })
    ));
}
