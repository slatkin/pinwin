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
//! `waitpid`, so the daemon never becomes the app's parent. [`pidfd_watch`]
//! says when the loop drops that descriptor from its poll set, so a panel that
//! outlives its launched process sleeps instead of spinning. The rule itself
//! is a pure function, the poll entry and the pidfd take their inputs as
//! parameters, so all three are tested without a compositor (`port-to-rust`
//! D10).

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

/// Whether the daemon reports the app (row 7.4, D3, spec "Hand-off app is
/// reported"): the panel ended because the app exited and it never had a
/// connection. An app that connected and then exited is not reported.
#[must_use]
pub(super) const fn exited_without_window(app_exited: bool, ever_connected: bool) -> bool {
    app_exited && !ever_connected
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

/// The pidfd's entry in the panel's poll set (D3): the app's descriptor until
/// the loop has seen the exit, and `poll`'s ignored descriptor after it.
///
/// A pidfd stays readable once the process exits, and `poll` reports a
/// readable descriptor on every pass, so a panel that kept watching it would
/// spin for as long as a forked child or a remaining connection keeps it alive
/// (spec "Launcher process forks"). `poll` skips a negative descriptor, so the
/// panel sleeps until a Wayland event, a command or the last connection
/// closes instead.
#[must_use]
pub(super) const fn pidfd_watch(pidfd: RawFd, app_exited: bool) -> libc::pollfd {
    libc::pollfd {
        // A negative descriptor is `poll`'s own "ignore this entry" value and
        // is not a descriptor, so nothing closes it.
        fd: if app_exited { -1 } else { pidfd },
        events: libc::POLLIN,
        revents: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::fd::AsRawFd;
    use std::process::Command;
    use std::time::Duration;

    use super::{PidFd, ends, exited_without_window, pidfd_watch};

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

    /// "Hand-off app is reported" (spec "Hand-off app is reported", row
    /// 7.4): a panel reports its app only when the app exited and the panel
    /// never had a connection. An app that connected before it exited is
    /// not reported.
    #[test]
    fn an_exited_app_is_reported_only_when_it_never_connected() {
        assert!(exited_without_window(true, false));
        assert!(!exited_without_window(true, true));
        assert!(!exited_without_window(false, false));
        assert!(!exited_without_window(false, true));
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

    /// An exited app's pidfd leaves the poll set (spec "Launcher process
    /// forks", D3): the entry names no descriptor, so `poll` ignores it and
    /// the panel's loop sleeps even though the pidfd stays readable.
    #[test]
    fn the_poll_set_drops_the_pidfd_of_an_exited_app() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("a short-lived child");
        let pidfd = PidFd::open(child.id()).expect("the child's pidfd");
        child.wait().expect("the child is reaped");
        assert!(
            triggered(
                pidfd_watch(pidfd.as_raw_fd(), false),
                Duration::from_secs(2)
            ),
            "a watched pidfd reports the exit"
        );
        assert_eq!(pidfd_watch(pidfd.as_raw_fd(), true).fd, -1);
        assert!(
            !triggered(
                pidfd_watch(pidfd.as_raw_fd(), true),
                Duration::from_millis(20)
            ),
            "the loop no longer wakes on the readable pidfd"
        );
    }

    /// Whether one poll entry reports an event within `timeout`, so a test
    /// never waits without a bound.
    fn triggered(watched: libc::pollfd, timeout: Duration) -> bool {
        let mut watched = watched;
        let timeout = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: `watched` is one live `pollfd` and the count is its length;
        // `poll` reads and writes only that value.
        let ready = unsafe { libc::poll(std::ptr::addr_of_mut!(watched), 1, timeout) };
        assert!(ready >= 0, "poll failed: {}", io::Error::last_os_error());
        ready > 0 && watched.revents != 0
    }

    /// Whether `pidfd` is readable within `timeout`, which is how the panel's
    /// loop learns that the app exited. The wait is bounded, so a regression
    /// fails the test instead of hanging it.
    fn readable(pidfd: &PidFd, timeout: Duration) -> bool {
        triggered(pidfd_watch(pidfd.as_raw_fd(), false), timeout)
    }
}
