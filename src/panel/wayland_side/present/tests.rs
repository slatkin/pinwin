//! Display-free tests for the present step (replace-gtk-with-wayland D10):
//! the plan's commits, the frame input's device sizes and draw offset, the
//! output disposition, the retry rule and the finish's end-state actions.

use super::*;
use crate::guard::Poisoned;
use crate::panel::wayland_side::renderer::{FontSetup, Renderer};
use crate::render::frame_gate::Damage;
use crate::render::text_pass::test_support;
use crate::term::{PngDecoder, PtySink, Terminal};

use super::super::crop::upload_wide;

fn scale(units: u32) -> FractionalScale {
    FractionalScale::from_120ths(units)
}

/// A test theme distinct from the ink the pixel test draws.
fn theme() -> ThemeColours {
    ThemeColours {
        background: [10, 20, 30],
        foreground: [200, 150, 100],
    }
}

/// A sink with nowhere to write: the pixel test never writes to a pty.
struct NullSink;
impl PtySink for NullSink {
    fn write_pty(&mut self, _data: &[u8]) {}
}

/// Decodes nothing: the pixel test places no kitty images.
struct NoDecoder;
impl PngDecoder for NoDecoder {
    fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
        None
    }
}

/// A clean frame commits nothing at all: the gate drew nothing, so the
/// committed state is already current and no request may go out.
#[test]
fn a_clean_frame_commits_nothing() {
    assert_eq!(
        present_plan(&FrameOutcome::Clean, (100, 50), false),
        PresentPlan::Nothing
    );
    assert_eq!(
        present_plan(&FrameOutcome::Clean, (100, 50), true),
        PresentPlan::Nothing,
        "a clean frame commits nothing under a tween either"
    );
}

/// A damaged frame attaches its buffer with one damage per rectangle and
/// commits — unless a tween owns the commits, in which case nothing live is
/// presented.
#[test]
fn a_damaged_frame_plans_one_damage_per_rectangle_and_a_tween_plans_nothing() {
    let damage = Damage::from_rects(vec![
        DeviceRect::new(0, 0, 100, 16).expect("rect"),
        DeviceRect::new(0, 16, 100, 16).expect("rect"),
    ]);
    let outcome = FrameOutcome::Damage(damage);
    let PresentPlan::Frame(rects) = present_plan(&outcome, (100, 50), false) else {
        panic!("a damaged frame plans a present");
    };
    assert_eq!(rects.len(), 2, "one damage per rectangle");
    assert_eq!(
        present_plan(&outcome, (100, 50), true),
        PresentPlan::Nothing,
        "a tween owns the commits; nothing live is presented"
    );
}

/// A damage rectangle is clamped to the buffer it is damaged in: one that
/// lies past the buffer's edge drops, and one that overflows it shrinks to
/// the overlap.
#[test]
fn a_damage_rectangle_is_clamped_to_the_buffer() {
    let damage = Damage::from_rects(vec![
        DeviceRect::new(0, 0, 200, 16).expect("overflows the width"),
        DeviceRect::new(0, 40, 100, 16).expect("overflows the height"),
        DeviceRect::new(150, 0, 100, 16).expect("lies past the width"),
        DeviceRect::new(0, 0, 50, 16).expect("inside"),
    ]);
    let PresentPlan::Frame(rects) = present_plan(&FrameOutcome::Damage(damage), (100, 50), false)
    else {
        panic!("a damaged frame plans a present");
    };
    assert_eq!(
        rects,
        vec![
            DeviceRect::new(0, 0, 100, 16).expect("clamped width"),
            DeviceRect::new(0, 40, 100, 10).expect("clamped height"),
            DeviceRect::new(0, 0, 50, 16).expect("untouched"),
        ]
    );
}

