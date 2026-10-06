//! The frame gate's tests (row 4.8): the escalation cases, the skip, and
//! the pixel parity between the always-full and the gated path. GTK-free
//! (`replace-gtk-with-wayland` D10): everything runs without a display.

use super::paint_frame_gated;
use super::{Damage, Fingerprint, FrameGate, FrameOutcome, FramePlan, GateInput, RowSet};
use crate::fontconfig::ThemeColours;
use crate::guard::Poisoned;
use crate::layout::Accent;
use crate::render::canvas::{Canvas, CanvasColor};
use crate::render::geom::{FrameInput, PainterMetrics, device_px};
use crate::render::image_pass::ImagePass;
use crate::render::painter::paint_frame;
use crate::render::text_pass::test_support;
use crate::term::Terminal;
use crate::term::cells::{Cursor, CursorStyle, FrameDirty, Rgb};
use std::num::NonZeroU16;

// ---------------------------------------------------------------------------
// The pure decision

/// The fingerprint of an 8-column, 4-row frame at the 8x16 pitch, scale 1.
fn fingerprint() -> Fingerprint {
    Fingerprint {
        metrics: PainterMetrics::new(8.0, 16.0, 12.0, 1.0).expect("test metrics are valid"),
        logical: (64, 64),
        device: (64, 64),
        draw_offset: 0.0,
        focused: false,
        theme_background: CanvasColor::from_rgba(10, 20, 30, 255),
        theme_foreground: CanvasColor::from_rgba(200, 150, 100, 255),
        accent: None,
        cols: 8,
        rows: 4,
        default_background: Rgb::default(),
        default_foreground: Rgb::default(),
    }
}

/// A gate input over `fingerprint` with nothing changed: clean, no dirty
/// rows, no cursor, no images, no font change.
fn input(fingerprint: &Fingerprint) -> GateInput {
    GateInput {
        fingerprint: fingerprint.clone(),
        cursor: None,
        images: false,
        font_changed: false,
        dirty: FrameDirty::Clean,
        dirty_rows: Vec::new(),
    }
}

/// A visible block cursor at cell (3, 1).
fn cursor() -> Cursor {
    Cursor {
        has_value: true,
        x: 3,
        y: 1,
        style: CursorStyle::Block,
        wide_tail: false,
    }
}

