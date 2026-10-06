//! The Nerd Font glyph constraints over swash geometry (row 4.5): the port
//! of `NerdGlyph`, `nerd_scale_factors` and `nerd_constrain` in
//! `super::text` (which ported ghostty's `Glyph.RenderOptions.Constraint`,
//! `nerd_constrain` in `src/render.c`, design D5 there). The arithmetic and
//! the test expectations carry over unchanged; what changes is the frame
//! the numbers live in and the output: the old path scaled an already
//! rasterized layout through cairo, this module produces a
//! [`PlacementTransform`](crate::render::glyph::PlacementTransform) swash
//! applies to the outline before rasterizing (see [`placement`]).
//!
//! Frames, both y up:
//!
//! * [`constrain`] works in the frame relative to the cell's bottom-left
//!   corner — the old `NerdGlyph` frame, which `draw_text` built from
//!   Pango's y-down ink box with `y = baseline - (ink bottom - baseline)`.
//! * [`PlacementTransform`](crate::render::glyph::PlacementTransform) works
//!   in the frame relative to the baseline, y up — swash's outline frame.
//!   [`placement`] converts between the two; [`ink_to_cell_frame`] is that
//!   conversion as its own tested function.
//!
//! Everything here is device pixels: the spec's "Cell text on the device
//! pixel lattice" requires a constrained glyph to sit at the same device
//! pixel offset in every cell at any output scale, so the metrics the
//! constraints are expressed against are the logical [`CellMetrics`] numbers
//! multiplied by the unsnapped output scale ([`NerdMetrics::scaled`]). The
//! transform never depends on the column or row — only on the glyph, the
//! constraint, the metrics and the constraint width — so the same glyph
//! renders the same way in every cell.

use crate::nerd_font::{Align, Constraint, Height, Size};
use crate::render::cell_metrics::CellMetrics;
use crate::render::glyph::{Glyph, GlyphImage, PlacementTransform};

#[cfg(test)]
mod tests;

/// A glyph's box relative to the cell's bottom-left corner, y up (the port
/// of the old `NerdGlyph`). `y` is the box's bottom edge above the cell's
/// bottom; `x` is its left edge right of the cell's left edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NerdGlyph {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl NerdGlyph {
    /// A box from its parts (the tests use it).
    #[must_use]
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        NerdGlyph {
            x,
            y,
            width,
            height,
        }
    }

    /// The box's left edge, right of the cell's left edge.
    #[must_use]
    pub fn x(self) -> f64 {
        self.x
    }

    /// The box's bottom edge, above the cell's bottom.
    #[must_use]
    pub fn y(self) -> f64 {
        self.y
    }

    /// The box's width.
    #[must_use]
    pub fn width(self) -> f64 {
        self.width
    }

    /// The box's height.
    #[must_use]
    pub fn height(self) -> f64 {
        self.height
    }
}

/// The Nerd Font numbers the constraints are expressed against, in device
/// pixels: the old `NerdMetrics` (`g_nerd_*` in the C glue), whose values
/// were logical pixels, scaled by the output scale. The fields are private
/// (port-to-rust D6); the accessors carry the meaning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NerdMetrics {
    face_w: f64,
    face_h: f64,
    face_y: f64,
    icon_h: f64,
    icon_h_single: f64,
    cell_w: f64,
    cell_h: f64,
}

impl NerdMetrics {
    /// Metrics from their parts. `None` when a value is not finite, or a
    /// size is negative — a font measurement cannot produce either.
    #[must_use]
    pub fn new(
        face_w: f64,
        face_h: f64,
        face_y: f64,
        icon_h: f64,
        icon_h_single: f64,
        cell_w: f64,
        cell_h: f64,
    ) -> Option<Self> {
        let sizes = [face_w, face_h, icon_h, icon_h_single, cell_w, cell_h];
        if sizes.iter().any(|value| !value.is_finite() || *value < 0.0) || !face_y.is_finite() {
            return None;
        }
        Some(NerdMetrics {
            face_w,
            face_h,
            face_y,
            icon_h,
            icon_h_single,
            cell_w,
            cell_h,
        })
    }

