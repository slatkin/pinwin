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
//! budgeted drain — is display-free and takes the fd as a parameter, so unit
//! tests exercise it against a real pipe or pty master (D10). The read
//! source itself is a calloop registration; its twin for the panel thread
//! lives in the `calloop` submodule, whose readiness callback runs the same
//! `drain` under the shared [`crate::guard`] helper (D5).
//!
//! The GTK-widget-dependent piece of `src/pty.c` — applying the drawing
//! area's allocation to the grid before resizing — has no counterpart
//! here: the panel thread's grid sizing (`panel::wayland_side::sizing`)
//! decides the grid and applies the winsize through the caller-supplied
//! callback that drives [`Pty::resize`].
//!
//! The read source for the panel thread lives beside it in the `calloop`
//! submodule (`replace-gtk-with-wayland` D2); the thread attaches it in
//! `run_loop`, and a teardown or a hangup retires the shared fd slot.

mod calloop;

pub use self::calloop::{PtySource, attach_calloop};

use std::io;
use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::guard::Poisoned;
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
    // Approved per-instance (#13): the grid geometry is screen-bounded and
    // the ioctl winsize fields are u16 by ABI.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "approved #13: screen-bounded geometry into u16 ioctl fields"
    )]
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
            off += n.cast_unsigned();
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
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The outcome of one main-loop dispatch of the pty read source.
#[derive(Debug, PartialEq, Eq)]
enum Drain {
    /// The dispatch drained to `EAGAIN` or yielded within its budget: the
    /// source stays installed.
    Dispatched,
    /// EOF or a read error: the child side is gone. Drop the read source and leave the host's process lifetime
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
    feed: &mut dyn FnMut(&[u8]),
    started_us: i64,
    budget_us: i64,
    now_us: &dyn Fn() -> i64,
) -> Drain {
    let mut buf = vec![0u8; READ_BUF_LEN].into_boxed_slice();
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
            feed(&buf[..n.cast_unsigned()]);
            if pty_yield(started_us, now_us(), budget_us) {
                return Drain::Dispatched;
            }
            continue;
        }
        if n < 0 {
            match errno() {
                libc::EINTR => continue,
                libc::EAGAIN => return Drain::Dispatched,
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
#[derive(Clone, Debug)]
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

/// The pty state for one panel: the host-supplied master fd. The shared fd slot is the only
/// state the [`PtyWriter`] needs across objects; the read source is a
/// calloop registration the panel thread attaches (`calloop::attach_calloop`)
/// and retire is the shared slot going to -1.
#[derive(Debug)]
pub struct Pty {
    /// The host-owned master fd, retired to -1 when the read source is gone.
    /// The library never closes the real descriptor (D7).
    fd: Arc<AtomicI32>,
    /// Latched when a read-source panic is caught (D5).
    poisoned: Poisoned,
}

impl Pty {
    /// Take the host-supplied master fd. `poisoned` is the panel's shared D5
    /// latch: a read-source panic latches it so the rest of the panel's glue
    /// code stops too.
    #[must_use]
    pub fn new(poisoned: Poisoned, fd: RawFd) -> Self {
        Pty {
            fd: Arc::new(AtomicI32::new(fd)),
            poisoned,
        }
    }

    /// The write handle to hand to `Terminal::new` as its [`PtySink`].
    #[must_use]
    pub fn writer(&self) -> PtyWriter {
        PtyWriter {
            fd: Arc::clone(&self.fd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::sync::Mutex;
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    /// The winsize ioctls need a real tty; a `/dev/ptmx` master answers
    /// `TIOCSWINSZ`/`TIOCGWINSZ` without unlocking its slave.
    fn pty_master() -> File {
        File::open("/dev/ptmx").expect("open /dev/ptmx")
    }

    /// A connected pipe pair (read end, write end), blocking like a fresh fd.
    fn pipe_pair() -> (File, File) {
        let mut fds: [libc::c_int; 2] = [0; 2];
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
            off += usize::try_from(n).expect("positive read count");
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
        let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut ws) };
        assert!(result >= 0, "TIOCGWINSZ");
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
                    libc::sigaction(libc::SIGWINCH, &raw const action, std::ptr::null_mut()) == 0,
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
        write_end.write_all(&[7]).expect("write one byte");
        set_non_blocking(read_end.as_raw_fd()).expect("set non-blocking");
        let mut byte = [0u8; 1];
        // SAFETY: `read_end` is open and `byte` is writable.
        let n = unsafe { libc::read(read_end.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
        assert_eq!(n, 1);
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
        let pty = Pty::new(Poisoned::new(), -1);
        let mut writer = pty.writer();
        writer.write_pty(b"gone");
    }

    /// Without a budget the drain reads until `EAGAIN` in full-size chunks.
    #[test]
    fn drain_without_budget_drains_until_eagain() {
        let (read_end, mut write_end) = pipe_pair();
        set_non_blocking(read_end.as_raw_fd()).expect("non-blocking");
        let payload = vec![42u8; 60_000];
        write_end.write_all(&payload).expect("fill the pipe");

        let fed = Arc::new(AtomicUsize::new(0));
        let fed_for_feed = Arc::clone(&fed);
        let outcome = drain(
            read_end.as_raw_fd(),
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
        let payload = vec![7u8; 60_000];
        write_end.write_all(&payload).expect("fill the pipe");

        let fed = Arc::new(AtomicUsize::new(0));
        let fed_for_feed = Arc::clone(&fed);
        // The fake clock advances 2000 us per check: two reduced chunks
        // (16384 bytes each) put the dispatch past the 4000 us budget.
        let clock = AtomicUsize::new(0);
        let outcome = drain(
            read_end.as_raw_fd(),
            &mut |data| {
                fed_for_feed.fetch_add(data.len(), Ordering::Relaxed);
            },
            1000,
            PTY_BUDGET_US,
            &|| {
                let tick = clock.fetch_add(1, Ordering::Relaxed);
                1000 + 2000 * i64::try_from(tick).expect("few ticks") + 2000
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
        let fed_for_feed = Arc::clone(&fed);
        let outcome = drain(
            read_end.as_raw_fd(),
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
}