/// One fingerprint-mutation case of the everything sweep.
type Mutation = (&'static str, Box<dyn Fn(&mut Fingerprint)>);

#[test]
fn a_fresh_gate_starts_with_everything() {
    let mut gate = FrameGate::new();
    assert_eq!(gate.plan(&input(&fingerprint())), FramePlan::Everything);
}

#[test]
fn an_unchanged_clean_frame_skips() {
    let mut gate = FrameGate::new();
    let fingerprint = fingerprint();
    gate.record(fingerprint.clone(), None, false);
    assert_eq!(gate.plan(&input(&fingerprint)), FramePlan::Skip);
}

/// A font change — a rebuilt text pass — redraws everything, even with a
/// clean render state.
#[test]
fn a_font_change_is_everything() {
    let mut gate = FrameGate::new();
    let fingerprint = fingerprint();
    gate.record(fingerprint.clone(), None, false);
    let mut changed = input(&fingerprint);
    changed.font_changed = true;
    assert_eq!(gate.plan(&changed), FramePlan::Everything);
}

/// Every fingerprint field the canvas' pixels depend on: one change is
/// everything.
#[test]
fn a_size_scale_cell_grid_offset_focus_or_colour_change_is_everything() {
    let base = fingerprint();
    // The accent enters the fingerprint through [`FrameInput::new`] — the
    // type's fields are private (port-to-rust D6).
    let accent_input = frame(
        true,
        Some(Accent::new(
            [255, 0, 0],
            NonZeroU16::new(2).expect("nonzero"),
        )),
        0.0,
        1.0,
    );
    let accent = accent_input.accent();
    let cases: Vec<Mutation> = vec![
        (
            "logical size",
            Box::new(|f: &mut Fingerprint| f.logical.0 = 63),
        ),
        (
            "device size",
            Box::new(|f: &mut Fingerprint| f.device.1 = 63),
        ),
        (
            "scale (via metrics)",
            Box::new(|f: &mut Fingerprint| {
                f.metrics =
                    PainterMetrics::new(8.0, 16.0, 12.0, 1.5).expect("test metrics are valid");
            }),
        ),
        (
            "cell size (via metrics)",
            Box::new(|f: &mut Fingerprint| {
                f.metrics =
                    PainterMetrics::new(9.0, 16.0, 12.0, 1.0).expect("test metrics are valid");
            }),
        ),
        (
            "ascent (via metrics)",
            Box::new(|f: &mut Fingerprint| {
                f.metrics =
                    PainterMetrics::new(8.0, 16.0, 13.0, 1.0).expect("test metrics are valid");
            }),
        ),
        (
            "draw offset",
            Box::new(|f: &mut Fingerprint| f.draw_offset = 5.0),
        ),
        ("focus", Box::new(|f: &mut Fingerprint| f.focused = true)),
        (
            "theme background",
            Box::new(|f: &mut Fingerprint| {
                f.theme_background = CanvasColor::from_rgba(0, 20, 30, 255);
            }),
        ),
        (
            "theme foreground",
            Box::new(|f: &mut Fingerprint| {
                f.theme_foreground = CanvasColor::from_rgba(200, 150, 0, 255);
            }),
        ),
        ("grid columns", Box::new(|f: &mut Fingerprint| f.cols = 7)),
        ("grid rows", Box::new(|f: &mut Fingerprint| f.rows = 3)),
        (
            "default colour",
            Box::new(|f: &mut Fingerprint| f.default_foreground = Rgb { r: 1, g: 2, b: 3 }),
        ),
        (
            "the accent configured",
            Box::new(move |f: &mut Fingerprint| f.accent = accent),
        ),
    ];
    for (name, mutate) in cases {
        let mut changed = base.clone();
        mutate(&mut changed);
        assert_ne!(changed, base, "{name} must change the fingerprint");
        let mut gate = FrameGate::new();
        gate.record(base.clone(), None, false);
        assert_eq!(
            gate.plan(&input(&changed)),
            FramePlan::Everything,
            "{name} redraws everything"
        );
    }
}

/// A frame with kitty placements is everything, and so is the frame after
/// one — the deleted image's pixels must be cleared.
#[test]
fn a_frame_with_images_or_after_images_is_everything() {
    let fingerprint = fingerprint();

    let mut gate = FrameGate::new();
    gate.record(fingerprint.clone(), None, false);
    let mut images = input(&fingerprint);
    images.images = true;
    assert_eq!(gate.plan(&images), FramePlan::Everything, "images now");

    let mut gate = FrameGate::new();
    gate.record(fingerprint.clone(), None, true);
    assert_eq!(
        gate.plan(&input(&fingerprint)),
        FramePlan::Everything,
        "images before"
    );
}

/// A `FULL` render state redraws everything regardless of its row flags.
#[test]
fn a_full_dirty_state_is_everything() {
    let mut gate = FrameGate::new();
    let fingerprint = fingerprint();
    gate.record(fingerprint.clone(), None, false);
    let mut full = input(&fingerprint);
    full.dirty = FrameDirty::Full;
    assert_eq!(gate.plan(&full), FramePlan::Everything);
}

/// A partial frame repaints the dirty rows and their spill neighbours.
#[test]
fn a_partial_frame_repaints_the_dirty_rows_and_their_neighbours() {
    let mut gate = FrameGate::new();
    let fingerprint = fingerprint();
    gate.record(fingerprint.clone(), None, false);
    let mut partial = input(&fingerprint);
    partial.dirty = FrameDirty::Partial;
    partial.dirty_rows = vec![2];
    assert_eq!(
        gate.plan(&partial),
        FramePlan::Rows {
            changed: RowSet::from_rows(&[2], 4).expect("row in range"),
            repaint: RowSet::from_rows(&[1, 2, 3], 4).expect("rows in range"),
        }
    );

    // The spill neighbours clamp at the grid's edges.
    let mut gate = FrameGate::new();
    gate.record(fingerprint.clone(), None, false);
    let mut partial = input(&fingerprint);
    partial.dirty = FrameDirty::Partial;
    partial.dirty_rows = vec![0, 3];
    assert_eq!(
        gate.plan(&partial),
        FramePlan::Rows {
            changed: RowSet::from_rows(&[0, 3], 4).expect("rows in range"),
            repaint: RowSet::from_rows(&[0, 1, 2, 3], 4).expect("rows in range"),
        }
    );
}

/// A moved, hidden, restyled or blinking-off cursor marks its old and its
/// new row, on top of the row flags.
#[test]
fn a_cursor_change_marks_its_old_and_new_row() {
    let fingerprint = fingerprint();

    // A clean frame whose cursor appeared: the cursor's row and its spill
    // neighbours.
    let mut gate = FrameGate::new();
    gate.record(fingerprint.clone(), None, false);
    let mut appeared = input(&fingerprint);
    appeared.cursor = Some(cursor());
    assert_eq!(
        gate.plan(&appeared),
        FramePlan::Rows {
            changed: RowSet::from_rows(&[1], 4).expect("row in range"),
            repaint: RowSet::from_rows(&[0, 1, 2], 4).expect("rows in range"),
        },
        "a shown cursor redraws its row"
    );

    // A restyle is a change too: same row, same plan.
    let mut gate = FrameGate::new();
    gate.record(fingerprint.clone(), Some(cursor()), false);
    let mut restyled = input(&fingerprint);
    restyled.cursor = Some(Cursor {
        style: CursorStyle::Bar,
        ..cursor()
    });
    assert_eq!(
        gate.plan(&restyled),
        FramePlan::Rows {
            changed: RowSet::from_rows(&[1], 4).expect("row in range"),
            repaint: RowSet::from_rows(&[0, 1, 2], 4).expect("rows in range"),
        },
        "a restyled cursor redraws its row"
    );

    // A blink toggles the drawn cursor without any row flag.
    let mut gate = FrameGate::new();
    gate.record(fingerprint.clone(), Some(cursor()), false);
    let mut blink_off = input(&fingerprint);
    blink_off.cursor = None;
    assert_eq!(
        gate.plan(&blink_off),
        FramePlan::Rows {
            changed: RowSet::from_rows(&[1], 4).expect("row in range"),
            repaint: RowSet::from_rows(&[0, 1, 2], 4).expect("rows in range"),
        },
        "a blinking-off cursor redraws its row"
    );

    // A cursor move with row flags adds both cursor rows to the flags'.
    let mut gate = FrameGate::new();
    let mut moved = cursor();
    moved.y = 3;
    gate.record(fingerprint.clone(), Some(moved), false);
    let mut partial = input(&fingerprint);
    partial.dirty = FrameDirty::Partial;
    partial.dirty_rows = vec![0];
    partial.cursor = Some(cursor());
    let expected = RowSet::from_rows(&[0, 1, 3], 4).expect("rows in range");
    let FramePlan::Rows { changed, repaint } = gate.plan(&partial) else {
        panic!("a partial frame with a cursor move is Rows");
    };
    assert_eq!(changed, expected, "the flags' row and the cursor's rows");
    assert_eq!(repaint, expected.expanded(4));
}

/// A dirty state whose rows are all out of range is not a state a partial
/// repaint can express: everything.
#[test]
fn a_partial_frame_without_valid_rows_is_everything() {
    let mut gate = FrameGate::new();
    let fingerprint = fingerprint();
    gate.record(fingerprint.clone(), None, false);
    let mut partial = input(&fingerprint);
    partial.dirty = FrameDirty::Partial;
    partial.dirty_rows = vec![9, 10];
    assert_eq!(gate.plan(&partial), FramePlan::Everything);
}

/// The row set: clamped, sorted, deduplicated, empty refused.
#[test]
fn a_row_set_clamps_sorts_and_dedups() {
    assert!(RowSet::from_rows(&[], 4).is_none());
    assert!(RowSet::from_rows(&[4, 5], 4).is_none(), "out of range only");
    assert!(RowSet::from_rows(&[-1], 4).is_none());
    let set = RowSet::from_rows(&[3, 0, 3, 1], 4).expect("rows in range");
    assert_eq!(set.rows(), &[0, 1, 3]);
    assert!(set.contains(3));
    assert!(!set.contains(2));
    assert_eq!(set.expanded(4).rows(), &[0, 1, 2, 3]);
    assert_eq!(
        RowSet::from_rows(&[1], 4)
            .expect("in range")
            .expanded(4)
            .rows(),
        &[0, 1, 2]
    );
}

// ---------------------------------------------------------------------------
// The pixel path

/// A sink with nowhere to write (the tests never write to a pty).
struct NullSink;
impl crate::term::PtySink for NullSink {
    fn write_pty(&mut self, _data: &[u8]) {}
}

/// Decodes nothing; these tests place no kitty images.
struct NoDecoder;
impl crate::term::PngDecoder for NoDecoder {
    fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
        None
    }
}

