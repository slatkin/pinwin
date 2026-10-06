//! Display-free tests for the panel renderer (replace-gtk-with-wayland
//! D10): the cell the font setup measures, the error a font that cannot
//! resolve at all returns, and the renderer's frame draws — sizes, the
//! gate's outcomes, the damage of a changed row, the scale and focus
//! changes and the explicit invalidate — without a compositor.

use std::error::Error as _;
use std::num::NonZeroU16;

use super::{FontSetup, FontSetupError, Renderer};
use crate::fontconfig::{FontConfig, ThemeColours};
use crate::guard::Poisoned;
use crate::layout::{Accent, CellSize};
use crate::render::font::FontBook;
use crate::render::frame_gate::{Damage, FrameOutcome};
use crate::render::geom::{DeviceRect, FrameInput, device_px};
use crate::term::{PngDecoder, PtySink, Terminal};

/// The family the tests pin; CI installs it (design D10). The same family
/// the painter and font tests pin.
const FAMILY: &str = "JetBrainsMono Nerd Font";
/// The size in points the tests run at: the Ghostty default 11.
const SIZE: f64 = 11.0;

/// The test theme, distinct from every colour the tests draw.
const THEME: ThemeColours = ThemeColours {
    background: [10, 20, 30],
    foreground: [200, 150, 100],
};

/// A sink with nowhere to write (the tests never write to a pty).
struct NullSink;
impl PtySink for NullSink {
    fn write_pty(&mut self, _data: &[u8]) {}
}

/// Decodes nothing; these tests place no kitty images.
struct NoDecoder;
impl PngDecoder for NoDecoder {
    fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
        None
    }
}

/// The test family's font setup, or `None` (printed) when the machine
/// lacks it — the font-dependent tests skip with a printed message only
/// then; a broken `FontBook` or a failed lookup is an `expect`, so a
/// regression cannot hide behind "not installed".
fn setup() -> Option<FontSetup> {
    let book = FontBook::new().expect("the font book opens");
    if !book.has_family(FAMILY) {
        println!("skipped: {FAMILY} is not installed");
        return None;
    }
    Some(FontSetup::resolve_over(book, &config()).expect("the family resolves"))
}

/// The config the tests resolve: the test family at the test size.
fn config() -> FontConfig {
    FontConfig {
        family: Some(FAMILY.to_owned()),
        size: SIZE,
    }
}

/// The test setup's cell as the plain `(width, height)` pair.
fn cell_of(setup: &FontSetup) -> (i32, i32) {
    let cell = setup.cell().expect("the cell fits");
    (cell.width().get(), cell.height().get())
}

/// A renderer over `setup`'s font at `scale`, with the test theme.
fn renderer(setup: FontSetup, scale: f64, accent: Option<Accent>) -> Renderer {
    Renderer::new(setup, scale, THEME, accent).expect("the renderer builds")
}

/// An 8-column, 4-row terminal at the `cell` pitch, with a line of text
/// pushed into its first row.
fn terminal(cell: (i32, i32)) -> Terminal {
    let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
    assert!(
        terminal.push_size(8, 4, cell.0, cell.1),
        "the test grid pushes"
    );
    terminal
}

/// Frame input for the 8-column, 4-row frame at `cell` and `scale`.
fn frame(cell: (i32, i32), scale: f64, focused: bool, accent: Option<Accent>) -> FrameInput {
    let (cw, ch) = cell;
    let (lw, lh) = (8 * cw, 4 * ch);
    let device_w = u32::try_from(device_px(f64::from(lw) * scale)).expect("frame fits u32");
    let device_h = u32::try_from(device_px(f64::from(lh) * scale)).expect("frame fits u32");
    FrameInput::new(
        u32::try_from(lw).expect("logical fits u32"),
        u32::try_from(lh).expect("logical fits u32"),
        device_w,
        device_h,
        0.0,
        focused,
        THEME,
        accent,
    )
}

/// A colour's bytes in the canvas' memory order: blue, green, red, alpha.
fn bytes(rgb: [u8; 3]) -> [u8; 4] {
    [rgb[2], rgb[1], rgb[0], 255]
}

