//! The cell metrics from the font, computed with swash: the cell
//! pitch, the truncated ascent, the baseline and the Nerd Font numbers the
//! constraints are expressed against.
//!
//! Size conversion matches Pango: the font size in points places the font at
//! `points * 96 / 72` pixels (the 96 dpi convention), and everything else is
//! font units times `pixels / units_per_em`. Pixel rounding follows
//! FreeType's hinted metrics, which is what the old Pango numbers carry:
//! the scaling is quantized to FreeType's 26.6 fixed point (whole 1/64 px
//! units — float dust below 1/64 px can never cross a pixel boundary
//! there, while a raw `f64` `ceil` turns Lekton Nerd Font's ascent at
//! size 14, `14.000000000000002` px, into 15 and grows every cell row by
//! a pixel), then the ascent is ceiled up to the pixel, the descent
//! ceiled up in magnitude (FreeType floors its negative `descender`), and
//! each hinted advance is rounded to a whole pixel. The derived formulas — widest digit advance for
//! the width, ascent plus descent for the height, a zero line gap, the
//! baseline rounded away from zero, `0.75 * ascent` for the cap height and
//! `(2*cap + face_h) / 3` for the one-cell icon height — carry over
//! unchanged from the Pango `measure`.
//!
//! GTK-free like the font module (replace-gtk-with-wayland D10): the tests
//! run without a display. The panel thread's font setup and the painter
//! modules consume the metrics.

use super::font::Face;
use super::geom::{PainterMetrics, device_px};

#[cfg(test)]
mod tests;

/// A cell-metrics computation failed. Returned as an error — never a panic.
#[derive(Debug, PartialEq)]
pub enum MetricsError {
    /// The face bytes do not parse (the `Face` came from a broken file).
    UnparsableFace,
    /// The font size is not finite and positive, so no pixel size exists.
    BadSize(f64),
    /// The font's units-per-em is zero, so no scale factor exists.
    BadUnitsPerEm,
    /// No digit maps to a glyph with a positive advance, so no cell width
    /// exists.
    NoDigits,
}

impl std::fmt::Display for MetricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnparsableFace => f.write_str("the face does not parse"),
            Self::BadSize(size) => write!(f, "the font size {size} is not finite and positive"),
            Self::BadUnitsPerEm => f.write_str("the font's units-per-em is zero"),
            Self::NoDigits => f.write_str("the font has no digit glyphs"),
        }
    }
}

impl std::error::Error for MetricsError {}

/// The cell metrics computed from the font: the cell pitch and
/// ascent in whole logical pixels, the baseline, and the Nerd Font numbers
/// the constraints are expressed against. The fields are private
/// (port-to-rust D6); the accessors carry the meaning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    /// The horizontal cell pitch, in logical pixels, at least 1.
    cell_w: i32,
    /// The vertical cell pitch, in logical pixels, at least 1.
    cell_h: i32,
    /// The font ascent, truncated to logical pixels (`g_ascent`): the value
    /// `PainterMetrics::new` takes, and what the underline band hangs from.
    ascent: i32,
    /// Pixels from the cell's bottom to the baseline (`g_cell_baseline`).
    baseline: i32,
    /// The face box width (the hinted digit advance).
    face_w: f64,
    /// The face box height (the hinted line box).
    face_h: f64,
    /// The face box top relative to the cell's bottom (`g_nerd_face_y`).
    face_y: f64,
    /// The full-height icon box (`g_nerd_icon_h`).
    icon_h: f64,
    /// The one-cell icon height, the `font_patcher` heuristic
    /// `(2*cap + face_h) / 3` (`g_nerd_icon_h_single`).
    icon_h_single: f64,
}

impl CellMetrics {
    /// The horizontal cell pitch, in logical pixels.
    #[must_use]
    pub fn cell_w(&self) -> i32 {
        self.cell_w
    }

    /// The vertical cell pitch, in logical pixels.
    #[must_use]
    pub fn cell_h(&self) -> i32 {
        self.cell_h
    }

    /// The font ascent, truncated to logical pixels: the value
    /// [`PainterMetrics::new`] takes, and what the underline depends on.
    #[must_use]
    pub fn ascent(&self) -> i32 {
        self.ascent
    }

    /// Pixels from the cell's bottom to the baseline.
    #[must_use]
    pub fn baseline(&self) -> i32 {
        self.baseline
    }

    /// The face box width, in logical pixels.
    #[must_use]
    pub fn face_w(&self) -> f64 {
        self.face_w
    }

    /// The face box height, in logical pixels.
    #[must_use]
    pub fn face_h(&self) -> f64 {
        self.face_h
    }

