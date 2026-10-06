//! Display-free pixel tests for the text pass's placement (the spec
//! requirement "Cell text on the device pixel lattice", replace-gtk-with-
//! wayland D10): one test per scenario. The draw rules — colour, layer
//! order, the cursor's glyph redraw, the tween offset — live in
//! `tests/draw_tests.rs`. Real system fonts, real shaping, no display.

use crate::render::canvas::Canvas;
use crate::render::painter::paint_frame;
use crate::render::text_pass::test_support::{
    HIDE_CURSOR, Rig, block, cell_at, common_size, px, theme_bytes, utf8,
};
use crate::render::text_pass::text_owns;
use crate::term::cells::{Cell, StyleFlags, Wide};

mod draw_tests;

// ---- The scenarios of "Cell text on the device pixel lattice" ----

/// Identical letters render alike at a fractional scale: a row of the same
/// letter at 1.8 draws the same device pixels out of every cell's snapped
/// top-left corner.
#[test]
fn identical_letters_render_alike_at_a_fractional_scale() {
    let Some(rig) = Rig::new() else { return };
    let scale = 1.8;
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"AAAAAAAA");
    let canvas = rig.painted(&mut terminal, scale, rig.cell_h, 0.0);
    let m = rig.metrics(scale, rig.cell_h);
    let rects: Vec<crate::render::geom::DeviceRect> =
        (0..8).map(|col| m.cell_rect(col, 0)).collect();
    let (w, h) = common_size(&rects);
    let first = block(&canvas, rects[0], w, h);
    assert!(
        first.iter().any(|pixel| *pixel != theme_bytes()),
        "the glyph has ink, so the comparison is not vacuous"
    );
    for (col, rect) in rects.iter().enumerate().skip(1) {
        assert_eq!(
            block(&canvas, *rect, w, h),
            first,
            "column {col} renders column 0's pixels at {scale}"
        );
    }
}

/// Cell height that is not a whole device pixel count: 19 logical pixels
/// at 1.8 — the glyph sits at the same device pixel offset from the top of
/// its cell in every row.
#[test]
fn the_glyph_sits_at_the_same_offset_in_every_row_of_a_19_pixel_cell() {
    let Some(rig) = Rig::new() else { return };
    let scale = 1.8;
    let cell_h = 19;
    let mut terminal = rig.terminal(cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    let canvas = rig.painted(&mut terminal, scale, cell_h, 0.0);
    let m = rig.metrics(scale, cell_h);
    let rects: Vec<crate::render::geom::DeviceRect> =
        (0..4).map(|row| m.cell_rect(0, row)).collect();
    let (w, h) = common_size(&rects);
    let first = block(&canvas, rects[0], w, h);
    assert!(
        first.iter().any(|pixel| *pixel != theme_bytes()),
        "the glyph has ink, so the comparison is not vacuous"
    );
    for (row, rect) in rects.iter().enumerate().skip(1) {
        assert_eq!(
            block(&canvas, *rect, w, h),
            first,
            "row {row} renders row 0's pixels at {scale}"
        );
    }
}

/// Constrained glyph: the nerd-font icon the constraints scale and centre
/// sits at the same device pixel offset inside each cell at 1.5, at
/// different columns.
#[test]
fn the_constrained_glyph_sits_alike_in_every_column_at_1_5() {
    let Some(rig) = Rig::new() else { return };
    let scale = 1.5;
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    let icon: Vec<u8> = (0..8).flat_map(|_| utf8(0xEA61)).collect();
    terminal.push_pty_data(&icon);
    let walked = crate::render::text_pass::test_support::cells(&mut terminal);
    let icon_cell = walked
        .iter()
        .find(|cell| cell.len > 0)
        .expect("an icon cell");
    assert_eq!(icon_cell.len, 3, "the icon is one cell's cluster");
    let canvas = rig.painted(&mut terminal, scale, rig.cell_h, 0.0);
    let m = rig.metrics(scale, rig.cell_h);
    let rects: Vec<crate::render::geom::DeviceRect> =
        (0..8).map(|col| m.cell_rect(col, 0)).collect();
    let (w, h) = common_size(&rects);
    let first = block(&canvas, rects[0], w, h);
    assert!(
        first.iter().any(|pixel| *pixel != theme_bytes()),
        "the constrained icon has ink, so the comparison is not vacuous"
    );
    for (col, rect) in rects.iter().enumerate().skip(1) {
        assert_eq!(
            block(&canvas, *rect, w, h),
            first,
            "column {col} renders column 0's constrained pixels at {scale}"
        );
    }
}

/// Text sizes stay logical: the cell size from `cell_metrics` and the
/// `PainterMetrics` logical cell at 1.8 equal their values at 1. The reply
/// to `CSI 16 t` is pinned by the row 4.6 cell-metrics test; this pins the
/// painter's inputs.
#[test]
fn text_sizes_stay_logical() {
    let Some(rig) = Rig::new() else { return };
    let (cell_w, cell_h) = rig.test.cell_metrics.cell_size();
    for scale in [1.0, 1.8] {
        let m = rig
            .test
            .cell_metrics
            .painter_metrics(scale)
            .expect("the metrics are valid");
        assert_eq!(m.cell_w(), cell_w, "the logical cell width at {scale}");
        assert_eq!(m.cell_h(), cell_h, "the logical cell height at {scale}");
        assert_eq!(
            m.ascent(),
            f64::from(rig.test.cell_metrics.ascent()),
            "the logical ascent at {scale}"
        );
    }
}

// ---- Plausibility and the pure ownership ----

/// At scale 1 the output is plausible: a letter has ink inside its cell
/// and the pixels outside the glyph stay the background.
#[test]
fn a_letter_has_ink_inside_its_cell_at_scale_1() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"M");
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    let cell = m.cell_rect(0, 0);
    let ink = (cell.y()..cell.y() + cell.h())
        .flat_map(move |y| (cell.x()..cell.x() + cell.w()).map(move |x| (x, y)))
        .filter(|(x, y)| canvas.pixel(px(*x), px(*y)).expect("inside") != theme_bytes())
        .count();
    assert!(ink > 0, "the letter drew ink inside its cell");
    assert!(
        ink < usize::try_from(cell.w() * cell.h()).expect("fits"),
        "the glyph does not fill the cell"
    );
    // The pixels outside the glyph — the cell's top corners, far from any
    // stem — stay the background.
    for (x, y) in [
        (cell.x() + 1, cell.y() + 1),
        (cell.x() + cell.w() - 2, cell.y() + 1),
    ] {
        assert_eq!(
            canvas.pixel(px(x), px(y)),
            Some(theme_bytes()),
            "the corner at ({x}, {y}) stays background"
        );
    }
    // The next cell, which holds no glyph, stays bare background.
    let next = m.cell_rect(1, 0);
    assert_eq!(
        canvas.pixel(px(next.x()), px(next.y())),
        Some(theme_bytes()),
        "the empty cell stays background"
    );
}