/// The frame input's device size is the logical size times the resolved
/// scale, rounded, at the scales the tests pin.
#[test]
fn the_frame_input_is_the_device_size_at_the_tested_scales() {
    let theme = theme();
    for (units, expect_w, expect_h) in [
        (120, 360, 720),
        (150, 450, 900),
        (180, 540, 1080),
        (216, 648, 1296),
    ] {
        let input = frame_input(
            (360, 720),
            scale(units),
            Side::Left,
            360,
            0,
            false,
            theme,
            None,
        )
        .expect("the frame input builds");
        let (lw, lh) = input.logical_size();
        assert_eq!((lw, lh), (360, 720), "the logical size is the configure");
        assert_eq!(
            input.device_size(),
            (expect_w, expect_h),
            "the device size at {}",
            f64::from(units) / 120.0
        );
    }
}

/// A logical size no buffer could hold yields no frame input, and the
/// caller skips the frame.
#[test]
fn an_unbufferable_size_yields_no_frame_input() {
    let theme = theme();
    assert!(
        frame_input(
            (u32::MAX, 720),
            scale(150),
            Side::Left,
            0,
            0,
            false,
            theme,
            None
        )
        .is_none(),
        "the scaled width does not fit u32"
    );
}

/// The docked-edge draw offset keeps a widened grid's stale content glued
/// to the docked edge of a right-docked panel, reads the live grid once the
/// terminal produced output again, and shifts a left-docked panel by
/// nothing. The shift snaps to a whole device pixel at the frame's scale.
#[test]
fn the_draw_offset_keeps_the_widened_grid_at_the_docked_edge() {
    let scale_150 = 1.5;
    // A right-docked panel widened from 40 to 60 columns (360 to 540 px):
    // the stale 360 px content shifts by the surface width minus itself.
    assert_eq!(
        docked_edge_offset(Side::Right, 540, 540, 360, scale_150),
        f64::from(180),
        "the stale grid stays at the docked edge"
    );
    // Once the terminal produced output for the new width, the live grid
    // fills the surface and the shift is zero.
    assert_eq!(
        docked_edge_offset(Side::Right, 540, 540, 0, scale_150),
        0.0,
        "the live grid fills the surface"
    );
    // A narrowing leaves no stale record, so the live (narrower) grid
    // shifts by the whole difference.
    assert_eq!(
        docked_edge_offset(Side::Right, 540, 360, 0, scale_150),
        f64::from(180),
        "the narrower live grid shifts to the docked edge"
    );
    // A left-docked panel never shifts.
    assert_eq!(
        docked_edge_offset(Side::Left, 540, 360, 360, scale_150),
        0.0,
        "left docking needs no shift"
    );
    // A frame without a width yet shifts by nothing.
    assert_eq!(
        docked_edge_offset(Side::Right, 0, 360, 0, scale_150),
        0.0,
        "no width, no shift"
    );
    // The shift snaps to a whole device pixel: 189 px at 1.5 snaps to the
    // nearest 1/1.5 device pixel step, and the frame carries the snapped
    // value.
    let snapped = OutputScale::new(scale_150).snap_edge(f64::from(189));
    assert_eq!(
        docked_edge_offset(Side::Right, 550, 361, 0, scale_150),
        snapped,
        "the snapped value is what the frame carries"
    );
}

/// The frame input carries the offset the docked-edge rule computes.
#[test]
fn the_frame_input_carries_the_docked_edge_offset() {
    let theme = theme();
    let input = frame_input(
        (540, 720),
        scale(180),
        Side::Right,
        540,
        360,
        false,
        theme,
        None,
    )
    .expect("the frame input builds");
    assert_eq!(
        input.draw_offset(),
        f64::from(180),
        "the stale grid's shift"
    );
}

/// The output disposition: a request while a tween owns the commits marks
/// the holder stale, and one outside a tween draws a live frame.
#[test]
fn output_during_a_tween_marks_the_holder_stale_and_outside_it_draws_live() {
    assert_eq!(
        output_disposition(true),
        OutputDisposition::MarkHolderStale,
        "the wide cache is a snapshot; the next tween frame redraws it"
    );
    assert_eq!(
        output_disposition(false),
        OutputDisposition::DrawLiveFrame,
        "outside a tween the request is a live frame"
    );
}

