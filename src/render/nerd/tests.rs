//! The constraint arithmetic's tests: the old text pass's `constrain` unit
//! tests, ported with their expectations unchanged. The numbers are pure
//! f64 arithmetic — no font, no display (replace-gtk-with-wayland D10).

use super::{
    InkBox, NerdGlyph, NerdMetrics, constrain, constrain as _, ink_to_cell_frame, placement,
};
use crate::fontconfig::FontConfig;
use crate::nerd_font::{Align, Constraint, Height, Size, constraint};
use crate::render::cell_metrics::measure;
use crate::render::font::{Face, FontBook};
use crate::render::glyph::{Glyph, GlyphCache, GlyphImage, GlyphRequest, Ppem, Synthesis};

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

/// The bridge from the constrained box to the swash placement transform,
/// against the real fonts (display-free, replace-gtk-with-wayland D10).
mod bridge {
    use super::*;

    /// The family the bridge tests gate on; CI installs it (design D10).
    const FAMILY: &str = "JetBrainsMono Nerd Font";
    /// The size in points the tests run at: the Ghostty default 11.
    const SIZE: f64 = 11.0;
    /// nf-cod-lightbulb, a Nerd Font icon the text pass owns: the table
    /// constrains it (FitCover1, icon height, centred) and the terminal
    /// face carries the glyph.
    const ICON_CP: u32 = 0xEA61;

    /// The regular face of the test family, or `None` (printed) when the
    /// machine lacks it. A broken `FontBook` or a failed lookup is not a
    /// skip: it fails the test.
    fn face() -> Option<Face> {
        let book = FontBook::new().expect("the font book opens");
        if !book.has_family(FAMILY) {
            println!("skipped: {FAMILY} is not installed");
            return None;
        }
        let config = FontConfig {
            family: Some(FAMILY.to_owned()),
            size: SIZE,
        };
        let faces = book.family_faces(&config).expect("the family's faces load");
        Some(faces.regular().clone())
    }

    /// A glyph's rasterized image size, in pixels.
    fn image_dims(glyph: &Glyph) -> (f64, f64) {
        match glyph.image() {
            GlyphImage::Mask(mask) => (
                num_traits::cast(mask.width()).expect("the mask width fits f64"),
                num_traits::cast(mask.height()).expect("the mask height fits f64"),
            ),
            GlyphImage::Color(pixmap) => (f64::from(pixmap.width()), f64::from(pixmap.height())),
            other => panic!("expected a rasterized image, got {other:?}"),
        }
    }

    #[test]
    fn the_ink_box_converts_into_the_cell_frame() {
        // The ink bottom sits 3px below the baseline; the baseline sits 5px
        // above the cell's bottom, so the box's bottom edge is 2px above
        // the cell's bottom.
        let glyph = ink_to_cell_frame(&InkBox::new(2.0, -3.0, 8.0, 12.0), 5.0);
        assert_eq!(glyph, NerdGlyph::new(2.0, 2.0, 8.0, 12.0));
        // An ink box above the baseline adds on top of the baseline.
        let glyph = ink_to_cell_frame(&InkBox::new(0.0, 1.5, 4.0, 6.0), 5.0);
        assert_eq!(glyph, NerdGlyph::new(0.0, 6.5, 4.0, 6.0));
    }

    #[test]
    fn an_unconstrained_or_empty_glyph_places_nowhere() {
        let metrics = NerdMetrics::new(10.0, 20.0, 2.0, 18.0, 14.0, 10.0, 10.0)
            .expect("the test metrics are valid");
        // The unconstrained constraint never places.
        let ink = InkBox::new(1.0, 2.0, 8.0, 12.0);
        assert!(placement(&Constraint::NONE, &metrics, 5.0, 1, &ink).is_none());
        // A real constraint over an empty ink box (a space, a render miss)
        // has nothing to place.
        let c = constraint(ICON_CP).expect("the lightbulb is constrained");
        assert!(placement(&c, &metrics, 5.0, 1, &InkBox::new(0.0, 0.0, 0.0, 0.0)).is_none());
    }

