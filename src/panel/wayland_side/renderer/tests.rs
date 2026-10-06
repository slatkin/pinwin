//! Display-free tests for the panel renderer's font setup
//! (replace-gtk-with-wayland D10): the cell the font setup measures, and
//! the error a font that cannot resolve at all returns, without a
//! compositor. The renderer's frame tests join with the renderer.

use super::{FontSetup, FontSetupError};
use crate::fontconfig::FontConfig;
use crate::layout::CellSize;
use crate::render::font::FontBook;
use std::error::Error as _;

/// The family the tests pin; CI installs it (design D10). The same family
/// the painter and font tests pin.
const FAMILY: &str = "JetBrainsMono Nerd Font";
/// The size in points the tests run at: the Ghostty default 11.
const SIZE: f64 = 11.0;

/// The test family's font setup, or `None` (printed) when the machine
/// lacks it — the font-dependent tests skip with a printed message only
/// then; a broken `FontBook` or a failed lookup is an `expect`, so a
/// regression cannot hide behind "not installed".
fn setup() -> Option<FontSetup> {
    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(FAMILY) {
        println!("skipped: {FAMILY} is not installed");
        return None;
    }
    Some(FontSetup::resolve_over(book, &config()).expect("the family resolves"))
}

/// The config the tests resolve: the test family at the test size.
fn config() -> FontConfig {
    FontConfig {
        family: Some(FAMILY.to_owned()),
        size: SIZE,
    }
}

/// The cell the test family measures is the row 4.6 gate's cell — the
/// same 9 by 20 the old Pango metrics give, which sets the panel width.
/// The Pango comparison itself is not duplicated here; the gate test in
/// `src/render/cell_metrics` pins it.
#[test]
fn the_test_family_measures_the_expected_cell() {
    let Some(setup) = setup() else {
        return;
    };
    assert_eq!(
        setup.cell(),
        CellSize::new(9, 20),
        "the cell matches the row 4.6 gate"
    );
}

/// A font that cannot be resolved at all — every face load fails, so not
/// even `monospace` comes out — is the font error, never a panic, and the
/// error chain carries the cause.
#[test]
fn an_unresolvable_font_setup_is_an_error() {
    let Ok(mut book) = FontBook::new() else {
        println!("skipped: fontconfig is unavailable");
        return;
    };
    // Test seam (D10): every face load fails until the book is dropped.
    book.fail_all_loads();
    let error = FontSetup::resolve_over(book, &config()).expect_err("no face loads");
    assert!(
        matches!(error, FontSetupError::Font(_)),
        "the font lookup is what failed: {error}"
    );
    assert!(!error.to_string().is_empty(), "the error displays");
    assert!(error.source().is_some(), "the cause is carried");
}
