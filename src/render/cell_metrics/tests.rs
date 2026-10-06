//! Display-free tests for the cell metrics (replace-gtk-with-wayland D10):
//! the real-font gate against the old Pango numbers, the pure rounding
//! rules, the `CSI 16 t` reply and the painter metrics at a fractional
//! scale.

use super::{
    MetricsError, ceil_units_26_6_to_px, derive, hinted_vertical_metrics, measure,
    quantize_26_6_units, round_away_from_zero, round_units_26_6_to_px,
};
use crate::fontconfig::FontConfig;
use crate::render::font::FontBook;
use crate::term::{PngDecoder, PtySink, Terminal};

/// The family the real-font tests gate on; the `snap-text-origins` spike
/// measured the old Pango metrics with it.
const FAMILY: &str = "JetBrainsMono Nerd Font";
const SIZE: f64 = 11.0;

/// The family the boundary gate runs on: its ascent sits exactly on the
/// pixel boundary at size 14, which is what the 26.6 quantization is for.
const LEKTON: &str = "Lekton Nerd Font";
const LEKTON_SIZE: f64 = 14.0;

/// The regular face of `family` at `size`, or `None` (printed) when the
/// machine lacks it — the font tests only run where CI installs the font.
/// A broken `FontBook` or a failed face lookup is not a skip: it fails the
/// test, so a row 4.4 or 4.5 regression cannot hide behind "not installed".
fn regular_face(family: &str, size: f64) -> Option<crate::render::font::Face> {
    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(family) {
        return None;
    }
    let config = FontConfig {
        family: Some(family.to_owned()),
        size,
    };
    let faces = book.family_faces(&config).expect("the family's faces load");
    Some(faces.regular().clone())
}

/// Gate (row 4.6): the swash metrics give `JetBrainsMono Nerd Font` 11 the
/// same cell the old Pango metrics give — 9 by 20, ascent 15, baseline 5.
/// If a faithful variant produced a different cell size, this change would
/// stop and ask the user: the cell size sets the panel width.
#[test]
fn jetbrains_mono_nerd_font_11_matches_the_old_pango_cell() {
    let Some(face) = regular_face(FAMILY, SIZE) else {
        println!("skipped: {FAMILY} is not installed");
        return;
    };
    let metrics = measure(&face, SIZE).expect("the font metrics compute");
    assert_eq!(metrics.cell_w(), 9, "the cell width matches Pango's");
    assert_eq!(metrics.cell_h(), 20, "the cell height matches Pango's");
    assert_eq!(metrics.ascent(), 15, "the truncated ascent matches Pango's");
    assert_eq!(metrics.baseline(), 5, "the baseline matches Pango's");
    // The Nerd numbers the old path reports for this font.
    assert_eq!(metrics.face_w(), 9.0);
    assert_eq!(metrics.face_h(), 20.0);
    assert_eq!(metrics.face_y(), 0.0);
    assert_eq!(metrics.icon_h(), 20.0);
    assert_eq!(metrics.icon_h_single(), (2.0 * 0.75 * 15.0 + 20.0) / 3.0);
    assert_eq!(metrics.cell_size(), (9.0, 20.0));
}

/// Bad sizes are refused before the face is touched, so a caller without a
/// display never reaches the font.
#[test]
fn bad_sizes_error_instead_of_panic() {
    let Some(face) = regular_face(FAMILY, SIZE) else {
        println!("skipped: {FAMILY} is not installed");
        return;
    };
    for size in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            measure(&face, size),
            Err(MetricsError::BadSize(_))
        ));
    }
}

/// Pure rounding tests over `derive`, built from numbers, not from a font.
#[test]
fn the_derived_metrics_follow_the_old_formulas() {
    // Whole-pixel inputs pass through unchanged: a 9 by 20 cell with the
    // baseline at the descent.
    let metrics = derive(15.0, 5.0, 9.0);
    assert_eq!(metrics.cell_w(), 9);
    assert_eq!(metrics.cell_h(), 20);
    assert_eq!(metrics.ascent(), 15);
    assert_eq!(metrics.baseline(), 5);
    assert_eq!(metrics.face_y(), 0.0);
    // Cap height 0.75 * ascent, icon height the patcher heuristic.
    assert_eq!(metrics.icon_h_single(), (2.0 * 0.75 * 15.0 + 20.0) / 3.0);
}

/// The ascent truncates (the old `ascent / scale as i32`), and the cell
/// pitch never drops below one pixel.
#[test]
fn the_ascent_truncates_and_the_cell_is_at_least_one() {
    // Hinted ascent 15.9 truncates to 15; the cell height keeps the sum.
    let metrics = derive(15.9, 5.0, 9.4);
    assert_eq!(metrics.ascent(), 15, "the ascent truncates, not rounds");
    assert_eq!(metrics.cell_h(), 21, "ascent plus descent, rounded");
    assert_eq!(metrics.cell_w(), 9, "the digit width rounds");
    // A degenerate font still produces a one-pixel cell.
    let tiny = derive(1.0, 1.0, 0.0);
    assert_eq!(tiny.cell_w(), 1);
    assert_eq!(tiny.cell_h(), 2);
}

