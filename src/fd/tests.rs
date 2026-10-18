//! The shared fd helpers' tests (spec "Command-line exit codes", D12): a
//! bounded wait that expires, a ready descriptor that returns at once, the
//! `EINTR` retry that keeps the time left, a real error that is not a
//! timeout, and the one raw-descriptor hand-over.
//!
//! The waits use a pipe from the standard library, so the tests need no
//! display and no daemon.

use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use super::{owned_fd, poll, retry_interrupted};

/// A wait with no event expires at its deadline instead of blocking.
#[test]
fn a_wait_with_no_event_expires_at_its_deadline() {
    let (reader, _writer) = io::pipe().expect("a pipe");
    let mut fds = [libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    }];
    let started = Instant::now();
    let ready = poll(&mut fds, Some(Duration::from_millis(30))).expect("the wait ends");
    let elapsed = started.elapsed();
    assert_eq!(ready, 0, "nothing was written to the pipe");
    assert!(
        elapsed >= Duration::from_millis(30),
        "the wait expired after {elapsed:?}"
    );
}

/// A descriptor that already has data is ready at once.
#[test]
fn a_ready_descriptor_returns_at_once() {
    let (reader, mut writer) = io::pipe().expect("a pipe");
    writer.write_all(b"x").expect("the write");
    let mut fds = [libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    }];
    assert_eq!(
        poll(&mut fds, Some(Duration::from_secs(1))).expect("the wait"),
        1
    );
    assert_ne!(fds[0].revents & libc::POLLIN, 0);
}

/// `None` waits without a deadline and still returns for a ready descriptor.
#[test]
fn an_unbounded_wait_returns_for_a_ready_descriptor() {
    let (reader, mut writer) = io::pipe().expect("a pipe");
    writer.write_all(b"x").expect("the write");
    let mut fds = [libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    }];
    assert_eq!(poll(&mut fds, None).expect("the wait"), 1);
}

/// A spurious `EINTR` does not end the wait: the retry calls again with the
/// time that is left, so the deadline still bounds the whole call.
#[test]
fn an_interrupted_wait_retries_with_the_time_left() {
    let deadline = Instant::now() + Duration::from_millis(60);
    let mut timeouts = Vec::new();
    let ready = retry_interrupted(Some(deadline), |timeout| {
        timeouts.push(timeout);
        if timeouts.len() == 1 {
            std::thread::sleep(Duration::from_millis(10));
            Err(io::Error::from(io::ErrorKind::Interrupted))
        } else {
            Ok(1)
        }
    })
    .expect("the retry reaches the event");
    assert_eq!(ready, 1);
    assert_eq!(timeouts.len(), 2, "the interrupted call was retried once");
    assert!(
        timeouts[1] < timeouts[0],
        "the retry keeps only the time that is left: {timeouts:?}"
    );
}

/// A real poll error is returned as an error, so a client can tell it from
/// its own timeout (spec "Command-line exit codes").
#[test]
fn a_failed_wait_is_an_error() {
    let error = retry_interrupted(None, |_| Err(io::Error::from_raw_os_error(libc::EINVAL)))
        .expect_err("a failed wait is no event");
    assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
}

/// `owned_fd` refuses a negative return with its `errno`, and takes over a
/// fresh descriptor otherwise.
#[test]
fn a_negative_return_is_an_error_and_a_fresh_descriptor_is_owned() {
    // `close(-1)` fails with EBADF and leaves `errno` set, which is the shape
    // every C call gives `owned_fd` on failure.
    // SAFETY: `close` only closes a descriptor, and -1 names none.
    let failed = unsafe { libc::close(-1) };
    // SAFETY: the negative return names no descriptor, so `owned_fd` only
    // reads `errno`.
    let error = unsafe { owned_fd(failed.into()) }.expect_err("no descriptor");
    assert_eq!(error.raw_os_error(), Some(libc::EBADF));

    // SAFETY: `dup` duplicates the test process's standard input, which the
    // test keeps open.
    let duplicated = unsafe { libc::dup(0) };
    assert!(
        duplicated >= 0,
        "dup failed: {}",
        io::Error::last_os_error()
    );
    // SAFETY: `duplicated` is fresh from `dup` and nothing else owns it.
    let owned = unsafe { owned_fd(duplicated.into()) }.expect("a fresh descriptor");
    assert!(owned.as_raw_fd() >= 0);
}
