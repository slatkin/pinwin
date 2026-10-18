//! The daemon `pinwin run` starts when no daemon answers (replace-terminal-
//! with-app-host D12, spec "Run starts the daemon"): the same binary in daemon
//! mode, detached from the caller's terminal and session, and the bounded wait
//! for it to report readiness over one inherited pipe.
//!
//! The intermediate child forks once more and exits at once, so `pinwin run`
//! reaps it and the daemon it leaves is reparented to init: the daemon is no
//! child of the process that `exec`s the app, and nothing stays a zombie under
//! an app that does not wait. The daemon runs in a new session (`setsid`) with
//! its three standard streams on `/dev/null`, so closing the caller's terminal
//! neither hangs it up nor writes to a gone terminal. Two starts that race are
//! settled by the daemon's own socket bind: the loser sees a live daemon,
//! reports readiness too and exits 0, and both `pinwin run` calls connect to
//! the winner (spec "Two first runs").
//!
//! The daemon reports over the write end of a pipe whose read end stays with
//! `pinwin run`: exactly one `R` byte once it accepts requests, or an `F` byte
//! and then its error message before it exits. EOF with nothing means it died
//! without a report. The parent reads the whole report to EOF before it
//! classifies it, so a message whose first byte is `R` cannot read as ready.
//! The wait is bounded by [`START_TIMEOUT`], so a daemon that hangs cannot
//! hang `pinwin run`. The read takes its descriptor and deadline as parameters, so
//! its tests run without a display or a daemon (`port-to-rust` D10).

use std::fmt;
use std::fs::File;
use std::io::{self, Read as _};
use std::os::fd::{AsRawFd as _, RawFd};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::ready::{self, READY_FD_ENV};

/// How long `pinwin run` waits for a daemon it started to report readiness:
/// the same bound as a client's reply wait (D12).
pub const START_TIMEOUT: Duration = crate::ipc::REPLY_TIMEOUT;

/// How much of the daemon's startup message `pinwin run` reads and reports.
const MAX_READY_MESSAGE: usize = 8 * 1024;

/// What the daemon wrote to its readiness pipe before the deadline.
#[derive(Debug)]
enum Readiness {
    /// The single byte `R`: the daemon accepts requests.
    Ready,
    /// The daemon cannot start; this is the message it reported.
    Failed(String),
    /// The pipe stayed silent for the whole deadline.
    Silent,
}

/// Why `pinwin run` did not get a daemon (spec "Run starts the daemon").
#[derive(Debug)]
pub enum StartError {
    /// The daemon process could not be started.
    Spawn(io::Error),
    /// The daemon reported that it cannot start, with its message.
    Failed(String),
    /// The daemon reported nothing in [`START_TIMEOUT`], so it may still be
    /// starting or it may be stuck.
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
            Self::Failed(message) => write!(f, "the daemon could not start: {message}"),
            Self::NoAnswer { path, timeout } => write!(
                f,
                "the daemon did not answer on {} within {} s",
                path.display(),
                timeout.as_secs()
            ),
        }
    }
}

impl std::error::Error for StartError {}

/// Start `pinwin daemon` in the background and wait up to [`START_TIMEOUT`]
/// for it to report readiness (spec "Run starts the daemon"). The caller
/// sends its own request once this returns, so it talks to the winner of a
/// race between two starts (D12).
///
/// # Errors
///
/// [`StartError::Spawn`] when the process cannot start,
/// [`StartError::Failed`] with the daemon's message when it reports that it
/// cannot start, and [`StartError::NoAnswer`] when the bound passes with no
/// report.
pub fn start_and_wait(path: &Path) -> Result<(), StartError> {
    let program = std::env::current_exe().map_err(StartError::Spawn)?;
    let (ready, writer) = ready::pipe().map_err(StartError::Spawn)?;
    let mut command = Command::new(program);
    command.arg("daemon");
    // The daemon must outlive the caller's terminal, so none of its standard
    // streams is the terminal's, and a later stderr write reaches no reader
    // (D12).
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(READY_FD_ENV, writer.as_raw_fd().to_string());
    detach(&mut command, writer.as_raw_fd());
    let mut child = command.spawn().map_err(StartError::Spawn)?;
    // The parent's write end must go before the wait, or the pipe never
    // reaches EOF when the daemon exits without a report.
    drop(writer);
    // The intermediate child exits as soon as it has forked the daemon, so
    // this wait is short. Reaping it here is what keeps no zombie under the
    // app this process becomes; the daemon itself is init's child now (D12).
    child.wait().map_err(StartError::Spawn)?;
    match read_readiness(&ready, Instant::now() + START_TIMEOUT) {
        Readiness::Ready => Ok(()),
        Readiness::Failed(message) => Err(StartError::Failed(message)),
        Readiness::Silent => Err(StartError::NoAnswer {
            path: path.to_owned(),
            timeout: START_TIMEOUT,
        }),
    }
}