/// The baseline rounds away from zero, the C formula.
#[test]
fn the_baseline_rounds_away_from_zero() {
    assert_eq!(round_away_from_zero(5.2), 5.7, "the half step points away");
    assert_eq!(round_away_from_zero(-5.2), -5.7);
    assert_eq!(round_away_from_zero(0.0), 0.5, "zero rounds up");
    // Through `derive`: the fractional baseline lands on the rounded value.
    // ascent 10.9 + descent 4.1 = 15.0 exactly, so the cell is 15 high and
    // the face box fills it: the baseline is the descent, rounded.
    let metrics = derive(10.9, 4.1, 9.0);
    assert_eq!(metrics.cell_h(), 15);
    assert_eq!(metrics.baseline(), 4);
    assert_eq!(metrics.face_y(), f64::from(metrics.baseline()) - 4.1);
}

/// The error variants render like the rest of the font errors (the style of
/// `font/error.rs`); the digit-skip path of the measure walk is covered by
/// the real-font gate.
#[test]
fn the_errors_render_like_the_font_errors() {
    // The error renders like the rest of the font errors (same style as
    // `font/error.rs`).
    assert_eq!(
        MetricsError::NoDigits.to_string(),
        "the font has no digit glyphs"
    );
    assert_eq!(
        MetricsError::BadUnitsPerEm.to_string(),
        "the font's units-per-em is zero"
    );
    assert_eq!(
        MetricsError::UnparsableFace.to_string(),
        "the face does not parse"
    );
    assert_eq!(
        MetricsError::BadSize(-3.0).to_string(),
        "the font size -3 is not finite and positive"
    );
}

/// Records every pty write so the `CSI 16 t` reply can be asserted.
struct RecordingSink {
    writes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl PtySink for RecordingSink {
    fn write_pty(&mut self, data: &[u8]) {
        self.writes
            .lock()
            .expect("sink lock")
            .extend_from_slice(data);
    }
}

/// Decodes nothing; the metrics unit does not exercise PNG decoding.
struct NoDecoder;

impl PngDecoder for NoDecoder {
    fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
        None
    }
}

/// A size whose pixels-per-em leaves `FreeType`'s 26.6 range is refused
/// with `BadSize`, not computed into a cell clamped to the canvas
/// maximum: 1e18 points and one point past the 65535 px ceiling (49152
/// points at the 96 dpi convention) both fail, while a size just below
/// the ceiling still measures.
#[test]
fn an_oversized_size_errors_instead_of_saturating() {
    let Some(face) = regular_face(FAMILY, SIZE) else {
        println!("skipped: {FAMILY} is not installed");
        return;
    };
    assert!(
        matches!(measure(&face, 1e18), Err(MetricsError::BadSize(_))),
        "an astronomical size is refused"
    );
    assert!(
        matches!(measure(&face, 49_152.0), Err(MetricsError::BadSize(_))),
        "one point past the 65535 px-per-em ceiling is refused"
    );
    assert!(
        measure(&face, 49_150.0).is_ok(),
        "a size just below the ceiling still measures"
    );
}

/// The 26.6 quantize-and-ceil: `FreeType` scales in 1/64 px units, so dust
/// below 1/64 px cannot cross a pixel boundary there. A raw `f64` ceil
/// turned Lekton's ascent at size 14, `14.000000000000002` px, into 15;
/// the quantized value stays 14. A real fraction still ceils up.
#[test]
fn the_26_6_quantize_ceils_dust_away() {
    assert_eq!(quantize_26_6_units(14.000_000_000_000_002), Some(896));
    assert_eq!(ceil_units_26_6_to_px(896), Some(14.0));
    assert_eq!(
        ceil_units_26_6_to_px(quantize_26_6_units(14.02).expect("finite")),
        Some(15.0),
        "a real fraction ceils up"
    );
    assert_eq!(
        ceil_units_26_6_to_px(quantize_26_6_units(15.0).expect("finite")),
        Some(15.0)
    );
    assert_eq!(
        ceil_units_26_6_to_px(quantize_26_6_units(0.0).expect("finite")),
        Some(0.0)
    );
    // The advance rounds half up on the quantized value: 8.5 px (544
    // units) rounds to 9, 8.49 px (543 units) stays 8.
    assert_eq!(round_units_26_6_to_px(544), Some(9.0));
    assert_eq!(round_units_26_6_to_px(543), Some(8.0));
    // A negative or non-finite distance is not a 26.6 value.
    assert_eq!(quantize_26_6_units(-1.0), None);
    assert_eq!(quantize_26_6_units(f64::NAN), None);
    assert_eq!(quantize_26_6_units(f64::INFINITY), None);
}

