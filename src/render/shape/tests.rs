//! Display-free tests for the shaper (replace-gtk-with-wayland D10): real
//! system fonts, real shaping, no display. The font-dependent tests skip
//! with a printed message only when the family is not installed; a broken
//! `FontBook` or a failed lookup is an `expect`, so a shaping
//! regression cannot hide behind "not installed".

use std::sync::Arc;

use super::shaper::TextShaper;
use super::{ShapeError, ShapedCluster};
use crate::fontconfig::FontConfig;
use crate::render::font::{Face, FamilyFaces, FontBook, Style};
use crate::render::glyph::{Ppem, Synthesis};

/// The family the tests pin; CI installs it (design D10).
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

/// The faces of `family` as the panel loads them, or `None` (printed) when
/// the machine lacks the family.
fn faces_of(book: &FontBook, family: &str) -> Option<FamilyFaces> {
    if !book.has_family(family) {
        println!("skipped: {family} is not installed");
        return None;
    }
    let config = FontConfig {
        family: Some(family.to_owned()),
        size: SIZE,
    };
    Some(book.family_faces(&config).expect("the family's faces load"))
}

/// A shaper over the test family's faces, or `None` (printed) when the
/// machine lacks the family.
fn shaper() -> Option<TextShaper> {
    let book = FontBook::new().expect("the font book opens");
    let faces = faces_of(&book, FAMILY)?;
    Some(TextShaper::new(faces, book))
}

