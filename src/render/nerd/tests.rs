//! The constraint arithmetic's tests: the old text pass's `constrain` unit
//! tests, ported with their expectations unchanged. The numbers are pure
//! f64 arithmetic — no font, no display (replace-gtk-with-wayland D10).

use super::{NerdGlyph, NerdMetrics, constrain};
use crate::nerd_font::{Align, Constraint, Height, Size};

/// 10x20 face, cell pitch 10x10, the baseline 2px below the face top.
fn metrics() -> NerdMetrics {
    NerdMetrics::new(10.0, 20.0, 2.0, 18.0, 14.0, 10.0, 10.0).expect("the test metrics are valid")
}

fn glyph(x: f64, y: f64, width: f64, height: f64) -> NerdGlyph {
    NerdGlyph::new(x, y, width, height)
}

#[test]
fn no_size_and_no_align_leaves_the_glyph_alone() {
    let c = Constraint::NONE;
    let out = constrain(&c, &metrics(), glyph(1.0, 2.0, 8.0, 12.0), 1);
    assert_eq!(out, glyph(1.0, 2.0, 8.0, 12.0));
}

#[test]
fn cover_fills_the_constrained_box() {
    let mut c = Constraint::NONE;
    c.size = Size::Cover;
    // Two cells wide, no padding: the target box is 2*face_w x face_h.
    let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 5.0, 5.0), 2);
    assert!((out.width() - 20.0).abs() < 1e-9, "width {out:?}");
    assert!((out.height() - 20.0).abs() < 1e-9, "height {out:?}");
}

#[test]
fn fit_never_grows_the_glyph() {
    let mut c = Constraint::NONE;
    c.size = Size::Fit;
    // A wide, short glyph fits the box without being scaled.
    let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 8.0, 4.0), 1);
    assert!((out.width() - 8.0).abs() < 1e-9, "width {out:?}");
    assert!((out.height() - 4.0).abs() < 1e-9, "height {out:?}");

    // A tall, narrow glyph must shrink to fit the face height.
    let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 2.0, 40.0), 1);
    assert!((out.height() - 20.0).abs() < 1e-9, "height {out:?}");
    assert!((out.width() - 1.0).abs() < 1e-9, "width {out:?}");
}

#[test]
fn fit_cover1_grows_to_one_cell_but_no_more() {
    let mut c = Constraint::NONE;
    c.size = Size::FitCover1;
    c.height = Height::Icon;
    // A one-cell-wide constraint would fit the glyph to 20x18 (icon
    // height), but the FitCover1 rule regrows from the single-cell scale:
    // min(1*face_w, icon_h_single) = min(10, 14) = 10 -- the glyph covers
    // exactly one cell, not two.
    let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 1.0, 1.0), 2);
    assert!((out.width() - 10.0).abs() < 1e-9, "width {out:?}");
    assert!((out.height() - 10.0).abs() < 1e-9, "height {out:?}");
}

#[test]
fn stretch_uses_the_cell_grid_and_clamps_negative_padding() {
    let mut c = Constraint::NONE;
    c.size = Size::Stretch;
    c.pad_left = -1.0;
    c.pad_right = -1.0;
    c.pad_top = -1.0;
    c.pad_bottom = -1.0;
    // Stretched glyphs measure the cell pitch: 10x10 here.
    let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 5.0, 5.0), 1);
    assert!((out.width() - 10.0).abs() < 1e-9, "width {out:?}");
    assert!((out.height() - 10.0).abs() < 1e-9, "height {out:?}");
}

#[test]
fn align_end_bottoms_the_glyph_with_its_padding() {
    let mut c = Constraint::NONE;
    c.size = Size::Cover;
    c.align_vertical = Align::End;
    c.pad_top = 0.25;
    let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 10.0, 10.0), 1);
    // The 10x10 glyph covers one cell; end_y = face_y + (face_h - height
    // - pad_top * face_h) = 2 + (20 - 10 - 5) = 7.
    assert!((out.y() - 7.0).abs() < 1e-9, "y {out:?}");
}

#[test]
fn max_xy_ratio_limits_the_width() {
    let mut c = Constraint::NONE;
    c.size = Size::Cover;
    c.max_xy_ratio = Some(0.5);
    // Covering one cell would make the glyph 20 high; the ratio caps the
    // width at half its height.
    let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 10.0, 10.0), 1);
    assert!(
        (out.width() - out.height() * 0.5).abs() < 1e-9,
        "box {out:?}"
    );
}

#[test]
fn metrics_reject_non_finite_and_negative_values() {
    assert!(NerdMetrics::new(10.0, 20.0, 0.0, 18.0, 14.0, 10.0, 10.0).is_some());
    assert!(NerdMetrics::new(f64::NAN, 20.0, 0.0, 18.0, 14.0, 10.0, 10.0).is_none());
    assert!(NerdMetrics::new(10.0, f64::INFINITY, 0.0, 18.0, 14.0, 10.0, 10.0).is_none());
    assert!(NerdMetrics::new(10.0, 20.0, f64::NAN, 18.0, 14.0, 10.0, 10.0).is_none());
    assert!(NerdMetrics::new(-1.0, 20.0, 0.0, 18.0, 14.0, 10.0, 10.0).is_none());
    assert!(NerdMetrics::new(10.0, 20.0, 0.0, 18.0, 14.0, -1.0, 10.0).is_none());
}