/// The Lekton Nerd Font numbers through the pure hinted-metric path (no
/// font needed): upem 1000, hhea ascent 750, descent 250. At the sizes
/// whose raw ascent carries float dust (7, 14, 28) `FreeType`'s 26.6
/// scaling keeps the ascent on its pixel and the descent ceils to the
/// next one — exactly what the old Pango path measures, one row less
/// than the raw `ceil` gave at 14.
#[test]
fn the_lekton_hinted_vertical_metrics_match_pango() {
    for (size, ascent, descent, cell_h) in [
        (7.0, 7.0, 3.0, 10),
        (14.0, 14.0, 5.0, 19),
        (28.0, 28.0, 10.0, 38),
    ] {
        let hinted = hinted_vertical_metrics(750.0, 250.0, 1000.0, size * 96.0 / 72.0)
            .expect("the Lekton numbers are finite");
        assert_eq!(hinted, (ascent, descent), "at font-size {size}");
        // Through `derive`: the cell height is the hinted sum, the
        // truncated ascent the hinted ascent, the baseline the descent.
        let metrics = derive(ascent, descent, 9.0);
        assert_eq!(metrics.cell_h(), cell_h, "at font-size {size}");
        assert_eq!(f64::from(metrics.ascent()), ascent, "at font-size {size}");
    }
}

/// Gate (row 4.6): Lekton Nerd Font 14 — the font whose ascent sits on
/// the pixel boundary and exposed the raw `ceil` — gives the same cell
/// the old Pango path measures: 9 by 19, ascent 14, baseline 5. Skipped
/// only where the font is not installed; a face-lookup error fails.
#[test]
fn lekton_nerd_font_14_matches_the_old_pango_cell() {
    let Some(face) = regular_face(LEKTON, LEKTON_SIZE) else {
        println!("skipped: {LEKTON} is not installed");
        return;
    };
    let metrics = measure(&face, LEKTON_SIZE).expect("the font metrics compute");
    assert_eq!(metrics.cell_w(), 9, "the cell width matches Pango's");
    assert_eq!(metrics.cell_h(), 19, "the cell height matches Pango's");
    assert_eq!(metrics.ascent(), 14, "the truncated ascent matches Pango's");
    assert_eq!(metrics.baseline(), 5, "the baseline matches Pango's");
    // The face box fills the cell: the hinted ascent and descent sum to
    // the whole-pixel line box the old path carries.
    assert_eq!(metrics.face_w(), 9.0);
    assert_eq!(metrics.face_h(), 19.0);
    assert_eq!(metrics.face_y(), 0.0);
    assert_eq!(metrics.icon_h(), 19.0);
    assert_eq!(metrics.icon_h_single(), (2.0 * 0.75 * 14.0 + 19.0) / 3.0);
}

/// The `CSI 16 t` reply matches the drawn cell: the terminal answers with
/// the cell size `push_size` received, and that size comes from the new
/// metrics.
#[test]
fn the_csi_16_t_reply_matches_the_drawn_cell() {
    let Some(face) = regular_face(FAMILY, SIZE) else {
        println!("skipped: {FAMILY} is not installed");
        return;
    };
    let metrics = measure(&face, SIZE).expect("the font metrics compute");
    let (cell_w, cell_h) = (metrics.cell_w(), metrics.cell_h());

    let writes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut terminal = Terminal::new(
        crate::guard::Poisoned::new(),
        RecordingSink {
            writes: std::sync::Arc::clone(&writes),
        },
        NoDecoder,
        || {},
    );
    assert!(terminal.push_size(8, 4, cell_w, cell_h));
    terminal.push_pty_data(b"\x1b[16t");
    let reply = writes.lock().expect("sink lock").clone();
    assert_eq!(
        reply,
        format!("\x1b[6;{cell_h};{cell_w}t").into_bytes(),
        "the reply reports the drawn cell"
    );
}

/// The painter metrics at scale 1.5 carry the logical cell and the
/// truncated ascent, and the cell rectangles tile at that scale.
#[test]
fn painter_metrics_at_1_5_keep_the_logical_cell_and_truncated_ascent() {
    let Some(face) = regular_face(FAMILY, SIZE) else {
        println!("skipped: {FAMILY} is not installed");
        return;
    };
    let metrics = measure(&face, SIZE).expect("the font metrics compute");
    let painter = metrics
        .painter_metrics(1.5)
        .expect("a whole-pixel cell is always valid");
    assert_eq!(painter.cell_w(), 9.0, "the logical cell width");
    assert_eq!(painter.cell_h(), 20.0, "the logical cell height");
    assert_eq!(painter.ascent(), 15.0, "the truncated ascent");
    assert_eq!(painter.scale(), 1.5);
    // Two adjacent cells share their snapped edge at 1.5, so the drawn grid
    // tiles with no seam.
    for col in 0..4 {
        let cell = painter.cell_rect(col, 0);
        let next = painter.cell_rect(col + 1, 0);
        assert_eq!(cell.x() + cell.w(), next.x());
    }
    // A degenerate scale falls back to 1, as `PainterMetrics::new` documents.
    assert_eq!(metrics.painter_metrics(0.0).expect("valid").scale(), 1.0);
}

/// A `CellMetrics` copies and compares like a value: the metrics travel
/// through the painter's frame state.
#[test]
fn the_metrics_are_a_copyable_value() {
    let metrics = derive(15.0, 5.0, 9.0);
    let copy = metrics;
    assert_eq!(copy, metrics);
    assert_eq!(copy.cell_size(), metrics.cell_size());
}
