//! The text pass (port-to-rust D3): drawing one cell's grapheme through
//! Pango, with Ghostty's Nerd Font glyph constraints applied, plus the UTF-8
//! first-codepoint helper the sprite ranges share. Ported from `draw_text`,
//! `nerd_constrain` in `src/render.c` (design D5 there).

use pango::FontDescription;

use super::metrics::{CellMetrics, NerdMetrics};
use crate::nerd_font::{Align, Constraint, Height, Size, constraint};
use crate::term::cells::{Cell, StyleFlags, first_codepoint};

/// A glyph's box relative to the cell's bottom-left corner (`NerdGlyph`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NerdGlyph {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

fn max(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

fn min(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

/// The scale factors for one constraint against `group`, given `min_w` cells
/// of constraint width (`nerd_scale_factors`).
fn scale_factors(c: &Constraint, m: &NerdMetrics, group: &NerdGlyph, min_w: u32) -> (f64, f64) {
    let multi = min_w > 1;

    if c.size == Size::None {
        return (1.0, 1.0);
    }

    let pad_w = f64::from(min_w) - (c.pad_left + c.pad_right);
    let pad_h = 1.0 - (c.pad_bottom + c.pad_top);
    let target_w = pad_w * m.face_w;
    let target_h = pad_h
        * if c.height == Height::Icon {
            if multi { m.icon_h } else { m.icon_h_single }
        } else {
            m.face_h
        };
    let mut w = target_w / group.width;
    let mut h = target_h / group.height;

    match c.size {
        Size::Fit => {
            h = min(1.0, min(w, h));
            w = h;
        }
        Size::Cover => {
            h = min(w, h);
            w = h;
        }
        Size::FitCover1 => {
            h = min(w, h);
            if multi && h > 1.0 {
                let (_, single_h) = scale_factors(c, m, group, 1);
                h = max(1.0, single_h);
            }
            w = h;
        }
        // Stretch.
        _ => {}
    }

    if let Some(max_xy_ratio) = c.max_xy_ratio
        && max_xy_ratio >= 0.0
        && group.width * w > group.height * h * max_xy_ratio
    {
        w = group.height * h * max_xy_ratio / group.width;
    }

    (w, h)
}

/// Ghostty normalises every Nerd Font icon to a canonical box before drawing:
/// `constraint` maps the codepoint to a constraint (scale rule, padding,
/// alignment, relative geometry) and the glyph is scaled and moved to fit.
/// A port of `Glyph.RenderOptions.Constraint` (`nerd_constrain`).
pub(crate) fn constrain(
    c: &Constraint,
    metrics: &NerdMetrics,
    glyph: NerdGlyph,
    constraint_width: u32,
) -> NerdGlyph {
    let mut m = *metrics;
    let mut cc = *c;

    if c.size == Size::Stretch {
        // Stretched glyphs are scaled and aligned to the grid, not the face;
        // negative padding would only cause overlap there.
        m.face_w = m.cell_w;
        m.face_h = m.cell_h;
        m.face_y = 0.0;
        if cc.pad_bottom < 0.0 {
            cc.pad_bottom = 0.0;
        }
        if cc.pad_top < 0.0 {
            cc.pad_top = 0.0;
        }
        if cc.pad_left < 0.0 {
            cc.pad_left = 0.0;
        }
        if cc.pad_right < 0.0 {
            cc.pad_right = 0.0;
        }
    }

    let min_w: u32 = if cc.size == Size::Stretch && m.face_w > 0.9 * m.face_h {
        1
    } else {
        min(
            f64::from(cc.max_constraint_width),
            f64::from(constraint_width),
        ) as u32
    };

    let mut group = NerdGlyph {
        width: glyph.width / cc.relative_width,
        height: glyph.height / cc.relative_height,
        x: glyph.x - glyph.width / cc.relative_width * cc.relative_x,
        y: glyph.y - glyph.height / cc.relative_height * cc.relative_y,
    };
    group.x = glyph.x - group.width * cc.relative_x;
    group.y = glyph.y - group.height * cc.relative_y;

    let (width_factor, height_factor) = scale_factors(&cc, &m, &group, min_w);
    let center_x = group.x + group.width / 2.0;
    let center_y = group.y + group.height / 2.0;
    group.width *= width_factor;
    group.height *= height_factor;
    group.x = center_x - group.width / 2.0;
    group.y = center_y - group.height / 2.0;

    // Vertical alignment, relative to the cell's bottom.
    if !(cc.size == Size::None && cc.align_vertical == Align::None) {
        let start_y = m.face_y + cc.pad_bottom * m.face_h;
        let end_y = m.face_y + (m.face_h - group.height - cc.pad_top * m.face_h);
        let center = f64::midpoint(start_y, end_y);

        match cc.align_vertical {
            Align::Start => group.y = start_y,
            Align::End => group.y = end_y,
            Align::Center | Align::Center1 => group.y = center,
            Align::None => {
                group.y = if end_y < start_y {
                    center
                } else {
                    max(start_y, min(group.y, end_y))
                };
            }
        }
    }

    // Horizontal alignment, relative to the face's left edge.
    if !(cc.size == Size::None && cc.align_horizontal == Align::None) {
        let span = m.face_w + f64::from(min_w.saturating_sub(1)) * m.cell_w;
        let start_x = cc.pad_left * m.face_w;
        let end_x = span - group.width - cc.pad_right * m.face_w;

        match cc.align_horizontal {
            Align::Start => group.x = start_x,
            Align::End => group.x = max(start_x, end_x),
            Align::Center => group.x = max(start_x, f64::midpoint(start_x, end_x)),
            Align::Center1 => {
                let single_end_x = m.face_w - group.width - cc.pad_right * m.face_w;
                group.x = max(start_x, f64::midpoint(start_x, single_end_x));
            }
            Align::None => group.x = max(start_x, min(group.x, end_x)),
        }
    }

    NerdGlyph {
        width: width_factor * glyph.width,
        height: height_factor * glyph.height,
        x: group.x + group.width * cc.relative_x,
        y: group.y + group.height * cc.relative_y,
    }
}

/// Borrowed fonts for one draw pass, selecting the variant a cell's style
/// bits name (`draw_text`'s `g_font` selection).
pub(crate) struct FontsRef<'a> {
    pub regular: &'a FontDescription,
    pub bold: &'a FontDescription,
    pub italic: &'a FontDescription,
    pub bold_italic: &'a FontDescription,
}

impl FontsRef<'_> {
    pub(super) fn for_flags(&self, bold: bool, italic: bool) -> &FontDescription {
        match (bold, italic) {
            (true, true) => self.bold_italic,
            (true, false) => self.bold,
            (false, true) => self.italic,
            (false, false) => self.regular,
        }
    }
}