/// Read the daemon's readiness report from `fd` until EOF or `deadline`,
/// then classify the whole report. At most [`MAX_READY_MESSAGE`] bytes are
/// read, so a daemon cannot make `pinwin run` keep reading or allocate without
/// a bound; a pipe that stays open and silent reaches `deadline` as
/// [`Readiness::Silent`].
fn read_readiness(fd: &File, deadline: Instant) -> Readiness {
    let mut report = Vec::new();
    loop {
        if report.len() >= MAX_READY_MESSAGE {
            return classify(&report);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Readiness::Silent;
        }
        let mut watched = [libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        match crate::fd::poll(&mut watched, Some(remaining)) {
            Ok(0) | Err(_) => return Readiness::Silent,
            Ok(_) => {}
        }
        let mut chunk = [0_u8; 1024];
        let mut reader = fd;
        match reader.read(&mut chunk) {
            Ok(0) => return classify(&report),
            Ok(count) => report.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return classify(&report),
        }
    }
}

/// What the complete report `report` says (D12): the one ready byte, a
/// failure marker followed by the daemon's message, or the text a daemon that
/// wrote no marker left behind. The report is classified only after EOF, so a
/// failure message whose first byte is `R` is never mistaken for ready.
fn classify(report: &[u8]) -> Readiness {
    match report {
        [] => Readiness::Failed("it exited before it was ready".to_owned()),
        [byte] if *byte == ready::READY_BYTE => Readiness::Ready,
        [marker, message @ ..] if *marker == ready::FAILURE_BYTE => {
            Readiness::Failed(trimmed(message))
        }
        _ => Readiness::Failed(trimmed(report)),
    }
}

/// The trimmed text of `bytes`, so the parent prints the daemon's message
/// without its trailing newline.
fn trimmed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

/// Detach the spawned command from the caller with a second fork (D12): the
/// intermediate child exits at once, the caller reaps it, and the daemon is
/// reparented to init, so no process stays a zombie under the app the caller
/// `exec`s. The daemon also gets its own session, so no terminal hangs it up,
/// and only its copy of the readiness pipe's write end survives the exec.
/// SIGCHLD is deliberately left alone: the `exec`ed app would inherit the
/// disposition.
fn detach(command: &mut Command, ready_fd: RawFd) {
    // SAFETY: `fork`, `setsid`, `fcntl` and `_exit` are async-signal-safe and
    // only the child's own process state changes between `fork` and `exec`;
    // every failure is returned as the spawn error, so `spawn` reports it
    // instead of the daemon leaving no report.
    unsafe {
        command.pre_exec(move || {
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
            // The pipe is close-on-exec in every process that copies it; the
            // daemon's copy must stay open across its exec to report.
            if libc::fcntl(ready_fd, libc::F_SETFD, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    use super::{MAX_READY_MESSAGE, Readiness, StartError, read_readiness};
    use crate::daemon::ready::pipe;

    /// The report `fd` carries within `timeout`.
    fn read_within(fd: &std::fs::File, timeout: Duration) -> Readiness {
        read_readiness(fd, Instant::now() + timeout)
    }

    /// The ready byte ends the wait at once (spec "Run starts the daemon").
    #[test]
    fn a_ready_byte_is_a_ready_daemon() {
        let (read, mut write) = pipe().expect("a readiness pipe");
        write.write_all(b"R").expect("write the ready byte");
        drop(write);
        assert!(matches!(
            read_within(&read, Duration::from_secs(1)),
            Readiness::Ready
        ));
    }

    /// The daemon's framed message before EOF is reported trimmed, so
    /// `pinwin run` shows why it could not start (spec "Daemon cannot start").
    #[test]
    fn a_message_before_eof_is_a_failure() {
        let (read, mut write) = pipe().expect("a readiness pipe");
        write
            .write_all(b"Fpinwin: bad\n")
            .expect("write the daemon's message");
        drop(write);
        match read_within(&read, Duration::from_secs(1)) {
            Readiness::Failed(message) => assert_eq!(message, "pinwin: bad"),
            readiness => panic!("expected the daemon's message, got {readiness:?}"),
        }
    }

    /// A failure message whose first byte is `R` is not the ready byte, so
    /// `pinwin run` reports the real cause instead of a connect error (spec
    /// "Daemon cannot start").
    #[test]
    fn a_failure_that_starts_with_the_ready_byte_is_a_failure() {
        let (read, mut write) = pipe().expect("a readiness pipe");
        write
            .write_all(b"FRead-only file system (os error 30)\n")
            .expect("write the daemon's message");
        drop(write);
        match read_within(&read, Duration::from_secs(1)) {
            Readiness::Failed(message) => {
                assert_eq!(message, "Read-only file system (os error 30)");
            }
            readiness => panic!("expected the daemon's message, got {readiness:?}"),
        }
    }

    /// The ready byte followed by anything else is not a ready report, so a
    /// message that happens to start with `R` cannot pass as ready.
    #[test]
    fn the_ready_byte_with_more_bytes_is_a_failure() {
        let (read, mut write) = pipe().expect("a readiness pipe");
        write
            .write_all(b"Rmore")
            .expect("write the daemon's report");
        drop(write);
        match read_within(&read, Duration::from_secs(1)) {
            Readiness::Failed(message) => assert_eq!(message, "Rmore"),
            readiness => panic!("expected the whole report, got {readiness:?}"),
        }
    }

    /// EOF with no byte is a daemon that ended before it was ready (spec "Run
    /// starts the daemon").
    #[test]
    fn an_empty_eof_is_a_daemon_that_ended_early() {
        let (read, write) = pipe().expect("a readiness pipe");
        drop(write);
        match read_within(&read, Duration::from_secs(1)) {
            Readiness::Failed(message) => {
                assert_eq!(message, "it exited before it was ready");
            }
            readiness => panic!("expected an early end, got {readiness:?}"),
        }
    }

    /// A pipe that stays open and silent ends as no answer after the whole
    /// bound (spec "Run starts the daemon": "wait a bounded time").
    #[test]
    fn a_silent_pipe_gives_up_after_the_bound() {
        let (read, _write) = pipe().expect("a readiness pipe");
        let timeout = Duration::from_millis(30);
        let started = Instant::now();
        assert!(matches!(read_within(&read, timeout), Readiness::Silent));
        assert!(
            started.elapsed() >= timeout,
            "the wait uses its whole bound"
        );
    }

    /// A report longer than the bound is trimmed, so a daemon cannot make
    /// `pinwin run` read without a bound.
    #[test]
    fn a_long_message_is_trimmed() {
        let (read, mut write) = pipe().expect("a readiness pipe");
        let mut report = vec![b'x'; MAX_READY_MESSAGE + 128];
        report[0] = b'F';
        write.write_all(&report).expect("write the long message");
        drop(write);
        match read_within(&read, Duration::from_secs(1)) {
            Readiness::Failed(message) => {
                assert_eq!(
                    message.len(),
                    MAX_READY_MESSAGE - 1,
                    "the message is trimmed"
                );
            }
            readiness => panic!("expected the trimmed message, got {readiness:?}"),
        }
    }

    /// A start error that carries the daemon's message reports it, so
    /// `pinwin run` prints one line that names the cause (spec "Daemon cannot
    /// start").
    #[test]
    fn a_start_error_reports_the_daemons_message() {
        let error = StartError::Failed("config.toml:2: invalid value 'middle' for edge".to_owned());
        assert_eq!(
            error.to_string(),
            "the daemon could not start: config.toml:2: invalid value 'middle' for edge"
        );
    }
}
