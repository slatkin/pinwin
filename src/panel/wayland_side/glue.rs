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
use super::sizing::Grid;
use super::state::{PanelState, apply_pty_size};

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
/// its callbacks latch, the stale pre-resize grid width its output clears,
/// and the pty the writer and the fd slot come from. The terminal is
/// shared as an `Rc<RefCell<_>>` because the seat links and the tween's
/// render bundle hold clones, and the pty read source feeds it from the
/// same thread.
///
/// The `queue_draw` closure runs inside the terminal's callbacks: it clears
/// the stale grid record — the terminal produced output, so the drawn grid
/// is the live one again (the GTK path's `note_terminal_output`) — and sets
/// the repaint flag, which the loop reads and clears after each dispatch —
/// the draw step turns the flag into a frame.
/// The byte path's return: the shared terminal, the repaint flag its
/// callbacks latch, the stale pre-resize record its output clears, and the
/// pty the writer and the fd slot come from.
pub(crate) type BytePath = (Rc<RefCell<Terminal>>, Rc<Cell<bool>>, Rc<Cell<i32>>, Pty);

pub(crate) fn byte_path(poisoned: Poisoned, fd: RawFd, cell: CellSize) -> BytePath {
    // The pty winsize's pixel fields (the glib paths' input): a measured
    // cell is positive, so the conversion cannot fail; the fallback keeps
    // the fields positive rather than panicking (D5).
    let cell_w = u32::try_from(cell.width().get()).unwrap_or(1);
    let cell_h = u32::try_from(cell.height().get()).unwrap_or(1);
    let pty = Pty::new(poisoned.clone(), fd, cell_w, cell_h);
    let repaint = Rc::new(Cell::new(false));
    let stale_grid_px = Rc::new(Cell::new(0));
    let terminal = Rc::new(RefCell::new(Terminal::new(
        poisoned,
        pty.writer(),
        PngCrateDecoder,
        {
            let repaint = Rc::clone(&repaint);
            let stale = Rc::clone(&stale_grid_px);
            move || {
                stale.set(0);
                repaint.set(true);
            }
        },
    )));
    (terminal, repaint, stale_grid_px, pty)
}

impl PanelState {
    /// The one grid push sink (row 8.1): every push the sizing decides — a
    /// plain apply's, a mid-tween configure's and the deferred tween-end's —
    /// runs through it, each exactly once (the sizing's pushed-grid memory
    /// never repeats one). The closure owns clones of the shared terminal
    /// and the repaint flag, so the callers hold `&mut self` while it runs.
    /// `None` on a headless state — the tests — where the sink degrades to
    /// the winsize-only push the pre-8.1 thread had.
    pub(crate) fn grid_sink(&self) -> impl FnMut(Grid) + 'static {
        let terminal = self.terminal.clone();
        let repaint = Rc::clone(&self.repaint);
        let stale = Rc::clone(&self.stale_grid_px);
        let fd = self.startup.fd;
        move |grid| push_grid(terminal.as_ref(), &repaint, stale.as_ref(), fd, grid)
    }
}

/// Push one derived grid to the terminal and the pty (row 8.1), the GTK
/// path's `apply_size` order: the terminal first — a terminal that cannot
/// be allocated leaves the previous grid and the pty winsize in place —
/// then the winsize with `SIGWINCH`, then the repaint request the
/// post-resize repaint needs. A widening push also records the previous
/// grid's pixel width as the stale pre-resize content still on screen (the
/// vt does not rewrap), the narrowest since the last terminal output; the
/// terminal's own output latches the same flag later and clears the
/// record, when the vt answers the new width.
fn push_grid(
    terminal: Option<&Rc<RefCell<Terminal>>>,
    repaint: &Cell<bool>,
    stale: &Cell<i32>,
    fd: RawFd,
    grid: Grid,
) {
    // Unreachable for a derived grid — the configure height bounds the rows
    // — handled: no push at all rather than a truncated one.
    let Ok(rows) = i32::try_from(grid.rows()) else {
        return;
    };
    if let Some(terminal) = terminal {
        let previous_cols = terminal.borrow().cols();
        let pushed = terminal.borrow_mut().push_size(
            i32::from(grid.cols()),
            rows,
            grid.cell_width(),
            grid.cell_height(),
        );
        if !pushed {
            return;
        }
        if i32::from(grid.cols()) > i32::from(previous_cols)
            && let Some(previous_px) = i32::from(previous_cols).checked_mul(grid.cell_width())
        {
            // The narrowest stale width wins: a widening after a widening
            // without any output in between keeps the narrower of the two
            // (the GTK path's `note_grid_widened`).
            let current = stale.get();
            stale.set(if current > 0 {
                current.min(previous_px)
            } else {
                previous_px
            });
        }
    }
    apply_pty_size(fd, grid);
    repaint.set(true);
}

