//! The daemon `pinwin run` starts when no daemon answers (replace-terminal-
//! with-app-host D12, spec "Run starts the daemon"): the same binary in daemon
//! mode, detached from the caller's terminal and session, and the bounded wait
//! for the display's socket to answer.
//!
//! The intermediate child forks once more and exits at once, so `pinwin run`
//! reaps it and the daemon it leaves is reparented to init: the daemon is no
//! child of the process that `exec`s the app, and nothing stays a zombie under
//! an app that does not wait. The daemon runs in a new session (`setsid`) with
//! stdin, stdout and stderr on `/dev/null`, so closing the caller's terminal
//! neither hangs it up nor writes to a gone terminal. Two starts that race are
//! settled by the daemon's own socket bind: the loser sees a live daemon and
//! exits 0, and both `pinwin run` calls connect to the winner (spec "Two first
//! runs").
//!
//! The wait is a probe of the socket, bounded by [`START_TIMEOUT`] and polled
//! at [`POLL_INTERVAL`], so an already running daemon answers at once and a
//! daemon that cannot start makes `pinwin run` report it instead of hanging.
//! The probe takes the socket path as a parameter, so its tests run without a
//! display or a daemon (`port-to-rust` D10).

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read as _};
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

/// How much of the daemon's startup stderr `pinwin run` reports back.
const MAX_STARTUP_OUTPUT: u64 = 8 * 1024;

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
        /// What the started daemon wrote on stderr before it gave up, trimmed;
        /// empty when it wrote nothing.
        output: String,
    },
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "could not start the daemon: {error}"),
            Self::NoAnswer {
                path,
                timeout,
                output,
            } => {
                write!(
                    f,
                    "the daemon did not answer on {} within {} s",
                    path.display(),
                    timeout.as_secs()
                )?;
                if !output.is_empty() {
                    write!(f, ": {output}")?;
                }
                Ok(())
            }
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
    // The daemon must outlive the caller's terminal, so neither stdin nor
    // stdout is the terminal's and neither can keep it open. Its early stderr
    // is collected instead, so a daemon that cannot start can still say why
    // (spec "Wayland layer-shell is required", "Config file").
    let (output, log) = StartupOutput::create(path).map_err(StartError::Spawn)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    detach(&mut command);
    let mut child = command.spawn().map_err(StartError::Spawn)?;
    // The intermediate child exits as soon as it has forked the daemon, so
    // this wait is short. Reaping it here is what keeps no zombie under the
    // app this process becomes; the daemon itself is init's child now (D12).
    child.wait().map_err(StartError::Spawn)?;
    if wait_until(|| UnixStream::connect(path).is_ok(), START_TIMEOUT) {
        // Dropping the log removes it; the detached daemon keeps writing into
        // the unlinked file, which nobody reads.
        Ok(())
    } else {
        Err(StartError::NoAnswer {
            path: path.to_owned(),
            timeout: START_TIMEOUT,
            output: output.read(),
        })
    }
}

/// The file that collects the daemon's early stderr, so `pinwin run` can
/// report why a daemon it started did not answer. Removing it on drop means
/// no log survives a start, and the detached daemon's later stderr goes to
/// the unlinked file and reaches no reader.
struct StartupOutput(PathBuf);

impl StartupOutput {
    /// Create the log in the socket's directory, or in the directory that
    /// will hold it before the daemon binds: under the user's own runtime
    /// directory, so reading it back cannot follow a path another user chose.
    /// A socket path without a usable directory falls back to the system temp
    /// directory.
    fn create(socket: &Path) -> io::Result<(Self, File)> {
        let dir = socket
            .parent()
            .filter(|dir| dir.is_dir())
            .or_else(|| {
                socket
                    .parent()
                    .and_then(Path::parent)
                    .filter(|dir| dir.is_dir())
            })
            .map_or_else(std::env::temp_dir, Path::to_path_buf);
        let path = dir.join(format!("pinwin-start-{}.log", std::process::id()));
        let file = File::create(&path)?;
        Ok((Self(path), file))
    }

    /// What the daemon wrote, trimmed: the message a start error carries.
    fn read(&self) -> String {
        let mut text = String::new();
        if let Ok(file) = File::open(&self.0) {
            let _ = file.take(MAX_STARTUP_OUTPUT).read_to_string(&mut text);
        }
        text.trim().to_owned()
    }
}

impl Drop for StartupOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Detach the spawned command from the caller with a second fork (D12): the
/// intermediate child exits at once, the caller reaps it, and the daemon is
/// reparented to init, so no process stays a zombie under the app the caller
/// `exec`s. The daemon also gets its own session, so no terminal hangs it up.
/// SIGCHLD is deliberately left alone: the `exec`ed app would inherit the
/// disposition.
fn detach(command: &mut Command) {
    // SAFETY: `fork` and `setsid` are async-signal-safe and only the child's
    // own process state changes between `fork` and `exec`; every failure is
    // returned as the spawn error, so `spawn` reports it instead of the daemon
    // leaving no answer.
    unsafe {
        command.pre_exec(|| {
            match libc::fork() {
                -1 => return Err(io::Error::last_os_error()),
                // The intermediate child leaves the daemon to init and exits
                // without running its destructors or flushing the caller's
                // buffers, which `_exit` guarantees.
                0 => {}
                _ => libc::_exit(0),
            }
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Poll `probe` until it answers or `timeout` passes, sleeping at most
/// [`POLL_INTERVAL`] between two probes and never past the deadline.
fn wait_until(mut probe: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if probe() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
    }
}

#[cfg(test)]
mod tests {
    use super::{StartError, StartupOutput, wait_until};
    use std::fs;
    use std::io::Write as _;
    use std::os::unix::net::{UnixListener, UnixStream};
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
        assert!(wait_until(
            || UnixStream::connect(&path).is_ok(),
            Duration::from_secs(1)
        ));
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
        assert!(!wait_until(|| UnixStream::connect(&path).is_ok(), timeout));
        assert!(
            started.elapsed() >= timeout,
            "the wait uses its whole bound"
        );
    }

    /// The daemon's early stderr is read back trimmed, so `pinwin run` can
    /// show why a daemon it started did not answer (spec "Wayland layer-shell
    /// is required", "Config file").
    #[test]
    fn the_startup_output_carries_the_daemons_message() {
        let path = socket_path("startup-output");
        let (output, mut log) = StartupOutput::create(&path).expect("a startup log");
        log.write_all(b"pinwin: no layer-shell on this compositor\n")
            .expect("write the daemon's message");
        assert_eq!(output.read(), "pinwin: no layer-shell on this compositor");
    }

    /// A start error that carries the daemon's message reports it, alongside
    /// the socket and the bound (spec "Run starts the daemon").
    #[test]
    fn a_start_error_reports_the_daemons_message() {
        let error = StartError::NoAnswer {
            path: PathBuf::from("/run/user/1000/pinwin/wayland-0.sock"),
            timeout: Duration::from_secs(2),
            output: "pinwin: no layer-shell on this compositor".to_owned(),
        };
        let message = error.to_string();
        assert!(message.contains("did not answer"), "message = {message}");
        assert!(
            message.contains("no layer-shell on this compositor"),
            "message = {message}"
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
        );
        assert!(answered);
        assert_eq!(probes, 3, "no probe runs after the answer");
    }
}
