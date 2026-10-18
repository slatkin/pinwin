//! The raw file descriptors the daemon, the proxy and the client share
//! (replace-terminal-with-app-host D12): one bounded [`poll`] wait, the retry
//! that keeps a wait's deadline across an interrupted call, and the single
//! place a raw descriptor becomes an [`OwnedFd`].
//!
//! Every wait in the crate goes through [`poll`], so no site treats `EINTR`
//! as "not ready": the retry recomputes the time left against the deadline, so
//! a spurious signal cannot turn a client's bounded wait into a false timeout
//! (spec "Command-line exit codes"). [`owned_fd`] keeps the unsafe conversion
//! in one place, with the caller's promise that the descriptor is fresh, so the
//! eventfd, the signalfd, the memfd and the pidfd constructors cannot drift
//! apart.
//!
//! Everything here is display-free and takes its times as parameters, so the
//! tests run without a compositor (`port-to-rust` D10).

use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

/// Wait until one of `fds` reports an event, at most `timeout`; `None` waits
/// forever.
///
/// The call retries an interrupted wait (`EINTR`) with the time that is left,
/// so a signal does not end a client's or a daemon connection's bounded wait
/// early. Any other error is returned as it is, so the caller can tell a real
/// failure from the deadline (spec "Command-line exit codes").
///
/// # Errors
///
/// The error of `poll`, such as `EINVAL` for an unsupported timeout or
/// `ENOMEM` when the kernel cannot allocate its descriptor table.
pub fn poll(fds: &mut [libc::pollfd], timeout: Option<Duration>) -> io::Result<usize> {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    retry_interrupted(deadline, |timeout_ms| {
        let Ok(count) = libc::nfds_t::try_from(fds.len()) else {
            return Err(io::Error::other(
                "the poll set holds more descriptors than the kernel can count",
            ));
        };
        // SAFETY: `fds` is a live slice of `count` `pollfd` entries for the
        // duration of the call, and `poll` writes only their `revents`
        // fields.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), count, timeout_ms) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        usize::try_from(ready)
            .map_err(|error| io::Error::other(format!("poll returned an unusable count: {error}")))
    })
}

/// Run `call` with the timeout that is left until `deadline`, retrying while
/// it reports `EINTR`; `None` waits forever.
fn retry_interrupted(
    deadline: Option<Instant>,
    mut call: impl FnMut(libc::c_int) -> io::Result<usize>,
) -> io::Result<usize> {
    loop {
        let timeout_ms = match deadline {
            None => -1,
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                // Round up: a timeout truncated to whole milliseconds would
                // end every bounded wait a fraction of a millisecond early.
                let millis = left.as_nanos().div_ceil(1_000_000);
                libc::c_int::try_from(millis).unwrap_or(libc::c_int::MAX)
            }
        };
        match call(timeout_ms) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

/// Take ownership of a descriptor a C call just returned.
///
/// # Safety
///
/// `ret` must be a fresh descriptor that no other owner holds: this function
/// takes it over and closes it when the returned [`OwnedFd`] drops. The caller
/// must not use the raw descriptor again except through that owner.
///
/// # Errors
///
/// The error `errno` holds when `ret` is negative, or an error when `ret`
/// does not fit a [`RawFd`].
pub unsafe fn owned_fd(ret: libc::c_long) -> io::Result<OwnedFd> {
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = RawFd::try_from(ret).map_err(|error| {
        io::Error::other(format!("a descriptor that does not fit a RawFd: {error}"))
    })?;
    // SAFETY: the caller promises `ret` is a fresh descriptor that nothing
    // else owns, so this `OwnedFd` is its only owner.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests;