/// The retry rule: only a present the pool refused latches the request
/// again; a drawn, clean or idle step consumed it, and no request stays
/// gone.
#[test]
fn the_retry_rule_latches_only_a_refused_present() {
    assert!(latch_after_service(true, true), "a busy present retries");
    assert!(
        !latch_after_service(true, false),
        "a served request is consumed"
    );
    assert!(
        !latch_after_service(false, true),
        "no request, nothing to latch"
    );
}

/// The finish's end state: with a viewporter the source crop is unset and
/// the destination moves to the final logical size; without one nothing is
/// pending; without a configure height only the source is unset.
#[test]
fn the_finish_end_state_unsets_the_crop_and_resets_the_destination() {
    assert_eq!(
        finish_end_state(true, Some(720), 1080),
        vec![
            FinishViewport::UnsetSource,
            FinishViewport::Destination(1080, 720)
        ],
        "the crop goes and the destination is the final logical size"
    );
    assert_eq!(
        finish_end_state(false, Some(720), 1080),
        Vec::new(),
        "without a viewporter no crop existed and nothing is pending"
    );
    assert_eq!(
        finish_end_state(true, None, 1080),
        vec![FinishViewport::UnsetSource],
        "no height yet leaves the destination at the last eased frame's"
    );
}

/// The full configure draw: the frame the input builds draws ink at the
/// device size into the renderer's canvas, and the copy of that canvas into
/// a fake slot's bytes — a `Vec` standing in for the pool slot — carries
/// the same inked pixels at the device size. Font-dependent: skipped, with
/// a printed message, where the test family is not installed.
#[test]
fn a_full_configure_draw_copies_inked_pixels_at_the_device_size() {
    let Ok(book) = crate::render::font::FontBook::new() else {
        println!("skipped: fontconfig is unavailable");
        return;
    };
    if !book.has_family(test_support::FAMILY) {
        println!("skipped: {} is not installed", test_support::FAMILY);
        return;
    }
    let config = crate::fontconfig::FontConfig {
        family: Some(test_support::FAMILY.to_owned()),
        size: test_support::SIZE,
    };
    let setup = FontSetup::resolve_over(book, &config).expect("the family resolves");
    let cell = setup.cell().expect("the cell fits");
    let scale_150 = scale(180);
    let logical = (
        u32::try_from(8 * cell.width().get()).expect("width"),
        u32::try_from(4 * cell.height().get()).expect("height"),
    );
    let theme = theme();
    let input = frame_input(logical, scale_150, Side::Left, 0, 0, false, theme, None)
        .expect("the frame input builds");

    let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
    assert!(
        terminal.push_size(8, 4, cell.width().get(), cell.height().get()),
        "the test grid pushes"
    );
    terminal.push_pty_data(b"AAAA");

    let mut renderer = Renderer::new(setup, 1.5, theme, None).expect("the renderer builds");
    assert!(
        matches!(
            renderer.draw(&mut terminal, &input),
            FrameOutcome::Damage(_)
        ),
        "the first configure frame draws"
    );
    let canvas = renderer.canvas().expect("the canvas exists");
    assert_eq!(
        canvas.size(),
        input.device_size(),
        "the canvas is the frame's device size"
    );

    // The fake slot: bytes the size of one device frame.
    let (device_w, device_h) = input.device_size();
    let bytes = usize::try_from(u64::from(device_w) * u64::from(device_h) * 4)
        .expect("the slot fits usize");
    let mut slot = vec![0_u8; bytes];
    upload_wide(canvas, &mut slot).expect("the canvas copies");
    let theme_bytes = [
        theme.background[2],
        theme.background[1],
        theme.background[0],
        255,
    ];
    let inked = (0..slot.len())
        .step_by(4)
        .any(|at| slot[at..at + 4] != theme_bytes);
    assert!(inked, "the copied slot carries the frame's ink");
}
