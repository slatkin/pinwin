//! The calloop twin of the glib pty read source (`replace-gtk-with-wayland`
//! D2): the same [`drain`] — the same hangup handling and the same bounded
//! tween budget — dispatched from the panel thread's calloop loop instead of
//! the GTK main context. The entry point is `pub` for reachability (the
//! same choice `panel::wayland_side` made); the glib source went with the
//! GTK path, and this module is the only pty read source.
//!
//! Panics never cross into calloop (D5): the readiness callback runs the
//! drain under the shared [`crate::guard`] with the caller's poisoned latch,
//! and a caught panic removes the source and retires the fd slot exactly
//! like a hangup. The fd itself is never closed here — the host owns it
//! (D7) — hangup and teardown only retire the shared slot, so later writes
//! and resizes become no-ops.

use std::io;
use std::os::fd::{AsFd, BorrowedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};

use crate::guard::{Poisoned, guard};

use super::{Drain, PTY_BUDGET_US, Pty, drain, set_non_blocking};

/// The registration handle of one calloop pty read source (D2): the teardown
/// twin of `Pty::detach` on the glib path. [`PtySource::remove`] takes the
/// source out early and retires the fd slot; dropping the whole loop takes a
/// still-installed source with it, and a hangup removes the source on its
/// own.
///
/// The fields are private: a host outside the crate can only remove the
/// source, never re-register or move it.
pub struct PtySource<'l, D> {
    /// The loop the source is registered on (`LoopHandle` is a cheap clone).
    /// [`attach_calloop`] takes its own clone by value.
    handle: LoopHandle<'l, D>,
    token: RegistrationToken,
    /// The shared fd slot, retired again on teardown for the no-op writes
    /// and resizes the hangup already guarantees.
    fd_slot: Arc<AtomicI32>,
}

/// Not printable state: a registration token and a loop handle. The shape
/// follows the panel handle's `finish_non_exhaustive` debug impls.
impl<D> std::fmt::Debug for PtySource<'_, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtySource").finish_non_exhaustive()
    }
}

impl<D> PtySource<'_, D> {
    /// Remove the source at teardown: the fd slot retires, so later writes
    /// and resizes are no-ops, and the descriptor itself stays open — the
    /// host owns it (D7). A no-op when the source already removed itself on
    /// hangup.
    pub fn remove(self) {
        // A token the source retired on hangup no longer matches its slot
        // (calloop versions the entry), so this drops the stale request.
        self.handle.remove(self.token);
        self.fd_slot.store(-1, Ordering::Relaxed);
    }
}

impl Pty {
    /// The read-source inputs the panel thread's calloop attach needs
    /// (row 8.1): the fd and the shared slot — the one the
    /// [`super::PtyWriter`]
    /// reads, so a hangup or a teardown retires the writes with the reads —
    /// and the panel's shared D5 latch the drain runs under. The pty module
    /// owns them; the panel thread only attaches. The tween flag is the
    /// tween driver's own `Arc` (row 8.1), not this handle's glib-path one.
    pub(crate) fn read_source(&self) -> (RawFd, Arc<AtomicI32>, Poisoned) {
        (
            self.fd.load(Ordering::Relaxed),
            Arc::clone(&self.fd),
            self.poisoned.clone(),
        )
    }
}

