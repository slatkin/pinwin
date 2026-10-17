//! The daemon `pinwin run` starts when no daemon answers (replace-terminal-
//! with-app-host D12, spec "Run starts the daemon"): the same binary in daemon
//! mode, detached from the caller's terminal and session, and the bounded wait
//! for the display's socket to answer.
//!
//! The child runs `pinwin daemon` in a new session (`setsid`) with stdin,
//! stdout and stderr on `/dev/null`, so closing the caller's terminal neither
//! hangs it up nor writes to a gone terminal. Two starts that race are settled
//! by the daemon's own socket bind: the loser sees a live daemon and exits 0,
//! and both `pinwin run` calls connect to the winner (spec "Two first runs").
//!
//! The wait is a probe of the socket, bounded by [`START_TIMEOUT`] and polled
//! at [`POLL_INTERVAL`], so an already running daemon answers at once and a
//! daemon that cannot start makes `pinwin run` report it instead of hanging.
//! The probe takes the socket path as a parameter, so its tests run without a
//! display or a daemon (`port-to-rust` D10).

use std::fmt;
use std::io;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long `pinwin run` waits for a daemon it started to answer: the same
/// bound as a client's reply wait (D12).
pub const START_TIMEOUT: Duration = crate::ipc::REPLY_TIMEOUT;

/// How long the wait sleeps between two probes of the socket.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Why `pinwin run` did not get a daemon (spec "Run starts the daemon").
#[derive(Debug)]
pub enum StartError {
    /// The daemon process could not be started.
    Spawn(io::Error),
    /// The daemon was started but no daemon answered on the path within
    /// [`START_TIMEOUT`].
    NoAnswer {
        /// The socket path nothing answered on.
        path: PathBuf,
        /// The bound the wait gave up after.
        timeout: Duration,
    },
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "could not start the daemon: {error}"),
            Self::NoAnswer { path, timeout } => write!(
                f,
                "the daemon did not answer on {} within {} s",
                path.display(),
                timeout.as_secs()
            ),
        }
    }
}

impl std::error::Error for StartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(error) => Some(error),
            Self::NoAnswer { .. } => None,
        }
    }
}

/// Start `pinwin daemon` in the background and wait up to [`START_TIMEOUT`]
/// for the display's socket at `path` to answer (spec "Run starts the
/// daemon"). The caller sends its own request once this returns, so it talks
/// to the winner of a race between two starts (D12).
pub fn start_and_wait(path: &Path) -> Result<(), StartError> {
    let program = std::env::current_exe().map_err(StartError::Spawn)?;
    let mut command = Command::new(program);
    command.arg("daemon");
    // The daemon must outlive the caller's terminal, so none of the three
    // standard streams is the terminal's and none can keep it open.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: `setsid` is async-signal-safe and changes only the child's own
    // session between `fork` and `exec`; a failure becomes the spawn error.
    let spawned = unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
        command.spawn()
    };
    let child = spawned.map_err(StartError::Spawn)?;
    // Dropping the child leaves it running; the daemon has its own session
    // and serves the display until `quit` or a signal ends it (D12).
    drop(child);
    if wait_until_answering(path, START_TIMEOUT) {
        Ok(())
    } else {
        Err(StartError::NoAnswer {
            path: path.to_owned(),
            timeout: START_TIMEOUT,
        })
    }
}

/// Wait until a connect to `path` succeeds, bounded by `timeout`.
fn wait_until_answering(path: &Path, timeout: Duration) -> bool {
    wait_until(|| UnixStream::connect(path).is_ok(), timeout, POLL_INTERVAL)
}

/// Poll `probe` until it answers or `timeout` passes, sleeping at most
/// `interval` between two probes and never past the deadline.
fn wait_until(mut probe: impl FnMut() -> bool, timeout: Duration, interval: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if probe() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep(interval.min(deadline.saturating_duration_since(now)));
    }
}

#[cfg(test)]
mod tests {
    use super::{POLL_INTERVAL, wait_until, wait_until_answering};
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A unique short socket path under the system temp dir, so its length
    /// stays below the Unix socket limit.
    fn socket_path(tag: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "pinwin-spawn-test-{}-{tag}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// An already answering socket ends the wait at once (spec "Run starts
    /// the daemon").
    #[test]
    fn the_wait_ends_when_the_socket_answers() {
        let path = socket_path("answering");
        let listener = UnixListener::bind(&path).expect("bind the test socket");
        let started = Instant::now();
        assert!(wait_until_answering(&path, Duration::from_secs(1)));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the first probe answers"
        );
        drop(listener);
        fs::remove_file(&path).expect("remove the test socket");
    }

    /// No socket is reported after the bound, not waited for forever (spec
    /// "Run starts the daemon": "wait a bounded time").
    #[test]
    fn the_wait_gives_up_after_the_bound() {
        let path = socket_path("missing");
        let timeout = Duration::from_millis(30);
        let started = Instant::now();
        assert!(!wait_until_answering(&path, timeout));
        assert!(
            started.elapsed() >= timeout,
            "the wait uses its whole bound"
        );
    }

    /// A probe that starts answering mid-wait ends it, so a daemon that
    /// binds a moment after the first probe is not reported missing.
    #[test]
    fn the_wait_follows_a_probe_that_starts_answering() {
        let mut probes = 0_u32;
        let answered = wait_until(
            || {
                probes += 1;
                probes >= 3
            },
            Duration::from_secs(1),
            POLL_INTERVAL,
        );
        assert!(answered);
        assert_eq!(probes, 3, "no probe runs after the answer");
    }
}