/// 'A' shapes to one glyph whose advance is the face's em-width fraction —
/// `JetBrains Mono` advances 0.6 em — and sits near the cell width the
/// metrics compute from the same font.
#[test]
fn a_plain_letter_shapes_with_a_cell_width_advance() {
    let Some(mut shaper) = shaper() else { return };
    let cluster = shaper
        .shape("A", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    let glyphs = cluster.glyphs();
    assert_eq!(glyphs.len(), 1, "one letter, one glyph: {glyphs:?}");
    let advance = f64::from(glyphs[0].x_advance());
    let ppem = f64::from(ppem_at(SIZE).value());
    assert!(
        (advance / ppem - 0.6).abs() < 0.01,
        "the advance {advance} is 0.6 em of {ppem}"
    );

    let book = FontBook::new().expect("the font book opens");
    let faces = faces_of(&book, FAMILY).expect("the family's faces load");
    let face = faces.regular().clone();
    let cell_w = crate::render::cell_metrics::measure(&face, SIZE)
        .expect("the cell metrics compute")
        .cell_w();
    assert!(
        (advance - f64::from(cell_w)).abs() <= 1.0,
        "the advance {advance} is near the cell width {cell_w}"
    );
    assert!(cluster.advance() > 0.0, "the cluster advances");
}

/// "e" plus combining acute shapes to the base and the mark, or composes
/// to one glyph through ccmp — whichever the font's shaping table does.
/// What this font really does: `JetBrainsMono` Nerd Font composes the pair
/// into the precomposed glyph, so the cluster is one glyph with the same
/// advance as 'e' alone; the assertion accepts either outcome but demands
/// the mark never advances the pen on its own.
#[test]
fn a_cluster_with_a_combining_mark_keeps_the_pen_honest() {
    let Some(mut shaper) = shaper() else { return };
    let base = shaper
        .shape("e", Style::Regular, ppem_at(SIZE))
        .expect("the base shapes");
    let marked = shaper
        .shape("e\u{0301}", Style::Regular, ppem_at(SIZE))
        .expect("the marked cluster shapes");
    let glyphs = marked.glyphs();
    assert!(!glyphs.is_empty(), "the marked cluster draws");
    let base_advance = base.advance();
    assert!(
        f64::from(marked.advance()) <= f64::from(base_advance) + 0.5,
        "the mark never widens the cell past its base: {} vs {}",
        marked.advance(),
        base_advance
    );
    if glyphs.len() == 2 {
        // The pair kept base and mark apart: the mark carries a nonzero
        // offset that places it over the base, and no advance.
        let mark = glyphs[1];
        assert_ne!(mark.id(), glyphs[0].id(), "the mark is its own glyph");
        assert_ne!(
            (mark.x_offset(), mark.y_offset()),
            (0.0, 0.0),
            "the mark is offset onto the base"
        );
    } else {
        // The pair composed into one glyph (ccmp or a preformed glyph).
        assert_eq!(glyphs.len(), 1, "either two glyphs or one: {glyphs:?}");
    }
}

/// A style the family really has is not synthesized; a style it lacks is.
/// The family's faces are rebuilt without bold and italic, so the
/// synthesis rules run without depending on which families the machine
/// ships.
#[test]
fn the_face_choice_synthesizes_only_what_is_missing() {
    let Some(mut shaper) = shaper() else { return };
    let plain = shaper
        .shape("A", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert!(!plain.synthesis().bold() && !plain.synthesis().italic());
    let bold = shaper
        .shape("A", Style::Bold, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert!(!bold.synthesis().bold(), "the family has a real bold face");

    // The same family with the optional styles missing: bold and italic
    // synthesize on the regular face, and bold-italic takes the nearest
    // face (italic, when present) and synthesizes what is missing.
    let book = FontBook::new().expect("the font book opens");
    let faces = faces_of(&book, FAMILY).expect("the family's faces load");
    let italic_only = FamilyFaces::new(
        faces.family().to_owned(),
        faces.regular().clone(),
        None,
        faces.italic().cloned(),
        None,
    );
    let mut shaper = TextShaper::new(italic_only, book);
    let bold = shaper
        .shape("A", Style::Bold, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert!(bold.synthesis().bold() && !bold.synthesis().italic());
    let italic = shaper
        .shape("A", Style::Italic, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert!(!italic.synthesis().bold() && !italic.synthesis().italic());
    let bold_italic = shaper
        .shape("A", Style::BoldItalic, ppem_at(SIZE))
        .expect("the cluster shapes");
    // The italic face carries the slant, so only bold synthesizes.
    assert!(bold_italic.synthesis().bold() && !bold_italic.synthesis().italic());

    // With no style faces at all, both synthesize on the regular face.
    let book = FontBook::new().expect("the font book opens");
    let faces = faces_of(&book, FAMILY).expect("the family's faces load");
    let regular_only = FamilyFaces::new(
        faces.family().to_owned(),
        faces.regular().clone(),
        None,
        None,
        None,
    );
    let mut shaper = TextShaper::new(regular_only, book);
    let bold_italic = shaper
        .shape("A", Style::BoldItalic, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert!(bold_italic.synthesis().bold() && bold_italic.synthesis().italic());
}

/// A cluster the primary font lacks falls back to another face, and the
/// cell's own synthesis flags apply to the fallback. Skipped when no font
/// covers the fire emoji.
#[test]
fn an_uncovered_cluster_falls_back_to_another_face() {
    let mut book = FontBook::new().expect("the font book opens");
    let Some(faces) = faces_of(&book, FAMILY) else {
        return;
    };
    let expected = match book.fallback_face('🔥') {
        Ok(Some(face)) => face,
        Ok(None) => {
            println!("skipped: no font covers the fire emoji");
            return;
        }
        Err(error) => panic!("the fallback lookup failed: {error}"),
    };
    let mut shaper = TextShaper::new(faces, book);
    let cluster = shaper
        .shape("🔥", Style::BoldItalic, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(
        cluster.face().path(),
        expected.path(),
        "the cluster shaped on the fallback face"
    );
    assert!(
        cluster.synthesis().bold() && cluster.synthesis().italic(),
        "the fallback face synthesizes the cell's bold and italic"
    );
    assert!(
        !cluster.glyphs().is_empty(),
        "the emoji face covers the sequence"
    );
}

/// The cache: a repeated cluster is a hit with the same `Arc`, the cap
/// clears on overflow, and an error is not cached.
#[test]
fn the_cache_hits_and_clears_and_never_caches_errors() {
    let Some(mut shaper) = shaper() else { return };
    let first = shaper
        .shape("A", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    let again = shaper
        .shape("A", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(shaper.hits(), 1, "the repeat is a cache hit");
    assert_eq!(shaper.misses(), 1);
    assert!(
        Arc::ptr_eq(&first, &again),
        "the hit is the same shared cluster"
    );

    // Another style is another key.
    shaper
        .shape("A", Style::Bold, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(shaper.misses(), 2, "the style is part of the key");

    // A smaller cache clears on overflow instead of growing past it.
    let book = FontBook::new().expect("the font book opens");
    let faces = faces_of(&book, FAMILY).expect("the family's faces load");
    let mut small = TextShaper::with_cap(faces, book, 2);
    for cluster in ["a", "b", "c"] {
        small
            .shape(cluster, Style::Regular, ppem_at(SIZE))
            .expect("the cluster shapes");
    }
    assert_eq!(small.clears(), 1, "the third insert cleared the cache");
    assert_eq!(small.len(), 1, "the fresh generation holds one cluster");
    assert_eq!(small.hits(), 0);

    // An error is not cached: shaping bytes that are not a font fails
    // twice, and the cache stays empty. The bytes travel beside the face,
    // the same seam `GlyphRequest::from_parts` gives the rasterizer's
    // tests.
    let book = FontBook::new().expect("the font book opens");
    let faces = faces_of(&book, FAMILY).expect("the family's faces load");
    let face = faces.regular().clone();
    let mut shaper = TextShaper::new(faces, book);
    let garbage: std::sync::Arc<[u8]> = std::sync::Arc::from(&b"not a font file"[..]);
    for _ in 0..2 {
        assert!(matches!(
            shaper.shape_miss(
                &face,
                &garbage,
                "A",
                Synthesis::new(false, false),
                ppem_at(SIZE),
            ),
            Err(ShapeError::UnparsableFace)
        ));
    }
    assert_eq!(shaper.len(), 0, "errors are not cached");
    assert_eq!(shaper.hits(), 0);
}

/// A cache hit skips the face choice: the choice is a pure function of the
/// key (cluster text, style, ppem), so it runs only on a miss — the
/// charmap walk and the parser pass it costs are paid once per distinct
/// request, not per frame.
#[test]
fn a_cache_hit_does_not_rerun_the_face_choice() {
    let Some(mut shaper) = shaper() else { return };
    shaper
        .shape("A", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(shaper.face_choices(), 1, "the first request chose a face");
    shaper
        .shape("A", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(shaper.hits(), 1, "the repeat is a hit");
    assert_eq!(
        shaper.face_choices(),
        1,
        "the hit did not run the face choice"
    );

    // A different cluster or style is a new key and chooses again.
    shaper
        .shape("B", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    shaper
        .shape("A", Style::Bold, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(shaper.face_choices(), 3, "each new key chose once");
}

/// The named instance applies in shaping: the named-instance Bold of the
/// variable-only Cantarell shapes 'A' to different metrics than the
/// default instance of the same file. Skipped when fontconfig matches no
/// variable face for the family.
#[test]
fn the_named_instance_applies_in_shaping() {
    let book = FontBook::new().expect("the font book opens");
    let Some(faces) = faces_of(&book, VARIABLE_FAMILY) else {
        return;
    };
    let Some(bold) = faces.bold().cloned() else {
        println!("skipped: {VARIABLE_FAMILY} matched no real bold face");
        return;
    };
    if bold.instance().is_none() {
        println!("skipped: fontconfig matched no named instance for {VARIABLE_FAMILY} bold");
        return;
    }
    let mut shaper = TextShaper::new(faces, book);
    let bold_shaped = shaper
        .shape("A", Style::Bold, ppem_at(SIZE))
        .expect("the cluster shapes");
    let regular_shaped = shaper
        .shape("A", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(
        bold_shaped.face().instance(),
        bold.instance(),
        "the bold cluster shaped on the named instance"
    );
    assert_ne!(
        (bold_shaped.advance(), bold_shaped.glyphs()[0].id()),
        (regular_shaped.advance(), regular_shaped.glyphs()[0].id()),
        "the named instance shapes differently from the default instance"
    );
}

/// An empty cluster shapes to an empty result, not an error.
#[test]
fn an_empty_cluster_shapes_to_an_empty_result() {
    let Some(mut shaper) = shaper() else { return };
    let cluster = shaper
        .shape("", Style::Regular, ppem_at(SIZE))
        .expect("the empty cluster shapes");
    assert!(
        cluster.glyphs().is_empty(),
        "an empty cluster draws nothing"
    );
    assert_eq!(cluster.advance(), 0.0);
}

/// The shaped cluster's y convention, on a real mark: a mark that hangs
/// above its base carries a positive (up) offset. Uses the two-glyph
/// outcome of the combining-mark cluster when the font produces it.
#[test]
fn the_y_offsets_point_up_for_a_mark_above_its_base() {
    let Some(mut shaper) = shaper() else { return };
    let cluster = shaper
        .shape("e\u{0301}", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    let glyphs = cluster.glyphs();
    if glyphs.len() == 2 {
        assert!(
            glyphs[1].y_offset() > 0.0,
            "the acute mark sits above the base: {:?}",
            glyphs[1]
        );
    } else {
        println!("noted: the font composes e plus U+0301 into one glyph");
    }
}

/// The shaped cluster's face is the one the style chose, carried through
/// to the rasterizer: a `ShapedCluster` re-rasterizes its glyphs through
/// the same `Face` it reports.
#[test]
fn the_shaped_cluster_carries_its_face() {
    let Some(mut shaper) = shaper() else { return };
    let cluster = shaper
        .shape("A", Style::Bold, ppem_at(SIZE))
        .expect("the cluster shapes");
    let face = cluster.face();
    assert!(face.parse().is_some(), "the carried face parses");
    assert_ne!(
        face.parse()
            .expect("the face parses")
            .charmap()
            .map(u32::from('A')),
        0,
        "the carried face covers the cluster"
    );
}

/// `ShapedCluster` and `ShapeError` are the module's public face; a
/// cluster reports its parts through the accessors only.
#[test]
fn a_shaped_cluster_reports_its_parts() {
    let Some(mut shaper) = shaper() else { return };
    let cluster = shaper
        .shape("A", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    let ShapedCluster { .. } = &*cluster;
    assert_eq!(cluster.glyphs().len(), 1);
    assert_eq!(
        cluster.advance(),
        cluster.glyphs()[0].x_advance(),
        "one glyph: the cluster advance is its own"
    );
}

/// The keycap cluster "1" U+FE0F U+20E3: the primary face maps the digit
/// but neither the variation selector (ignored, it needs no glyph) nor the
/// enclosing keycap mark, and a mark needs a glyph like any other code
/// point — so the cell falls back to a face that covers U+20E3, the way
/// the old Pango path's itemization picked an emoji font for it. Skipped
/// when no installed font covers U+20E3; a failed lookup is a failure,
/// never a skip.
#[test]
fn a_keycap_cluster_falls_back_to_a_face_that_covers_the_mark() {
    let mut book = FontBook::new().expect("the font book opens");
    let Some(faces) = faces_of(&book, FAMILY) else {
        return;
    };
    let primary = faces.regular().clone();
    assert!(
        !primary.covers('\u{20E3}'),
        "the test family must lack the keycap mark for this test to prove anything"
    );
    let expected = match book.fallback_face('\u{20E3}') {
        Ok(Some(face)) => face,
        Ok(None) => {
            println!("skipped: no installed font covers U+20E3");
            return;
        }
        Err(error) => panic!("the fallback lookup failed: {error}"),
    };
    let mut shaper = TextShaper::new(faces, book);
    let cluster = shaper
        .shape("1\u{FE0F}\u{20E3}", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(
        cluster.face().path(),
        expected.path(),
        "the keycap cluster shaped on the face fontconfig ranks for U+20E3"
    );
    assert!(
        cluster.face().covers('\u{20E3}'),
        "the chosen face covers the keycap mark"
    );
}

/// "e" plus combining acute in `JetBrainsMono` does not fall back: the face
/// maps U+0301, and a mark the face covers needs no fallback any more than
/// a covered base does. Skipped when the machine's face lacks U+0301,
/// because then the test cannot separate mark coverage from fallback
/// behaviour.
#[test]
fn a_mark_the_face_maps_does_not_trigger_a_fallback() {
    let book = FontBook::new().expect("the font book opens");
    let Some(faces) = faces_of(&book, FAMILY) else {
        return;
    };
    let primary = faces.regular().clone();
    if !primary.covers('\u{0301}') {
        println!("skipped: {FAMILY} does not map U+0301 on this machine");
        return;
    }
    let mut shaper = TextShaper::new(faces, book);
    let cluster = shaper
        .shape("e\u{0301}", Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    assert_eq!(
        cluster.face().path(),
        primary.path(),
        "a mark the face maps never moves the cluster to a fallback"
    );
}

/// A mark the primary face lacks, with a base the primary covers: the
/// cluster moves to the fallback only when the fallback covers the base
/// too — the whole cluster shapes on one face — and stays on the primary
/// face when no fallback covers both, rather than drawing the base as
/// notdef. What this machine does: the fallback fontconfig ranks for the
/// Tibetan vowel sign I (U+0F72) is Noto Serif Tibetan, which covers the
/// mark but not the Latin base, so the cluster stays on the primary face;
/// for U+1AB0 the fallback is Noto Sans, which covers base and mark, so
/// the cluster moves.
#[test]
fn an_uncovered_mark_moves_the_cluster_only_when_the_fallback_covers_the_base() {
    let book = FontBook::new().expect("the font book opens");
    let Some(faces) = faces_of(&book, FAMILY) else {
        return;
    };
    let primary = faces.regular().clone();
    assert!(primary.covers('e'), "the test family covers the base");
    one_mark_case(
        FontBook::new().expect("the font book opens"),
        faces.clone(),
        &primary,
        "U+1AB0",
        '\u{1AB0}',
    );
    one_mark_case(
        FontBook::new().expect("the font book opens"),
        faces,
        &primary,
        "U+0F72",
        '\u{0F72}',
    );
}

/// One (base, mark) case of the uncovered-mark test above: the cluster
/// moves to the fallback only when the fallback covers the base too, and
/// stays on the primary face when no fallback covers both.
fn one_mark_case(mut book: FontBook, faces: FamilyFaces, primary: &Face, name: &str, mark: char) {
    assert!(
        !primary.covers(mark),
        "the test family lacks {name} for this test to prove anything"
    );
    let fallback = match book.fallback_face(mark) {
        Ok(Some(face)) => face,
        Ok(None) => {
            println!("skipped: no installed font covers {name}");
            return;
        }
        Err(error) => panic!("the fallback lookup failed: {error}"),
    };
    let mut shaper = TextShaper::new(faces, book);
    let cluster = shaper
        .shape(&format!("e{mark}"), Style::Regular, ppem_at(SIZE))
        .expect("the cluster shapes");
    if fallback.covers('e') {
        println!("noted: the fallback for {name} covers base and mark; the cluster moved to it");
        assert_eq!(
            cluster.face().path(),
            fallback.path(),
            "a fallback covering the whole cluster takes the cluster"
        );
    } else {
        println!(
            "noted: the fallback for {name} lacks the base; the cluster stayed on the primary face"
        );
        assert_eq!(
            cluster.face().path(),
            primary.path(),
            "a fallback that lacks the base never takes the cluster"
        );
    }
}