/// Install the pty read source on a calloop loop (D2), the calloop twin of
/// `Pty::attach`'s read-source half: the fd goes into non-blocking mode —
/// the drain reads until `EAGAIN` — and the source is registered for
/// level-triggered read readiness. The initial winsize is the caller's job
/// ([`apply_winsize`](super::apply_winsize)); this registers the read path only.
///
/// `fd_slot` is the shared fd slot the [`PtyWriter`](super::PtyWriter) reads:
/// the source retires it to -1 on hangup and [`PtySource::remove`] retires it
/// at teardown, so a write after either is a no-op. `tween_active` bounds the
/// drain to [`PTY_BUDGET_US`] per dispatch while a width tween runs. `feed`
/// is the terminal's byte sink (the glue's `Terminal::push_pty_data`), taken
/// as a parameter so the tests run without a display (D10).
///
/// # Errors
/// A fd that cannot be made non-blocking, a fd below zero, or a failed
/// registration — the same sticky degradation the glib `attach` reports.
pub fn attach_calloop<D>(
    handle: LoopHandle<'_, D>,
    fd: RawFd,
    fd_slot: Arc<AtomicI32>,
    tween_active: Arc<AtomicBool>,
    poisoned: Poisoned,
    feed: impl FnMut(&[u8]) + 'static,
) -> io::Result<PtySource<'_, D>> {
    if fd < 0 {
        return Err(io::Error::from_raw_os_error(libc::EBADF));
    }
    set_non_blocking(fd)?;
    let source = Generic::new(FdSource(fd), Interest::READ, Mode::Level);
    let slot = Arc::clone(&fd_slot);
    let mut feed = Box::new(feed);
    let token = handle
        .insert_source(source, move |_readiness, _io, _loop_data| {
            let outcome = guard(&poisoned, || {
                let started = now_us();
                let budget = if tween_active.load(Ordering::Relaxed) {
                    PTY_BUDGET_US
                } else {
                    0
                };
                // No hangup condition to pass (D2): calloop's poller folds
                // `EPOLLHUP`/`EPOLLERR` into plain readability — its
                // `Readiness::error` stays clear for fd sources — so unlike
                // the glib source's `G_IO_HUP` there is no readiness bit to
                // consult, and the drain's read result (EOF, or `EIO` on a
                // pty master) is the hangup detector.
                drain(fd, false, feed.as_mut(), started, budget, &now_us)
            });
            match outcome {
                Ok(Drain::Dispatched) => Ok(PostAction::Continue),
                // Hangup, or a caught panic (D5): stop reading. The host's
                // process lifetime and its descriptor stay untouched (D2).
                Ok(Drain::HungUp) | Err(_) => {
                    fd_slot.store(-1, Ordering::Relaxed);
                    Ok(PostAction::Remove)
                }
            }
        })
        .map_err(|insert| insert.error)?;
    Ok(PtySource {
        handle,
        token,
        fd_slot: slot,
    })
}

/// A borrowed raw fd as an [`AsFd`] for calloop's [`Generic`]: the source
/// never owns the descriptor — the host does (D7) — so `as_fd` only borrows
/// it for the poller, and dropping the source deregisters it without closing
/// anything.
struct FdSource(RawFd);

impl AsFd for FdSource {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: `self.0` is a caller-owned open descriptor for the
        // source's lifetime; the borrow takes no ownership and closes
        // nothing.
        unsafe { BorrowedFd::borrow_raw(self.0) }
    }
}

