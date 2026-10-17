//! The panel thread (replace-terminal-with-app-host D1, D2, D3, D5, D12,
//! D15): one wl-proxy `State` for each panel, the loop that serves it, and
//! the layer-shell check the daemon runs before it starts.
//!
//! A panel owns one thread and one [`State`]: one upstream connection to the
//! compositor, one acceptor (the socket the app connects to) and every client
//! connection of the app (D1). The thread polls that state's `poll_fd`, the
//! daemon's command eventfd and the app's pidfd, so a command never waits for
//! Wayland traffic and the app's exit needs no `waitpid` (D3, D12). Row 6.1
//! is the skeleton: every client request passes through unchanged, the panel
//! has no layer surface yet, and the change commands are answered without
//! effect until rows 6.2 to 6.6 add routing, popups, geometry and outputs.
//!
//! The whole loop runs under the shared panic guard (D5): a panic in one
//! panel drops that panel's state, which closes its app's Wayland
//! connections, and ends only that panel's handle (spec "Panic in one
//! panel"). A panel also ends when the app has exited and no connection is
//! left (D3, spec "Panel lifetime follows the app") and when the daemon drops
//! its handle, which is what `quit` does (D12, spec "Quit").
//!
//! [`spawn`] is the API rows 7.1 to 7.3 call: it starts the thread, waits for
//! the panel's socket and returns the `WAYLAND_DISPLAY` to give the app (D2)
//! together with the handle the daemon registers.
//!
//! [`check`] is the layer-shell check at daemon start (D12, spec "Wayland
//! layer-shell is required"): the daemon connects to the compositor once,
//! looks for `zwlr_layer_shell_v1` and exits 1 before any panel exists when
//! the compositor does not answer or lacks it.

use std::cell::Cell;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread;
use std::time::Duration;

use wl_proxy::acceptor::Acceptor;
use wl_proxy::baseline::Baseline;
use wl_proxy::client::Client;
use wl_proxy::client::ClientHandler;
use wl_proxy::object::{Object, ObjectCoreApi};
use wl_proxy::state::{Destructor, State, StateError, StateHandler};

use crate::config::PanelSettings;
use crate::daemon::lifetime::Lifetime;
use crate::daemon::panel::{Command, EventFd, PanelHandle, PanelReply, PanelRequest};
use crate::geometry::Size;
use crate::guard::{Poisoned, guard};
use crate::ipc::PanelStatus;
use crate::name::PanelName;

// The paths are explicit because a test harness can include this file the way
// `tests/daemon.rs` includes `src/daemon.rs`, and a module loaded that way
// searches its children beside the included file (`src/daemon.rs` does the
// same).
#[path = "proxy/check.rs"]
pub mod check;

#[cfg(test)]
#[path = "proxy/tests.rs"]
mod tests;

/// How many `wayland-N` names the acceptor tries before it gives up: the
/// display the app connects to is `wayland-1` unless another proxy already
/// took it, and the panel's socket is one of the app's own files in the
/// runtime directory.
const ACCEPTOR_TRIES: u32 = 1000;

/// How long one pass waits for the state, the command eventfd and the pidfd.
/// The tick is what ends a panel whose daemon dropped its handle: a closed
/// channel does not wake the eventfd, so the loop notices it at its next
/// pass (D12, spec "Quit").
const TICK: Duration = Duration::from_millis(100);

/// One panel's socket and the handle the daemon registers (D2, D12).
#[derive(Debug)]
pub struct Started {
    display: String,
    handle: PanelHandle,
}

impl Started {
    /// The display the app connects with (D2): the base name of the panel's
    /// socket, for `WAYLAND_DISPLAY`.
    #[must_use]
    pub fn display(&self) -> &str {
        &self.display
    }

    /// The handle the daemon registers under the panel's name (D12). A
    /// refused name drops the handle, which ends the panel thread.
    #[must_use]
    pub fn into_handle(self) -> PanelHandle {
        self.handle
    }
}

/// Start one panel's thread (D1, D2): its `State`, acceptor, command eventfd
/// and the app's pidfd, all served under the panic guard. Returns once the
/// panel's socket exists, so the caller can hand `WAYLAND_DISPLAY` to the
/// app.
pub fn spawn(
    name: PanelName,
    settings: PanelSettings,
    pid: u32,
    lifetime: Arc<Lifetime>,
) -> io::Result<Started> {
    let (commands, incoming) = mpsc::channel::<Command>();
    let wake = Arc::new(EventFd::new()?);
    let (ready, waited) = mpsc::sync_channel(1);
    let thread_wake = Arc::clone(&wake);
    thread::Builder::new()
        .name(format!("pinwin panel {name}"))
        .spawn(move || {
            panel_thread(PanelThread {
                name,
                settings,
                pid,
                lifetime,
                commands: incoming,
                wake: thread_wake,
                ready,
            });
        })?;
    let display = waited.recv().map_err(io::Error::other)??;
    Ok(Started {
        display,
        handle: PanelHandle::new(pid, commands, wake),
    })
}

