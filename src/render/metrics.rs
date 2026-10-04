//! Cell and font metrics for the render row (port-to-rust D3), ported from
//! `cell_metrics_update` in `src/render.c`. The metrics are derived from the
//! Pango font once and drive every draw decision: cell pitch, the baseline the
//! text pass pins to, and the face metrics the Nerd Font constraints are
//! expressed against.

use pango::FontDescription;

use crate::fontconfig;

/// The four font variants a cell's style bits can select (`g_font`,
/// `g_font_bold`, `g_font_italic`, `g_font_bold_italic`). Built once from the
/// Ghostty config; Pango font descriptions are not `Clone`, so each variant is
/// constructed fresh.
#[derive(Debug)]
pub(crate) struct Fonts {
    pub regular: FontDescription,
    pub bold: FontDescription,
    pub italic: FontDescription,
    pub bold_italic: FontDescription,
}

impl Fonts {
    /// Load the font from the Ghostty config (`font_config_load` plus the
    /// description setup at the top of `cell_metrics_update`).
    pub fn load() -> Fonts {
        let config = fontconfig::load_font_config();
        let mut regular = FontDescription::new();
        regular.set_family(config.effective_family());
        regular.set_size((config.size * f64::from(pango::SCALE)) as i32);

        let mut bold = FontDescription::new();
        bold.set_family(config.effective_family());
        bold.set_size((config.size * f64::from(pango::SCALE)) as i32);
        bold.set_weight(pango::Weight::Bold);

        let mut italic = FontDescription::new();
        italic.set_family(config.effective_family());
        italic.set_size((config.size * f64::from(pango::SCALE)) as i32);
        italic.set_style(pango::Style::Italic);

        let mut bold_italic = FontDescription::new();
        bold_italic.set_family(config.effective_family());
        bold_italic.set_size((config.size * f64::from(pango::SCALE)) as i32);
        bold_italic.set_weight(pango::Weight::Bold);
        bold_italic.set_style(pango::Style::Italic);

        Fonts {
            regular,
            bold,
            italic,
            bold_italic,
        }
    }
}

/// The Nerd Font constraints are expressed against Ghostty's grid metrics
/// (`g_nerd_*` in `glue_internal.h`): the face box is the unrounded line box,
/// the baseline is rounded to the pixel grid, and the "icon height" is the
/// font_patcher heuristic (2*cap + line height) / 3 for one-cell constraints.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NerdMetrics {
    pub face_w: f64,
    pub face_h: f64,
    pub face_y: f64,
    pub icon_h: f64,
    pub icon_h_single: f64,
    /// The cell pitch, for the stretch path and the horizontal span.
    pub cell_w: f64,
    pub cell_h: f64,
}

/// The Pango-derived drawing metrics (`g_cell_*`, `g_ascent`,
/// `g_cell_baseline`, `g_nerd_*`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CellMetrics {
    /// The horizontal cell pitch in pixels.
    pub cell_w: i32,
    /// The vertical cell pitch in pixels.
    pub cell_h: i32,
    /// The font ascent in pixels (`g_ascent`).
    pub ascent: i32,
    /// Pixels from the cell's bottom to the baseline (`g_cell_baseline`).
    pub baseline: i32,
    pub nerd: NerdMetrics,
}

/// Measure the font on `context` and derive the cell metrics. GTK-free: any
/// `pango::Context` works, so tests can build one from a pangocairo font map
/// (`cell_metrics_update` in `render.c`).
pub(crate) fn measure(context: &pango::Context, font: &FontDescription) -> CellMetrics {
    // SAFETY: none; `metrics` is a safe pango call.
    let metrics = context.metrics(Some(font), None);
    let ascent = metrics.ascent();
    let descent = metrics.descent();
    let digit = metrics.approximate_digit_width();
    let scale = f64::from(pango::SCALE);

    let cell_w = ((f64::from(digit) + scale / 2.0) / scale) as i32;
    let cell_h = ((f64::from(ascent + descent) + scale / 2.0) / scale) as i32;
    let cell_w = cell_w.max(1);
    let cell_h = cell_h.max(1);
    let ascent_px = (f64::from(ascent) / scale) as i32;

    let face_w = f64::from(digit) / scale;
    let face_h = f64::from(ascent + descent) / scale;
    let descent_px = f64::from(descent) / scale;
    // Half line gap is 0 in Pango.
    let face_baseline = descent_px;
    let mut cell_baseline = face_baseline - (f64::from(cell_h) - face_h) / 2.0;
    let cap_h = 0.75 * f64::from(ascent) / scale;

    // Round the baseline away from zero to the pixel grid, as the C does with
    // `(int32_t)(cell_baseline + 0.5 * (cell_baseline < 0 ? -1 : 1))`.
    cell_baseline =
        (cell_baseline + 0.5 * if cell_baseline < 0.0 { -1.0 } else { 1.0 }) as i32 as f64;

    CellMetrics {
        cell_w,
        cell_h,
        ascent: ascent_px,
        baseline: cell_baseline as i32,
        nerd: NerdMetrics {
            face_w,
            face_h,
            face_y: cell_baseline - face_baseline,
            icon_h: face_h,
            icon_h_single: (2.0 * cap_h + face_h) / 3.0,
            cell_w: f64::from(cell_w),
            cell_h: f64::from(cell_h),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::font_lock;

    fn context() -> pango::Context {
        use pango::prelude::*;
        // The default font map, like the GTK widget's context in production.
        pangocairo::FontMap::default().create_context()
    }

    #[test]
    fn metrics_are_at_least_one_cell() {
        let _guard = font_lock::guard();
        let fonts = Fonts::load();
        let metrics = measure(&context(), &fonts.regular);
        assert!(metrics.cell_w >= 1);
        assert!(metrics.cell_h >= 1);
        assert!(metrics.ascent >= 0);
        // The baseline sits between the cell's top and bottom.
        assert!((-metrics.cell_h..=metrics.cell_h).contains(&metrics.baseline));
        assert!(metrics.nerd.face_w > 0.0);
        assert!(metrics.nerd.face_h > 0.0);
        assert!(metrics.nerd.icon_h_single > 0.0);
    }
}
