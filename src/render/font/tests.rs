//! The font module's tests (display-free, D10): fontconfig runs against the
//! real system fonts, so the font-dependent tests skip with a message when
//! the pinned family is not installed.

use super::family::{MatchedFace, family_name_matches, style_matches};
use super::*;
use crate::fontconfig::{DEFAULT_FONT_SIZE, FontConfig};
use ::fontconfig::{
    FC_SLANT_ITALIC, FC_SLANT_ROMAN, FC_WEIGHT_BOLD, FC_WEIGHT_REGULAR, FontconfigError,
};

fn book() -> Option<FontBook> {
    match FontBook::new() {
        Ok(book) => Some(book),
        Err(error) => {
            eprintln!("skipping: fontconfig unavailable: {error}");
            None
        }
    }
}

fn skip_unless_family(book: &FontBook, family: &str) -> bool {
    if book.has_family(family) {
        return true;
    }
    eprintln!("skipping: {family} is not installed");
    false
}

/// The family this machine's tests pin. Font-dependent tests skip when it
/// is not installed (CI installs it — design D10).
const TEST_FAMILY: &str = "JetBrains Mono";

#[test]
fn configured_family_resolves_to_a_matching_file() {
    let Some(book) = book() else { return };
    if !skip_unless_family(&book, TEST_FAMILY) {
        return;
    }

    let config = FontConfig {
        family: Some(TEST_FAMILY.to_owned()),
        size: 11.0,
    };
    let faces = book.family_faces(&config).unwrap();
    assert_eq!(faces.family(), TEST_FAMILY);
    let regular = faces.regular();
    assert!(regular.path().is_file(), "{}", regular.path().display());
    assert_eq!(faces.bold().map(|f| f.path().is_file()), Some(true));
    assert_eq!(faces.italic().map(|f| f.path().is_file()), Some(true));
    assert_eq!(faces.bold_italic().map(|f| f.path().is_file()), Some(true));
    // The name fontconfig reports for the file is the requested family.
    let name = book.best_match(TEST_FAMILY, Style::Regular).unwrap();
    assert!(family_name_matches(TEST_FAMILY, &name.family));
}

#[test]
fn missing_config_resolves_to_monospace_11() {
    let Some(book) = book() else { return };

    let config = FontConfig::default();
    assert_eq!(config.effective_family(), "monospace");
    assert_eq!(config.size, DEFAULT_FONT_SIZE);

    let faces = book.family_faces(&config).unwrap();
    assert_eq!(faces.family(), "monospace");
    assert!(faces.regular().path().is_file());
    assert!(faces.regular().parse().is_some(), "swash parses it");
}

#[test]
fn unknown_family_falls_back_to_monospace() {
    let Some(book) = book() else { return };

    let config = FontConfig {
        family: Some("NoSuchFamilyPinwinZzz".to_owned()),
        size: 11.0,
    };
    let faces = book.family_faces(&config).unwrap();
    // Not the unknown name, not the unrelated face fontconfig best-matched
    // for it: the monospace fallback, and still a real file.
    assert_eq!(faces.family(), "monospace");
    assert!(faces.regular().path().is_file());
}

#[test]
fn the_four_style_faces_resolve_for_the_test_family() {
    let Some(book) = book() else { return };
    if !skip_unless_family(&book, TEST_FAMILY) {
        return;
    }

    let config = FontConfig {
        family: Some(TEST_FAMILY.to_owned()),
        size: 11.0,
    };
    let faces = book.family_faces(&config).unwrap();
    // JetBrains Mono ships all four styles; each is really the requested
    // style, since `style_matches` demands the exact weight and slant.
    assert!(faces.bold().is_some(), "bold resolved");
    assert!(faces.italic().is_some(), "italic resolved");
    assert!(faces.bold_italic().is_some(), "bold-italic resolved");
}

#[test]
fn style_matches_rejects_off_weight_and_slant_matches() {
    // The pure seam: fontconfig's best match for a style the family does
    // not ship comes back with the closest weight or slant, and that is
    // "missing" (row 4.5 synthesizes).
    let regular_only = MatchedFace {
        family: "Only Regular".to_owned(),
        file: "/fonts/only-regular.ttf".to_owned(),
        index: 0,
        weight: FC_WEIGHT_REGULAR,
        slant: FC_SLANT_ROMAN,
    };
    assert!(style_matches(Style::Regular, &regular_only));
    assert!(!style_matches(Style::Bold, &regular_only));
    assert!(!style_matches(Style::Italic, &regular_only));
    assert!(!style_matches(Style::BoldItalic, &regular_only));

    let mut bold_no_italic = regular_only.clone();
    bold_no_italic.weight = FC_WEIGHT_BOLD;
    assert!(style_matches(Style::Bold, &bold_no_italic));
    assert!(!style_matches(Style::BoldItalic, &bold_no_italic));

    let mut italic_no_bold = regular_only.clone();
    italic_no_bold.slant = FC_SLANT_ITALIC;
    assert!(style_matches(Style::Italic, &italic_no_bold));
    assert!(!style_matches(Style::BoldItalic, &italic_no_bold));
}