/// Everything one panel's thread takes: the panel's name and settings, the
/// app's pid, the daemon it reports its end to, and the two ends of the
/// channels that connect it to the daemon (D1, D2, D12).
struct PanelThread {
    name: PanelName,
    settings: PanelSettings,
    pid: u32,
    lifetime: Arc<Lifetime>,
    commands: Receiver<Command>,
    wake: Arc<EventFd>,
    ready: SyncSender<io::Result<String>>,
}

/// The panel thread: build the panel, hand its display back to [`spawn`] and
/// serve it until it ends.
fn panel_thread(thread: PanelThread) {
    let PanelThread {
        name,
        settings,
        pid,
        lifetime,
        commands,
        wake,
        ready,
    } = thread;
    let panel = match Panel::open(&settings, pid, wake, commands) {
        Ok(panel) => panel,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let display = panel.display.clone();
    if ready.send(Ok(display)).is_err() {
        // The daemon stopped waiting for the panel, so it also dropped the
        // handle: there is nobody to serve and the socket closes with this
        // thread.
        return;
    }
    guarded(&lifetime, &name, move || panel.run());
}

/// Run one panel's loop under the panic guard (D5) and then report the
/// panel's end (spec "Panic in one panel": that panel ends, other panels and
/// the daemon keep working). The body owns the panel, so a panic drops its
/// state on the way out, which closes the app's connections.
fn guarded(lifetime: &Lifetime, name: &PanelName, body: impl FnOnce()) {
    let poisoned = Poisoned::new();
    if guard(&poisoned, body).is_err() {
        eprintln!("pinwin: the panel '{name}' ended after a panic");
    }
    lifetime.panel_ended(name);
}

/// What the loop does after it answers the commands that arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Serve {
    /// No command is waiting and the daemon's channel is open.
    Continue,
    /// The daemon dropped the panel's handle, which `quit` does (spec
    /// "Quit").
    End,
}

/// Which of the panel's descriptors woke the loop.
struct Ready {
    wake: bool,
    exited: bool,
}

/// One panel's loop state (D1, D3, D12): the `State`, its acceptor, the
/// destructor that destroys the state on drop, the command channel and
/// eventfd, and the app's pidfd.
struct Panel {
    /// The panel's `WAYLAND_DISPLAY` (D2).
    display: String,
    state: Rc<State>,
    /// Kept so the panel owns its socket; the state also holds a copy for
    /// its poll set.
    _acceptor: Rc<Acceptor>,
    /// Destroys the state when the panel ends, which closes the app's
    /// connections (D5, spec "Panic in one panel").
    _destructor: Destructor,
    wake: Arc<EventFd>,
    commands: Receiver<Command>,
    pidfd: PidFd,
    /// The app's open Wayland connections: the panel ends only after the app
    /// exited *and* this returned to zero (D3).
    connections: Rc<Cell<usize>>,
    settings: PanelSettings,
}

impl Panel {
    /// Open the panel's upstream connection and acceptor (D1, D12). The
    /// state's handler counts the app's connections, the acceptor announces
    /// each one through `new_client`, and the pidfd reports the app's exit
    /// (D3).
    fn open(
        settings: &PanelSettings,
        pid: u32,
        wake: Arc<EventFd>,
        commands: Receiver<Command>,
    ) -> io::Result<Self> {
        let state = State::builder(Baseline::V6)
            .build()
            .map_err(|error| state_error(&error))?;
        let destructor = state.create_destructor();
        let connections = Rc::new(Cell::new(0));
        state.set_handler(PanelState {
            connections: Rc::clone(&connections),
        });
        // `create_acceptor` registers the socket in the state's own poll set
        // and accepts on `dispatch`, so this loop watches only the state's
        // poll fd instead of a fourth descriptor, and `new_client` is where a
        // later row installs the handler of each app connection (D1).
        let acceptor = state
            .create_acceptor(ACCEPTOR_TRIES)
            .map_err(|error| state_error(&error))?;
        let display = acceptor.display().to_owned();
        let pidfd = PidFd::open(pid)?;
        Ok(Self {
            display,
            state,
            _acceptor: acceptor,
            _destructor: destructor,
            wake,
            commands,
            pidfd,
            connections,
            settings: settings.clone(),
        })
    }

    /// Serve the panel until it ends (D3, D5, D12).
    fn run(mut self) {
        let mut app_exited = false;
        loop {
            match self.step(&mut app_exited) {
                Ok(false) => {}
                Ok(true) => return,
                Err(error) => {
                    eprintln!("pinwin: the panel ended: {error}");
                    return;
                }
            }
        }
    }

