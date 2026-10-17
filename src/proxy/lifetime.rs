//! One panel's lifetime (replace-terminal-with-app-host D3, spec "Panel
//! lifetime follows the app"): the app's pidfd and the rule that ends the
//! panel.
//!
//! A panel ends when the launched process has exited *and* no app connection
//! is left ([`ends`]). The rule covers every spec scenario: an app that exits
//! (the pidfd reports it and no connection is left); an app that never
//! connects (the launched process exits with none); an app that hands its
//! launch to a running copy and exits (the same, with no connection); and a
//! launcher that forks a child which keeps the connection (the panel stays
//! until that connection closes).
//!
//! [`PidFd`] is the descriptor the panel's loop polls for the process's exit
//! (D1, D3). `pidfd_open` needs Linux 5.3 or later, and a pidfd needs no
//! `waitpid`, so the daemon never becomes the app's parent. The rule itself
//! is a pure function and the pidfd takes the process ID as a parameter, so
//! both are tested without a compositor (`port-to-rust` D10).

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

/// Whether the panel has ended (D3, spec "Panel lifetime follows the app"):
/// the app's process has exited and no connection of the app is left.
///
/// An app that has not exited keeps its panel with no connection, so a panel
/// waits for its first connection as long as the process runs. A connection
/// that outlives the launched process (a forked child, a hand-off) keeps the
/// panel until it closes.
#[must_use]
pub(super) const fn ends(app_exited: bool, connections: usize) -> bool {
    app_exited && connections == 0
}

/// The app's pidfd (D3): readable when the process exits, so the panel ends
/// without parenting the app. `pidfd_open` needs Linux 5.3 or later.
#[derive(Debug)]
pub(super) struct PidFd(OwnedFd);

impl PidFd {
    /// Open the pidfd of the app's process.
    pub(super) fn open(pid: u32) -> io::Result<Self> {
        // SAFETY: `pidfd_open` takes the process ID and no flags and reads no
        // pointer; a failure is a negative return, not a panic.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let Ok(fd) = RawFd::try_from(fd) else {
            return Err(io::Error::other(
                "pidfd_open returned a descriptor that does not fit a RawFd",
            ));
        };
        // SAFETY: `fd` is a fresh descriptor the call above returned and
        // nothing else owns, so this `OwnedFd` is its only owner.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
}

impl AsRawFd for PidFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::process::Command;
    use std::time::Duration;

    use super::{PidFd, ends};

    /// "App never connects" (spec "App never connects"): a launched process
    /// that exits without a connection ends its panel.
    #[test]
    fn an_exited_app_without_a_connection_ends_its_panel() {
        assert!(ends(true, 0));
    }

    /// A running app keeps its panel even before its first connection, so a
    /// panel whose app is slow to connect does not end under it (D3).
    #[test]
    fn a_running_app_keeps_its_panel() {
        assert!(!ends(false, 0));
        assert!(!ends(false, 1));
    }

    /// "Launcher process forks" (spec "Launcher process forks"): the
    /// launcher exits, but the child's connection keeps the panel until that
    /// connection closes.
    #[test]
    fn a_connection_keeps_the_panel_after_the_launcher_exits() {
        assert!(!ends(true, 1));
        assert!(ends(true, 0));
    }

    /// "App hands off to a running copy" (spec "App hands off to a running
    /// copy"): the launched process exits with no connection, so the panel
    /// ends rather than waiting for a window that goes elsewhere.
    #[test]
    fn a_hand_off_ends_the_panel() {
        assert!(ends(true, 0));
    }

    /// The pidfd becomes readable when a short-lived child exits (D3): the
    /// loop's exit watch works without a compositor and without parenting the
    /// app.
    #[test]
    fn the_pidfd_reports_the_process_exit() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("a short-lived child");
        let pidfd = PidFd::open(child.id()).expect("the child's pidfd");
        child.wait().expect("the child is reaped");
        assert!(
            readable(&pidfd, Duration::from_secs(2)),
            "the pidfd reports the child's exit"
        );
    }

    /// Whether `pidfd` is readable within `timeout`, which is how the panel's
    /// loop learns that the app exited. The wait is bounded, so a regression
    /// fails the test instead of hanging it.
    fn readable(pidfd: &PidFd, timeout: Duration) -> bool {
        use std::os::fd::AsRawFd as _;

        let mut watched = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: `watched` is one live `pollfd` and the count is its length;
        // `poll` reads and writes only that array.
        let ready = unsafe { libc::poll(std::ptr::addr_of_mut!(watched), 1, timeout) };
        assert!(ready >= 0, "poll failed: {}", io::Error::last_os_error());
        ready > 0 && watched.revents != 0
    }
}
