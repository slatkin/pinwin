//! The pty side (port-to-rust D3, D7): the host-supplied master fd, the
//! read/write paths the terminal drives, the winsize resize and the
//! `SIGWINCH` raise. Ported from `src/pty.c`.
//!
//! The host owns the fd and the child process: there is no fork, no child
//! wait and no child environment here, and the library never closes the fd
//! (D7). When the child side goes away the read source is dropped and the fd
//! slot retires to -1, so later writes and resizes become no-ops; the host's
//! process lifetime is untouched.
//!
//! The core — non-blocking setup, the winsize ioctl, the write loop and the
//! budgeted drain — is GTK-free and takes the fd as a parameter, so unit
//! tests exercise it against a real pipe or pty master (D10). The glib fd
//! source is the only GTK-thread piece; its callback is a C trampoline, so it
//! catches unwinds like every other boundary (D5) through the shared
//! [`crate::guard`] helper.
//!
//! The GTK-widget-dependent piece of `src/pty.c` — applying the drawing
//! area's allocation to the grid before resizing — is not ported here: it
//! belongs to the render/surface rows (tasks 3.6/3.7/4.1) and drives
//! [`Pty::resize`] once the grid is known.

use std::io;
use std::os::fd::RawFd;
use std::os::raw::c_int;
use std::os::raw::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use crate::guard::{Poisoned, guard};
use crate::layout::pty_yield;
use crate::term::PtySink;

/// While a tween runs, bound how long one main-loop dispatch may spend
/// draining the pty (reading and parsing) before yielding back to the frame
/// clock: an image re-transmit burst otherwise blocks every frame until fully
/// parsed, which showed as ticks spaced 40-100 ms apart and the tween
/// collapsing into a snap. No tween, no budget: the old drain-until-EAGAIN
/// behavior (`src/pty.c`).
pub const PTY_BUDGET_US: i64 = 4000;

/// The read buffer of one dispatch, as `src/pty.c` sized it.
const READ_BUF_LEN: usize = 65536;