#[cfg(test)]
mod tests {
    use super::super::{Inner, Startup};
    use super::*;
    use crate::fontconfig::FontConfig;
    use crate::layout::{Keyboard, Layout, OutputSize, Side};
    use crate::panel::PinwinError;
    use crate::panel::handshake::{Handshake, map_start};
    use crate::panel::wayland_side::sizing::Sizing;
    use crate::pty::attach_calloop;
    use crate::render::font::FontBook;
    use crate::render::text_pass::test_support;
    use std::num::NonZeroU16;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Instant;

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
        let (terminal, repaint, _stale, _pty) = byte_path(Poisoned::new(), -1, cell);
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

    /// The stale pre-resize record follows the widen rule (dispatch D4c,
    /// the GTK path's `note_grid_widened`): a widening push records the
    /// previous grid's pixel width, a widening after a widening without
    /// output in between keeps the narrower of the two, and the terminal's
    /// output clears it and latches the repaint flag.
    #[test]
    fn the_byte_path_records_and_clears_the_widened_grid() {
        let cell = CellSize::new(9, 16).expect("test cell size is non-zero");
        let (terminal, repaint, stale, _pty) = byte_path(Poisoned::new(), -1, cell);
        assert!(
            terminal.borrow_mut().push_size(40, 4, 9, 16),
            "the test grid pushes"
        );
        assert_eq!(stale.get(), 0, "no widen recorded yet");

        // A grid of `cols` columns at the test cell, the way the sizing
        // derives one for a configure.
        let grid_of = |cols: u16| {
            let mut sizing = Sizing::new(NonZeroU16::new(cols).expect("cols"), cell);
            let mut out = None;
            sizing.configure(64, &mut |grid| out = Some(grid));
            out.expect("the configure pushes")
        };
        // A widening push from 40 to 120 columns records 40 * 9.
        let push = |cols: u16, stale: &Cell<i32>, repaint: &Cell<bool>| {
            push_grid(Some(&terminal), repaint, stale, -1, grid_of(cols));
        };
        push(120, stale.as_ref(), repaint.as_ref());
        assert_eq!(stale.get(), 360, "the previous grid's pixel width");
        assert!(repaint.get(), "the push latched the repaint flag");

        // A widening after a widening without output in between keeps the
        // narrower of the two stale widths.
        repaint.set(false);
        push(200, stale.as_ref(), repaint.as_ref());
        assert_eq!(stale.get(), 360, "the narrower stale width wins");

        // The terminal's output clears the record and latches the repaint
        // flag: the drawn grid is the live one again.
        terminal.borrow_mut().push_pty_data(b"hi");
        assert_eq!(stale.get(), 0, "the output cleared the record");
        assert!(repaint.get(), "the output latched the repaint flag");
    }

