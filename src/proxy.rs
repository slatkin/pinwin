//! The panel thread (replace-terminal-with-app-host D1, D2, D3, D5, D12,
//! D15): one wl-proxy `State` for each panel, the loop that serves it, and
//! the layer-shell check the daemon runs before it starts.
//!
//! A panel owns one thread and one [`State`]: one upstream connection to the
//! compositor, one acceptor (the socket the app connects to) and every client
//! connection of the app (D1). The thread polls that state's `poll_fd`, the
//! daemon's command eventfd and the app's pidfd until the pidfd reports the
//! exit, so a command never waits for Wayland traffic and the app's exit
//! needs no `waitpid` (D3, D12). Row 6.2
//! adds window routing (D4): the app's first toplevel becomes the panel's
//! layer surface, every later one passes through, and the panel's `shown` and
//! `has_window` reflect that slot. Row 6.3 adds popups and the per-window
//! decorations (D5): a popup of the panel window opens as a popup of its
//! layer surface, and each decoration object is answered by its toplevel's
//! route. Row 6.4 adds the geometry apply (D9): the reserve surface carries
//! the exclusive zone, a percent size resolves against the panel's output
//! read from `zxdg_output_v1`, and the panel window's buffer is cropped to
//! the app's window geometry through a proxy-owned `wp_viewport`. Row 6.5
//! adds output handling (D11): the panel docks on the output its settings
//! name, a `set-output` re-docks the panel window on the chosen output, and
//! a `closed` layer surface is replaced with a new one instead of closing
//! the app's window. Row 6.6 gives the layout commands their live apply
//! ([`apply`]): edge, size, reserve mode and layer change a running panel in
//! place, on the layer surface it already docks with. Row 8.2 adds hide and
//! show ([`hide`]), and row 8.3 the keyboard modes ([`keyboard`]):
//! `set-keyboard` changes the live interactivity, `focus` shows a hidden
//! panel so its map gives the keyboard, and an on-demand launch withholds
//! the mode until after its first buffer so it steals no focus. Row 9.1 adds
//! the focus ring ([`ring`]): four desynchronized sub-surfaces of the panel
//! surface, shown while the panel holds the keyboard (D10).
//!
//! The whole loop runs under the shared panic guard (D5): a panic in one
//! panel drops that panel's state, which closes its app's Wayland
//! connections, and ends only that panel's handle (spec "Panic in one
//! panel"). A panel also ends when the app has exited and no connection is
//! left (D3, spec "Panel lifetime follows the app") and when the daemon drops
//! its handle, which is what `quit` does (D12, spec "Quit"). Row 7.3 gives
//! the end rule and the pidfd their own module ([`lifetime`]) and frees a
//! slot whose connection went away without destroying its window
//! ([`window`]).
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
use std::fmt;
use std::io;
use std::os::fd::AsRawFd;
use std::rc::{Rc, Weak};
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
use crate::daemon::events::Bus;
use crate::daemon::lifetime::Lifetime;
use crate::daemon::panel::{AppId, Command, EventFd, PanelHandle, PanelReply, PanelRequest};
use crate::guard::{Poisoned, guard};
use crate::ipc::{ErrorReply, PanelStatus};
use crate::name::PanelName;
use lifetime::{PidFd, ends, exited_without_window, pidfd_watch};
use registry::ClientDisplay;
use routing::Routing;

// The layer-shell check at daemon start (D12, spec "Wayland layer-shell is
// required"), which `src/main.rs` calls before the socket bind.
pub(crate) mod check;

// The D4 window-routing handlers and the pure slot rule they decide with.
mod routing;

// The panel's lifetime (row 7.3, D3): the app's pidfd and the end rule.
mod lifetime;

// The panel window's local toplevel handler and the slot's per-connection
// owner (row 7.3, D3, D4).
mod window;

// The panel window's dock on one output (D11): the layer surface, its
// handler, the `set-output` decision and the re-docks.
mod dock;

// The live apply of the layout commands (row 6.6): edge, size, reserve mode
// and layer change a running panel in place, on the layer surface it already
// docks with.
mod apply;

mod slot;

// The settings-to-protocol mappers the routing and the reserve share (D9).
mod mapping;

// The proxy's own globals and the output-size tracking behind percent
// geometry (D9).
mod output;

