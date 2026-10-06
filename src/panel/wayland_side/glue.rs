//! The panel thread's glue (replace-gtk-with-wayland row 8.1): the pieces
//! the thread builds for itself before the surfaces bind — the font setup
//! it measures its cell with, the renderer the frames and the tween's wide
//! draw read, the shared terminal and the repaint flag its callbacks latch,
//! and the pty the terminal's writer comes from. The thread assembles them
//! in [`super::run_thread`], so the sizing and the surfaces meet the
//! measured cell from the first configure (D3).
//!
//! Panics never cross back into calloop or the compositor (D5): the pieces
//! here are constructors whose failure paths return errors instead of
//! panicking, and the closure they hand out only sets a flag — the terminal
//! wraps its own callbacks in the shared guard.

use std::cell::{Cell, RefCell};
use std::os::fd::RawFd;
use std::rc::Rc;

use crate::guard::Poisoned;
use crate::layout::{Accent, CellSize};
use crate::pty::Pty;
use crate::render::png::PngCrateDecoder;
use crate::term::Terminal;

use super::super::handshake::StartOutcome;
use super::renderer::{FontSetup, FontSetupError, Renderer};

/// The font the thread starts with (row 8.1): [`FontSetup::load`] — the one
/// font load the panel performs, on the thread that owns the text pass —
/// mapped onto the start handshake.
///
/// # Errors
/// [`StartOutcome::Internal`] when the font cannot be resolved at all; the
/// `monospace` fallback is normal operation, not this error.
pub(crate) fn font_start() -> Result<FontSetup, StartOutcome> {
    font_outcome(FontSetup::load())
}

/// The mapping the start handshake sees for the font load: an unresolvable
/// font is not the spec's "no display" case — the display may be fine — so
/// the start fails through the internal path, the same class a caught panic
/// reports (D5). A function of its input so the tests drive the mapping
/// through the broken-`FontBook` seam (`FontBook::fail_all_loads`).
pub(crate) fn font_outcome(
    setup: Result<FontSetup, FontSetupError>,
) -> Result<FontSetup, StartOutcome> {
    match setup {
        Ok(setup) => Ok(setup),
        // The error's class is the decision, not its text: the handshake
        // reports the outcome, and the cause stays in the load's error
        // chain for the log.
        Err(_error) => Err(StartOutcome::Internal),
    }
}

/// The thread's renderer (row 8.1): [`Renderer::new`] over the font setup
/// the thread measured, with the Ghostty theme and the startup accent, at
/// scale 1 — the resolved scale until the compositor reports a preferred
/// one, which [`super::state::PanelState::sync_renderer_scale`] fixes after
/// the bind and on every scale change. Wrapped in an `Rc<RefCell<_>>` so
/// the tween's render bundle and the live frames borrow it without owning
/// it.
///
/// # Errors
/// [`StartOutcome::Internal`] when the scale yields no painter metrics —
/// unreachable for a measured cell's positive pitch, handled rather than
/// unwrapped (D5).
pub(crate) fn thread_renderer(
    accent: Option<Accent>,
    setup: FontSetup,
) -> Result<Rc<RefCell<Renderer>>, StartOutcome> {
    let theme = crate::fontconfig::load_theme_colours();
    Renderer::new(setup, 1.0, theme, accent)
        .map(|renderer| Rc::new(RefCell::new(renderer)))
        .ok_or(StartOutcome::Internal)
}

/// The thread's byte path (row 8.1): the shared terminal, the repaint flag
/// its callbacks latch, and the pty the writer and the fd slot come from.
/// The terminal is shared as an `Rc<RefCell<_>>` because the seat links and
/// the tween's render bundle hold clones, and the pty read source feeds it
/// from the same thread.
///
/// The `queue_draw` closure runs inside the terminal's callbacks: it sets
/// the repaint flag, which the loop reads and clears after each dispatch —
/// the draw step turns the flag into a frame.
pub(crate) fn byte_path(
    poisoned: Poisoned,
    fd: RawFd,
    cell: CellSize,
) -> (Rc<RefCell<Terminal>>, Rc<Cell<bool>>, Pty) {
    // The pty winsize's pixel fields (the glib paths' input): a measured
    // cell is positive, so the conversion cannot fail; the fallback keeps
    // the fields positive rather than panicking (D5).
    let cell_w = u32::try_from(cell.width().get()).unwrap_or(1);
    let cell_h = u32::try_from(cell.height().get()).unwrap_or(1);
    let pty = Pty::new(poisoned.clone(), fd, cell_w, cell_h);
    let repaint = Rc::new(Cell::new(false));
    let terminal = Rc::new(RefCell::new(Terminal::new(
        poisoned,
        pty.writer(),
        PngCrateDecoder,
        {
            let repaint = Rc::clone(&repaint);
            move || repaint.set(true)
        },
    )));
    (terminal, repaint, pty)
}