/// The theme background's canvas bytes.
fn theme_bytes() -> [u8; 4] {
    bytes(THEME.background)
}

/// The cell the test family measures is the row 4.6 gate's cell — the
/// same 9 by 20 the old Pango metrics give, which sets the panel width.
/// The Pango comparison itself is not duplicated here; the gate test in
/// `src/render/cell_metrics` pins it.
#[test]
fn the_test_family_measures_the_expected_cell() {
    let Some(setup) = setup() else {
        return;
    };
    assert_eq!(
        setup.cell(),
        CellSize::new(9, 20),
        "the cell matches the row 4.6 gate"
    );
}

/// A font that cannot be resolved at all — every face load fails, so not
/// even `monospace` comes out — is the font error, never a panic, and the
/// error chain carries the cause.
#[test]
fn an_unresolvable_font_setup_is_an_error() {
    let Ok(mut book) = FontBook::new() else {
        println!("skipped: fontconfig is unavailable");
        return;
    };
    // Test seam (D10): every face load fails until the book is dropped.
    book.fail_all_loads();
    let error = FontSetup::resolve_over(book, &config()).expect_err("no face loads");
    assert!(
        matches!(error, FontSetupError::Font(_)),
        "the font lookup is what failed: {error}"
    );
    assert!(!error.to_string().is_empty(), "the error displays");
    assert!(error.source().is_some(), "the cause is carried");
}

/// A renderer draws a frame of a fed terminal into its canvas at the
/// scales the pixel tests run at: the canvas is the frame's device size,
/// and the text's ink appears over the theme background.
#[test]
fn a_renderer_draws_a_fed_frame_into_its_canvas() {
    for scale in [1.0, 1.5] {
        let Some(setup) = setup() else {
            return;
        };
        let cell = cell_of(&setup);
        let mut panel = renderer(setup, scale, None);
        let mut term = terminal(cell);
        term.push_pty_data(b"AAAA");
        let test_frame = frame(cell, scale, false, None);
        let outcome = panel.draw(&mut term, &test_frame);
        assert!(
            matches!(outcome, FrameOutcome::Damage(_)),
            "the first frame draws at {scale}"
        );
        let canvas = panel.canvas().expect("the canvas exists");
        assert_eq!(
            canvas.size(),
            test_frame.device_size(),
            "the canvas is the device size at {scale}"
        );
        let theme = theme_bytes();
        let inked = (0..canvas.size().1)
            .any(|y| (0..canvas.size().0).any(|x| canvas.pixel(x, y) != Some(theme)));
        assert!(inked, "the frame drew ink at {scale}");
    }
}

/// A second identical draw of an unchanged terminal is `Clean` and leaves
/// the canvas byte-identical: nothing drew and nothing may be committed.
#[test]
fn a_second_identical_draw_is_clean_and_leaves_the_canvas_byte_identical() {
    let Some(setup) = setup() else {
        return;
    };
    let cell = cell_of(&setup);
    let mut panel = renderer(setup, 1.0, None);
    let mut term = terminal(cell);
    // The cursor hidden before the first frame, so the cursor's state
    // cannot differ between the two draws.
    term.push_pty_data(b"\x1b[?25lhello");
    let test_frame = frame(cell, 1.0, false, None);
    panel.draw(&mut term, &test_frame);
    let before = panel.canvas().expect("the canvas exists").data().to_vec();

    let second = panel.draw(&mut term, &test_frame);
    assert_eq!(second, FrameOutcome::Clean, "an unchanged frame is clean");
    assert_eq!(
        panel.canvas().expect("the canvas exists").data(),
        &*before,
        "the canvas is byte-identical"
    );
}