    /// The face box top relative to the cell's bottom, in logical pixels.
    #[must_use]
    pub fn face_y(&self) -> f64 {
        self.face_y
    }

    /// The full-height icon box, in logical pixels.
    #[must_use]
    pub fn icon_h(&self) -> f64 {
        self.icon_h
    }

    /// The one-cell icon height, in logical pixels.
    #[must_use]
    pub fn icon_h_single(&self) -> f64 {
        self.icon_h_single
    }

    /// The cell pitch as `f64`, the form the Nerd Font constraints and
    /// [`PainterMetrics`] work in.
    #[must_use]
    pub fn cell_size(&self) -> (f64, f64) {
        (f64::from(self.cell_w), f64::from(self.cell_h))
    }

    /// The [`PainterMetrics`] for one frame at `scale`: the logical cell
    /// pitch and the truncated logical ascent. A `scale` that is not
    /// positive and finite falls back to 1 inside [`PainterMetrics::new`]
    /// (the same degenerate handling `OutputScale::new` has), so `None`
    /// only appears when the cell pitch is not finite and positive —
    /// which the whole-pixel pitch a [`CellMetrics`] carries never is.
    #[must_use]
    pub fn painter_metrics(&self, scale: f64) -> Option<PainterMetrics> {
        PainterMetrics::new(
            f64::from(self.cell_w),
            f64::from(self.cell_h),
            f64::from(self.ascent),
            scale,
        )
    }
}

/// Measure `face` at `size` points (the Ghostty `font-size`) and derive the
/// cell metrics. See the module comment for the size conversion and the
/// pixel rounding.
pub fn measure(face: &Face, size: f64) -> Result<CellMetrics, MetricsError> {
    if !size.is_finite() || size <= 0.0 {
        return Err(MetricsError::BadSize(size));
    }
    // Pango's 96 dpi convention: the font is placed at points * 96/72 px.
    // FreeType computes the hinted metrics in 26.6 fixed point, whose
    // largest pixel size is 65535; beyond it the scaling is not
    // representable, so refuse the size instead of clamping the cell to
    // the canvas maximum.
    let pixels_per_em = size * 96.0 / 72.0;
    if pixels_per_em > MAX_PIXELS_PER_EM {
        return Err(MetricsError::BadSize(size));
    }
    let font = face.parse().ok_or(MetricsError::UnparsableFace)?;
    let metrics = font.metrics(&[]);
    let units_per_em = metrics.units_per_em;
    if units_per_em == 0 {
        return Err(MetricsError::BadUnitsPerEm);
    }
    let scale = pixels_per_em / f64::from(units_per_em);

    // FreeType's hinted metrics, which the old Pango numbers carry: the
    // ascent ceils up to the pixel, the descent ceils up in magnitude
    // (FreeType floors its negative `descender`), and each hinted advance
    // rounds to a whole pixel — each after the 26.6 quantization the
    // module comment describes. swash's descent is a positive distance, so
    // the magnitude guard only covers a font that reports it negatively.
    let (ascent_px, descent_px) = hinted_vertical_metrics(
        f64::from(metrics.ascent),
        f64::from(metrics.descent),
        f64::from(units_per_em),
        pixels_per_em,
    )
    .ok_or(MetricsError::BadSize(size))?;
    let raw_digit = digit_advance_px(&font, scale).ok_or(MetricsError::NoDigits)?;
    let digit_px = quantize_26_6_units(raw_digit)
        .and_then(round_units_26_6_to_px)
        .ok_or(MetricsError::BadSize(size))?;

    Ok(derive(ascent_px, descent_px, digit_px))
}

/// `FreeType`'s largest pixel size: the hinted metrics are computed in 26.6
/// fixed point, whose whole range is 0 to 65535 1/64-px units.
const MAX_PIXELS_PER_EM: f64 = 65535.0;

/// Quantize a non-negative pixel distance to whole 1/64-pixel units — the
/// 26.6 fixed point `FreeType` scales the hinted metrics in, with
/// `FT_MulFix` rounding to the nearest unit. Dust below 1/64 px cannot
/// cross a pixel boundary there, so the rounding below runs on these
/// units, not on the raw `f64`. `None` when the distance is negative,
/// not finite, or its unit count leaves the `i64` range; `measure` turns
/// `None` into [`MetricsError::BadSize`].
fn quantize_26_6_units(px: f64) -> Option<i64> {
    if !px.is_finite() || px < 0.0 {
        return None;
    }
    let units = px * 64.0;
    if !units.is_finite() {
        return None;
    }
    num_traits::cast(units.round())
}

