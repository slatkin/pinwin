use super::super::{Inner, Startup};
use super::*;
use crate::fontconfig::FontConfig;
use crate::layout::{CellSize, Keyboard, Layout, OutputSize, Side};
use crate::panel::PinwinError;
use crate::panel::handshake::{Handshake, map_start};
use crate::panel::wayland_side::sizing::Sizing;
use crate::panel::wayland_side::state::Wiring;
use crate::panel::wayland_side::tween_draw::TweenRender;
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
    Startup::new(
        -1,
        Layout::new(
            Side::Left,
            NonZeroU16::new(40).expect("test columns"),
            0,
            0,
            0,
            0,
        ),
        Keyboard::OnDemand,
        None,
    )
}

/// A live handle state like a started panel's, for the thread-side
/// tests.
fn live_inner() -> Arc<Inner> {
    Arc::new(Inner {
        poisoned: Poisoned::new(),
        live: AtomicBool::new(true),
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
    let poisoned = Poisoned::new();
    // The test family's own font setup wires through: the state's render
    // bundle is the one `thread_renderer` builds from the measured font,
    // the production path, not the other tests' fallback.
    let renderer = thread_renderer(None, setup).expect("the renderer builds");
    let mut state = PanelState::new(
        Handshake::new(mpsc::channel().0),
        poisoned.clone(),
        live_inner(),
        startup(),
        cell,
        test_wiring_with(&poisoned, renderer),
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
    let BytePath {
        terminal, repaint, ..
    } = byte_path(Poisoned::new(), -1);
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

/// The stale pre-resize record follows the widen rule: a widening push
/// records the
/// previous grid's pixel width, a widening after a widening without
/// output in between keeps the narrower of the two, and the terminal's
/// output clears it and latches the repaint flag.
#[test]
fn the_byte_path_records_and_clears_the_widened_grid() {
    let cell = CellSize::new(9, 16).expect("test cell size is non-zero");
    let BytePath {
        terminal,
        repaint,
        stale_grid_px: stale,
        ..
    } = byte_path(Poisoned::new(), -1);
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
        push_grid(
            Some(&terminal),
            repaint,
            stale,
            -1,
            FractionalScale::from_120ths(120),
            grid_of(cols),
        );
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

/// The winsize-only degrade keeps its direct unit test (typed-publish-path
/// D4): a push with no terminal — the arm [`PanelState::grid_sink_at`]
/// never passes — still applies the winsize and latches the repaint flag,
/// and records no stale width, because the stale record is the terminal
/// push's side effect.
#[test]
fn a_push_without_a_terminal_still_applies_the_winsize() {
    let master = std::fs::File::open("/dev/ptmx").expect("open /dev/ptmx");
    block_sigwinch();
    let cell = CellSize::new(9, 16).expect("test cell size is non-zero");
    let mut sizing = Sizing::new(NonZeroU16::new(40).expect("test columns"), cell);
    let repaint = Cell::new(false);
    let stale = Cell::new(0);
    sizing.configure(1080, &mut |grid| {
        push_grid(
            None,
            &repaint,
            &stale,
            master.as_raw_fd(),
            FractionalScale::from_120ths(120),
            grid,
        );
    });
    let ws = read_winsize(master.as_raw_fd());
    assert_eq!(
        (ws.ws_col, ws.ws_row),
        (40, 1080 / 16),
        "the winsize went out"
    );
    assert_eq!(
        (ws.ws_xpixel, ws.ws_ypixel),
        (40 * 9, (1080 / 16) * 16),
        "the scale-1 device pixels"
    );
    assert!(repaint.get(), "the push latched the repaint flag");
    assert_eq!(stale.get(), 0, "no terminal, no stale record");
}

/// The seat links the thread's pieces build: a focus
/// enter through the seat side flips the flag the renderer draws the
/// accent from and latches the repaint request the loop serves, and a
/// leave clears it and latches one too.
#[test]
fn a_focus_enter_through_the_seat_links_flips_the_flag_and_requests_a_repaint() {
    use crate::panel::wayland_side::seat::SeatSide;

    let BytePath {
        terminal, repaint, ..
    } = byte_path(Poisoned::new(), -1);
    let focused = Rc::new(Cell::new(false));
    let draw_offset = Rc::new(Cell::new(0.0));
    let links = seat_links(&terminal, &repaint, &draw_offset, &focused, Poisoned::new());
    let mut seat = SeatSide::new(links);
    assert!(!focused.get(), "unfocused at start");

    seat.keyboard_entered();
    assert!(focused.get(), "the enter set the flag");
    assert!(repaint.get(), "the enter requested a repaint");

    seat.keyboard_left();
    assert!(!focused.get(), "the leave cleared the flag");
    assert!(repaint.get(), "the leave requested a repaint too");
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
    let BytePath {
        terminal,
        repaint,
        pty,
        ..
    } = byte_path(Poisoned::new(), master.as_raw_fd());
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

/// A state wired to the thread's byte path over a real pty
/// master: the production sink pushes the terminal grid and the pty
/// winsize through it (D10 — the compositor paths stay out).
/// The state-over-pty fixture's return: the state wired to
/// the thread's byte path pieces over a real pty master.
type StateOverPty = (
    PanelState,
    Rc<RefCell<Terminal>>,
    Rc<Cell<bool>>,
    Rc<Cell<i32>>,
    Pty,
);

fn state_over_pty(master: &std::fs::File) -> StateOverPty {
    let cell = CellSize::new(9, 16).expect("test cell size is non-zero");
    let startup = Startup::new(
        master.as_raw_fd(),
        layout(Side::Left, 40, 0, 0, 0, 0),
        Keyboard::OnDemand,
        None,
    );
    let BytePath {
        terminal,
        repaint,
        stale_grid_px: stale_px,
        pty,
    } = byte_path(Poisoned::new(), startup.fd());
    let poisoned = Poisoned::new();
    let draw_offset = Rc::new(Cell::new(0.0));
    let focused = Rc::new(Cell::new(false));
    let links = seat_links(
        &terminal,
        &repaint,
        &draw_offset,
        &focused,
        poisoned.clone(),
    );
    let state = PanelState::new(
        Handshake::new(mpsc::channel().0),
        poisoned.clone(),
        live_inner(),
        startup,
        cell,
        Wiring {
            repaint: Rc::clone(&repaint),
            stale_grid_px: Rc::clone(&stale_px),
            draw_offset,
            seat_links: links,
            render: TweenRender {
                terminal: Rc::clone(&terminal),
                renderer: test_renderer(),
            },
        },
    );
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
fn assert_pushed(master: &std::fs::File, terminal: &Rc<RefCell<Terminal>>, cols: u16, rows: u32) {
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
    assert_eq!(state.panel_size, None, "no configure named a size yet");
    state.tween.begin(360, 1080, 200, Instant::now(), None);
    state.tween_finished(1080, &mut counted);
    assert_eq!(pushes.get(), 1, "the finish pushes once");
    assert_pushed(&master, &terminal, 120, 1080 / 16);
    assert!(repaint.get(), "the finish latched the repaint flag");
    assert_eq!(
        state.panel_size,
        Some((1080, 1080)),
        "the finish records the final logical size"
    );
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