#[test]
fn family_name_matches_keep_aliases_and_reject_unrelated_faces() {
    assert!(family_name_matches("JetBrains Mono", "JetBrains Mono"));
    // A Nerd Font alias with dropped spaces still matches.
    assert!(family_name_matches("JetBrainsMono NF", "JetBrainsMono NF"));
    assert!(family_name_matches("Foo Bar", "foobar"));
    // An unknown request best-matched to an unrelated family is rejected.
    assert!(!family_name_matches("NoSuchFamilyPinwinZzz", "Noto Sans"));
    assert!(!family_name_matches("Anything", ""));
    assert!(!family_name_matches("", "Noto Sans"));
}

#[test]
fn a_fallback_face_is_found_for_an_emoji() {
    let Some(mut book) = book() else { return };

    let fire = '\u{1F525}';
    let Some(face) = book.fallback_face(fire).unwrap() else {
        eprintln!("skipping: no emoji font is installed");
        return;
    };
    // The fallback really covers the code point, and it is not the
    // primary monospace face.
    assert!(face.covers(fire), "the fallback covers U+1F525");
    assert!(face.path().is_file());
    assert_ne!(face.path(), book_primary_path(&book));
    // Emoji render in colour or as symbols through a dedicated face;
    // assert the resolved face is one of those families.
    let family = face.path().to_string_lossy().to_lowercase();
    assert!(
        family.contains("emoji") || family.contains("symbol"),
        "the emoji fallback is a colour or symbol face, got {family}"
    );
}

fn book_primary_path(book: &FontBook) -> std::path::PathBuf {
    book.family_faces(&FontConfig::default())
        .unwrap()
        .regular()
        .path()
        .to_path_buf()
}

#[test]
fn a_second_fallback_lookup_hits_the_cache() {
    let Some(mut book) = book() else { return };
    if !skip_unless_family(&book, TEST_FAMILY) {
        return;
    }

    let fire = '\u{1F525}';
    let Some(first) = book.fallback_face(fire).unwrap() else {
        eprintln!("skipping: no emoji font is installed");
        return;
    };
    assert_eq!(book.fallback_lookups(), 1);

    let second = book.fallback_face(fire).unwrap().unwrap();
    assert_eq!(second.path(), first.path());
    assert_eq!(second.index(), first.index());
    // Still one real lookup: the cache answered the second call.
    assert_eq!(book.fallback_lookups(), 1);
}

#[test]
fn a_codepoint_no_font_covers_is_cached_as_not_found() {
    let Some(mut book) = book() else { return };

    // U+0378 is unassigned in Unicode; no installed font claims it.
    let unassigned = '\u{0378}';
    assert!(book.fallback_face(unassigned).unwrap().is_none());
    assert_eq!(book.fallback_lookups(), 1);

    assert!(book.fallback_face(unassigned).unwrap().is_none());
    assert_eq!(book.fallback_lookups(), 1, "the not-found answer is cached");
}

#[test]
fn loaded_faces_parse_with_swash_and_cover_their_letters() {
    let Some(book) = book() else { return };
    if !skip_unless_family(&book, TEST_FAMILY) {
        return;
    }

    let config = FontConfig {
        family: Some(TEST_FAMILY.to_owned()),
        size: 11.0,
    };
    let faces = book.family_faces(&config).unwrap();
    let regular = faces.regular();
    let font = regular.parse().expect("swash parses the regular face");
    assert_ne!(font.charmap().map(u32::from('A')), 0, "covers 'A'");
    assert_ne!(font.charmap().map(u32::from('0')), 0, "covers '0'");
    assert!(regular.covers('A'), "Face::covers agrees with the charmap");
    // An obviously absent code point is not covered.
    assert!(!regular.covers('\u{1F525}'), "no emoji in JetBrains Mono");
}

#[test]
fn font_error_displays_and_implements_error() {
    let errors = [
        FontError::Unavailable,
        FontError::Lookup(FontconfigError::NoMatch),
        FontError::NotAFont {
            path: std::path::PathBuf::from("/fonts/nope.ttf"),
        },
        FontError::BadIndex(-1),
    ];
    for error in errors {
        assert_ne!(error.to_string(), "");
        let boxed: Box<dyn std::error::Error> = Box::new(error);
        assert_ne!(boxed.to_string(), "");
    }
    let read = FontError::Read {
        path: std::path::PathBuf::from("/fonts/nope.ttf"),
        source: std::io::Error::other("gone"),
    };
    assert!(std::error::Error::source(&read).is_some());
}