    /// The face box width (the hinted digit advance).
    #[must_use]
    pub fn face_w(self) -> f64 {
        self.face_w
    }

    /// The face box height (the hinted line box).
    #[must_use]
    pub fn face_h(self) -> f64 {
        self.face_h
    }

    /// The face box top relative to the cell's bottom.
    #[must_use]
    pub fn face_y(self) -> f64 {
        self.face_y
    }

    /// The full-height icon box.
    #[must_use]
    pub fn icon_h(self) -> f64 {
        self.icon_h
    }

    /// The one-cell icon height, the `font_patcher` heuristic
    /// `(2*cap + face_h) / 3`.
    #[must_use]
    pub fn icon_h_single(self) -> f64 {
        self.icon_h_single
    }

    /// The horizontal cell pitch.
    #[must_use]
    pub fn cell_w(self) -> f64 {
        self.cell_w
    }

    /// The vertical cell pitch.
    #[must_use]
    pub fn cell_h(self) -> f64 {
        self.cell_h
    }

    /// The same metrics at another output scale: every field multiplied by
    /// `scale`. A `scale` that is not positive and finite falls back to 1,
    /// like `OutputScale::new` does. The values stay unsnapped — the spec
    /// requires the transform to be identical in every cell at one scale,
    /// so no per-cell or per-row snapping may enter it.
    #[must_use]
    pub fn scaled(self, scale: f64) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        NerdMetrics {
            face_w: self.face_w * scale,
            face_h: self.face_h * scale,
            face_y: self.face_y * scale,
            icon_h: self.icon_h * scale,
            icon_h_single: self.icon_h_single * scale,
            cell_w: self.cell_w * scale,
            cell_h: self.cell_h * scale,
        }
    }

    /// The metrics [`CellMetrics`] carries, in logical pixels, scaled to
    /// `scale` device pixels per logical pixel (see [`Self::scaled`]). The
    /// logical values are whole pixels except the face and icon boxes, so
    /// the device values carry the fractional part a fractional scale
    /// produces — exactly what the constraints must work against.
    #[must_use]
    pub fn from_cell_metrics(metrics: &CellMetrics, scale: f64) -> Option<Self> {
        let (cell_w, cell_h) = metrics.cell_size();
        Self::new(
            metrics.face_w(),
            metrics.face_h(),
            metrics.face_y(),
            metrics.icon_h(),
            metrics.icon_h_single(),
            cell_w,
            cell_h,
        )
        .map(|logical| logical.scaled(scale))
    }
}

/// A rasterized glyph's ink box, relative to the baseline, y up, in device
/// pixels: the box the unconstrained rasterization produced (see
/// [`Glyph::placement`] for the y-up convention). The input to
/// [`placement`] and [`ink_to_cell_frame`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InkBox {
    left: f64,
    bottom: f64,
    width: f64,
    height: f64,
}

impl InkBox {
    /// A box from its parts (the tests use it).
    #[must_use]
    pub fn new(left: f64, bottom: f64, width: f64, height: f64) -> Self {
        InkBox {
            left,
            bottom,
            width,
            height,
        }
    }

    /// The ink box of a rasterized [`Glyph`]: the placement's left and top
    /// edges and the image's size. An empty glyph (a space, or a render
    /// miss) has a zero box.
    #[must_use]
    pub fn from_glyph(glyph: &Glyph) -> Self {
        let (width, height) = match glyph.image() {
            GlyphImage::Empty => (0.0, 0.0),
            GlyphImage::Mask(mask) => {
                // A mask's pixel count cannot approach the f64 precision
                // limit; the fallback is unreachable for a real mask.
                (
                    num_traits::cast::<usize, f64>(mask.width()).unwrap_or(0.0),
                    num_traits::cast::<usize, f64>(mask.height()).unwrap_or(0.0),
                )
            }
            GlyphImage::Color(pixmap) => (f64::from(pixmap.width()), f64::from(pixmap.height())),
        };
        let placement = glyph.placement();
        InkBox {
            left: f64::from(placement.left()),
            bottom: f64::from(placement.top()) - height,
            width,
            height,
        }
    }

    /// The box's left edge, right of the baseline origin.
    #[must_use]
    pub fn left(self) -> f64 {
        self.left
    }