/// Put an open descriptor into non-blocking mode (`src/pty.c` attach).
pub fn set_non_blocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is a caller-owned open descriptor for both calls.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Apply a winsize to the master fd: grid size plus pixel size, and
/// `raise(SIGWINCH)` after a successful update only. A failed ioctl leaves
/// the winsize untouched and raises nothing. The host's event loop learns of
/// resizes from `SIGWINCH`; the disposition is process-wide, so the raise
/// reaches the host's handler from any thread, and a host without a handler
/// ignores the signal (`src/pty.c`).
pub fn apply_winsize(fd: RawFd, cols: i32, rows: i32, cell_w: u32, cell_h: u32) -> io::Result<()> {
    let ws = libc::winsize {
        ws_col: cols as u16,
        ws_row: rows as u16,
        ws_xpixel: (i64::from(cols) * i64::from(cell_w)) as u16,
        ws_ypixel: (i64::from(rows) * i64::from(cell_h)) as u16,
    };
    // SAFETY: `fd` is an open descriptor and `ws` is a valid winsize for the
    // duration of the call.
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raise` is async-signal-safe and `SIGWINCH` needs no argument.
    unsafe { libc::raise(libc::SIGWINCH) };
    Ok(())
}

/// Write response bytes back to the pty master, looping until the buffer is
/// drained or the fd gives up (`src/pty.c` `glue_pty_write`). A retryable
/// error (`EINTR`, or `EAGAIN`/`EWOULDBLOCK` — the same value on Linux)
/// spins again, as the C did; any other error drops the rest.
pub fn write_pty(fd: RawFd, data: &[u8]) {
    let mut off = 0;
    while off < data.len() {
        // SAFETY: `fd` is an open descriptor and the slice covers the region
        // handed to `write` for the duration of the call.
        let n = unsafe { libc::write(fd, data[off..].as_ptr().cast(), data.len() - off) };
        if n > 0 {
            off += n as usize;
            continue;
        }
        if n < 0 && matches!(errno(), libc::EINTR | libc::EAGAIN) {
            continue;
        }
        break;
    }
}

/// The thread-local `errno`, for classifying a failed syscall.
fn errno() -> i32 {
    // SAFETY: reads the thread-local errno on the Linux/glibc runtime this
    // crate targets.
    unsafe { *libc::__errno_location() }
}

/// The outcome of one main-loop dispatch of the pty read source.
#[derive(Debug, PartialEq, Eq)]
enum Drain {
    /// The dispatch drained to `EAGAIN` without a hangup condition, or
    /// yielded within its budget: the source stays installed.
    Dispatched,
    /// EOF, a read error, or `EAGAIN` with a hangup condition: the child side
    /// is gone. Drop the read source and leave the host's process lifetime
    /// alone (`src/pty.c`).
    HungUp,
}

/// Drain the pty for one main-loop dispatch, feeding every read chunk to
/// `feed` (the terminal's `push_pty_data`). A `budget_us` of zero or less
/// means unbounded: read until `EAGAIN`. Otherwise stop once the dispatch has
/// consumed the budget of wall time since `started_us`, reading in reduced
/// chunks so the budget check lands between parses instead of once per
/// 64 KB (`src/pty.c`).
fn drain(
    fd: RawFd,
    hangup_condition: bool,
    feed: &mut dyn FnMut(&[u8]),
    started_us: i64,
    budget_us: i64,
    now_us: &dyn Fn() -> i64,
) -> Drain {
    let mut buf = [0u8; READ_BUF_LEN];
    loop {
        let chunk = if budget_us > 0 {
            READ_BUF_LEN / 4
        } else {
            READ_BUF_LEN
        };
        // SAFETY: `fd` is an open descriptor and `buf` is writable for `chunk`
        // bytes for the duration of the call.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), chunk) };
        if n > 0 {
            feed(&buf[..n as usize]);
            if pty_yield(started_us, now_us(), budget_us) {
                return Drain::Dispatched;
            }
            continue;
        }
        if n < 0 {
            match errno() {
                libc::EINTR => continue,
                libc::EAGAIN => {
                    if hangup_condition {
                        return Drain::HungUp;
                    }
                    return Drain::Dispatched;
                }
                _ => return Drain::HungUp,
            }
        }
        // EOF: the child side is gone.
        return Drain::HungUp;
    }
}

/// A handle for writing terminal response bytes to the pty master. It shares
/// the fd slot with the [`Pty`], which retires it to -1 when the read source
/// is gone, so a write after hangup or teardown is a no-op like the
/// `g_pty_fd < 0` guard in `src/pty.c`. This is the [`PtySink`] the glue
/// hands to `Terminal::new`.
#[derive(Clone)]
pub struct PtyWriter {
    fd: Arc<AtomicI32>,
}

impl PtyWriter {
    fn write(&self, data: &[u8]) {
        let fd = self.fd.load(Ordering::Relaxed);
        if fd < 0 {
            return;
        }
        write_pty(fd, data);
    }
}

impl PtySink for PtyWriter {
    fn write_pty(&mut self, data: &[u8]) {
        self.write(data);
    }
}

/// The pty state for one panel: the host-supplied master fd, its glib read
/// source, the cell size used for the winsize pixel fields, and the tween
/// switch that bounds the drain. Lives on the GTK thread (D4); the shared fd
/// slot is the only state the [`PtyWriter`] needs across objects.
pub struct Pty {
    /// The host-owned master fd, retired to -1 when the read source is gone.
    /// The library never closes the real descriptor (D7).
    fd: Arc<AtomicI32>,
    cell_w: u32,
    cell_h: u32,
    /// Whether a width tween is running: the read drain then bounds itself to
    /// [`PTY_BUDGET_US`] per dispatch. The anim row drives this (task 3.7),
    /// standing in for `glue_anim_active()` in `src/pty.c`.
    tween_active: Arc<AtomicBool>,
    /// Latched when a read-source panic is caught (D5).
    poisoned: Poisoned,
    /// The glib source id, 0 once the source removed itself or teardown
    /// removed it. Shared so the callback can retire it on hangup.
    source: Arc<AtomicU32>,
    /// Whether the fd was attached; sticky once set, like `g_attached`
    /// (`src/pty.c`).
    attached: bool,
}

impl Pty {
    /// Take the host-supplied master fd and the current cell size.
    pub fn new(fd: RawFd, cell_w: u32, cell_h: u32) -> Self {
        Pty {
            fd: Arc::new(AtomicI32::new(fd)),
            cell_w,
            cell_h,
            tween_active: Arc::new(AtomicBool::new(false)),
            poisoned: Poisoned::new(),
            source: Arc::new(AtomicU32::new(0)),
            attached: false,
        }
    }

    /// The write handle to hand to `Terminal::new` as its [`PtySink`].
    pub fn writer(&self) -> PtyWriter {
        PtyWriter {
            fd: self.fd.clone(),
        }
    }

    /// Record a new cell size for the winsize pixel fields (the glue updates
    /// this when the font changes).
    pub fn set_cell_size(&mut self, cell_w: u32, cell_h: u32) {
        self.cell_w = cell_w;
        self.cell_h = cell_h;
    }

    /// Set whether a width tween is running (the anim row, task 3.7).
    pub fn set_tween_active(&self, active: bool) {
        self.tween_active.store(active, Ordering::Relaxed);
    }

    /// Whether a callback panic was caught in the read source (D5).
    pub fn poisoned(&self) -> bool {
        self.poisoned.is_poisoned()
    }

    /// Whether the fd was attached (sticky, like `g_attached`).
    pub fn attached(&self) -> bool {
        self.attached
    }

    /// Whether the read source is gone (hangup or teardown): writes and
    /// resizes are no-ops from here on.
    pub fn hung_up(&self) -> bool {
        self.fd.load(Ordering::Relaxed) < 0
    }

    /// Take the host-supplied master fd: non-blocking, the initial winsize
    /// (grid plus pixel size) and the read source. The host owns the child
    /// side, so there is nothing else to set up (D7). A fd that cannot be
    /// used only degrades to no terminal; it never exits (D3).
    ///
    /// Like `src/pty.c`, a failed winsize ioctl does not stop the attach: the
    /// source is installed with the previous winsize. A failed non-blocking
    /// switch does, and since the attached flag was already set the failure is
    /// sticky. `cols`/`rows` are the effective grid at attach time.
    pub fn attach(
        &mut self,
        cols: i32,
        rows: i32,
        feed: impl FnMut(&[u8]) + 'static,
    ) -> io::Result<()> {
        if self.attached {
            return Ok(());
        }
        let fd = self.fd.load(Ordering::Relaxed);
        if fd < 0 {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        self.attached = true;
        set_non_blocking(fd)?;
        let _ = apply_winsize(fd, cols, rows, self.cell_w, self.cell_h);

        let state = Box::into_raw(Box::new(SourceState {
            fd: self.fd.clone(),
            tween_active: self.tween_active.clone(),
            poisoned: self.poisoned.clone(),
            source: self.source.clone(),
            feed: Box::new(feed),
        }));
        // SAFETY: `g_unix_fd_add_full` is stable GLib API (2.36) that glib-sys
        // does not declare; the library is linked against the same
        // libglib-2.0. `state` is an owned Box handed to the source, freed by
        // `destroy_source_state` when the source is destroyed, and the
        // callback/notify pair match the signature GLib expects.
        let id = unsafe {
            g_unix_fd_add_full(
                gtk4::glib::ffi::G_PRIORITY_DEFAULT,
                fd,
                gtk4::glib::ffi::G_IO_IN | gtk4::glib::ffi::G_IO_HUP | gtk4::glib::ffi::G_IO_ERR,
                Some(on_pty_readable),
                state.cast(),
                Some(destroy_source_state),
            )
        };
        if id == 0 {
            // The docs give no failure semantics beyond "the ID (greater than
            // 0)" (docs.gtk.org/glib-unix/func.fd_add_full.html); the
            // implementation does: `g_unix_fd_add_full` (glib `glib-unix.c`)
            // registers the destroy notify with `g_source_set_callback` and
            // then calls `g_source_attach` + `g_source_unref`, and the final
            // unref finalizes the source, whose callback teardown invokes the
            // destroy notify (`gmain.c` `g_source_unref_internal` →
            // `g_source_callback_unref`). So a 0 from a failed attach has
            // already freed `state` by the time the 0 is returned — freeing
            // here would double-free. The other 0 path, the `function != NULL`
            // guard, fires before the notify is registered, but is
            // unreachable because we pass a compile-time `Some`. Either way
            // the notify owns the state exactly once and this arm only
            // reports the failure.
            return Err(io::Error::last_os_error());
        }
        self.source.store(id, Ordering::Relaxed);
        Ok(())
    }

    /// Apply a new winsize for the current grid, raising `SIGWINCH` only
    /// after a successful ioctl. A no-op once the fd is retired.
    pub fn resize(&self, cols: i32, rows: i32) {
        let fd = self.fd.load(Ordering::Relaxed);
        if fd < 0 {
            return;
        }
        let _ = apply_winsize(fd, cols, rows, self.cell_w, self.cell_h);
    }

    /// Remove the read source at teardown. The fd stays open — the host owns
    /// it (D7) — but it is retired, so later writes and resizes are no-ops.
    pub fn detach(&mut self) {
        let id = self.source.swap(0, Ordering::Relaxed);
        if id != 0 {
            // SAFETY: `id` is a live source id this module installed and has
            // not removed itself.
            unsafe { gtk4::glib::ffi::g_source_remove(id) };
        }
        self.fd.store(-1, Ordering::Relaxed);
        self.attached = false;
    }
}

/// Removing the source at teardown keeps no read closure outliving the panel;
/// the fd itself stays open for the host.
impl Drop for Pty {
    fn drop(&mut self) {
        self.detach();
    }
}

/// The byte feeder one read source drives: the glue's
/// `Terminal::push_pty_data` closure, or a test recorder.
type Feed = Box<dyn FnMut(&[u8])>;

/// The state one glib read source owns: the shared fd slot it retires on
/// hangup, the tween and poison flags it shares with the [`Pty`], and the
/// byte feeder (the glue's `Terminal::push_pty_data`).
struct SourceState {
    fd: Arc<AtomicI32>,
    tween_active: Arc<AtomicBool>,
    poisoned: Poisoned,
    source: Arc<AtomicU32>,
    feed: Feed,
}

/// Retire the fd slot and the source id once the source stops.
fn retire(fd: &AtomicI32, source: &AtomicU32) {
    fd.store(-1, Ordering::Relaxed);
    source.store(0, Ordering::Relaxed);
}

/// The `GUnixFDSourceFunc` trampoline: drain the pty for one dispatch (D5
/// guard; unwinding out of here would cross into GLib).
///
/// # Safety
/// `user_data` must be the `SourceState` pointer `g_unix_fd_add_full` was
/// given, alive until the source's destroy notify runs.
unsafe extern "C" fn on_pty_readable(
    fd: c_int,
    condition: gtk4::glib::ffi::GIOCondition,
    user_data: *mut c_void,
) -> c_int {
    if user_data.is_null() {
        return gtk4::glib::ffi::G_SOURCE_REMOVE;
    }
    // SAFETY: the caller guarantees `user_data` points at the live state, and
    // GLib never re-enters the callback for one source concurrently.
    let state = unsafe { &mut *user_data.cast::<SourceState>() };
    if state.poisoned.is_poisoned() {
        retire(&state.fd, &state.source);
        return gtk4::glib::ffi::G_SOURCE_REMOVE;
    }
    let result = guard(&state.poisoned, || {
        let budget = if state.tween_active.load(Ordering::Relaxed) {
            PTY_BUDGET_US
        } else {
            0
        };
        let started = gtk4::glib::monotonic_time();
        let hangup_condition =
            condition & (gtk4::glib::ffi::G_IO_HUP | gtk4::glib::ffi::G_IO_ERR) != 0;
        drain(
            fd,
            hangup_condition,
            state.feed.as_mut(),
            started,
            budget,
            &|| gtk4::glib::monotonic_time(),
        )
    });
    match result {
        Ok(Drain::Dispatched) => gtk4::glib::ffi::G_SOURCE_CONTINUE,
        // Hangup, or a panic was caught: stop reading. The host's process
        // lifetime and its descriptor stay untouched.
        Ok(Drain::HungUp) | Err(_) => {
            retire(&state.fd, &state.source);
            gtk4::glib::ffi::G_SOURCE_REMOVE
        }
    }
}

/// # Safety
/// `user_data` must be the `SourceState` pointer `g_unix_fd_add_full` was
/// given, not yet freed.
unsafe extern "C" fn destroy_source_state(user_data: *mut c_void) {
    if user_data.is_null() {
        return;
    }
    // The notify runs on GLib's teardown path, not behind a poisoned flag:
    // there is no latch left to set, but it still must not unwind across the
    // C boundary (D5). The shared guard swallows the unwind and logs the
    // payload on a throwaway flag. The source is already being destroyed
    // either way.
    let _ = guard(&Poisoned::new(), || {
        // SAFETY: the caller guarantees the pointer came from `Box::into_raw` and
        // the source is being destroyed, so no dispatch is using it.
        drop(unsafe { Box::from_raw(user_data.cast::<SourceState>()) });
    });
}

// Declared here because glib-sys does not bind `g_unix_fd_add_full` (its gir
// bindings skip the glib-unix header); the symbol comes from the same
// libglib-2.0 glib-sys links.
unsafe extern "C" {
    fn g_unix_fd_add_full(
        priority: c_int,
        fd: c_int,
        condition: gtk4::glib::ffi::GIOCondition,
        function: Option<
            unsafe extern "C" fn(c_int, gtk4::glib::ffi::GIOCondition, *mut c_void) -> c_int,
        >,
        user_data: *mut c_void,
        notify: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::sync::Mutex;
    use std::sync::Once;
    use std::sync::atomic::AtomicUsize;

    /// The winsize ioctls need a real tty; a `/dev/ptmx` master answers
    /// `TIOCSWINSZ`/`TIOCGWINSZ` without unlocking its slave.
    fn pty_master() -> File {
        File::open("/dev/ptmx").expect("open /dev/ptmx")
    }

    /// A connected pipe pair (read end, write end), blocking like a fresh fd.
    fn pipe_pair() -> (File, File) {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: `fds` is a writable two-element array for the call.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
        // SAFETY: the descriptors are owned by these `File`s from here on.
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }

    /// Read `len` bytes from `file` (blocking: the pipe was not made
    /// non-blocking unless the test asks for it).
    fn read_exact(file: &File, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut off = 0;
        while off < len {
            // SAFETY: `file` is open and the remaining slice is writable.
            let n =
                unsafe { libc::read(file.as_raw_fd(), out[off..].as_mut_ptr().cast(), len - off) };
            assert!(n > 0, "read failed at {off}");
            off += n as usize;
        }
        out
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
        assert!(
            unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } >= 0,
            "TIOCGWINSZ"
        );
        ws
    }

    /// Signal handlers are process-wide and `raise` targets the calling
    /// thread, but the flag and the handler are shared by every test thread;
    /// the tests that observe or raise `SIGWINCH` serialize on this mutex.
    static SIGWINCH_LOCK: Mutex<()> = Mutex::new(());
    static SIGWINCH_SEEN: AtomicBool = AtomicBool::new(false);

    extern "C" fn record_sigwinch(_: libc::c_int) {
        SIGWINCH_SEEN.store(true, Ordering::Relaxed);
    }

    /// Install the `SIGWINCH` recorder once for the test process.
    fn install_sigwinch_recorder() {
        static INSTALLED: Once = Once::new();
        INSTALLED.call_once(|| {
            // SAFETY: `action` is a zeroed sigaction whose handler and flags
            // are set; the recorder is async-signal-safe.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = record_sigwinch as *const () as usize;
                action.sa_flags = libc::SA_RESTART;
                assert!(
                    libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut()) == 0,
                    "sigaction"
                );
            }
        });
    }

    /// Run `body` serialized against every other `SIGWINCH`-raising test,
    /// reporting whether the raise was observed.
    fn sigwinch_during(body: impl FnOnce()) -> bool {
        let _guard = SIGWINCH_LOCK.lock().expect("sigwinch lock");
        install_sigwinch_recorder();
        SIGWINCH_SEEN.store(false, Ordering::Relaxed);
        body();
        SIGWINCH_SEEN.load(Ordering::Relaxed)
    }

    /// A successful winsize ioctl on a pty master applies the grid and pixel
    /// size and raises `SIGWINCH`.
    #[test]
    fn winsize_applies_and_raises_sigwinch() {
        let master = pty_master();
        let seen = sigwinch_during(|| {
            apply_winsize(master.as_raw_fd(), 80, 24, 8, 16).expect("winsize on a pty master");
        });
        assert!(seen, "SIGWINCH after a successful winsize ioctl");
        let ws = read_winsize(master.as_raw_fd());
        assert_eq!((ws.ws_col, ws.ws_row), (80, 24));
        assert_eq!((ws.ws_xpixel, ws.ws_ypixel), (640, 384));
    }

    /// A failed ioctl (a pipe is no tty) raises nothing.
    #[test]
    fn failed_winsize_ioctl_raises_nothing() {
        let (read_end, _write_end) = pipe_pair();
        let seen = sigwinch_during(|| {
            assert!(
                apply_winsize(read_end.as_raw_fd(), 80, 24, 8, 16).is_err(),
                "TIOCSWINSZ on a pipe must fail"
            );
        });
        assert!(!seen, "a failed ioctl must not raise SIGWINCH");
    }

    /// `set_non_blocking` makes later reads report `WouldBlock` instead of
    /// blocking.
    #[test]
    fn set_non_blocking_makes_reads_would_block() {
        let (read_end, mut write_end) = pipe_pair();
        use std::io::Write;
        write_end.write_all(&[7]).expect("write one byte");
        set_non_blocking(read_end.as_raw_fd()).expect("set non-blocking");
        let mut byte = [0u8; 1];
        // SAFETY: `read_end` is open and `byte` is writable.
        assert_eq!(
            unsafe { libc::read(read_end.as_raw_fd(), byte.as_mut_ptr().cast(), 1) },
            1
        );
        // SAFETY: as above; the pipe is now empty.
        let n = unsafe { libc::read(read_end.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
        assert_eq!(n, -1);
        assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
    }

    /// The write loop delivers every byte to the other pipe end.
    #[test]
    fn write_pty_round_trips() {
        let (read_end, write_end) = pipe_pair();
        write_pty(write_end.as_raw_fd(), b"hello pty");
        assert_eq!(read_exact(&read_end, 9), b"hello pty");
    }

    /// A writer over a retired fd slot is a no-op, like `g_pty_fd < 0`.
    #[test]
    fn writer_over_a_retired_fd_writes_nothing() {
        let pty = Pty::new(-1, 8, 16);
        assert!(pty.hung_up());
        let mut writer = pty.writer();
        writer.write_pty(b"gone");
    }

    /// Without a budget the drain reads until `EAGAIN` in full-size chunks.
    #[test]
    fn drain_without_budget_drains_until_eagain() {
        let (read_end, mut write_end) = pipe_pair();
        set_non_blocking(read_end.as_raw_fd()).expect("non-blocking");
        use std::io::Write;
        let payload = vec![42u8; 60_000];
        write_end.write_all(&payload).expect("fill the pipe");

        let fed = Arc::new(AtomicUsize::new(0));
        let fed_for_feed = fed.clone();
        let outcome = drain(
            read_end.as_raw_fd(),
            false,
            &mut |data| {
                fed_for_feed.fetch_add(data.len(), Ordering::Relaxed);
            },
            1000,
            0,
            &|| 1000,
        );
        assert_eq!(outcome, Drain::Dispatched);
        assert_eq!(fed.load(Ordering::Relaxed), 60_000);
    }

    /// With a tween budget the drain stops mid-data once the budget is spent.
    #[test]
    fn drain_yields_within_the_budget() {
        let (read_end, mut write_end) = pipe_pair();
        set_non_blocking(read_end.as_raw_fd()).expect("non-blocking");
        use std::io::Write;
        let payload = vec![7u8; 60_000];
        write_end.write_all(&payload).expect("fill the pipe");

        let fed = Arc::new(AtomicUsize::new(0));
        let fed_for_feed = fed.clone();
        // The fake clock advances 2000 us per check: two reduced chunks
        // (16384 bytes each) put the dispatch past the 4000 us budget.
        let clock = AtomicUsize::new(0);
        let outcome = drain(
            read_end.as_raw_fd(),
            false,
            &mut |data| {
                fed_for_feed.fetch_add(data.len(), Ordering::Relaxed);
            },
            1000,
            PTY_BUDGET_US,
            &|| {
                let tick = clock.fetch_add(1, Ordering::Relaxed);
                1000 + 2000 * tick as i64 + 2000
            },
        );
        assert_eq!(outcome, Drain::Dispatched);
        assert_eq!(fed.load(Ordering::Relaxed), 32_768);
    }

    /// EOF (the child side closed) hangs the source up.
    #[test]
    fn drain_hangs_up_on_eof() {
        let (read_end, write_end) = pipe_pair();
        drop(write_end);
        let fed = Arc::new(AtomicUsize::new(0));
        let fed_for_feed = fed.clone();
        let outcome = drain(
            read_end.as_raw_fd(),
            false,
            &mut |data| {
                fed_for_feed.fetch_add(data.len(), Ordering::Relaxed);
            },
            1000,
            0,
            &|| 1000,
        );
        assert_eq!(outcome, Drain::HungUp);
        assert_eq!(fed.load(Ordering::Relaxed), 0);
    }

    /// `EAGAIN` together with a hangup condition hangs the source up even
    /// though the read itself would just be retried.
    #[test]
    fn drain_hangs_up_on_eagain_with_a_hangup_condition() {
        let (read_end, _write_end) = pipe_pair();
        set_non_blocking(read_end.as_raw_fd()).expect("non-blocking");
        let outcome = drain(read_end.as_raw_fd(), true, &mut |_| {}, 1000, 0, &|| 1000);
        assert_eq!(outcome, Drain::HungUp);
    }

    /// Attach puts the fd into non-blocking mode, applies the initial
    /// winsize and installs the glib read source; detach removes the source
    /// and retires the fd slot without closing the descriptor.
    #[test]
    fn attach_installs_the_source_and_initial_winsize() {
        let master = pty_master();
        let raw = master.as_raw_fd();
        let mut pty = Pty::new(raw, 8, 16);
        let fed = Arc::new(AtomicUsize::new(0));
        let fed_for_feed = fed.clone();
        let seen = sigwinch_during(|| {
            pty.attach(80, 24, move |data| {
                fed_for_feed.fetch_add(data.len(), Ordering::Relaxed);
            })
            .expect("attach a pty master");
        });
        assert!(seen, "the initial winsize raises SIGWINCH");
        assert!(pty.attached());
        assert!(!pty.hung_up());
        assert!(!pty.poisoned());

        let ws = read_winsize(raw);
        assert_eq!((ws.ws_col, ws.ws_row), (80, 24));
        assert_eq!((ws.ws_xpixel, ws.ws_ypixel), (640, 384));
        // SAFETY: `raw` is open; F_GETFL takes no argument.
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        assert_ne!(flags & libc::O_NONBLOCK, 0, "attached fd is non-blocking");

        // A second attach is a no-op, and a resize through the still-open fd
        // applies without another attach. The resize raises `SIGWINCH` like
        // any successful winsize ioctl, so it runs under the same lock as
        // every other raising test instead of interleaving with them.
        pty.attach(80, 24, |_| {})
            .expect("second attach is a no-op");
        sigwinch_during(|| pty.resize(100, 30));
        let ws = read_winsize(raw);
        assert_eq!((ws.ws_col, ws.ws_row), (100, 30));

        pty.detach();
        assert!(pty.hung_up());
        assert!(!pty.attached());
        // The descriptor itself is still open and usable (the host owns it).
        let ws = read_winsize(raw);
        assert_eq!((ws.ws_col, ws.ws_row), (100, 30));
    }

    /// Attaching an already-retired fd is an error, not a panic.
    #[test]
    fn attach_with_a_dead_fd_is_an_error() {
        let mut pty = Pty::new(-1, 8, 16);
        assert!(pty.attach(80, 24, |_| {}).is_err());
        assert!(!pty.attached());
    }

    /// Resizing a retired fd is a no-op, like `g_pty_fd < 0`.
    #[test]
    fn resize_over_a_retired_fd_is_a_noop() {
        let pty = Pty::new(-1, 8, 16);
        pty.resize(80, 24);
    }
}
