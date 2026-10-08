//! The frame gate (`replace-gtk-with-wayland` D5): what a frame
//! must redraw. The pinned libghostty-vt's render state tracks dirtiness
//! on two independent layers — a global state
//! (`GHOSTTY_RENDER_STATE_DATA_DIRTY`: `DIRTY_FALSE`, `PARTIAL`, `FULL`)
//! and a per-row dirty flag (`GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY`,
//! conservative: a row may be flagged without its content changing). The
//! gate turns those, plus everything the render state's flags do NOT
//! cover, into one `FramePlan`:
//!
//! * `FramePlan::Skip` — the frame is unchanged: draw nothing, touch no
//!   pixel, commit nothing.
//! * `FramePlan::Rows` — a partial repaint: the changed rows and their
//!   glyph-spill neighbours.
//! * everything — a full repaint, whenever the render state says `FULL` or
//!   any input it does not cover changed.
//!
//! What the render state's dirty flags do not cover, and how each is
//! treated (every case has a test):
//!
//! * **The first frame** — no previous frame established the canvas, so
//!   there is nothing to keep: everything.
//! * **A size change** (device size, scale, cell size, grid size), **a
//!   font change**, **a theme or default-colour change**, **a draw-offset
//!   change** and **a focus change** — the canvas' pixels do not survive
//!   any of these: everything. The focus accent draws over the whole
//!   surface edge, so even a focus-only change redraws everything.
//! * **The cursor moved, shown, hidden or restyled, and a blinking
//!   cursor** — the render state marks the cursor's own rows dirty on a
//!   move (the dirty tests pin that) but not on a restyle, and a blink
//!   toggles the drawn shape without any row flag: the gate compares the
//!   cursor state frame to frame and adds the old and the new cursor
//!   rows. The row flags stay independent and are added either way.
//! * **Kitty image placements** — a placement changing or being deleted
//!   does not necessarily mark any row dirty, and the render state gives
//!   no per-placement dirty data at all: any frame with placements, and
//!   any frame whose previous frame had placements (the deleted image's
//!   pixels must be cleared), is everything.
//!
//! When in doubt, everything: correctness beats savings. The partial
//! repaint region of a dirty row is the row's full-width device band plus
//! one row above and below, clamped to the grid: a glyph and its
//! anti-aliasing can spill across one row edge (ascenders, descenders),
//! and a spill into a row that is not repainted would be lost, differing
//! from a full repaint. A spill crossing two row edges would need ink
//! twice the cell pitch tall, which the cell metrics and the nerd-font
//! constraint do not produce; one row of headroom covers the rest.
//!
//! The caller owns the [`crate::render::frame_gate::FrameGate`] across
//! frames, beside the text pass
//! and the image pass, and hands it to
//! [`crate::render::frame_gate::paint_frame_gated`]. When an
//! event outside the gate's view invalidates the canvas — a fresh
//! `wl_shm` buffer from the pool, a lost frame — the caller forces the
//! next frame to everything with
//! [`crate::render::frame_gate::FrameGate::invalidate`].
//!
//! GTK-free (`replace-gtk-with-wayland` D10): the tests run without a
//! display.

use super::canvas::{Canvas, CanvasColor};
use super::geom::{DeviceRect, FrameAccent, FrameInput, PainterMetrics, device_px, logical_px};
use super::image_pass::ImagePass;
use super::painter::paint_open_frame;
use super::text_pass::TextPass;
use super::{accent, bands, bg, cursor, sprite};
use crate::term::Terminal;
use crate::term::cells::{Cursor, FrameDirty, Rgb};

/// What one frame must redraw. The dirty rows of [`FramePlan::Rows`] are
/// sorted, deduplicated and non-empty: an empty change set is
/// [`FramePlan::Skip`], not an empty partial repaint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FramePlan {
    /// The frame is unchanged: draw nothing and commit nothing.
    Skip,
    /// A partial repaint: `changed` rows carry a change (the damage to
    /// report), `repaint` is the superset the painter draws (the spill
    /// neighbours included).
    Rows { changed: RowSet, repaint: RowSet },
    /// A full repaint.
    Everything,
}

