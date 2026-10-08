//! The font module's tests (display-free, D10): fontconfig runs against the
//! real system fonts, so the font-dependent tests skip with a message when
//! the pinned family is not installed.

use std::path::Path;
use std::sync::Arc;

use super::face::split_index;
use super::fallback::{CACHE_CAP, FallbackCache};
use super::family::{MatchedFace, family_name_matches, style_matches};
use super::*;
use crate::fontconfig::{DEFAULT_FONT_SIZE, FontConfig};
use ::fontconfig::{
    FC_SLANT_ITALIC, FC_SLANT_OBLIQUE, FC_SLANT_ROMAN, FC_WEIGHT_BLACK, FC_WEIGHT_BOLD,
    FC_WEIGHT_DEMIBOLD, FC_WEIGHT_EXTRABLACK, FC_WEIGHT_EXTRABOLD, FC_WEIGHT_MEDIUM,
    FC_WEIGHT_REGULAR, FontconfigError,
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
    // "missing" (synthesis fills it).
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

    // The thresholds: bold runs from demibold (180) through black (210), so
    // a family whose boldest face is extrabold or black still gets a real
    // bold face; italic accepts oblique (110), so an oblique-only family is
    // not synthesized on top of a real face.
    let mut medium = regular_only.clone();
    medium.weight = FC_WEIGHT_MEDIUM;
    assert!(style_matches(Style::Regular, &medium));

    let mut demibold = regular_only.clone();
    demibold.weight = FC_WEIGHT_DEMIBOLD;
    assert!(style_matches(Style::Bold, &demibold), "demibold is bold");

    let mut extrabold = regular_only.clone();
    extrabold.weight = FC_WEIGHT_EXTRABOLD;
    assert!(style_matches(Style::Bold, &extrabold), "extrabold is bold");

    let mut black = regular_only.clone();
    black.weight = FC_WEIGHT_BLACK;
    assert!(style_matches(Style::Bold, &black), "black is bold");

    let mut extrablack = regular_only.clone();
    extrablack.weight = FC_WEIGHT_EXTRABLACK;
    assert!(!style_matches(Style::Bold, &extrablack), "beyond black");

    let mut oblique = regular_only.clone();
    oblique.slant = FC_SLANT_OBLIQUE;
    assert!(style_matches(Style::Italic, &oblique), "oblique is italic");
    assert!(
        !style_matches(Style::Regular, &oblique),
        "oblique is not roman"
    );

    let mut bold_oblique = bold_no_italic.clone();
    bold_oblique.slant = FC_SLANT_OBLIQUE;
    assert!(style_matches(Style::BoldItalic, &bold_oblique));
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
    // The fallback really covers the code point, and it is not the primary
    // monospace face — coverage and difference are the contract, since a
    // system may ship its emoji face under any file name (OpenMoji,
    // Twemoji, ...).
    assert!(face.covers(fire), "the fallback covers U+1F525");
    assert!(face.path().is_file());
    assert_ne!(face.path(), book_primary_path(&book));
}

fn book_primary_path(book: &FontBook) -> std::path::PathBuf {
    book.family_faces(&FontConfig::default())
        .unwrap()
        .regular()
        .path()
        .to_path_buf()
}

#[test]
fn fontconfig_index_splits_into_file_index_and_named_instance() {
    // The packing fontconfig's fcfreetype.c builds: the low 16 bits are the
    // face index within the file, the high 16 bits the named instance plus
    // one (0 there means the default instance).
    assert_eq!(split_index(0).ok(), Some((0, None)));
    assert_eq!(split_index(3).ok(), Some((3, None)));
    // Cantarell-VF.otf bold matches with instance 3 in face 0.
    assert_eq!(split_index(262_144).ok(), Some((0, Some(3))));
    assert_eq!(split_index(262_144 + 2).ok(), Some((2, Some(3))));
    assert_eq!(split_index(65_535).ok(), Some((65_535, None)));
    assert!(matches!(split_index(-1), Err(FontError::BadIndex(-1))));
}

/// Variable-font families fontconfig matches through named instances: their
/// matched `FC_INDEX` carries the instance in the high 16 bits, which swash
/// must not see as a face index.
const VARIABLE_FAMILIES: [&str; 2] = ["Cantarell", "Adwaita Sans"];