    /// The box's bottom edge, above the baseline (negative when the glyph
    /// descends below it).
    #[must_use]
    pub fn bottom(self) -> f64 {
        self.bottom
    }

    /// The box's width.
    #[must_use]
    pub fn width(self) -> f64 {
        self.width
    }

    /// The box's height.
    #[must_use]
    pub fn height(self) -> f64 {
        self.height
    }
}

/// Convert a baseline-relative ink box into the cell-bottom-left frame
/// [`constrain`] works in: the box's bottom edge sits `baseline` device
/// pixels above the cell's bottom, and its left edge is already measured
/// from the cell's left edge. The one conversion between the two frames of
/// the module doc.
fn ink_to_cell_frame(ink: &InkBox, baseline: f64) -> NerdGlyph {
    NerdGlyph::new(
        ink.left(),
        ink.bottom() + baseline,
        ink.width(),
        ink.height(),
    )
}

/// The placement transform one constrained glyph rasterizes with: the
/// constraint's target box, computed in the cell-bottom-left frame, turned
/// into the scale-and-offset swash applies to the outline before
/// rasterizing (`x' = sx*x + ox`, `y' = sy*y + oy`, baseline-relative, y
/// up — the same frame [`PlacementTransform`] documents).
///
/// `None` when the constraint neither sizes nor positions the glyph (the
/// glyph draws at its natural placement), when the ink box is empty (a
/// space or a render miss has nothing to place), or when the constrained
/// box or the transform degenerates — a non-finite or unquantizable value,
/// which validated metrics and a cell-sized constraint cannot produce and
/// which degrades to the unconstrained draw rather than failing the frame.
#[must_use]
pub fn placement(
    constraint: &Constraint,
    metrics: &NerdMetrics,
    baseline: f64,
    constraint_width: u32,
    ink: &InkBox,
) -> Option<PlacementTransform> {
    if !constraint.does_anything() {
        return None;
    }
    if ink.width() <= 0.0 || ink.height() <= 0.0 {
        return None;
    }
    let constrained = constrain(
        constraint,
        metrics,
        ink_to_cell_frame(ink, baseline),
        constraint_width,
    );
    if constrained.width() <= 0.0 || constrained.height() <= 0.0 {
        return None;
    }
    // The scale maps the unconstrained ink box onto the constrained box;
    // the offsets then move the scaled box into place. Both are
    // baseline-relative y up, the frame the outline transform applies in.
    let scale_x = constrained.width() / ink.width();
    let scale_y = constrained.height() / ink.height();
    let offset_x = constrained.x() - scale_x * ink.left();
    let offset_y = constrained.y() - baseline - scale_y * ink.bottom();
    PlacementTransform::new(scale_x, scale_y, offset_x, offset_y).ok()
}

fn max(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

fn min(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

/// The scale factors for one constraint against `group`, given `min_w` cells
/// of constraint width (the port of `nerd_scale_factors`).
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

/// Ghostty normalises every Nerd Font icon to a canonical box before
/// drawing: `constraint` maps the codepoint to a constraint (scale rule,
/// padding, alignment, relative geometry) and the glyph is scaled and moved
/// to fit. The port of the old `constrain`, which ported
/// `Glyph.RenderOptions.Constraint` (`nerd_constrain`).
///
/// `constraint_width` is the constraint width in cells — the cell's column
/// span, at least 1. The result's box stays in the cell-bottom-left frame
/// the input came in.
#[must_use]
pub fn constrain(
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

    // The constraint width is the smaller of the constraint's own maximum
    // and the cell's column span — both small non-negative cell counts, so
    // the minimum is exact integer arithmetic, no float truncation.
    let min_w: u32 = if cc.size == Size::Stretch && m.face_w > 0.9 * m.face_h {
        1
    } else {
        u32::from(cc.max_constraint_width).min(constraint_width)
    };

    let group_width = glyph.width / cc.relative_width;
    let group_height = glyph.height / cc.relative_height;
    let mut group = NerdGlyph {
        x: glyph.x - group_width * cc.relative_x,
        y: glyph.y - group_height * cc.relative_y,
        width: group_width,
        height: group_height,
    };

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