/// An 8-column, 4-row terminal at the 8x16 cell pitch the other render
/// tests use: a 64x64 logical frame.
fn terminal() -> Terminal {
    let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
    assert!(terminal.push_size(8, 4, 8, 16));
    terminal
}

/// Frame input for a 64x64 logical frame at `scale`.
fn frame(focused: bool, accent: Option<Accent>, draw_offset: f64, scale: f64) -> FrameInput {
    let device = device_px(f64::from(64) * scale);
    let device = u32::try_from(device).expect("frame fits u32");
    FrameInput::new(64, 64, device, device, draw_offset, focused, THEME, accent)
}

/// The test theme, distinct from every colour the tests draw.
const THEME: ThemeColours = ThemeColours {
    background: [10, 20, 30],
    foreground: [200, 150, 100],
};

/// Metrics for the 8x16 pitch at `scale`.
fn metrics(scale: f64) -> PainterMetrics {
    PainterMetrics::new(8.0, 16.0, 12.0, scale).expect("test metrics are valid")
}

/// The row-4.8 verify: a frame with no change draws nothing, touches no
/// pixel, and reports that there is nothing to commit.
#[test]
fn a_frame_with_no_change_draws_and_commits_nothing() {
    let Some(mut test) = test_support::text_pass() else {
        return;
    };
    let mut terminal = terminal();
    let frame = frame(false, None, 0.0, 1.0);
    let (w, h) = frame.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    let mut images = ImagePass::new();
    let mut gate = FrameGate::new();

    // The first frame establishes the canvas.
    let outcome = paint_frame_gated(
        &mut canvas,
        &metrics(1.0),
        &frame,
        &mut terminal,
        &mut test.pass,
        &mut images,
        &mut gate,
        false,
    );
    assert_ne!(outcome, FrameOutcome::Clean, "the first frame draws");

    // The next frame, with no pty traffic and no input change: nothing to
    // draw, nothing to commit, and the canvas byte-identical.
    let before = canvas.data().to_vec();
    let outcome = paint_frame_gated(
        &mut canvas,
        &metrics(1.0),
        &frame,
        &mut terminal,
        &mut test.pass,
        &mut images,
        &mut gate,
        false,
    );
    assert_eq!(outcome, FrameOutcome::Clean, "nothing to commit");
    assert_eq!(canvas.data(), before.as_slice(), "the canvas is untouched");
}