    /// The bridge end to end: the unconstrained icon's ink box turns into
    /// a transform whose rasterization lands inside the constrained box,
    /// at output scale 1 and at 1.5 (the metrics scaled, unsnapped).
    #[test]
    fn a_constrained_icon_lands_in_its_constrained_box() {
        let Some(face) = face() else { return };
        let c = constraint(ICON_CP).expect("the lightbulb is constrained");
        assert!(
            c.does_anything(),
            "the lightbulb's constraint does something"
        );
        let glyph_id = face
            .parse()
            .expect("the face parses")
            .charmap()
            .map(ICON_CP);
        assert_ne!(glyph_id, 0, "the terminal face covers the lightbulb");
        let cell_metrics = measure(&face, SIZE).expect("the cell metrics compute");

        let mut cache = GlyphCache::new();
        let mut transforms = Vec::new();
        for scale in [1.0, 1.5] {
            let ppem = Ppem::from_px(SIZE * 96.0 / 72.0 * scale).expect("the ppem is usable");
            let metrics = NerdMetrics::from_cell_metrics(&cell_metrics, scale)
                .expect("the metrics are valid");
            let baseline = f64::from(cell_metrics.baseline()) * scale;

            let unconstrained = cache
                .rasterize(&GlyphRequest::new(
                    &face,
                    glyph_id,
                    ppem,
                    Synthesis::new(false, false),
                    None,
                ))
                .expect("the glyph rasterizes");
            let ink = InkBox::from_glyph(&unconstrained);
            let transform =
                placement(&c, &metrics, baseline, 1, &ink).expect("the constrained glyph places");
            transforms.push(transform);

            let constrained = cache
                .rasterize(&GlyphRequest::new(
                    &face,
                    glyph_id,
                    ppem,
                    Synthesis::new(false, false),
                    Some(transform),
                ))
                .expect("the constrained glyph rasterizes");
            let (width, height) = image_dims(&constrained);
            let where_it_lands = constrained.placement();
            let left = f64::from(where_it_lands.left());
            let top = f64::from(where_it_lands.top());

            // The constrained box, back in the baseline-relative frame the
            // rasterization reports.
            let target = constrain(&c, &metrics, ink_to_cell_frame(&ink, baseline), 1);
            let want_left = target.x();
            let want_bottom = target.y() - baseline;
            let want_right = want_left + target.width();
            let want_top = want_bottom + target.height();
            assert!(
                (left - want_left).abs() <= 1.0,
                "scale {scale}: left {left} vs {want_left}"
            );
            assert!(
                (top - want_top).abs() <= 1.0,
                "scale {scale}: top {top} vs {want_top}"
            );
            assert!(
                (left + width - want_right).abs() <= 1.0,
                "scale {scale}: right {} vs {want_right}",
                left + width
            );
            assert!(
                (top - height - want_bottom).abs() <= 1.0,
                "scale {scale}: bottom {} vs {want_bottom}",
                top - height
            );
        }
        assert_ne!(
            transforms[0], transforms[1],
            "the scale 1.5 transform differs from scale 1.0"
        );
    }

    /// The transform never depends on the column or the row: the inputs
    /// carry no position, so the same glyph, constraint, metrics and width
    /// produce the same transform in every cell of a row — the spec's
    /// "same device pixel offset inside each cell".
    #[test]
    fn the_transform_depends_on_the_glyph_not_the_cell() {
        let Some(face) = face() else { return };
        let c = constraint(ICON_CP).expect("the lightbulb is constrained");
        let glyph_id = face
            .parse()
            .expect("the face parses")
            .charmap()
            .map(ICON_CP);
        let cell_metrics = measure(&face, SIZE).expect("the cell metrics compute");
        let ppem = Ppem::from_px(SIZE * 96.0 / 72.0).expect("the ppem is usable");
        let metrics =
            NerdMetrics::from_cell_metrics(&cell_metrics, 1.0).expect("the metrics are valid");
        let baseline = f64::from(cell_metrics.baseline());

        let mut cache = GlyphCache::new();
        let mut transform = None;
        for _column in 0..2 {
            let unconstrained = cache
                .rasterize(&GlyphRequest::new(
                    &face,
                    glyph_id,
                    ppem,
                    Synthesis::new(false, false),
                    None,
                ))
                .expect("the glyph rasterizes");
            let ink = InkBox::from_glyph(&unconstrained);
            let next =
                placement(&c, &metrics, baseline, 1, &ink).expect("the constrained glyph places");
            if let Some(previous) = transform {
                assert_eq!(next, previous, "the transform repeats across cells");
            }
            transform = Some(next);
        }
    }
}
