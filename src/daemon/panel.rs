//! The daemon's handle on one panel thread (replace-terminal-with-app-host
//! D12): the channel the command travels on, the eventfd that wakes the
//! panel's poll loop, and the bounded wait for the answer.
//!
//! A panel thread owns its state, its Wayland connection and its app's
//! connections (D1), so the daemon cannot call into it: it sends a
//! [`PanelRequest`] through [`PanelHandle::call`], writes the [`EventFd`] the
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
use std::time::Duration;

use super::locked;
use crate::cli::Amount;
use crate::config::{Keyboard, Layer};
use crate::geometry::{Edge, ReserveMode, Size};
use crate::ipc::{ErrorReply, PanelStatus};

/// What the daemon asks a panel to do (spec "Panel commands", "Status"): one
/// variant for each request that names a panel, without the name — the
/// registry already chose the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelRequest {
    /// Show or hide the panel.
    Toggle,
    /// Show the panel.
    Show,
    /// Hide the panel.
    Hide,
    /// Show the panel and give it the keyboard.
    Focus,
    /// Dock the panel at `edge`.
    SetEdge(Edge),
    /// Set the panel's size, over `ms` milliseconds or its `tween_ms`.
    SetSize { size: Size, ms: Option<u32> },
    /// Change the panel's size by `by`, over `ms` milliseconds or its
    /// `tween_ms`.
    Resize { by: Amount, ms: Option<u32> },
    /// Open the panel on another output.
    SetOutput(String),
    /// Draw the panel in another layer.
    SetLayer(Layer),
    /// Push or cover the strip the panel occupies.
    SetReserve(ReserveMode),
    /// Set the panel's keyboard interactivity.
    SetKeyboard(Keyboard),
    /// Report the panel's settings and view state.
    Status,
}

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
    /// The panel's settings and view state, for a [`PanelRequest::Status`].
    Status(PanelStatus),
}

/// One command on its way to a panel: the request and the channel the panel
/// answers on, so a panel thread serves the commands in the order the daemon
/// sent them (D12).
#[derive(Debug)]
pub struct Command {
    request: PanelRequest,
    reply: SyncSender<PanelReply>,
}

impl Command {
    /// What the panel has to do.
    #[must_use]
    pub const fn request(&self) -> &PanelRequest {
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
    commands: Mutex<Sender<Command>>,
    wake: Arc<EventFd>,
}

impl PanelHandle {
    /// The handle of the panel whose app has process ID `pid`, with the
    /// channel the panel thread reads and the eventfd it polls.
    #[must_use]
    pub fn new(pid: u32, commands: Sender<Command>, wake: Arc<EventFd>) -> Self {
        Self {
            pid,
            commands: Mutex::new(commands),
            wake,
        }
    }

    /// The app's process ID, as `pinwin run` reported it (D2, spec
    /// "Status").
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Ask the panel to carry `request` out and wait at most `wait` for its
    /// answer. The command goes on the channel first and the wake after it,
    /// so a panel that polls the eventfd always finds the command in the
    /// channel.
    pub fn call(&self, request: PanelRequest, wait: Duration) -> Result<PanelReply, CommandError> {
        let (reply, answer) = mpsc::sync_channel(1);
        let queued = locked(&self.commands).send(Command { request, reply });
        if queued.is_err() {
            return Err(CommandError::Ended);
        }
        self.wake.notify().map_err(CommandError::Wake)?;
        answer.recv_timeout(wait).map_err(|error| match error {
            RecvTimeoutError::Timeout => CommandError::Timeout(wait),
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