// The commit coordinator (row 8.1, D7): the dirty flag the app's four
// pending-state requests set and the queue of proxy changes that waits for
// the app's next commit, plus the surface handler that watches both.
mod commit;

// Hide and show (row 8.2, D6): the shown/hidden state, the buffer the next
// show attaches, the releases a hidden panel owes its app and the commits
// that carry them.
mod hide;

// Keyboard modes (row 8.3): `set-keyboard`, `focus` and the launch rule that
// keeps an on-demand panel from stealing the keyboard when it opens.
mod keyboard;

// The panel's focus ring (row 9.1, D10): four desynchronized subsurfaces of
// the panel surface, shown while the panel holds the keyboard.
mod ring;

// Explicit synchronization (row 8.2, D6): the acquire and release points a
// hidden panel holds until a commit carries a buffer.
mod syncobj;

// The reserve surface: the transparent layer surface that owns the
// exclusive zone (D9).
mod reserve;

// The panel window's viewport crop: the client-side title bar and shadow
// stay outside the panel (D5, D8).
mod viewport;

// The viewport composition (row 10.1, D8): the source rectangle and
// destination the proxy's viewport carries, composed from the visible
// fraction, the docked edge, the app's own viewport and the buffer.
mod compose;

// The panel surface's buffer sizes (row 10.1, D8): the bounds the viewport
// crop is clamped to, recorded from `wl_shm` and `zwp_linux_dmabuf_v1`.
mod buffers;

// The animated width transition (row 10.1, D8): the pure grow/shrink state
// machine and the driver that applies each frame through the commit
// coordinator, with the one-size-per-tween configure rule, plus the easing
// step (`src/anim.rs`) both use.
mod tween;

#[path = "anim.rs"]
mod anim;

// The D5 popup handlers: the recorded anchor rectangle and the configure a
// layer popup reports in the app's own coordinates.
mod popup;

// The registry layer: the proxy's own upstream registry, the app's registry
// pass-through and the xdg_wm_base that holds surfaces back (D4, D5).
mod registry;

// The D5 per-window decoration routing: each decoration object is answered
// by its toplevel's route.
mod decoration;

#[cfg(test)]
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

/// Why a panel did not start (D2): its name is taken, or its socket or
/// thread could not start.
#[derive(Debug)]
pub enum StartError {
    /// A panel of that name exists already, which a `run` reports and
    /// leaves alone (spec "Duplicate name").
    Refused(ErrorReply),
    /// The panel's `State`, acceptor or thread failed.
    Io(io::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) => f.write_str(error.message()),
            Self::Io(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for StartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Refused(_) => None,
            Self::Io(error) => Some(error),
        }
    }
}

/// One panel's socket (D2).
#[derive(Debug)]
pub struct Started {
    display: String,
}

impl Started {
    /// The display the app connects with (D2): the base name of the panel's
    /// socket, for `WAYLAND_DISPLAY`.
    #[must_use]
    pub fn display(&self) -> &str {
        &self.display
    }
}

/// Start one panel's thread (D1, D2): its `State`, acceptor, command eventfd
/// and the app's pidfd, all served under the panic guard. Returns once the
/// panel's socket exists and the daemon registered its handle, so the caller
/// can hand `WAYLAND_DISPLAY` to the app.
pub fn spawn(
    name: PanelName,
    settings: PanelSettings,
    app: String,
    pid: u32,
    lifetime: &Arc<Lifetime>,
) -> Result<Started, StartError> {
    let (commands, incoming) = mpsc::channel::<Command>();
    let wake = Arc::new(EventFd::new().map_err(StartError::Io)?);
    let app_id = AppId::default();
    // The tray names the oldest panel's app, so it re-reads its icon when this
    // app names itself. The callback holds the daemon weakly: the registry
    // holds the handle, which holds the id, so a strong reference would keep
    // the daemon alive through its own panel.
    let changed_lifetime = Arc::downgrade(lifetime);
    app_id.on_change(move || {
        if let Some(lifetime) = changed_lifetime.upgrade() {
            lifetime.app_id_changed();
        }
    });
    let (ready, waited) = mpsc::sync_channel(1);
    let (registered, start) = mpsc::sync_channel(1);
    let thread_wake = Arc::clone(&wake);
    let thread_lifetime = Arc::clone(lifetime);
    let thread_name = name.clone();
    let thread_app_id = app_id.clone();
    thread::Builder::new()
        .name(format!("pinwin panel {name}"))
        .spawn(move || {
            panel_thread(PanelThread {
                name: thread_name,
                settings,
                app,
                pid,
                lifetime: thread_lifetime,
                commands: incoming,
                wake: thread_wake,
                ready,
                start,
                app_id: thread_app_id,
            });
        })
        .map_err(StartError::Io)?;
    let display = waited
        .recv()
        .map_err(|error| StartError::Io(io::Error::other(error)))?
        .map_err(StartError::Io)?;
    // The handle is registered before the panel serves anything, so a
    // refused name ends the thread without its end report: that report
    // removes the registry entry of the name, which belongs to the panel
    // that keeps running (spec "Duplicate name").
    let handle = PanelHandle::with_app_id(pid, commands, wake, app_id);
    match lifetime.add_panel(name, handle) {
        Ok(()) => {
            let _ = registered.send(true);
            Ok(Started { display })
        }
        Err(error) => {
            let _ = registered.send(false);
            Err(StartError::Refused(error))
        }
    }
}

