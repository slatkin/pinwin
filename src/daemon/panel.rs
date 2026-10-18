//! The daemon's handle on one panel thread (replace-terminal-with-app-host
//! D12): the channel the command travels on, the eventfd that wakes the
//! panel's poll loop, and the bounded wait for the answer.
//!
//! A panel thread owns its state, its Wayland connection and its app's
//! connections (D1), so the daemon cannot call into it: it sends a
//! [`Request`] through [`PanelHandle::call`], writes the [`EventFd`] the
//! panel polls, and waits at most the caller's timeout for the
//! [`PanelReply`]. A panel that does not answer in time makes the request
//! fail with error kind `internal` (spec "The daemon survives bad input"),
//! so one stuck panel never stops another or the daemon.
//!
//! Everything here is display-free and takes its times as parameters, so a
//! plain test thread stands in for a panel (`port-to-rust` D10).

use std::fmt;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::locked;
use crate::ipc::{ErrorReply, PanelStatus, Request};

/// What a panel answers (spec "The daemon survives bad input"): the command
/// was carried out, it was refused with the kind the client gets, or it is
/// the status the daemon asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelReply {
    /// The command was carried out.
    Done,
    /// The panel refused the command, for example a layout the output cannot
    /// hold (`invalid_layout`).
    Refused(ErrorReply),
    /// The panel's settings and view state, for a [`Request::Status`].
    Status(PanelStatus),
}

/// One command on its way to a panel: the request and the channel the panel
/// answers on, so a panel thread serves the commands in the order the daemon
/// sent them (D12).
#[derive(Debug)]
pub struct Command {
    request: Request,
    reply: SyncSender<PanelReply>,
}

impl Command {
    /// What the panel has to do.
    #[must_use]
    pub const fn request(&self) -> &Request {
        &self.request
    }

    /// Answer the daemon. A daemon that stopped waiting for its bounded time
    /// (spec "The daemon survives bad input") has dropped the channel, so a
    /// late answer is not an error.
    pub fn answer(self, reply: PanelReply) {
        let _ = self.reply.send(reply);
    }
}

/// Why the daemon got no answer from a panel, which the router turns into
/// error kind `internal` (spec "The daemon survives bad input").
#[derive(Debug)]
pub enum CommandError {
    /// The panel thread has ended; its channel is closed.
    Ended,
    /// The panel did not answer within the wait.
    Timeout(Duration),
    /// The wake eventfd could not be written.
    Wake(io::Error),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ended => f.write_str("the panel has ended"),
            Self::Timeout(wait) => {
                write!(f, "the panel did not answer within {} ms", wait.as_millis())
            }
            Self::Wake(error) => write!(f, "the panel could not be woken: {error}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wake(error) => Some(error),
            Self::Ended | Self::Timeout(_) => None,
        }
    }
}

/// The hosted app's `app_id` (row 11.1, D14), shared by the panel thread that
/// reads it from the panel window's `xdg_toplevel.set_app_id` and the daemon's
/// tray, which names the oldest panel's app. The value is kept as the app sent
/// it: the tray checks it before using it as a theme icon name.
#[derive(Clone, Debug, Default)]
pub struct AppId(Arc<Mutex<AppIdState>>);

/// The [`AppId`] under its lock: the current value and the callback the panel
/// thread runs after a change, so the tray can re-read the icon. The callback
/// is shared and runs without the lock held.
#[derive(Default)]
struct AppIdState {
    value: Option<String>,
    changed: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl fmt::Debug for AppIdState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppIdState")
            .field("value", &self.value)
            .field("has_changed", &self.changed.is_some())
            .finish()
    }
}

impl AppId {
    /// Record `app_id` and return whether it differs from the current value.
    /// The change callback runs without the lock, so a reader never waits for
    /// it.
    pub fn set(&self, app_id: &str) -> bool {
        let changed = {
            let mut state = locked(&self.0);
            if state.value.as_deref() == Some(app_id) {
                return false;
            }
            state.value = Some(app_id.to_owned());
            state.changed.clone()
        };
        if let Some(changed) = changed {
            changed();
        }
        true
    }

    /// The app id the panel window last set, or `None` before it sets one.
    #[must_use]
    pub fn get(&self) -> Option<String> {
        locked(&self.0).value.clone()
    }

    /// Call `changed` after every later change, so the daemon can refresh the
    /// tray icon when the app names itself or renames itself.
    pub fn on_change(&self, changed: impl Fn() + Send + Sync + 'static) {
        locked(&self.0).changed = Some(Arc::new(changed));
    }
}

/// The daemon's handle on one panel thread (D12): the command channel and
/// the eventfd the panel's poll loop watches. The handle is shared by the
/// daemon's connection threads, so the registry holds it in an [`Arc`]. The
/// command channel is unbounded, because a panel that stops reading must not
/// hold the client's connection thread up: the wait below and the client's
/// own bound end the request instead (spec "Clients cannot block the
/// daemon").
#[derive(Debug)]
pub struct PanelHandle {
    pid: u32,
    commands: Sender<Command>,
    wake: Arc<EventFd>,
    app_id: AppId,
}

impl PanelHandle {
    /// The handle of the panel whose app has process ID `pid`, with the
    /// channel the panel thread reads, the eventfd it polls and the app id the
    /// panel thread records, so the registry and the tray read the same value.
    #[must_use]
    pub fn new(pid: u32, commands: Sender<Command>, wake: Arc<EventFd>, app_id: AppId) -> Self {
        Self {
            pid,
            commands,
            wake,
            app_id,
        }
    }