/// A sorted, deduplicated, non-empty set of viewport rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RowSet {
    rows: Vec<i32>,
}

impl RowSet {
    /// A set from unsorted, possibly duplicated rows, each clamped into
    /// `0..grid_rows`. `None` when no row survives — a frame with nothing
    /// to redraw is [`FramePlan::Skip`], and a partial repaint with no
    /// valid row is escalated to everything by the caller.
    pub(crate) fn from_rows(rows: &[i32], grid_rows: i32) -> Option<Self> {
        let mut rows: Vec<i32> = rows
            .iter()
            .copied()
            .filter(|row| *row >= 0 && *row < grid_rows)
            .collect();
        rows.sort_unstable();
        rows.dedup();
        if rows.is_empty() {
            None
        } else {
            Some(RowSet { rows })
        }
    }

    /// The set plus one row above and below each of its rows, clamped to
    /// the grid — the repaint region of the partial pass (the spill rule
    /// in the module comment). Every row of the set is in range, so each
    /// contributes at least itself: the result is never empty. The gate's
    /// plan applies the expansion exactly once, when it builds
    /// [`FramePlan::Rows`]; the painter draws the set as it is.
    #[must_use]
    pub(crate) fn expanded(&self, grid_rows: i32) -> Self {
        let mut rows = Vec::new();
        for &row in &self.rows {
            for neighbour in row - 1..=row + 1 {
                if neighbour >= 0 && neighbour < grid_rows {
                    rows.push(neighbour);
                }
            }
        }
        rows.sort_unstable();
        rows.dedup();
        RowSet { rows }
    }

    /// The rows, in ascending order.
    pub(crate) fn rows(&self) -> &[i32] {
        &self.rows
    }

    /// Whether `row` is in the set.
    pub(crate) fn contains(&self, row: i32) -> bool {
        self.rows.binary_search(&row).is_ok()
    }
}

/// What the caller must present after the frame: [`FrameOutcome::Clean`]
/// says nothing was drawn and nothing may be committed — no
/// `wl_surface` damage, no commit. [`FrameOutcome::Damage`] carries the
/// device rectangles that changed (the changed rows' full-width bands, or
/// the whole surface), snapped like every device rectangle, for
/// `wl_surface.damage_buffer`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Nothing was drawn; the surface's committed state still shows it.
    Clean,
    /// The device rectangles that changed.
    Damage(Damage),
}

/// The device rectangles one frame changed, for `wl_surface.damage_buffer`.
/// An empty list is a valid damage — a frame that drew nothing but must
/// still commit — and a no-op on the Wayland side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Damage {
    rects: Vec<DeviceRect>,
}

impl Damage {
    /// The whole device surface — the damage of a full repaint or of the
    /// degraded draw.
    #[must_use]
    pub fn whole_surface(width: u32, height: u32) -> Self {
        let width = i32::try_from(width).unwrap_or(i32::MAX);
        let height = i32::try_from(height).unwrap_or(i32::MAX);
        Damage {
            rects: DeviceRect::new(0, 0, width, height).into_iter().collect(),
        }
    }

    /// The damage from device rectangles; an empty list draws no damage.
    #[must_use]
    pub fn from_rects(rects: Vec<DeviceRect>) -> Self {
        Damage { rects }
    }

    /// The device rectangles that changed, for `wl_surface.damage_buffer`.
    #[must_use]
    pub fn rects(&self) -> &[DeviceRect] {
        &self.rects
    }
}

/// The frame-to-frame identity of everything a partial repaint keeps: the
/// geometry, the focus state, the colours. A change in any of them makes
/// the old canvas unusable, so the frame is everything.
#[derive(Clone, Debug, PartialEq)]
struct Fingerprint {
    metrics: PainterMetrics,
    logical: (u32, u32),
    device: (u32, u32),
    /// The tween's snapped draw offset; equality is enough, the value is
    /// snapped to the device pixel grid by the tween.
    draw_offset: f64,
    focused: bool,
    theme_background: CanvasColor,
    theme_foreground: CanvasColor,
    accent: Option<FrameAccent>,
    cols: u16,
    rows: u16,
    default_background: Rgb,
    default_foreground: Rgb,
}

