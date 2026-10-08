//! The shared D5 panic guard (port-to-rust task 4.2): `catch_unwind` at every
//! boundary that must not unwind, with a shared poisoned latch.
//!
//! `Cargo.toml` sets `panic = "unwind"` in every profile, so a panic caught
//! here unwinds out of the guarded body only; letting one cross a foreign
//! C frame — libghostty's FFI or a Wayland or calloop callback —
//! instead is undefined behaviour and aborts. Every
//! module that registers `extern "C"` trampolines or event-loop callbacks
//! runs its
//! bodies through [`guard`] or [`guard_default`] with one shared
//! [`Poisoned`] flag, so a single panic stops that panel's glue code instead
//! of killing the host.
//!
//! The helper is deliberately `PinwinError`-agnostic: the library's
//! `Panel` maps the `Err` case onto its own `Internal` error (port-to-rust
//! D5); nothing here needs to know about it.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared poisoned flag (D5): latched once a guarded body panicked, after
/// which later guarded calls short-circuit instead of running more glue code.
/// A newtype over an `Arc<AtomicBool>`, so the owner and every closure it
/// registered share one latch through clones.
#[derive(Clone, Debug, Default)]
pub struct Poisoned(Arc<AtomicBool>);

impl Poisoned {
    /// A clear flag.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// A flag that is already latched (for tests and callers seeding a
    /// shutdown).
    #[must_use]
    pub fn latched() -> Self {
        let flag = Self::new();
        flag.latch();
        flag
    }

    /// Whether a panic has been caught under this flag.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Latch the flag.
    pub fn latch(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Run `body` under `catch_unwind` (D5), latching `poisoned` on a panic.
///
/// Returns `Err(poisoned)` when the flag was already latched — the body does
/// not run — or when `body` panicked; the panic payload is described on
/// stderr before the latch. The `Err` value is the shared flag itself, so the
/// `Panel` in [`crate::panel`] maps it onto `PinwinError::Internal` at every
/// public entry point (D5: a panic reports Internal, never `NotRunning`).
pub fn guard<T>(poisoned: &Poisoned, body: impl FnOnce() -> T) -> Result<T, Poisoned> {
    if poisoned.is_poisoned() {
        return Err(poisoned.clone());
    }
    guard_always(poisoned, body)
}

/// The relay variant of [`guard`] (D5): the body runs even when the flag is
/// already latched, and a panic in it is caught, logged and latched as usual.
/// Use it only for calls that reset state owned by other modules — the stop
/// relays (`Anim`'s `on_stop`, which drops the render tween frame cache,
/// clears the pty's tween flag and fires the deferred grid resize): skipping
/// those because the latch is set would strand that state (a throttled pty
/// drain, an undropped cache) forever. Ordinary glue code keeps using
/// [`guard`], so a latched panel runs nothing further.
pub fn guard_always<T>(poisoned: &Poisoned, body: impl FnOnce() -> T) -> Result<T, Poisoned> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            log_panic(&payload);
            poisoned.latch();
            Err(poisoned.clone())
        }
    }
}

/// The `extern "C"` trampoline variant of [`guard`] (D5): return `default`
/// when the flag is already latched or `body` panicked, so a trampoline never
/// unwinds into C and always hands back a value the caller defined.
pub fn guard_default<T>(poisoned: &Poisoned, default: T, body: impl FnOnce() -> T) -> T {
    guard(poisoned, body).unwrap_or(default)
}