#[cfg(test)]
mod tests {
    use super::super::sizing::Grid;
    use super::super::state::PanelState;
    use super::super::{Inner, Startup};
    use super::*;
    use crate::fontconfig::FontConfig;
    use crate::layout::{Keyboard, Layout, Side};
    use crate::panel::PinwinError;
    use crate::panel::handshake::{Handshake, map_start};
    use crate::render::font::FontBook;
    use crate::render::text_pass::test_support;
    use std::num::NonZeroU16;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    /// A startup for the tests; the thread does not touch the pty fd in
    /// these tests, so a placeholder fd is fine here.
    fn startup() -> Startup {
        Startup {
            fd: -1,
            layout: Layout::new(
                Side::Left,
                NonZeroU16::new(40).expect("test columns"),
                0,
                0,
                0,
                0,
            ),
            keyboard: Keyboard::OnDemand,
            accent: None,
        }
    }

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<Inner> {
        Arc::new(Inner {
            id: 0,
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        })
    }

    /// The test family's font setup, or `None` (printed) when the machine
    /// lacks it — the same skip the renderer tests apply; a broken
    /// `FontBook` or a failed lookup is an `expect`, so a regression cannot
    /// hide behind "not installed".
    fn font_setup() -> Option<FontSetup> {
        let book = FontBook::new().expect("the font book opens");
        if !book.has_family(test_support::FAMILY) {
            println!("skipped: {} is not installed", test_support::FAMILY);
            return None;
        }
        Some(FontSetup::resolve_over(book, &test_config()).expect("the family resolves"))
    }

    /// The config the tests resolve: the test family at the test size.
    fn test_config() -> FontConfig {
        FontConfig {
            family: Some(test_support::FAMILY.to_owned()),
            size: test_support::SIZE,
        }
    }

    /// The cell the thread measures is the one the sizing derives from: a
    /// state built from the measured cell pushes the startup grid at that
    /// pitch, the same grid the surfaces' width comes from (D3).
    #[test]
    fn the_thread_measures_the_cell_the_sizing_uses() {
        let Some(setup) = font_setup() else {
            return;
        };
        let cell = setup.cell().expect("the measured cell");
        let mut state = PanelState::headless(
            Handshake::new(mpsc::channel().0),
            Poisoned::new(),
            live_inner(),
            startup(),
            cell,
        );
        let mut pushed: Vec<Grid> = Vec::new();
        state.configure_grid(1080, &mut |grid| pushed.push(grid));
        assert_eq!(pushed.len(), 1, "the configure pushes the startup grid");
        assert_eq!(pushed[0].cols(), 40, "the columns come from the layout");
        assert_eq!(
            (pushed[0].cell_width(), pushed[0].cell_height()),
            (cell.width().get(), cell.height().get()),
            "the measured pitch is the grid's pitch"
        );
    }

    /// The renderer the thread builds runs at the measured cell, so the
    /// frames and the tween's wide draw read the same pitch the sizing
    /// pushes.
    #[test]
    fn the_thread_builds_its_renderer_from_the_measured_font() {
        let Some(setup) = font_setup() else {
            return;
        };
        let cell = setup.cell().expect("the measured cell");
        let renderer = thread_renderer(None, setup).expect("the renderer builds");
        assert_eq!(
            renderer.borrow().cell(),
            Some(cell),
            "the renderer runs at the measured cell"
        );
    }

    /// A start whose font cannot be resolved at all — every face load
    /// fails, through the broken-`FontBook` seam the renderer tests use —
    /// fails the start as `Internal` through the handshake mapping, never
    /// as `NoDisplay` and never as a panic.
    #[test]
    fn an_unresolvable_font_fails_the_start_as_internal() {
        let Ok(mut book) = FontBook::new() else {
            println!("skipped: fontconfig is unavailable");
            return;
        };
        // Test seam (D10): every face load fails until the book is dropped.
        book.fail_all_loads();
        let error = FontSetup::resolve_over(book, &test_config()).expect_err("no face loads");
        assert!(
            matches!(font_outcome(Err(error)), Err(StartOutcome::Internal)),
            "the font failure maps onto the internal outcome"
        );
        // And the outcome the host waits on is the internal failure.
        assert_eq!(
            map_start(StartOutcome::Internal),
            Err(PinwinError::Internal)
        );
    }

    /// The byte path shares one repaint flag between the terminal's
    /// callbacks and the state: a resize alone latches nothing, and the
    /// terminal's output — the `queue_draw` closure — latches the flag the
    /// loop later reads and clears.
    #[test]
    fn the_byte_path_shares_one_flag_between_the_terminal_and_the_state() {
        let cell = CellSize::new(9, 16).expect("test cell size is non-zero");
        let (terminal, repaint, _pty) = byte_path(Poisoned::new(), -1, cell);
        assert!(
            terminal.borrow_mut().push_size(8, 4, 9, 16),
            "the test grid pushes"
        );
        assert!(!repaint.get(), "nothing fed yet, no repaint asked");

        terminal.borrow_mut().push_pty_data(b"hi");
        assert!(
            repaint.get(),
            "the terminal's output latched the repaint flag"
        );
    }
}