#[test]
fn a_variable_font_family_resolves_with_named_instances() {
    let Some(book) = book() else { return };
    let Some(family) = VARIABLE_FAMILIES
        .into_iter()
        .find(|family| book.has_family(family))
    else {
        eprintln!("skipping: no variable-font family is installed");
        return;
    };
    let config = FontConfig {
        family: Some(family.to_owned()),
        size: 11.0,
    };
    // Before the index split this errored: the bold match carries a named
    // instance in the high 16 bits, and swash rejected that as a face index.
    let faces = book.family_faces(&config).unwrap();
    let all = [
        Some(faces.regular().clone()),
        faces.bold().cloned(),
        faces.italic().cloned(),
        faces.bold_italic().cloned(),
    ];
    for face in all.into_iter().flatten() {
        assert!(
            face.parse().is_some(),
            "{} does not parse",
            face.path().display()
        );
    }
    // The named instance travels on the face, not inside the file index.
    let bold_match = book.best_match(family, Style::Bold).unwrap();
    if bold_match.index > 0xFFFF {
        let bold = faces.bold().expect("the matched bold face is a real style");
        assert!(
            bold.instance().is_some(),
            "the named instance is kept on the face"
        );
    }
}

#[test]
fn the_fallback_cache_clears_when_it_overflows() {
    let mut cache = FallbackCache::new();
    assert_eq!(cache.len(), 0);

    // Fill from the supplementary plane, away from the ASCII probes below.
    for i in 0..CACHE_CAP {
        let codepoint = char::from_u32(u32::try_from(i).unwrap() + 0x1_0000).unwrap();
        assert!(!cache.insert(codepoint, None), "no clear before the cap");
    }
    assert_eq!(cache.len(), CACHE_CAP);
    assert!(cache.get('a').is_none());

    // The insert onto a full cache clears it and starts over, so the cache
    // never holds more than the cap.
    assert!(cache.insert('a', None), "the overflowing insert clears");
    assert_eq!(cache.len(), 1);
    assert!(cache.get('a').is_some());
    assert!(cache.get('b').is_none(), "the cleared answers are gone");
}

#[test]
fn a_repeated_failing_lookup_runs_the_real_lookup_once() {
    let Some(mut book) = book() else { return };
    book.fail_all_loads();

    // 'A' is covered by many installed fonts, so candidates claim it and
    // every one of those loads fails; the walk ends not-found, and that
    // answer is cached like any other.
    assert!(
        book.fallback_face('A').unwrap().is_none(),
        "every load fails"
    );
    assert_eq!(book.fallback_lookups(), 1);

    assert!(book.fallback_face('A').unwrap().is_none());
    assert_eq!(book.fallback_lookups(), 1, "the failed walk is cached too");
}

#[test]
fn codepoints_on_one_font_share_one_allocation() {
    let Some(mut book) = book() else { return };
    let fire = '\u{1F525}';
    let balloon = '\u{1F388}';
    let Some(fire_face) = book.fallback_face(fire).unwrap() else {
        eprintln!("skipping: no emoji font is installed");
        return;
    };
    let Some(balloon_face) = book.fallback_face(balloon).unwrap() else {
        eprintln!("skipping: no emoji font is installed");
        return;
    };
    // Both code points resolve into the same face file, and the byte map
    // hands both faces the same allocation.
    assert_eq!(fire_face.path(), balloon_face.path());
    assert!(
        Arc::ptr_eq(&fire_face.bytes(), &balloon_face.bytes()),
        "one shared allocation for one font file"
    );
}

/// A family whose styles live in shared files: the Noto CJK families are
/// regional faces of two .ttc collections, so several of the four style
/// matches name the same file.
const TEST_TTC_FAMILY: &str = "Noto Sans CJK JP";

#[test]
fn a_family_whose_styles_share_a_file_reads_each_file_once() {
    let Some(book) = book() else { return };
    if !skip_unless_family(&book, TEST_TTC_FAMILY) {
        return;
    }
    let config = FontConfig {
        family: Some(TEST_TTC_FAMILY.to_owned()),
        size: 11.0,
    };
    let faces = book.family_faces(&config).unwrap();
    let all = [
        Some(faces.regular().clone()),
        faces.bold().cloned(),
        faces.italic().cloned(),
        faces.bold_italic().cloned(),
    ];
    let faces: Vec<&Face> = all.iter().flatten().collect();
    let mut files: Vec<&Path> = faces.iter().map(|face| face.path()).collect();
    files.sort_unstable();
    files.dedup();
    // One read per distinct file, not one per style face; on a system with
    // the split .ttc collections this is two reads for four styles.
    assert_eq!(book.byte_reads(), files.len(), "one read per distinct file");
    // Two faces from the same file share its allocation.
    for (i, a) in faces.iter().enumerate() {
        for b in faces.iter().take(i) {
            if a.path() == b.path() {
                assert!(
                    Arc::ptr_eq(&a.bytes(), &b.bytes()),
                    "faces of one file share its bytes"
                );
            }
        }
    }
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