/// The per-frame inputs the gate decides on: the fingerprint above, the
/// dirty data the render state reported at `frame_begin`, and the inputs
/// the render state's flags do not cover (the cursor, the image
/// placements, the font). Built by [`begin_frame`].
struct GateInput {
    fingerprint: Fingerprint,
    /// The frame's cursor as [`Terminal::cursor`] reports it; `None` when
    /// it is hidden, blinking off, or outside the viewport.
    cursor: Option<Cursor>,
    /// Whether the frame carries kitty placements.
    images: bool,
    /// The font changed since the last frame (a rebuilt text pass).
    font_changed: bool,
    /// The render state's global dirty state.
    dirty: FrameDirty,
    /// The render state's per-row dirty flags.
    dirty_rows: Vec<i32>,
}

/// What the gate remembers about the last frame it decided.
#[derive(Clone, Debug)]
struct LastFrame {
    fingerprint: Fingerprint,
    cursor: Option<Cursor>,
    images: bool,
}

/// The frame gate, owned by the caller across frames. A fresh gate's first
/// frame is everything.
#[derive(Debug, Default)]
pub struct FrameGate {
    last: Option<LastFrame>,
}

impl FrameGate {
    /// A gate that treats its first frame as everything.
    #[must_use]
    pub const fn new() -> Self {
        FrameGate { last: None }
    }

    /// Force the next frame to everything: the canvas the gate compares
    /// against was invalidated outside its view (a fresh `wl_shm` buffer
    /// from the pool, a lost frame).
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    /// Decide what this frame must redraw. Called with the frame open; the
    /// dirty data is the capture `frame_begin` made (see the `dirty`
    /// submodule of `term::cells`).
    fn plan(&mut self, input: &GateInput) -> FramePlan {
        let grid_rows = i32::from(input.fingerprint.rows);
        let Some(last) = &self.last else {
            return FramePlan::Everything; // first frame: nothing to keep
        };
        // Everything the render state's flags do not cover (the module
        // comment lists each): one change redraws everything.
        if input.font_changed || input.fingerprint != last.fingerprint {
            return FramePlan::Everything;
        }
        if input.images || last.images {
            return FramePlan::Everything;
        }
        // The cursor's rows, when the cursor changed. `None` cannot happen
        // for a changed cursor — one of the two always has a row — but an
        // empty set escalates below, like every surprise.
        let cursor_rows: Vec<i32> = if cursor_changed(input.cursor.as_ref(), last.cursor.as_ref()) {
            [input.cursor, last.cursor]
                .into_iter()
                .flatten()
                .map(|cursor| cursor.y)
                .collect()
        } else {
            Vec::new()
        };

        match input.dirty {
            FrameDirty::Full => FramePlan::Everything,
            FrameDirty::Clean if input.dirty_rows.is_empty() => {
                // The render state says nothing changed. The cursor can
                // still have moved, blinked, or changed style: its rows
                // are the frame's only possible change.
                let Some(changed) = RowSet::from_rows(&cursor_rows, grid_rows) else {
                    return FramePlan::Skip;
                };
                let repaint = changed.expanded(grid_rows);
                FramePlan::Rows { changed, repaint }
            }
            // Partial, or Clean with row flags the global state did not
            // mention (the two layers are independent): the row flags are
            // the change, plus the cursor rows.
            _ => {
                let mut rows = input.dirty_rows.clone();
                rows.extend(cursor_rows);
                let Some(changed) = RowSet::from_rows(&rows, grid_rows) else {
                    // A dirty state with no valid row is not a state a
                    // repaint can express: everything.
                    return FramePlan::Everything;
                };
                let repaint = changed.expanded(grid_rows);
                FramePlan::Rows { changed, repaint }
            }
        }
    }

    /// Record the frame just drawn, so the next decision compares against
    /// it.
    fn record(&mut self, fingerprint: Fingerprint, cursor: Option<Cursor>, images: bool) {
        self.last = Some(LastFrame {
            fingerprint,
            cursor,
            images,
        });
    }
}