    /// The app's process ID, as `pinwin run` reported it (D2, spec
    /// "Status").
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// The hosted app's `app_id` as its panel window last set it (row 11.1).
    #[must_use]
    pub fn app_id(&self) -> Option<String> {
        self.app_id.get()
    }

    /// Ask the panel to carry `request` out and wait at most `wait` for its
    /// answer. The command goes on the channel first and the wake after it,
    /// so a panel that polls the eventfd always finds the command in the
    /// channel.
    pub fn call(&self, request: &Request, wait: Duration) -> Result<PanelReply, CommandError> {
        self.send(request, wait)?.finish()
    }

    /// Queue `request` and wake the panel without waiting for the answer, so
    /// a caller can ask several panels at once and collect the answers with
    /// [`PendingCall::finish`]. `wait` runs from this call.
    pub fn send(&self, request: &Request, wait: Duration) -> Result<PendingCall, CommandError> {
        let (reply, answer) = mpsc::sync_channel(1);
        let queued = self.commands.send(Command {
            request: request.clone(),
            reply,
        });
        if queued.is_err() {
            return Err(CommandError::Ended);
        }
        self.wake.notify().map_err(CommandError::Wake)?;
        Ok(PendingCall {
            answer,
            wait,
            deadline: Instant::now() + wait,
        })
    }
}

/// A command a panel has been asked to carry out and has not yet answered.
#[derive(Debug)]
pub struct PendingCall {
    answer: mpsc::Receiver<PanelReply>,
    wait: Duration,
    deadline: Instant,
}

impl PendingCall {
    /// Wait for the panel's answer until the call's deadline.
    pub fn finish(self) -> Result<PanelReply, CommandError> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        self.answer.recv_timeout(left).map_err(|error| match error {
            RecvTimeoutError::Timeout => CommandError::Timeout(self.wait),
            RecvTimeoutError::Disconnected => CommandError::Ended,
        })
    }
}

/// The eventfd that wakes a panel's poll loop when a command is waiting
/// (D12), so the panel's thread never has to poll its channel. Every
/// [`notify`](Self::notify) adds one to the count and
/// [`drain`](Self::drain) reads the whole count in one go, so no wake is
/// lost between commands.
#[derive(Debug)]
pub struct EventFd(OwnedFd);

impl EventFd {
    /// A fresh eventfd, close-on-exec and non-blocking, so a panel's poll
    /// loop can watch it and the daemon can write it from another thread.
    pub fn new() -> io::Result<Self> {
        // SAFETY: `eventfd` writes its result through the file descriptor it
        // returns and reads no pointer; the flags are plain constants, and a
        // failure is a negative return, not a panic.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh descriptor that the call above returned and
        // nothing else owns, so this `OwnedFd` is its only owner.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Wake the panel: one more on the count. A counter that is already full
    /// means the panel has more wake-ups than it can take, which is not a
    /// failure of this call.
    pub fn notify(&self) -> io::Result<()> {
        let one = 1_u64;
        loop {
            // SAFETY: the buffer is a live `u64` and the length is its size,
            // as `eventfd` requires; the descriptor belongs to `self`.
            let written = unsafe {
                libc::write(
                    self.0.as_raw_fd(),
                    std::ptr::addr_of!(one).cast(),
                    size_of::<u64>(),
                )
            };
            if written >= 0 {
                return Ok(());
            }
            match io::Error::last_os_error() {
                error if error.kind() == io::ErrorKind::Interrupted => {}
                error if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                error => return Err(error),
            }
        }
    }

    /// Clear the wake after a command and return how many notifications it
    /// carried, so the panel's loop handles them all in one read; zero means
    /// nothing was waiting.
    pub fn drain(&self) -> io::Result<u64> {
        let mut count = [0_u8; size_of::<u64>()];
        loop {
            // SAFETY: the buffer is `size_of::<u64>()` bytes long, which is
            // the length `eventfd` reads and writes, and the descriptor
            // belongs to `self`.
            let read =
                unsafe { libc::read(self.0.as_raw_fd(), count.as_mut_ptr().cast(), count.len()) };
            if read >= 0 {
                return Ok(u64::from_ne_bytes(count));
            }
            match io::Error::last_os_error() {
                error if error.kind() == io::ErrorKind::Interrupted => {}
                error if error.kind() == io::ErrorKind::WouldBlock => return Ok(0),
                error => return Err(error),
            }
        }
    }
}

impl AsRawFd for EventFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::AppId;

    /// The panel window's `set_app_id` replaces the earlier id, reports a
    /// change only when the value differs, and runs the tray's callback after
    /// each real change (row 11.1, D14).
    #[test]
    fn a_later_app_id_replaces_the_earlier_one() {
        let app_id = AppId::default();
        let changes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&changes);
        app_id.on_change(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(app_id.get(), None, "the app has not named itself yet");
        assert!(app_id.set("org.example.Notes"), "the first id is a change");
        assert_eq!(app_id.get().as_deref(), Some("org.example.Notes"));
        assert_eq!(changes.load(Ordering::SeqCst), 1);

        assert!(!app_id.set("org.example.Notes"), "the same id is no change");
        assert_eq!(changes.load(Ordering::SeqCst), 1);

        assert!(app_id.set("org.example.Chat"), "a new id is a change");
        assert_eq!(app_id.get().as_deref(), Some("org.example.Chat"));
        assert_eq!(changes.load(Ordering::SeqCst), 2);
    }
}