/// Monotonic microseconds from a process-lifetime anchor: [`drain`] and
/// [`crate::layout::pty_yield`] only compare values this function produced,
/// so the anchor's position is arbitrary.
fn now_us() -> i64 {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    let anchor = ANCHOR.get_or_init(Instant::now);
    i64::try_from(anchor.elapsed().as_micros()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::sync::Mutex;
    use std::time::Duration;

    use calloop::EventLoop;

    /// A connected pty pair (master, slave): the master is the panel-side
    /// fd, the slave stands in for the hosted child (D10 — no display, no
    /// real child).
    fn pty_pair() -> (File, File) {
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
        let name = unsafe { CStr::from_ptr(ptr) };
        // The slave side is read-write: the test child writes into it.
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .open(name.to_str().expect("slave name is utf-8"))
            .expect("open the slave side");
        // SAFETY: the master descriptor is owned by this `File` from here on.
        unsafe { (File::from_raw_fd(master), slave) }
    }

    /// The master fd is still open and usable by the host after the panel is
    /// done with it (D7): the descriptor query succeeds and the winsize
    /// ioctl answers.
    fn assert_fd_still_open(fd: RawFd) {
        // SAFETY: `fd` is open; `F_GETFL` takes no argument, and
        // `TIOCGWINSZ` writes only into `ws`.
        unsafe {
            assert!(libc::fcntl(fd, libc::F_GETFL) >= 0, "F_GETFL");
            let mut ws = libc::winsize {
                ws_col: 0,
                ws_row: 0,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            assert!(
                libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut ws) >= 0,
                "TIOCGWINSZ"
            );
        }
    }

    /// Dispatch until `until` holds, failing after five seconds instead of
    /// hanging the test.
    fn dispatch_until(until: impl Fn() -> bool, event_loop: &mut EventLoop<()>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !until() {
            assert!(
                Instant::now() < deadline,
                "the loop never reached the condition"
            );
            event_loop
                .dispatch(Some(Duration::from_millis(100)), &mut ())
                .expect("dispatch");
        }
    }

    /// A dispatch that must sleep out its whole timeout: after the source is
    /// gone, the loop must not spin on a still-readable retired fd.
    fn dispatch_sleeps(event_loop: &mut EventLoop<()>) {
        let started = Instant::now();
        event_loop
            .dispatch(Some(Duration::from_millis(50)), &mut ())
            .expect("dispatch");
        assert!(
            started.elapsed() >= Duration::from_millis(25),
            "the loop spun instead of sleeping"
        );
    }

    /// A calloop source feeds the pty's bytes to the terminal: bytes written
    /// on the slave side reach the feed closure — the terminal's
    /// `push_pty_data` seam (D10) — through the loop, and the source stays
    /// installed with an idle loop afterwards.
    #[test]
    fn a_calloop_source_feeds_pty_bytes_to_the_terminal() {
        let (master, slave) = pty_pair();
        let raw = master.as_raw_fd();
        let payload = vec![42u8; 100_000];
        let writer_payload = payload.clone();
        // The writer writes through a clone so the slave itself stays open:
        // dropping it would hang the master up mid-test.
        let mut child = slave
            .try_clone()
            .expect("clone the slave side for the writer");
        let writer = std::thread::spawn(move || {
            child
                .write_all(&writer_payload)
                .expect("write to the slave");
        });

        let fd_slot = Arc::new(AtomicI32::new(raw));
        let fed = Arc::new(Mutex::new(Vec::new()));
        let fed_for_feed = Arc::clone(&fed);
        let mut event_loop = EventLoop::<()>::try_new().expect("event loop");
        let _source = attach_calloop(
            event_loop.handle(),
            raw,
            Arc::clone(&fd_slot),
            Arc::new(AtomicBool::new(false)),
            Poisoned::new(),
            move |data| {
                fed_for_feed
                    .lock()
                    .expect("feed lock")
                    .extend_from_slice(data);
            },
        )
        .expect("attach the calloop source");

        dispatch_until(
            || fed.lock().expect("feed lock").len() >= payload.len(),
            &mut event_loop,
        );
        writer.join().expect("the writer thread");
        assert_eq!(
            &*fed.lock().expect("feed lock"),
            &payload,
            "every byte arrives"
        );
        assert!(
            fd_slot.load(Ordering::Relaxed) >= 0,
            "a plain dispatch leaves the source installed"
        );
        dispatch_sleeps(&mut event_loop);
    }

    /// A hangup removes the source and leaves the fd open: the child side
    /// going away retires the fd slot, takes the source out of the loop —
    /// later dispatches sleep instead of spinning on the readable corpse —
    /// and the descriptor itself stays usable by the host (D7).
    #[test]
    fn a_hangup_removes_the_source_and_leaves_the_fd_open() {
        let (master, slave) = pty_pair();
        let raw = master.as_raw_fd();
        drop(slave);

        let fd_slot = Arc::new(AtomicI32::new(raw));
        let fed = Arc::new(Mutex::new(Vec::new()));
        let fed_for_feed = Arc::clone(&fed);
        let mut event_loop = EventLoop::<()>::try_new().expect("event loop");
        let _source = attach_calloop(
            event_loop.handle(),
            raw,
            Arc::clone(&fd_slot),
            Arc::new(AtomicBool::new(false)),
            Poisoned::new(),
            move |data| {
                fed_for_feed
                    .lock()
                    .expect("feed lock")
                    .extend_from_slice(data);
            },
        )
        .expect("attach the calloop source");

        dispatch_until(|| fd_slot.load(Ordering::Relaxed) < 0, &mut event_loop);
        assert_fd_still_open(raw);
        dispatch_sleeps(&mut event_loop);
        assert!(
            fed.lock().expect("feed lock").is_empty(),
            "a hangup with no pending data feeds nothing"
        );
    }

    /// Data written before the child side goes away is fully delivered
    /// before the hangup removes the source: the buffered bytes reach the
    /// terminal and the fd stays open afterwards.
    #[test]
    fn a_hangup_after_pending_bytes_still_delivers_them() {
        let (master, mut slave) = pty_pair();
        let raw = master.as_raw_fd();
        slave.write_all(b"last words").expect("write to the slave");
        drop(slave);

        let fd_slot = Arc::new(AtomicI32::new(raw));
        let fed = Arc::new(Mutex::new(Vec::new()));
        let fed_for_feed = Arc::clone(&fed);
        let mut event_loop = EventLoop::<()>::try_new().expect("event loop");
        let _source = attach_calloop(
            event_loop.handle(),
            raw,
            Arc::clone(&fd_slot),
            Arc::new(AtomicBool::new(false)),
            Poisoned::new(),
            move |data| {
                fed_for_feed
                    .lock()
                    .expect("feed lock")
                    .extend_from_slice(data);
            },
        )
        .expect("attach the calloop source");

        dispatch_until(|| fd_slot.load(Ordering::Relaxed) < 0, &mut event_loop);
        assert_eq!(
            &*fed.lock().expect("feed lock"),
            &b"last words".to_vec(),
            "the buffered bytes arrive before the hangup"
        );
        assert_fd_still_open(raw);
    }

    /// A panic inside the feed is caught at the boundary (D5): the shared
    /// latch is set, the fd slot is retired, the source is removed and the
    /// loop stays alive.
    #[test]
    fn a_panicking_feed_latches_and_removes_the_source() {
        let (master, mut slave) = pty_pair();
        let raw = master.as_raw_fd();
        slave.write_all(b"boom").expect("write to the slave");

        let fd_slot = Arc::new(AtomicI32::new(raw));
        let poisoned = Poisoned::new();
        let mut event_loop = EventLoop::<()>::try_new().expect("event loop");
        let _source = attach_calloop(
            event_loop.handle(),
            raw,
            Arc::clone(&fd_slot),
            Arc::new(AtomicBool::new(false)),
            poisoned.clone(),
            |_data| panic!("boom"),
        )
        .expect("attach the calloop source");

        dispatch_until(|| fd_slot.load(Ordering::Relaxed) < 0, &mut event_loop);
        assert!(poisoned.is_poisoned(), "the panic latches the shared flag");
        assert_fd_still_open(raw);
        dispatch_sleeps(&mut event_loop);
    }

    /// `remove` takes the source out at teardown: the fd slot retires so
    /// later writes are no-ops, the descriptor stays open, and the loop does
    /// not spin.
    #[test]
    fn remove_takes_the_source_out_and_retires_the_slot() {
        let (master, mut slave) = pty_pair();
        let raw = master.as_raw_fd();
        slave.write_all(b"unfed").expect("write to the slave");

        let fd_slot = Arc::new(AtomicI32::new(raw));
        let fed = Arc::new(Mutex::new(Vec::new()));
        let fed_for_feed = Arc::clone(&fed);
        let mut event_loop = EventLoop::<()>::try_new().expect("event loop");
        let source = attach_calloop(
            event_loop.handle(),
            raw,
            Arc::clone(&fd_slot),
            Arc::new(AtomicBool::new(false)),
            Poisoned::new(),
            move |data| {
                fed_for_feed
                    .lock()
                    .expect("feed lock")
                    .extend_from_slice(data);
            },
        )
        .expect("attach the calloop source");
        source.remove();

        assert!(fd_slot.load(Ordering::Relaxed) < 0, "the slot retires");
        assert_fd_still_open(raw);
        dispatch_sleeps(&mut event_loop);
        assert!(
            fed.lock().expect("feed lock").is_empty(),
            "a removed source feeds nothing"
        );
    }
}