/// Describe a caught panic payload on stderr (D5). The payload is
/// `Box<dyn Any + Send>`; most payloads are the `&'static str` or `String`
/// a `panic!`/`unwrap` produced.
fn log_panic(payload: &Box<dyn Any + Send>) {
    const PREFIX: &str = "pinwin: caught a panic in a guarded boundary: ";
    let message: &str = payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>");
    // Write straight to the raw stderr descriptor: this runs while a panic
    // unwinds, so the log must not itself panic and any redirection of
    // Rust's stderr handle must not swallow it (D5). Assembling the line
    // with `format!` would allocate, and an allocation panic here would
    // escape the guard into the C frames the guard exists to protect, so the
    // pieces are handed to `writev` as static/slice buffers instead. A
    // failed write is ignored — stderr logging is best-effort.
    let prefix = PREFIX.as_bytes();
    let message = message.as_bytes();
    let newline: &[u8] = b"\n";
    let iov = [
        libc::iovec {
            iov_base: prefix.as_ptr().cast_mut().cast(),
            iov_len: prefix.len(),
        },
        libc::iovec {
            iov_base: message.as_ptr().cast_mut().cast(),
            iov_len: message.len(),
        },
        libc::iovec {
            iov_base: newline.as_ptr().cast_mut().cast(),
            iov_len: newline.len(),
        },
    ];
    // SAFETY: fd 2 is the process stderr and each iovec points at a valid
    // buffer of its own `iov_len` bytes for the duration of the call. The
    // array holds three entries, so the count conversion cannot fail.
    unsafe {
        let _ = libc::writev(
            2,
            iov.as_ptr(),
            i32::try_from(iov.len()).expect("three iovecs fit in an int"),
        );
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A panic in the body is caught and latches the shared flag; a clean
    /// body leaves it clear.
    #[test]
    fn a_panicking_body_is_caught_and_latches_the_flag() {
        let poisoned = Poisoned::new();
        assert!(guard(&poisoned, || panic!("boom")).is_err());
        assert!(poisoned.is_poisoned());

        let fresh = Poisoned::new();
        assert!(guard(&fresh, || 7).is_ok_and(|value| value == 7));
        assert!(!fresh.is_poisoned());
    }

    /// Once latched, later guarded calls short-circuit: the body never runs
    /// and the trampoline variant hands back the caller's default.
    #[test]
    fn later_guarded_calls_short_circuit_with_the_default() {
        let poisoned = Poisoned::latched();
        let ran = Cell::new(false);
        let _ = guard(&poisoned, || {
            ran.set(true);
            1
        })
        .unwrap_err();
        assert!(!ran.get(), "a poisoned guard does not run the body");
        assert!(!guard_default(&poisoned, false, || true));
        assert_eq!(guard_default(&poisoned, "fallback", || "real"), "fallback");
    }

    /// The caught panic payload is described on stderr (D5). The raw
    /// descriptor write lets the test capture the line even under the test
    /// harness's output capture.
    #[test]
    fn the_panic_payload_is_logged_to_stderr() {
        // Redirect stderr into a pipe while the guard logs, then read the
        // pipe back. Parallel tests may write into the same pipe meanwhile,
        // so the assertion only looks for the guard's own prefix.
        let mut fds: [libc::c_int; 2] = [0; 2];
        // SAFETY: `pipe` writes two descriptors into the owned array.
        unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0, "pipe for stderr");
        };
        let (read_fd, write_fd) = (fds[0], fds[1]);
        // SAFETY: duplicating the live stderr descriptor.
        let saved_stderr = unsafe { libc::dup(2) };
        // SAFETY: redirecting stderr at a descriptor this test owns.
        unsafe {
            assert_eq!(libc::dup2(write_fd, 2), 2, "redirect stderr");
        };

        let poisoned = Poisoned::new();
        let caught = guard(&poisoned, || panic!("guard-log-marker"));

        // Restore stderr before anything else can observe the redirection.
        // SAFETY: restoring descriptors this test owns.
        unsafe {
            assert_eq!(libc::dup2(saved_stderr, 2), 2, "restore stderr");
            libc::close(saved_stderr);
            libc::close(write_fd);
        };

        assert!(caught.is_err());
        assert!(poisoned.is_poisoned());

        let mut buffer = [0u8; 1024];
        // SAFETY: `read` only fills `buffer`, and the write end is closed, so
        // this returns once everything written is drained.
        let len = unsafe { libc::read(read_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        // SAFETY: closing a descriptor this test owns.
        unsafe {
            libc::close(read_fd);
        };
        assert!(len > 0, "the guard wrote to stderr");
        // The length is positive per the assert above, so the conversion
        // cannot fail.
        let len = usize::try_from(len).expect("read length is positive");
        let logged = String::from_utf8_lossy(&buffer[..len]);
        assert!(
            logged.contains("pinwin: caught a panic in a guarded boundary"),
            "the guard's log line is missing: {logged}"
        );
        assert!(
            logged.contains("guard-log-marker"),
            "the panic payload is missing: {logged}"
        );
    }

    /// The relay variant runs the body even when the flag is already
    /// latched — a poisoned latch must not skip the state reset owned by
    /// another module — and a panic in that body is still caught and latched.
    #[test]
    fn guard_always_runs_the_body_even_when_latched() {
        let poisoned = Poisoned::latched();
        let ran = Cell::new(false);
        guard_always(&poisoned, || ran.set(true)).unwrap();
        assert!(ran.get(), "the relay body runs despite the latch");

        assert!(guard_always(&poisoned, || panic!("relay boom")).is_err());
        assert!(poisoned.is_poisoned());
    }

    /// Clones share one latch: poisoning one poisons every clone.
    #[test]
    fn the_poisoned_flag_is_shared_across_clones() {
        let poisoned = Poisoned::new();
        let clone = poisoned.clone();
        assert!(guard(&poisoned, || panic!("shared")).is_err());
        assert!(clone.is_poisoned());
        assert!(
            guard(&clone, || ()).is_err(),
            "the clone short-circuits too"
        );
    }
}