/// Ceil whole 26.6 units up to a whole pixel, in integer arithmetic:
/// `(units + 63) / 64` for the non-negative units the ascent and the
/// descent magnitude are. `None` when the units sit at the `i64` ceiling
/// or the pixel value leaves the `f64` range.
fn ceil_units_26_6_to_px(units: i64) -> Option<f64> {
    num_traits::cast(units.checked_add(63)? / 64)
}

/// Round whole 26.6 units to a whole pixel, half up — the rounding a
/// hinted advance reduces to. `None` like [`ceil_units_26_6_to_px`].
fn round_units_26_6_to_px(units: i64) -> Option<f64> {
    num_traits::cast(units.checked_add(32)? / 64)
}

/// The hinted ascent and descent in pixels, the way `FreeType` computes the
/// vertical metrics from the font's horizontal ones: the font-unit values
/// scaled by `pixels_per_em / units_per_em`, quantized to 26.6 fixed
/// point ([`quantize_26_6_units`]), then the ascent ceiled up to the
/// pixel and the descent magnitude ceiled up (`FreeType` floors its
/// negative `descender`). The descent comes back as a positive magnitude.
/// `None` when a value is not finite or leaves the 26.6 range; `measure`
/// turns that into [`MetricsError::BadSize`].
fn hinted_vertical_metrics(
    ascent_units: f64,
    descent_units: f64,
    units_per_em: f64,
    pixels_per_em: f64,
) -> Option<(f64, f64)> {
    let scale = pixels_per_em / units_per_em;
    let ascent = ceil_units_26_6_to_px(quantize_26_6_units(ascent_units * scale)?)?;
    let descent = ceil_units_26_6_to_px(quantize_26_6_units(descent_units.abs() * scale)?)?;
    Some((ascent, descent))
}

/// The widest advance of the digits 0 to 9, scaled to pixels — Pango's
/// `approximate_digit_width`. A digit that maps to no glyph, or to one with
/// a zero advance, is skipped; when no digit is usable the font has no cell
/// width and the caller fails with [`MetricsError::NoDigits`].
fn digit_advance_px(font: &swash::FontRef<'_>, scale: f64) -> Option<f64> {
    let charmap = font.charmap();
    let glyph_metrics = font.glyph_metrics(&[]);
    let mut widest: Option<f64> = None;
    for digit in '0'..='9' {
        let glyph = charmap.map(u32::from(digit));
        if glyph == 0 {
            continue; // notdef: the font has no glyph for the digit
        }
        let advance = f64::from(glyph_metrics.advance_width(glyph)) * scale;
        if advance > 0.0 {
            widest = Some(widest.map_or(advance, |current: f64| current.max(advance)));
        }
    }
    widest
}

/// The old `measure` formulas over the hinted pixel numbers: cell width from
/// the digit advance, cell height from ascent plus descent, the truncated
/// ascent, the baseline rounded away from zero, and the Nerd Font numbers.
/// A separate step so the rounding rules can be tested from numbers, not
/// from a font. All inputs are already whole pixels.
fn derive(ascent_px: f64, descent_px: f64, digit_px: f64) -> CellMetrics {
    let cell_w = device_px(digit_px).max(1);
    let face_h = ascent_px + descent_px;
    let cell_h = device_px(face_h).max(1);
    // The truncated ascent, exactly the old `(ascent / scale) as i32`: the
    // underline hangs from this value.
    let ascent = device_px(ascent_px.trunc());

    let face_w = digit_px;
    // The half line gap is 0, so the face baseline sits at the descent.
    let face_baseline = descent_px;
    let cell_baseline = face_baseline - (f64::from(cell_h) - face_h) / 2.0;
    // Round the baseline away from zero to the pixel grid, as the C does
    // with `(int32_t)(cell_baseline + 0.5 * (cell_baseline < 0 ? -1 : 1))`.
    let baseline = device_px(round_away_from_zero(cell_baseline).trunc());

    // Cap height heuristic, `0.75 * ascent`, as the old Pango path computes
    // it (not the font's own cap height).
    let cap_h = 0.75 * ascent_px;

    CellMetrics {
        cell_w,
        cell_h,
        ascent,
        baseline,
        face_w,
        face_h,
        face_y: f64::from(baseline) - face_baseline,
        icon_h: face_h,
        icon_h_single: (2.0 * cap_h + face_h) / 3.0,
    }
}

/// The C baseline rounding: add a half step pointing away from zero, then
/// truncate toward zero.
fn round_away_from_zero(value: f64) -> f64 {
    value + 0.5 * if value < 0.0 { -1.0 } else { 1.0 }
}