/// Whether the cursor's drawn state changed between two frames: position,
/// visibility, shape or wide-tail — everything the cursor layer draws.
fn cursor_changed(a: Option<&Cursor>, b: Option<&Cursor>) -> bool {
    a != b
}

/// Draw one frame of `terminal` into the caller-owned `canvas` under the
/// frame gate: a `FramePlan::Skip` frame draws and commits nothing, a
/// `FramePlan::Rows` frame repaints only the changed rows and their
/// spill neighbours, everything is [`super::painter::paint_frame`]'s full
/// draw. Returns the [`FrameOutcome`] the caller presents by.
///
/// The inputs mirror [`super::painter::paint_frame`]; `gate` is owned by
/// the caller across frames, and `font_changed` is set when the caller
/// rebuilt the text pass (a font change the render state's flags do not
/// cover).
///
/// A frame that cannot be opened keeps the degraded draw — the theme
/// background and the accent — and resets the gate: the canvas holds no
/// grid the next frame could keep, so the next open frame is everything.
pub fn paint_frame_gated(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    terminal: &mut Terminal,
    text: &mut TextPass,
    images: &mut ImagePass,
    gate: &mut FrameGate,
    font_changed: bool,
) -> FrameOutcome {
    // The tween's draw offset translates the grid layers, not the accent;
    // snapped to the device pixel grid by the tween, so the product is a
    // whole number of device pixels up to floating-point dust.
    let offset = device_px(frame.draw_offset() * metrics.scale());

    let Some(begin) = begin_frame(terminal, metrics, frame, gate, font_changed) else {
        let (device_w, device_h) = frame.device_size();
        fill_background(canvas, frame);
        accent::paint(canvas, frame, metrics);
        gate.invalidate();
        return FrameOutcome::Damage(Damage::whole_surface(device_w, device_h));
    };

    match begin.plan {
        FramePlan::Skip => {
            // The frame is unchanged: nothing draws, nothing commits. The
            // frame is still begun, so it must still end — `frame_end`'s
            // clean is what keeps the next frame clean too.
            terminal.frame_end();
            gate.record(begin.fingerprint, begin.cursor, begin.images);
            FrameOutcome::Clean
        }
        FramePlan::Rows { changed, repaint } => {
            paint_rows(canvas, metrics, frame, terminal, text, &repaint, offset);
            terminal.frame_end();
            gate.record(begin.fingerprint, begin.cursor, begin.images);
            FrameOutcome::Damage(Damage::from_rects(
                changed
                    .rows()
                    .iter()
                    .map(|&row| row_band(metrics, frame, row))
                    .collect(),
            ))
        }
        FramePlan::Everything => {
            let (device_w, device_h) = frame.device_size();
            fill_background(canvas, frame);
            paint_open_frame(canvas, metrics, frame, terminal, text, images, offset);
            gate.record(begin.fingerprint, begin.cursor, begin.images);
            FrameOutcome::Damage(Damage::whole_surface(device_w, device_h))
        }
    }
}

/// The full-width device band of one grid row: from the row's snapped top
/// to the next row's snapped top, the true last row down to the frame's
/// logical height. Exactly [`bg::run_rect`]'s rectangle for the columns
/// the frame's logical width spans — the same last-row rule, so the
/// partial repaint's clear and damage cannot drift from the full path's
/// background pass.
fn row_band(metrics: &PainterMetrics, frame: &FrameInput, row: i32) -> DeviceRect {
    let (logical_w, _) = frame.logical_size();
    // The columns the frame's logical width spans, rounded up: the band
    // clears the margin right of the last column too, where a full draw's
    // theme fill reaches.
    let cols = logical_px((f64::from(logical_w) / metrics.cell_w()).ceil());
    bg::run_rect(metrics, frame, row, 0, cols)
}

/// Fill the whole canvas with the theme background — the first layer of
/// every full draw.
fn fill_background(canvas: &mut Canvas, frame: &FrameInput) {
    let (device_w, device_h) = frame.device_size();
    if let (Ok(device_w), Ok(device_h)) = (i32::try_from(device_w), i32::try_from(device_h)) {
        canvas.fill_rect(0, 0, device_w, device_h, frame.background());
    }
}