/// A frame that cannot be opened keeps the degraded draw — background and
/// accent — reports full damage, and resets the gate.
#[test]
fn a_frame_that_cannot_open_degrades_and_reports_full_damage() {
    let Some(mut test) = test_support::text_pass() else {
        return;
    };
    let accent = Accent::new([255, 0, 0], NonZeroU16::new(2).expect("nonzero"));
    let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
    let frame = frame(true, Some(accent), 0.0, 1.0);
    let (w, h) = frame.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    let mut images = ImagePass::new();
    let mut gate = FrameGate::new();
    let outcome = paint_frame_gated(
        &mut canvas,
        &metrics(1.0),
        &frame,
        &mut terminal,
        &mut test.pass,
        &mut images,
        &mut gate,
        false,
    );
    assert_eq!(
        outcome,
        FrameOutcome::Damage(Damage::whole_surface(w, h)),
        "the degraded draw damages the whole surface"
    );
    assert_eq!(
        canvas.pixel(0, 0),
        Some([0, 0, 255, 255]),
        "the accent drew"
    );
    assert_eq!(
        canvas.pixel(32, 32),
        Some([30, 20, 10, 255]),
        "the theme background"
    );
}

/// The partial path paints the same pixels as the always-full path: the
/// same sequence of frames — a first frame, typed text, a cursor move, a
/// frame with no change, a scroll, a one-row change, a cursor move back,
/// a focus change — painted twice, once always-full and once through the
/// gate, with the canvases compared after every step.
#[test]
fn the_gated_path_matches_the_full_path_pixel_for_pixel() {
    let Some(mut full_test) = test_support::text_pass() else {
        return;
    };
    let Some(mut gated_test) = test_support::text_pass() else {
        return;
    };
    let mut full_terminal = terminal();
    let mut gated_terminal = terminal();
    let (w, h) = frame(false, None, 0.0, 1.0).device_size();
    let mut full_canvas = Canvas::new(w, h).expect("canvas size is valid");
    let mut gated_canvas = Canvas::new(w, h).expect("canvas size is valid");
    let mut full_images = ImagePass::new();
    let mut gated_images = ImagePass::new();
    let mut gate = FrameGate::new();

    // (bytes, focused) per step.
    let steps: &[(&[&[u8]], bool)] = &[
        (&[b"\x1b[?25l"], false),               // the first frame
        (&[b"hello"], false),                   // typed text
        (&[b"\x1b[2;3H"], false),               // the cursor moves
        (&[], false),                           // no change: the gate must skip
        (&[b"\x1b[1S"], false),                 // a scroll: the pin reports FULL
        (&[b"\x1b[4;2H", b"\x1b[44mX"], false), // one row changes
        (&[b"\x1b[3;1H"], false),               // the cursor moves back
        (&[], true),                            // the focus change: everything
    ];

    for (number, (bytes, focused)) in steps.iter().enumerate() {
        for bytes in *bytes {
            full_terminal.push_pty_data(bytes);
            gated_terminal.push_pty_data(bytes);
        }
        let frame_input = frame(*focused, None, 0.0, 1.0);

        // Always-full: the plain painter, into the same canvas every step.
        paint_frame(
            &mut full_canvas,
            &metrics(1.0),
            &frame_input,
            &mut full_terminal,
            &mut full_test.pass,
            &mut full_images,
        );

        // Through the gate.
        let outcome = paint_frame_gated(
            &mut gated_canvas,
            &metrics(1.0),
            &frame_input,
            &mut gated_terminal,
            &mut gated_test.pass,
            &mut gated_images,
            &mut gate,
            false,
        );

        // The pinned decisions: the first frame is everything, the
        // no-change step skips, the scroll is everything (the pin says
        // FULL), the focus change is everything.
        match number {
            0 => assert_eq!(
                outcome,
                FrameOutcome::Damage(Damage::whole_surface(w, h)),
                "step {number}: the first frame is everything"
            ),
            3 => assert_eq!(
                outcome,
                FrameOutcome::Clean,
                "step {number}: no change, no commit"
            ),
            4 | 7 => assert_eq!(
                outcome,
                FrameOutcome::Damage(Damage::whole_surface(w, h)),
                "step {number}: everything"
            ),
            _ => assert_ne!(
                outcome,
                FrameOutcome::Clean,
                "step {number}: something drew"
            ),
        }

        assert_eq!(
            gated_canvas.data(),
            full_canvas.data(),
            "step {number}: the gated canvas matches the full canvas"
        );
    }
}
