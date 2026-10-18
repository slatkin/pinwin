//! The readiness pipe between `pinwin run` and the daemon it starts
//! (replace-terminal-with-app-host D12, spec "Run starts the daemon"): both
//! ends start close-on-exec, the parent names the write end in
//! [`READY_FD_ENV`], and the daemon adopts it at start to report one `R` byte
//! once it accepts requests, or an `F` byte and then its error message before
//! it exits. The marker and the parent's read to EOF keep the two apart, so a
//! message whose first byte is `R` cannot read as ready.
//!
//! [`ReadyPipe::from_env`] accepts only an inherited pipe above the three
//! standard streams, so a manual `pinwin daemon`, a missing variable or a
//! stray value behaves as before. Adopting the descriptor sets `FD_CLOEXEC`
//! again, so no process the daemon might start keeps the pipe open.
//!
//! Everything here takes its descriptors from the caller, so the tests run
//! without a daemon or a display (`port-to-rust` D10).

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::RawFd;

use crate::fd::owned_fd;

/// The variable that names the write end of the pipe in the daemon's
/// environment. It is set on the spawned command only, so the app `pinwin run`
/// later `exec`s never sees it.
pub const READY_FD_ENV: &str = "PINWIN_READY_FD";

/// The byte the daemon writes when it accepts requests (D12).
pub const READY_BYTE: u8 = b'R';

/// The byte that introduces a failure report, so a message whose first byte
/// is `R` cannot read as ready (D12).
pub const FAILURE_BYTE: u8 = b'F';

/// A fresh pipe whose two ends are both close-on-exec: `pinwin run` keeps the
/// read end and passes the write end to the daemon.
///
/// # Errors
///
/// The error of `pipe2`, such as `EMFILE` when the process is out of
/// descriptors.
pub(crate) fn pipe() -> io::Result<(File, File)> {
    let mut fds: [RawFd; 2] = [-1; 2];
    // SAFETY: `fds` is a live array of two descriptors and `pipe2` writes
    // both of them or reports an error.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both are fresh descriptors from `pipe2`, and nothing else in
    // this process owns them.
    let read = unsafe { owned_fd(fds[0].into())? };
    // SAFETY: the same holds for the second descriptor.
    let write = unsafe { owned_fd(fds[1].into())? };
    Ok((File::from(read), File::from(write)))
}

/// The daemon's adopted write end of the readiness pipe.
#[derive(Debug)]
pub struct ReadyPipe(File);