    /// One pass: answer the daemon's commands, wait for the state, the
    /// command eventfd and the pidfd, dispatch what the state has, and say
    /// whether the panel has ended (`Ok(true)`): the state is gone, the
    /// daemon dropped the handle, or the app exited with no connection left
    /// (D3, spec "Panel lifetime follows the app", "Quit").
    fn step(&mut self, app_exited: &mut bool) -> io::Result<bool> {
        if self.serve() == Serve::End {
            return Ok(true);
        }
        let ready = self.wait()?;
        if ready.wake {
            self.wake.drain().map_err(io::Error::other)?;
            if self.serve() == Serve::End {
                return Ok(true);
            }
        }
        if ready.exited {
            *app_exited = true;
        }
        if self.state.is_destroyed() {
            return Ok(true);
        }
        self.state
            .dispatch(Some(Duration::ZERO))
            .map_err(|error| state_error(&error))?;
        if self.state.is_destroyed() {
            return Ok(true);
        }
        Ok(*app_exited && self.connections.get() == 0)
    }

    /// Take every command the daemon sent and answer it (row 5.3's channel
    /// and eventfd).
    fn serve(&self) -> Serve {
        loop {
            match self.commands.try_recv() {
                Ok(command) => {
                    let reply = self.answer(command.request());
                    command.answer(reply);
                }
                Err(TryRecvError::Empty) => return Serve::Continue,
                Err(TryRecvError::Disconnected) => return Serve::End,
            }
        }
    }

    /// The answer to one command. `Status` reports the settings and the view
    /// state; every other command succeeds without effect until rows 6.4 and
    /// 6.6 give it one (there is no layer surface to change yet).
    fn answer(&self, request: &PanelRequest) -> PanelReply {
        match request {
            PanelRequest::Status => PanelReply::Status(self.status()),
            _ => PanelReply::Done,
        }
    }

    /// The panel's settings and view state (spec "Status"). Row 6.1 has no
    /// window and no layer surface, so `has_window` is false; `shown` is the
    /// settings' own hidden flag, which row 8.2 applies; and a percent size
    /// has no pixel width until row 6.4 reads the output's length.
    fn status(&self) -> PanelStatus {
        let settings = self.settings.clone();
        let shown = !settings.hidden();
        let width = match settings.size() {
            Size::Pixels(px) => px.get(),
            Size::Percent(_) => 0,
        };
        PanelStatus::new(settings, shown, false, width)
    }

    /// Wait for the state's poll fd, the command eventfd or the app's pidfd,
    /// at most one tick. [`State::before_poll`] flushes the proxy's own
    /// requests first, so a request the loop just made is on its way before
    /// the thread sleeps (D12).
    fn wait(&self) -> io::Result<Ready> {
        self.state
            .before_poll()
            .map_err(|error| state_error(&error))?;
        let mut watched = [
            libc::pollfd {
                fd: self.state.poll_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let timeout = libc::c_int::try_from(TICK.as_millis()).unwrap_or(libc::c_int::MAX);
        loop {
            // SAFETY: `watched` is a live array of three `pollfd` values and
            // the count is its length; `poll` reads and writes only that
            // array.
            let ready = unsafe { libc::poll(watched.as_mut_ptr(), 3, timeout) };
            if ready >= 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        // A hangup is an exit for the pidfd and work for the state, so any
        // reported event counts.
        Ok(Ready {
            wake: watched[1].revents != 0,
            exited: watched[2].revents != 0,
        })
    }
}

/// The proxy state's handler (D1, D12): it counts the app's connections,
/// which the end rule needs (D3), and it logs the compositor's protocol
/// error, which ends this panel's connection (spec "App protocol error").
struct PanelState {
    connections: Rc<Cell<usize>>,
}

impl StateHandler for PanelState {
    fn new_client(&mut self, client: &Rc<Client>) {
        self.connections.set(self.connections.get() + 1);
        client.set_handler(PanelClient {
            connections: Rc::clone(&self.connections),
        });
    }

    fn display_error(
        self: Box<Self>,
        object: Option<&Rc<dyn Object>>,
        server_id: u32,
        error: u32,
        msg: &str,
    ) {
        // The error is fatal for the panel's upstream connection (spec "App
        // protocol error"): name the object so the log says what broke it.
        let interface = object.map_or("<deleted object>", |object| object.interface().name());
        eprintln!(
            "pinwin: the compositor ended a panel's connection: {interface}#{server_id} \
             code {error}: {msg}"
        );
    }
}

/// One app connection's handler (D3): the panel's connection count falls
/// when the app closes it.
struct PanelClient {
    connections: Rc<Cell<usize>>,
}

impl ClientHandler for PanelClient {
    fn disconnected(self: Box<Self>) {
        self.connections
            .set(self.connections.get().saturating_sub(1));
    }
}

/// The app's pidfd (D3): readable when the process exits, so the panel ends
/// without parenting the app. `pidfd_open` needs Linux 5.3 or later.
struct PidFd(OwnedFd);

impl PidFd {
    /// Open the pidfd of the app's process.
    fn open(pid: u32) -> io::Result<Self> {
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

/// The daemon's error for a state the proxy could not build or dispatch.
fn state_error(error: &StateError) -> io::Error {
    // The state's error type is not `Send` (it can carry a proxy object), so
    // its message is what the panel's loop can report; the cause is only
    // useful in a log line either way.
    io::Error::other(error.to_string())
}