/// A typed row is a partial repaint: the gate reports the damage of that
/// row's full-width device band alone (the frame-gate test's pattern —
/// the cursor hidden and parked before the first frame, so the change is
/// the written row alone).
#[test]
fn a_typed_row_returns_the_damage_of_that_row() {
    let Some(setup) = setup() else {
        return;
    };
    let cell = cell_of(&setup);
    let mut panel = renderer(setup, 1.0, None);
    let mut term = terminal(cell);
    term.push_pty_data(b"\x1b[?25l\x1b[3;1H");
    let test_frame = frame(cell, 1.0, false, None);
    panel.draw(&mut term, &test_frame);

    // One row changes: the pty writes an `X` at the parked cursor, in the
    // third row.
    term.push_pty_data(b"X");
    let outcome = panel.draw(&mut term, &test_frame);
    let FrameOutcome::Damage(damage) = outcome else {
        panic!("a changed frame draws: {outcome:?}");
    };
    assert_eq!(damage.rects().len(), 1, "one changed row");
    assert_eq!(
        damage.rects()[0],
        DeviceRect::new(0, 2 * cell.1, 8 * cell.0, cell.1).expect("the changed row's band"),
        "the damage is the changed row's band alone"
    );
}

/// A scale change resizes the canvas to the new device size and forces a
/// full repaint: the gate's fingerprint carries the metrics the scale
/// changed, so the next frame is everything even though the terminal is
/// unchanged.
#[test]
fn a_scale_change_resizes_the_canvas_and_forces_a_full_repaint() {
    let Some(setup) = setup() else {
        return;
    };
    let cell = cell_of(&setup);
    let mut panel = renderer(setup, 1.0, None);
    let mut term = terminal(cell);
    term.push_pty_data(b"\x1b[?25lhi");
    let small = frame(cell, 1.0, false, None);
    panel.draw(&mut term, &small);
    assert_eq!(
        panel.draw(&mut term, &small),
        FrameOutcome::Clean,
        "the unchanged frame is clean before the scale change"
    );

    panel.set_scale(1.5);
    let large = frame(cell, 1.5, false, None);
    let (w, h) = large.device_size();
    assert_eq!(
        panel.draw(&mut term, &large),
        FrameOutcome::Damage(Damage::whole_surface(w, h)),
        "the scale change is a full repaint"
    );
    assert_eq!(
        panel.canvas().expect("the canvas exists").size(),
        (w, h),
        "the canvas resized to the new device size"
    );
}

/// Recording the focus flips the renderer's flag, and the focused frame
/// the flag feeds draws the accent where the unfocused one drew none.
#[test]
fn set_focused_changes_the_accent_pixels() {
    let Some(setup) = setup() else {
        return;
    };
    let cell = cell_of(&setup);
    let accent = Accent::new([255, 0, 0], NonZeroU16::new(2).expect("nonzero"));
    let mut panel = renderer(setup, 1.0, Some(accent));
    let mut term = terminal(cell);
    term.push_pty_data(b"\x1b[?25l");
    let unfocused = frame(cell, 1.0, false, Some(accent));
    panel.draw(&mut term, &unfocused);
    assert!(!panel.focused(), "the renderer starts unfocused");
    assert_eq!(
        panel.canvas().expect("the canvas exists").pixel(0, 0),
        Some(theme_bytes()),
        "no accent while unfocused"
    );

    panel.set_focused(true);
    assert!(panel.focused(), "the flag flipped");
    let focused = frame(cell, 1.0, true, Some(accent));
    panel.draw(&mut term, &focused);
    assert_eq!(
        panel.canvas().expect("the canvas exists").pixel(0, 0),
        Some([0, 0, 255, 255]),
        "the accent drew on the focused frame"
    );
}

/// An explicit invalidate forces the next frame to everything: after a
/// clean frame, an invalidated gate repaints the whole surface though the
/// terminal is unchanged.
#[test]
fn invalidate_forces_a_full_repaint() {
    let Some(setup) = setup() else {
        return;
    };
    let cell = cell_of(&setup);
    let mut panel = renderer(setup, 1.0, None);
    let mut term = terminal(cell);
    term.push_pty_data(b"\x1b[?25lhi");
    let test_frame = frame(cell, 1.0, false, None);
    panel.draw(&mut term, &test_frame);
    assert_eq!(panel.draw(&mut term, &test_frame), FrameOutcome::Clean);

    panel.invalidate();
    let (w, h) = test_frame.device_size();
    assert_eq!(
        panel.draw(&mut term, &test_frame),
        FrameOutcome::Damage(Damage::whole_surface(w, h)),
        "the invalidated frame is everything"
    );
}