impl ReadyPipe {
    /// Adopt the descriptor `value` names, or `None` when the value is not a
    /// number, is one of the three standard streams, is closed, or is not a
    /// pipe (spec "Run starts the daemon": a manual daemon has no pipe).
    #[must_use]
    pub fn from_env(value: Option<&OsStr>) -> Option<Self> {
        let fd: RawFd = value?.to_str()?.parse().ok()?;
        if fd <= 2 {
            return None;
        }
        // SAFETY: `fcntl` only reads the flags of `fd`; a descriptor that is
        // not open reports -1.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
            return None;
        }
        // SAFETY: `stat` is a plain C struct with no invalid bit pattern and
        // `fstat` fills it before returning.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is open and `stat` is a valid out-parameter.
        if unsafe { libc::fstat(fd, std::ptr::addr_of_mut!(stat)) } == -1 {
            return None;
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
            return None;
        }
        // SAFETY: `fd` is the open pipe the parent inherited, and the daemon
        // starts no process that should keep it open.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return None;
        }
        // SAFETY: the descriptor is the one this process inherited from
        // `pinwin run`; nothing else in this process owns it, so the `File`
        // becomes its only owner.
        let owned = unsafe { owned_fd(fd.into()) }.ok()?;
        Some(Self(File::from(owned)))
    }

    /// Report that the daemon accepts requests (spec "Run starts the
    /// daemon"). A caller that gave up reading the pipe is not an error: the
    /// daemon keeps running for the next request.
    pub fn report_ready(&self) {
        self.write(&[READY_BYTE]);
    }

    /// Report why the daemon cannot start (spec "Daemon cannot start"):
    /// [`FAILURE_BYTE`] and then the message, so `pinwin run` prints the
    /// message and exits 1 at once, and a message whose first byte is `R`
    /// cannot read as the ready byte.
    pub fn report_failure(&self, message: &str) {
        let mut report = Vec::with_capacity(1 + message.len());
        report.push(FAILURE_BYTE);
        report.extend_from_slice(message.as_bytes());
        self.write(&report);
    }

    /// Write one report and ignore a reader that already went away.
    fn write(&self, bytes: &[u8]) {
        let mut pipe = &self.0;
        let _ = pipe.write_all(bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::io::Read as _;
    use std::os::fd::{AsRawFd as _, RawFd};

    use super::{ReadyPipe, pipe};

    /// The environment value for `fd`.
    fn value(fd: RawFd) -> String {
        fd.to_string()
    }

    /// A missing variable leaves a manual daemon alone.
    #[test]
    fn no_value_is_ignored() {
        assert!(ReadyPipe::from_env(None).is_none());
    }

    /// A value that is not a number is ignored.
    #[test]
    fn a_non_number_is_ignored() {
        assert!(ReadyPipe::from_env(Some(OsStr::new("abc"))).is_none());
    }

    /// The standard streams are never the readiness pipe.
    #[test]
    fn a_standard_stream_is_refused() {
        assert!(ReadyPipe::from_env(Some(OsStr::new("1"))).is_none());
    }

    /// A descriptor number that nothing holds is ignored.
    #[test]
    fn a_closed_descriptor_is_refused() {
        let closed: RawFd = 1000;
        // SAFETY: `fcntl` only reads the flags of the number, which no test
        // process opens.
        let flags = unsafe { libc::fcntl(closed, libc::F_GETFD) };
        assert_eq!(flags, -1, "the test descriptor is closed");
        assert!(ReadyPipe::from_env(Some(OsStr::new(&value(closed)))).is_none());
    }

    /// An open descriptor that is not a pipe is ignored.
    #[test]
    fn a_regular_file_is_refused() {
        let path =
            std::env::temp_dir().join(format!("pinwin-ready-test-{}.txt", std::process::id()));
        let file = fs::File::create(&path).expect("a test file");
        assert!(ReadyPipe::from_env(Some(OsStr::new(&value(file.as_raw_fd())))).is_none());
        drop(file);
        fs::remove_file(&path).expect("remove the test file");
    }

    /// A real pipe is adopted, and its report reaches the read end (spec "Run
    /// starts the daemon").
    #[test]
    fn a_pipe_is_adopted_and_reports() {
        let (mut read, write) = pipe().expect("a readiness pipe");
        let fd = write.as_raw_fd();
        let adopted = ReadyPipe::from_env(Some(OsStr::new(&value(fd))));
        // The test's `write` handle names the descriptor the adopted pipe
        // owns, so it must not close it a second time.
        std::mem::forget(write);
        let adopted = adopted.expect("a pipe is adopted");
        adopted.report_ready();
        drop(adopted);
        let mut report = String::new();
        read.read_to_string(&mut report)
            .expect("read the readiness report");
        assert_eq!(report, "R", "the ready byte reaches the parent");
    }

    /// A failure report carries the marker before its message, so the parent
    /// can tell it from the ready byte even when the message starts with `R`
    /// (spec "Daemon cannot start").
    #[test]
    fn a_failure_report_is_framed() {
        let (mut read, write) = pipe().expect("a readiness pipe");
        let fd = write.as_raw_fd();
        let adopted = ReadyPipe::from_env(Some(OsStr::new(&value(fd))));
        // The test's `write` handle names the descriptor the adopted pipe
        // owns, so it must not close it a second time.
        std::mem::forget(write);
        let adopted = adopted.expect("a pipe is adopted");
        adopted.report_failure("Read-only file system (os error 30)");
        drop(adopted);
        let mut report = String::new();
        read.read_to_string(&mut report)
            .expect("read the readiness report");
        assert_eq!(
            report, "FRead-only file system (os error 30)",
            "the failure marker precedes the daemon's message"
        );
    }
}