/// Everything one panel's thread takes: the panel's name and settings, the
/// app's pid, the daemon it reports its end to, and the two ends of the
/// channels that connect it to the daemon (D1, D2, D12).
struct PanelThread {
    name: PanelName,
    settings: PanelSettings,
    /// The command's file name, which a report names (row 7.4, D3).
    app: String,
    pid: u32,
    lifetime: Arc<Lifetime>,
    commands: Receiver<Command>,
    wake: Arc<EventFd>,
    ready: SyncSender<io::Result<String>>,
    /// Whether the daemon registered the panel's handle, so the thread knows
    /// its end report belongs to a panel of the registry (spec "Duplicate
    /// name").
    start: Receiver<bool>,
    /// The hosted app's `app_id`, shared with the panel's handle (row 11.1).
    app_id: AppId,
}

/// The panel thread: build the panel, hand its display back to [`spawn`] and
/// serve it until it ends.
fn panel_thread(thread: PanelThread) {
    let PanelThread {
        name,
        settings,
        app,
        pid,
        lifetime,
        commands,
        wake,
        ready,
        start,
        app_id,
    } = thread;
    let panel = match Panel::open(
        &settings,
        name.clone(),
        app,
        lifetime.bus().clone(),
        pid,
        wake,
        commands,
        app_id,
    ) {
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
    if !matches!(start.recv(), Ok(true)) {
        // The daemon refused the name, so no registry entry belongs to this
        // panel and its end report would remove the panel that has the name
        // (spec "Duplicate name"). Dropping the panel closes its socket.
        return;
    }
    guarded(&lifetime, &name, move || panel.run());
}

/// Run one panel's loop under the panic guard (D5) and then report the
/// panel's end (spec "Panic in one panel": that panel ends, other panels and
/// the daemon keep working). The body owns the panel, so a panic drops its
/// state on the way out, which closes the app's connections. The body's
/// `Some` is the D3 report of an app that exited without a window (row 7.4,
/// spec "Hand-off app is reported"); a panic is never that report.
fn guarded(lifetime: &Lifetime, name: &PanelName, body: impl FnOnce() -> Option<String>) {
    let poisoned = Poisoned::new();
    let no_window = if let Ok(no_window) = guard(&poisoned, body) {
        no_window
    } else {
        eprintln!("pinwin: the panel '{name}' ended after a panic");
        None
    };
    lifetime.panel_ended(name, no_window.as_deref());
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

/// How one pass ended (D3): keep serving, or stop with the report the daemon
/// needs. `End(Some(app))` is an app that exited without ever connecting
/// (row 7.4, spec "Hand-off app is reported"); every other end is `None`.
enum Step {
    Continue,
    End(Option<String>),
}

/// Which of the panel's descriptors woke the loop.
struct Ready {
    wake: bool,
    /// Whether the pidfd reported the exit, which it cannot do once the loop
    /// dropped it from its poll set (D3).
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
    /// Whether the app ever connected. A panel that ends with this false
    /// reports the app (row 7.4, D3, spec "Hand-off app is reported").
    ever_connected: Rc<Cell<bool>>,
    /// The window routing and the slot its status reads (D4): it also owns
    /// the live settings a change command alters (D11).
    routing: Rc<Routing>,
    /// The panel's name, which its events name (spec "Event stream").
    name: PanelName,
    /// The command's file name, which the D3 report names (row 7.4).
    app: String,
    /// The daemon's event bus, which a successful live apply publishes to
    /// (spec "Event stream").
    bus: Bus,
}

impl Panel {
    /// Open the panel's upstream connection and acceptor (D1, D12). The
    /// state's handler counts the app's connections, the acceptor announces
    /// each one through `new_client`, and the pidfd reports the app's exit
    /// (D3).
    fn open(
        settings: &PanelSettings,
        name: PanelName,
        app: String,
        bus: Bus,
        pid: u32,
        wake: Arc<EventFd>,
        commands: Receiver<Command>,
        app_id: AppId,
    ) -> io::Result<Self> {
        let state = State::builder(Baseline::V6)
            .build()
            .map_err(|error| state_error(&error))?;
        let destructor = state.create_destructor();
        let connections = Rc::new(Cell::new(0));
        let ever_connected = Rc::new(Cell::new(false));
        let routing = Routing::new(settings.clone());
        state.set_handler(PanelState {
            connections: Rc::clone(&connections),
            ever_connected: Rc::clone(&ever_connected),
            routing: Rc::clone(&routing),
            app_id,
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
            ever_connected,
            routing,
            name,
            app,
            bus,
        })
    }

    /// Serve the panel until it ends (D3, D5, D12). The app's name is the
    /// result when the end is the D3 report (row 7.4, spec "Hand-off app is
    /// reported"): the app exited and the panel never had a connection.
    fn run(mut self) -> Option<String> {
        let mut app_exited = false;
        loop {
            match self.step(&mut app_exited) {
                Ok(Step::Continue) => {}
                Ok(Step::End(report)) => return report,
                Err(error) => {
                    eprintln!("pinwin: the panel ended: {error}");
                    return None;
                }
            }
        }
    }

    /// One pass: answer the daemon's commands, apply the tween frames due,
    /// wait for the state, the command eventfd and the pidfd, dispatch what
    /// the state has, and say whether the panel has ended (D3, spec "Panel
    /// lifetime follows the app", "Quit").
    fn step(&mut self, app_exited: &mut bool) -> io::Result<Step> {
        if self.serve() == Serve::End {
            return Ok(Step::End(None));
        }
        // D8: a running tween applies the frames its clock owes and says how
        // soon the loop has to wake again.
        let frame_wait = tween::advance(&self.routing);
        let ready = self.wait(*app_exited, frame_wait)?;
        if ready.wake {
            self.wake.drain().map_err(io::Error::other)?;
            if self.serve() == Serve::End {
                return Ok(Step::End(None));
            }
        }
        if ready.exited {
            *app_exited = true;
        }
        if self.state.is_destroyed() {
            return Ok(Step::End(None));
        }
        self.state
            .dispatch(Some(Duration::ZERO))
            .map_err(|error| state_error(&error))?;
        if self.state.is_destroyed() {
            return Ok(Step::End(None));
        }
        if !ends(*app_exited, self.connections.get()) {
            return Ok(Step::Continue);
        }
        // A panel that never had a connection reports its app (row 7.4, D3,
        // spec "Hand-off app is reported"); one that had a connection does
        // not.
        let report =
            exited_without_window(*app_exited, self.ever_connected.get()).then(|| self.app.clone());
        Ok(Step::End(report))
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

    /// The answer to one command (spec "Status", "Unknown output", row
    /// 6.6). `Status` reports the settings and the view state, `set-output`
    /// validates the name and re-docks the panel window on the chosen
    /// output (D11), the layout commands are the live apply of row 6.6,
    /// `toggle`, `show` and `hide` are row 8.2's state changes (D6), and
    /// `set-keyboard` and `focus` are row 8.3's keyboard modes; every other
    /// command succeeds without effect until its row gives it one.
    fn answer(&self, request: &PanelRequest) -> PanelReply {
        match request {
            PanelRequest::Status => PanelReply::Status(self.status()),
            PanelRequest::SetOutput(name) => {
                match dock::answer_set_output(&self.routing, name, &self.name, &self.bus) {
                    Ok(()) => PanelReply::Done,
                    Err(error) => PanelReply::Refused(error),
                }
            }
            other => hide::answer(&self.routing, other, &self.name, &self.bus)
                .or_else(|| keyboard::answer(&self.routing, other, &self.name, &self.bus))
                .or_else(|| apply::answer(&self.routing, other, &self.name, &self.bus))
                .unwrap_or(PanelReply::Done),
        }
    }

    /// The panel's settings and view state (spec "Status"): `has_window` is
    /// the D4 slot's own answer, `shown` is the hidden state row 8.2 applies
    /// (D6), and a percent size resolves once the panel's output size is
    /// known (row 6.4, D9). The settings come from the routing, so a
    /// `set-output` shows in the reply (D11).
    fn status(&self) -> PanelStatus {
        let settings = self.routing.status_settings();
        let shown = !self.routing.hidden.is_hidden();
        let has_window = self.routing.has_window();
        let width = self.routing.resolved_width();
        PanelStatus::new(settings, shown, has_window, width)
    }

    /// Wait for the state's poll fd, the command eventfd or the app's pidfd,
    /// at most one tick. [`State::before_poll`] flushes the proxy's own
    /// requests first, so a request the loop just made is on its way before
    /// the thread sleeps (D12). The pidfd leaves the set once it reported the
    /// exit, so a panel that a forked child keeps alive sleeps instead of
    /// waking on the readable descriptor every pass (D3, spec "Launcher
    /// process forks").
    fn wait(&self, app_exited: bool, frame_wait: Option<Duration>) -> io::Result<Ready> {
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
            pidfd_watch(self.pidfd.as_raw_fd(), app_exited),
        ];
        // A running tween wakes the loop at its next frame (D8); otherwise
        // the tick is what ends a panel whose daemon dropped its handle.
        let sleep = frame_wait.unwrap_or(TICK).min(TICK);
        let timeout = libc::c_int::try_from(sleep.as_millis()).unwrap_or(libc::c_int::MAX);
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

/// The proxy state's handler (D1, D4, D12): it counts the app's connections,
/// which the end rule needs (D3), installs the window routing and the slot
/// owner of each new connection (D4, row 7.3), and logs the compositor's
/// protocol error, which ends this panel's connection (spec "App protocol
/// error").
struct PanelState {
    connections: Rc<Cell<usize>>,
    /// Latched by the first client, so the panel knows whether the app ever
    /// connected (row 7.4, D3).
    ever_connected: Rc<Cell<bool>>,
    routing: Rc<Routing>,
    /// The hosted app's `app_id`, which the panel window's toplevel handler
    /// records (row 11.1, D14).
    app_id: AppId,
}

impl StateHandler for PanelState {
    fn new_client(&mut self, client: &Rc<Client>) {
        self.connections.set(self.connections.get() + 1);
        self.ever_connected.set(true);
        client.set_handler(PanelClient {
            connections: Rc::clone(&self.connections),
            routing: Rc::clone(&self.routing),
            client: Rc::downgrade(client),
        });
        let display = client.display();
        display.set_handler(ClientDisplay::new(
            Rc::clone(&self.routing),
            display,
            self.app_id.clone(),
        ));
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

/// One app connection's handler (D3, row 7.3): the panel's connection count
/// falls when the app closes it, and a slot this connection owned is freed
/// even though a dying client never destroys its toplevel.
struct PanelClient {
    connections: Rc<Cell<usize>>,
    routing: Rc<Routing>,
    /// The connection this handler belongs to, kept weak so the handler does
    /// not keep its client alive.
    client: Weak<Client>,
}

impl ClientHandler for PanelClient {
    fn disconnected(self: Box<Self>) {
        self.connections
            .set(self.connections.get().saturating_sub(1));
        window::client_disconnected(&self.routing, &self.client);
    }
}

/// The daemon's error for a state the proxy could not build or dispatch.
fn state_error(error: &StateError) -> io::Error {
    // The state's error type is not `Send` (it can carry a proxy object), so
    // its message is what the panel's loop can report; the cause is only
    // useful in a log line either way.
    io::Error::other(error.to_string())
}