/// Draw the grapheme in `cell` with an already-set source colour
/// (`draw_text`).
pub(crate) fn draw_text(
    cr: &cairo::Context,
    layout: &pango::Layout,
    cell: &Cell,
    fonts: &FontsRef<'_>,
    cell_metrics: &CellMetrics,
) {
    let desc = fonts.for_flags(
        cell.flags.contains(StyleFlags::BOLD),
        cell.flags.contains(StyleFlags::ITALIC),
    );

    layout.set_font_description(Some(desc));
    layout.set_text(cell.text_str());

    // pango_cairo_show_layout puts the layout's top at the current point, and
    // that top is the layout's own ascent -- which changes when Pango fell
    // back to another font for a glyph (box drawing, nerd icons, emoji).
    // Pin the baseline to the cell's instead, or those glyphs drift off the
    // row they belong to.
    let baseline = (layout.baseline() + pango::SCALE / 2) / pango::SCALE;

    if cell.len > 0
        && let Some(c) = constraint(first_codepoint(cell.text_bytes()))
        && c.does_anything()
    {
        let (ink, _) = layout.pixel_extents();
        if ink.width() > 0 && ink.height() > 0 {
            let m = cell_metrics.nerd;
            // The glyph's box relative to the cell's bottom-left
            // corner.
            let glyph = NerdGlyph {
                width: f64::from(ink.width()),
                height: f64::from(ink.height()),
                x: f64::from(ink.x()),
                y: f64::from(cell_metrics.baseline)
                    - (f64::from(ink.y() + ink.height()) - f64::from(baseline)),
            };

            let constrained = constrain(
                &c,
                &m,
                glyph,
                if cell.cw >= 1 {
                    cell.cw.cast_unsigned()
                } else {
                    1
                },
            );

            let _ = cr.save();
            cr.translate(
                f64::from(cell.x) * f64::from(cell_metrics.cell_w) + constrained.x,
                (f64::from(cell.y) + 1.0) * f64::from(cell_metrics.cell_h)
                    - constrained.y
                    - constrained.height,
            );
            cr.scale(
                constrained.width / f64::from(ink.width()),
                constrained.height / f64::from(ink.height()),
            );
            cr.translate(-f64::from(ink.x()), -f64::from(ink.y()));
            cr.move_to(0.0, 0.0);
            pangocairo::functions::show_layout(cr, layout);
            let _ = cr.restore();
            return;
        }
    }

    cr.move_to(
        f64::from(cell.x) * f64::from(cell_metrics.cell_w),
        f64::from(cell.y) * f64::from(cell_metrics.cell_h) + f64::from(cell_metrics.ascent)
            - f64::from(baseline),
    );
    pangocairo::functions::show_layout(cr, layout);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(x: f64, y: f64, width: f64, height: f64) -> NerdGlyph {
        NerdGlyph {
            x,
            y,
            width,
            height,
        }
    }

    /// 10x20 face, cell pitch 10x10, the baseline 2px below the face top.
    fn metrics() -> NerdMetrics {
        NerdMetrics {
            face_w: 10.0,
            face_h: 20.0,
            face_y: 2.0,
            icon_h: 18.0,
            icon_h_single: 14.0,
            cell_w: 10.0,
            cell_h: 10.0,
        }
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
        assert!((out.width - 20.0).abs() < 1e-9, "width {out:?}");
        assert!((out.height - 20.0).abs() < 1e-9, "height {out:?}");
    }

    #[test]
    fn fit_never_grows_the_glyph() {
        let mut c = Constraint::NONE;
        c.size = Size::Fit;
        // A wide, short glyph fits the box without being scaled.
        let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 8.0, 4.0), 1);
        assert!((out.width - 8.0).abs() < 1e-9, "width {out:?}");
        assert!((out.height - 4.0).abs() < 1e-9, "height {out:?}");

        // A tall, narrow glyph must shrink to fit the face height.
        let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 2.0, 40.0), 1);
        assert!((out.height - 20.0).abs() < 1e-9, "height {out:?}");
        assert!((out.width - 1.0).abs() < 1e-9, "width {out:?}");
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
        assert!((out.width - 10.0).abs() < 1e-9, "width {out:?}");
        assert!((out.height - 10.0).abs() < 1e-9, "height {out:?}");
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
        assert!((out.width - 10.0).abs() < 1e-9, "width {out:?}");
        assert!((out.height - 10.0).abs() < 1e-9, "height {out:?}");
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
        assert!((out.y - 7.0).abs() < 1e-9, "y {out:?}");
    }

    #[test]
    fn max_xy_ratio_limits_the_width() {
        let mut c = Constraint::NONE;
        c.size = Size::Cover;
        c.max_xy_ratio = Some(0.5);
        // Covering one cell would make the glyph 20 high; the ratio caps the
        // width at half its height.
        let out = constrain(&c, &metrics(), glyph(0.0, 0.0, 10.0, 10.0), 1);
        assert!((out.width - out.height * 0.5).abs() < 1e-9, "box {out:?}");
    }
}