    /// A connected pty pair (master, slave): the master is the panel-side
    /// fd, the slave stands in for the hosted child (D10 — no display, no
    /// real child), like the calloop source's own tests.
    fn pty_pair() -> (std::fs::File, std::fs::File) {
        // SAFETY: `posix_openpt` takes only flags.
        let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        assert!(master >= 0, "posix_openpt");
        // SAFETY: `master` is an open pty master from the call above;
        // `grantpt` and `unlockpt` take only the descriptor, and `ptsname`
        // returns libc's static slave-name buffer.
        let ptr = unsafe {
            assert_eq!(libc::grantpt(master), 0, "grantpt");
            assert_eq!(libc::unlockpt(master), 0, "unlockpt");
            libc::ptsname(master)
        };
        assert!(!ptr.is_null(), "ptsname");
        // SAFETY: `ptsname` returned a pointer to a nul-terminated name.
        let name = unsafe { std::ffi::CStr::from_ptr(ptr) };
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name.to_str().expect("slave name is utf-8"))
            .expect("open the slave side");
        // SAFETY: the master descriptor is owned by this `File` from here on.
        unsafe { (std::fs::File::from_raw_fd(master), slave) }
    }

    /// Dispatch until `until` holds, failing after five seconds instead of
    /// hanging the test.
    fn dispatch_until(until: impl Fn() -> bool, event_loop: &mut calloop::EventLoop<()>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !until() {
            assert!(
                std::time::Instant::now() < deadline,
                "the loop never reached the condition"
            );
            event_loop
                .dispatch(Some(std::time::Duration::from_millis(100)), &mut ())
                .expect("dispatch");
        }
    }

    /// The thread's byte path feeds the pty's bytes into the terminal
    /// through the calloop read source: bytes written on the slave side
    /// reach the terminal — initialized here, the way the real thread's
    /// first `push_size` initializes it — and the terminal's output latches
    /// the repaint flag the loop reads. The source stays installed and the
    /// fd slot live for an idle loop afterwards.
    #[test]
    fn the_pty_source_feeds_the_terminal_and_latches_the_repaint_flag() {
        use std::io::Write as _;
        use std::os::fd::AsRawFd as _;

        let (master, mut slave) = pty_pair();
        let cell = CellSize::new(9, 16).expect("test cell size is non-zero");
        let (terminal, repaint, _stale, pty) = byte_path(Poisoned::new(), master.as_raw_fd(), cell);
        assert!(
            terminal.borrow_mut().push_size(8, 4, 9, 16),
            "the test grid pushes"
        );

        let mut event_loop = calloop::EventLoop::<()>::try_new().expect("event loop");
        let (fd, fd_slot, pty_poisoned) = pty.read_source();
        let _source = attach_calloop(
            event_loop.handle(),
            fd,
            Arc::clone(&fd_slot),
            Arc::new(AtomicBool::new(false)),
            pty_poisoned,
            {
                let terminal = Rc::clone(&terminal);
                move |data| terminal.borrow_mut().push_pty_data(data)
            },
        )
        .expect("attach the calloop source");

        slave.write_all(b"hello").expect("write to the slave");
        dispatch_until(|| repaint.get(), &mut event_loop);
        assert!(
            fd_slot.load(Ordering::Relaxed) >= 0,
            "a plain dispatch leaves the source installed"
        );
    }

    /// A winsize for the tests' layouts.
    fn layout(side: Side, cols: u16, top: i32, bottom: i32, left: i32, right: i32) -> Layout {
        Layout::new(
            side,
            NonZeroU16::new(cols).expect("test column count is non-zero"),
            top,
            bottom,
            left,
            right,
        )
    }

    /// An output size for the tests' applies.
    fn output(width: i32, height: i32) -> OutputSize {
        OutputSize::new(width, height).expect("test output size is non-zero")
    }

    /// A headless state wired to the thread's byte path over a real pty
    /// master: the production sink pushes the terminal grid and the pty
    /// winsize through it (D10 — the compositor paths stay out).
    /// The state-over-pty fixture's return: the headless state wired to
    /// the thread's byte path pieces over a real pty master.
    type StateOverPty = (
        PanelState,
        Rc<RefCell<Terminal>>,
        Rc<Cell<bool>>,
        Rc<Cell<i32>>,
        Pty,
    );

    fn state_over_pty(master: &std::fs::File) -> StateOverPty {
        let startup = Startup {
            fd: master.as_raw_fd(),
            layout: layout(Side::Left, 40, 0, 0, 0, 0),
            keyboard: Keyboard::OnDemand,
            accent: None,
        };
        let cell = CellSize::new(9, 16).expect("test cell size is non-zero");
        let (terminal, repaint, stale_px, pty) = byte_path(Poisoned::new(), startup.fd, cell);
        let mut state = PanelState::headless(
            Handshake::new(mpsc::channel().0),
            Poisoned::new(),
            live_inner(),
            startup,
            cell,
        );
        state.terminal = Some(Rc::clone(&terminal));
        state.repaint = Rc::clone(&repaint);
        state.stale_grid_px = Rc::clone(&stale_px);
        (state, terminal, repaint, stale_px, pty)
    }

    /// Read back the master's winsize.
    fn read_winsize(fd: RawFd) -> libc::winsize {
        let mut ws = libc::winsize {
            ws_col: 0,
            ws_row: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: `fd` is open and `ws` is writable for the call.
        let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut ws) };
        assert!(result >= 0, "TIOCGWINSZ");
        ws
    }

    /// Block `SIGWINCH` on this test thread for its lifetime: the winsize
    /// ioctls the sink performs raise it, and the pty module's tests record
    /// the signal process-wide — blocking it here keeps this test's raises
    /// out of their recording windows when the tests share one process
    /// (nextest runs each test in its own process anyway). The mask is
    /// never restored: the pending signal dies with the thread, and a
    /// restore would deliver it into the shared process instead.
    fn block_sigwinch() {
        // SAFETY: `set` and `old` are writable sigsets for the calls.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&raw mut set);
            libc::sigaddset(&raw mut set, libc::SIGWINCH);
            let mut old: libc::sigset_t = std::mem::zeroed();
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, &raw mut old),
                0,
                "block SIGWINCH"
            );
        }
    }

    /// The winsize and the terminal grid one push of the production sink
    /// leaves behind, read back from both ends.
    fn assert_pushed(
        master: &std::fs::File,
        terminal: &Rc<RefCell<Terminal>>,
        cols: u16,
        rows: u32,
    ) {
        let ws = read_winsize(master.as_raw_fd());
        assert_eq!(
            (ws.ws_col, ws.ws_row),
            (cols, u16::try_from(rows).expect("rows"))
        );
        assert_eq!(
            (ws.ws_xpixel, ws.ws_ypixel),
            (cols * 9, u16::try_from(rows * 16).expect("pixel height"))
        );
        let term = terminal.borrow();
        assert_eq!(
            (term.cols(), term.rows()),
            (cols, u16::try_from(rows).expect("rows"))
        );
        assert_eq!((term.cell_w(), term.cell_h()), (9, 16));
    }

    /// One apply pushes the winsize and the terminal grid exactly once,
    /// through the production sink: both ends read back the same derived
    /// grid, the push latched the repaint flag, and a repeat apply of the
    /// same layout pushes nothing.
    #[test]
    fn one_apply_pushes_the_winsize_and_the_terminal_grid_once() {
        let master = std::fs::File::open("/dev/ptmx").expect("open /dev/ptmx");
        block_sigwinch();
        let (mut state, terminal, repaint, _stale, _pty) = state_over_pty(&master);
        state.sizing.configure(1080, &mut |_| {});
        let target = layout(Side::Left, 120, 0, 0, 0, 0);

        let mut sink = state.grid_sink();
        let pushes = Cell::new(0usize);
        let mut counted = |grid: Grid| {
            pushes.set(pushes.get() + 1);
            sink(grid);
        };
        state
            .apply_against(output(1920, 1080), target, &mut counted)
            .expect("the layout fits the output");
        assert_eq!(pushes.get(), 1, "exactly one push");
        assert_pushed(&master, &terminal, 120, 1080 / 16);
        assert!(repaint.get(), "the push latched the repaint flag");

        // A repeat apply of the same layout derives the same grid and
        // pushes nothing: the winsize and the grid are unchanged.
        let mut sink = state.grid_sink();
        let pushes = Cell::new(0usize);
        let mut counted = |grid: Grid| {
            pushes.set(pushes.get() + 1);
            sink(grid);
        };
        state
            .apply_against(output(1920, 1080), target, &mut counted)
            .expect("the layout fits the output");
        assert_eq!(pushes.get(), 0, "the repeat apply pushes nothing");
        assert_pushed(&master, &terminal, 120, 1080 / 16);
    }

    /// The deferred tween-end push lands once, at the new columns and the
    /// recorded height: the staging pushes nothing, and the finish's single
    /// push reaches both the pty and the terminal.
    #[test]
    fn a_deferred_tween_end_push_pushes_the_winsize_and_the_grid_once() {
        let master = std::fs::File::open("/dev/ptmx").expect("open /dev/ptmx");
        block_sigwinch();
        let (mut state, terminal, repaint, _stale, _pty) = state_over_pty(&master);
        state.sizing.configure(1080, &mut |_| {});

        let mut sink = state.grid_sink();
        let pushes = Cell::new(0usize);
        let mut counted = |grid: Grid| {
            pushes.set(pushes.get() + 1);
            sink(grid);
        };
        let _ = state.stage_animated(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            200,
            &mut counted,
        );
        assert_eq!(pushes.get(), 0, "a staged animated apply pushes nothing");
        state.tween.begin(360, 1080, 200, Instant::now(), None);
        state.tween_finished(1080, &mut counted);
        assert_eq!(pushes.get(), 1, "the finish pushes once");
        assert_pushed(&master, &terminal, 120, 1080 / 16);
        assert!(repaint.get(), "the push latched the repaint flag");
    }

    /// A snap during a running tween pushes at once: the stop relay lifts
    /// the sizing defer and the deferred grid reaches the pty and the
    /// terminal in the same apply.
    #[test]
    fn a_snap_during_a_tween_pushes_the_winsize_and_the_grid_once() {
        let master = std::fs::File::open("/dev/ptmx").expect("open /dev/ptmx");
        block_sigwinch();
        let (mut state, terminal, repaint, _stale, _pty) = state_over_pty(&master);
        state.sizing.configure(1080, &mut |_| {});

        let mut sink = state.grid_sink();
        let pushes = Cell::new(0usize);
        let mut counted = |grid: Grid| {
            pushes.set(pushes.get() + 1);
            sink(grid);
        };
        let _ = state.stage_animated(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            200,
            &mut counted,
        );
        state.tween.begin(360, 1080, 200, Instant::now(), None);
        state
            .apply_snap_against(
                output(1920, 1080),
                layout(Side::Left, 120, 0, 0, 0, 12),
                &mut counted,
            )
            .expect("the layout fits the output");
        assert_eq!(pushes.get(), 1, "the snap pushes at once");
        assert_pushed(&master, &terminal, 120, 1080 / 16);
        assert!(repaint.get(), "the push latched the repaint flag");
    }
}