/// The frame's opening: begin it, collect the gate's inputs and decide.
/// `None` when the frame cannot be opened — the degraded draw.
fn begin_frame(
    terminal: &mut Terminal,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    gate: &mut FrameGate,
    font_changed: bool,
) -> Option<BeginFrame> {
    if !terminal.frame_begin() {
        return None;
    }
    let fingerprint = Fingerprint {
        metrics: *metrics,
        logical: frame.logical_size(),
        device: frame.device_size(),
        draw_offset: frame.draw_offset(),
        focused: frame.focused(),
        theme_background: frame.background(),
        theme_foreground: frame.foreground(),
        accent: frame.accent(),
        cols: terminal.cols(),
        rows: terminal.rows(),
        default_background: terminal.colors().background,
        default_foreground: terminal.colors().foreground,
    };
    let cursor = terminal.cursor();
    let images = terminal.frame_has_images();
    let input = GateInput {
        fingerprint: fingerprint.clone(),
        cursor,
        images,
        font_changed,
        dirty: terminal.frame_dirty(),
        dirty_rows: terminal.frame_dirty_rows().to_vec(),
    };
    let plan = gate.plan(&input);
    Some(BeginFrame {
        plan,
        fingerprint,
        cursor,
        images,
    })
}

/// What the opened frame decided, and what [`FrameGate::record`] needs
/// after it.
struct BeginFrame {
    plan: FramePlan,
    fingerprint: Fingerprint,
    cursor: Option<Cursor>,
    images: bool,
}

/// Repaint the `repaint` rows' region into the caller-owned canvas: each
/// row's full-width device band, cleared to the theme background first,
/// then the cell layers in [`paint_open_frame`]'s order clipped to the
/// region, then the part of the focus accent that falls inside it. The
/// caller reports the changed rows' bands as the damage; the spill
/// neighbours are repainted with their own unchanged pixels.
///
/// The region is `repaint` as the gate's plan built it — the changed rows
/// plus one neighbour above and below each, clamped to the grid, expanded
/// exactly once ([`FrameGate::plan`]). The walk filter clips every cell
/// pass at once — they all share the frame walk.
fn paint_rows(
    canvas: &mut Canvas,
    metrics: &PainterMetrics,
    frame: &FrameInput,
    terminal: &mut Terminal,
    text: &mut TextPass,
    repaint: &RowSet,
    offset: i32,
) {
    let bands: Vec<DeviceRect> = repaint
        .rows()
        .iter()
        .map(|&row| row_band(metrics, frame, row))
        .collect();

    // Clear the repainted rows to the theme background: the same first
    // layer a full draw fills, clipped to the region, so every layer after
    // blends over the same backdrop it would have had.
    for band in &bands {
        canvas.fill_rect(band.x(), band.y(), band.w(), band.h(), frame.background());
    }

    // The cell layers in the full draw's order, clipped by the walk filter.
    terminal.frame_walk_rows(repaint.rows());
    bg::paint(canvas, metrics, frame, terminal, offset);
    terminal.frame_rewind();
    sprite::paint(canvas, metrics, frame, terminal, offset);
    terminal.frame_rewind();
    let cursor_text = text.paint(canvas, metrics, frame, terminal, offset);
    terminal.frame_rewind();
    bands::paint(canvas, metrics, frame, terminal, offset);
    // The cursor draws only when its row is repainted; a cursor whose row
    // is clean keeps the pixels an earlier frame drew for it.
    let draw_cursor = terminal
        .cursor()
        .is_some_and(|cursor| repaint.contains(cursor.y));
    if draw_cursor {
        cursor::paint(canvas, metrics, terminal, offset, &cursor_text, text);
    }
    // No images here: any frame with placements is everything (the module
    // comment), so the partial path never has one to draw.
    terminal.frame_walk_rows(&[]);

    // The accent's part inside the region, last, exactly as the full draw
    // draws the accent on top: the region's clear erased it where it
    // crossed the repainted rows.
    accent::paint_rows(canvas, frame, metrics, &bands);
}

/// Repaint a single frame twice — always-full and gated — and compare the
/// canvases pixel for pixel after every step.
#[cfg(test)]
mod tests;