/// An INVISIBLE cell draws nothing. The pinned vt does not map SGR 8
/// (conceal) through the frame protocol — probed, as the sprite tests
/// probed it — so the flag reaches this test through a hand-built cell;
/// [`text_owns`] is the pure per-cell decision `TextPass::paint` skips by,
/// and it rejects exactly the skips the sprite pass rejects.
#[test]
fn an_invisible_cell_is_not_owned_by_the_text_pass() {
    let Some(rig) = Rig::new() else { return };
    let m = rig.metrics(1.0, rig.cell_h);
    let frame = rig.frame(1.0, rig.cell_h, 0.0);
    let invisible = Cell {
        flags: StyleFlags::INVISIBLE,
        ..cell_at(0, u32::from(b'A'))
    };
    assert!(
        !text_owns(&m, &frame, &invisible),
        "an INVISIBLE cell draws no text"
    );
    // The other skips hold too, and a plain letter is owned.
    assert!(
        !text_owns(&m, &frame, &Cell::default()),
        "no glyph, no text"
    );
    let tail = Cell {
        wide: Wide::SpacerTail,
        ..cell_at(1, u32::from(b'A'))
    };
    assert!(!text_owns(&m, &frame, &tail), "a spacer tail draws no text");
    assert!(text_owns(&m, &frame, &cell_at(0, u32::from(b'A'))));
}

/// The pure `paint_frame` entry keeps compiling for the pixel tests: a
/// canvas the size of a frame accepts the painter (the scenarios above go
/// through [`paint_frame`] via the rig).
#[test]
fn paint_frame_is_the_rigs_entry() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    let frame = rig.frame(1.0, rig.cell_h, 0.0);
    let (w, h) = frame.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    let mut pass = rig.pass();
    paint_frame(
        &mut canvas,
        &rig.metrics(1.0, rig.cell_h),
        &frame,
        &mut terminal,
        &mut pass,
    );
    assert_eq!(canvas.size(), (w, h));
}
