//! The daemon's lifetime (replace-terminal-with-app-host D12, spec "Daemon
//! lifetime preference", "Quit", "Reload"): the accept loop, the shutdown
//! trigger, the `quit` and `reload` requests, and the SIGINT and SIGTERM
//! handling.
//!
//! [`Lifetime`] is the state every connection thread shares: the registry,
//! the event bus, the config the daemon holds (and reloads) and the
//! [`Shutdown`] trigger. It answers the requests the registry leaves to the
//! daemon: `quit` ends every panel by closing its command channel and wakes
//! the accept loop, and `reload` reads the config file again and keeps the
//! previous config when the file is invalid. A panel thread reports its end
//! with [`Lifetime::panel_ended`], which exits the daemon when the last panel
//! ends and `exit_when_idle` is true; a daemon that has not had a panel yet
//! never exits on its own (spec "Daemon lifetime preference").
//!
//! [`Daemon`] owns the socket and runs the accept loop: it polls the
//! listener together with the shutdown eventfd and, when the daemon has
//! them, a [`SignalFd`] for SIGINT and SIGTERM (D12). The loop takes the
//! shutdown trigger from the eventfd rather than from a signal, so a test
//! fires it directly and no test sends a real signal; only
//! [`shutdown_signal`] maps a signal number to the `quit` behaviour, and
//! that mapping has its own test.
//!
//! The socket file is removed when the [`DaemonSocket`] drops, and the
//! [`Daemon`] owns it until the loop returns, so every exit path removes the
//! file (spec "Daemon socket"). The waits and the queue depth are
//! parameters, so a test runs fast (`port-to-rust` D10).

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::connection;
use super::events::Bus;
use super::panel::{EventFd, PanelHandle};
use super::registry::Registry;
use super::{DaemonSocket, locked};
use crate::config::{Config, DaemonSettings, PanelOptions, PanelSettings};
use crate::ipc::{ErrorKind, ErrorReply, Reply};
use crate::name::PanelName;

/// The daemon's shared lifetime (D12): what a connection thread needs to
/// answer a request, plus the state that decides when the accept loop ends.
#[derive(Debug)]
pub struct Lifetime {
    /// The panels, shared with every connection thread.
    registry: Registry,
    /// The events a connection's `subscribe` streams (D12).
    bus: Bus,
    /// The file [`Lifetime::reload`] reads and the path its errors name.
    config_path: PathBuf,
    /// The config the daemon holds: the file at start, or the file a valid
    /// `reload` read. A panel that starts takes its settings from it; a
    /// panel that exists keeps its own, so a reload never changes one (spec
    /// "Reload", "Runtime changes stay in memory").
    config: Mutex<Config>,
    /// The trigger the accept loop watches (D12).
    shutdown: Shutdown,
    /// Whether a panel ever started, so an idle daemon that never had one
    /// stays resident (spec "Daemon lifetime preference").
    had_panel: AtomicBool,
    /// The bound of every wait for a panel, so a test uses a short one
    /// (spec "The daemon survives bad input", `port-to-rust` D10).
    wait: Duration,
}

impl Lifetime {
    /// The lifetime of a daemon that reads `config_path` and answers a panel
    /// within `wait`, with a fresh event bus.
    pub fn new(config_path: PathBuf, config: Config, wait: Duration) -> io::Result<Self> {
        Self::with_bus(config_path, config, Bus::new(), wait)
    }

    /// [`Lifetime::new`] with the event bus the caller made, so a test sets
    /// the queue depth a slow subscriber needs (spec "Slow subscriber").
    pub fn with_bus(
        config_path: PathBuf,
        config: Config,
        bus: Bus,
        wait: Duration,
    ) -> io::Result<Self> {
        Ok(Self {
            registry: Registry::new(),
            bus,
            config_path,
            config: Mutex::new(config),
            shutdown: Shutdown::new()?,
            had_panel: AtomicBool::new(false),
            wait,
        })
    }

    /// The panels, keyed by name and listed in start order (D12).
    #[must_use]
    pub const fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The event bus a `subscribe` streams (spec "Event stream").
    #[must_use]
    pub const fn bus(&self) -> &Bus {
        &self.bus
    }

    /// The bound of every wait for a panel.
    #[must_use]
    pub const fn wait(&self) -> Duration {
        self.wait
    }

    /// Whether the accept loop should end (spec "Daemon lifetime
    /// preference", "Quit").
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.is_triggered()
    }

    /// Register a new panel under `name` (spec "Panel names", "Duplicate
    /// name"): the name must be free, and a registered panel makes the
    /// daemon eligible to exit with its last panel.
    pub fn add_panel(&self, name: PanelName, handle: PanelHandle) -> Result<(), ErrorReply> {
        self.registry.add(name, handle)?;
        self.had_panel.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Drop the panel named `name` when its app ends (spec "Name is free
    /// again"). When it was the last panel and `exit_when_idle` is true,
    /// the daemon exits (spec "Exit with the last panel"); otherwise it
    /// stays resident (spec "Stay resident"). False when no panel had the
    /// name.
    pub fn panel_ended(&self, name: &PanelName) -> bool {
        let removed = self.registry.remove(name);
        if removed && self.registry.is_empty() && self.exits_when_idle() {
            self.shutdown.trigger();
        }
        removed
    }

    /// End every panel and then the daemon (spec "Quit"): clearing the
    /// registry drops each panel's handle, which closes its command channel
    /// and so ends the panel thread, and the trigger wakes the accept loop.
    pub fn quit(&self) {
        self.registry.clear();
        self.shutdown.trigger();
    }

    /// Read the config file again (spec "Reload"): a valid file replaces the
    /// config for panels that start later and for the daemon preferences,
    /// and existing panels do not change. An invalid file keeps the previous
    /// config and replies with kind `invalid_config`, naming the file and
    /// the line.
    #[must_use]
    pub fn reload(&self) -> Reply {
        match Config::load(&self.config_path) {
            Ok(config) => {
                *locked(&self.config) = config;
                Reply::ok()
            }
            Err(error) => Reply::error(ErrorKind::InvalidConfig, error.to_string()),
        }
    }

    /// `name`'s settings for a panel that starts now: the options, the
    /// config file's tables and the built-in defaults (spec "Panel settings
    /// and precedence"). A reload changes what a new panel gets, never what
    /// an existing panel has.
    #[must_use]
    pub fn panel_settings(&self, name: &PanelName, options: &PanelOptions) -> PanelSettings {
        locked(&self.config).panel(name, options)
    }

    /// The daemon preferences of the config the daemon holds (spec "Daemon
    /// lifetime preference"), which a valid `reload` replaces.
    #[must_use]
    pub fn daemon_settings(&self) -> DaemonSettings {
        locked(&self.config).daemon()
    }

    /// Whether the daemon exits with its last panel: `exit_when_idle` is
    /// true and a panel has ended before (spec "Daemon lifetime
    /// preference": a daemon with no panel "SHALL NOT exit before its first
    /// panel has ended").
    fn exits_when_idle(&self) -> bool {
        self.had_panel.load(Ordering::SeqCst) && self.daemon_settings().exit_when_idle()
    }
}

/// The daemon's shutdown trigger (D12): a flag the accept loop checks and an
/// eventfd that wakes it from `poll`. A connection thread and a panel thread
/// both set it, so the flag is atomic and the wake only shortens the wait.
#[derive(Debug)]
struct Shutdown {
    triggered: AtomicBool,
    wake: EventFd,
}

impl Shutdown {
    /// An untriggered shutdown with a fresh eventfd.
    fn new() -> io::Result<Self> {
        Ok(Self {
            triggered: AtomicBool::new(false),
            wake: EventFd::new()?,
        })
    }

    /// Ask the accept loop to end. The flag is set first, so a wake that
    /// fails only delays the exit until the loop's next pass.
    fn trigger(&self) {
        self.triggered.store(true, Ordering::SeqCst);
        if let Err(error) = self.wake.notify() {
            eprintln!("pinwin: the daemon could not be woken: {error}");
        }
    }

    /// Whether the trigger was set.
    fn is_triggered(&self) -> bool {
        self.triggered.load(Ordering::SeqCst)
    }

    /// Read the wake eventfd, so the accept loop's next `poll` waits again.
    fn drain(&self) -> io::Result<u64> {
        self.wake.drain()
    }

    /// The descriptor the accept loop polls.
    fn wake_fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }
}

/// The signalfd the daemon reads SIGINT and SIGTERM from (D12). [`open`]
/// blocks the two signals in the calling thread and reads them through one
/// descriptor, so the accept loop waits for a signal with `poll` instead of
/// running a handler.
///
/// [`open`]: Self::open
#[derive(Debug)]
pub struct SignalFd(OwnedFd);

impl SignalFd {
    /// Block SIGINT and SIGTERM in this thread and open a signalfd for them.
    /// The thread keeps the mask, because that is the thread that reads the
    /// signals; the connection and panel threads it starts inherit it and so
    /// do not take the signals themselves.
    pub fn open() -> io::Result<Self> {
        // SAFETY: `sigset_t` is a plain C struct of integers with no invalid
        // bit pattern, so an all-zero value is valid for `sigemptyset`.
        let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        // SAFETY: `mask` is a valid `sigset_t` and the three calls only
        // write into it; they report failure as -1 and leave `errno` set.
        let filled = unsafe {
            libc::sigemptyset(std::ptr::addr_of_mut!(mask)) == 0
                && libc::sigaddset(std::ptr::addr_of_mut!(mask), libc::SIGINT) == 0
                && libc::sigaddset(std::ptr::addr_of_mut!(mask), libc::SIGTERM) == 0
        };
        if !filled {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `previous` is only an out-parameter of `pthread_sigmask`,
        // which fills it before returning.
        let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
        // SAFETY: `mask` and `previous` are valid `sigset_t` values.
        // `pthread_sigmask` returns the error number instead of `errno`.
        let blocked = unsafe {
            libc::pthread_sigmask(
                libc::SIG_BLOCK,
                std::ptr::addr_of!(mask),
                std::ptr::addr_of_mut!(previous),
            )
        };
        if blocked != 0 {
            return Err(io::Error::from_raw_os_error(blocked));
        }
        // SAFETY: `mask` holds the two signals to read, and -1 asks for a
        // new descriptor that `OwnedFd` takes over below.
        let fd = unsafe {
            libc::signalfd(
                -1,
                std::ptr::addr_of!(mask),
                libc::SFD_CLOEXEC | libc::SFD_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh descriptor from the call above and nothing
        // else owns it, so this `OwnedFd` is its only owner.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// The next pending signal, or `None` when none is queued. A signal
    /// number the C `int` cannot hold is ignored.
    fn next(&self) -> io::Result<Option<i32>> {
        // `signal_info` is uninitialized memory of the size and alignment
        // `read` writes; it is only assumed initialized after a full-size
        // read.
        let mut signal_info = std::mem::MaybeUninit::<libc::signalfd_siginfo>::uninit();
        // SAFETY: the buffer is `size_of::<signalfd_siginfo>()` bytes long,
        // which is the length `signalfd` reads, and the descriptor belongs to
        // `self`.
        let read = unsafe {
            libc::read(
                self.0.as_raw_fd(),
                signal_info.as_mut_ptr().cast(),
                size_of::<libc::signalfd_siginfo>(),
            )
        };
        if read < 0 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(None),
                _ => Err(error),
            };
        }
        let Ok(read) = usize::try_from(read) else {
            return Ok(None);
        };
        if read != size_of::<libc::signalfd_siginfo>() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a signalfd read returned less than one signal",
            ));
        }
        // SAFETY: the read returned the whole struct, so every field is
        // initialized.
        let signal_info = unsafe { signal_info.assume_init() };
        Ok(i32::try_from(signal_info.ssi_signo).ok())
    }
}

impl AsRawFd for SignalFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

/// Whether `signo` ends the daemon the way `quit` does (spec "Quit": "SIGINT
/// and SIGTERM to the daemon SHALL do the same as `quit`").
#[must_use]
pub fn shutdown_signal(signo: libc::c_int) -> bool {
    signo == libc::SIGINT || signo == libc::SIGTERM
}

/// The running daemon: its bound socket and the lifetime its connection
/// threads share. [`run`](Self::run) accepts connections until the lifetime
/// triggers shutdown, and dropping the daemon removes the socket file (spec
/// "Daemon socket").
#[derive(Debug)]
pub struct Daemon {
    socket: DaemonSocket,
    lifetime: Arc<Lifetime>,
}

impl Daemon {
    /// A daemon on `socket` that shares `lifetime` with its connection
    /// threads.
    #[must_use]
    pub fn new(socket: DaemonSocket, lifetime: Arc<Lifetime>) -> Self {
        Self { socket, lifetime }
    }

    /// Accept connections until the lifetime shuts down (D12). Every
    /// connection gets its own thread, so the loop never handles a client's
    /// bytes itself (spec "Clients cannot block the daemon").
    ///
    /// `signals` is the signalfd of the daemon's main thread; a test passes
    /// `None` and triggers [`Lifetime::quit`] or a panel's end instead.
    pub fn run(self, signals: Option<&SignalFd>) -> io::Result<()> {
        let listener = self.socket.listener();
        listener.set_nonblocking(true)?;
        let wake_fd = self.lifetime.shutdown.wake_fd();
        let signal_fd = signals.map_or(-1, AsRawFd::as_raw_fd);
        loop {
            if self.lifetime.is_shutdown() {
                return Ok(());
            }
            // The three descriptors stay an array of three in every pass,
            // with -1 for a daemon that reads no signal; `poll` ignores a -1
            // entry.
            let mut watched = [
                libc::pollfd {
                    fd: listener.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: signal_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: `watched` is a live array of valid `pollfd` entries for
            // the duration of the call, and `poll` writes only its `revents`
            // fields. A negative timeout waits until one of them is ready.
            let ready =
                unsafe { libc::poll(watched.as_mut_ptr(), watched.len() as libc::nfds_t, -1) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if watched[2].revents & libc::POLLIN != 0 {
                self.on_signals(signals)?;
            }
            if watched[1].revents != 0 {
                let _ = self.lifetime.shutdown.drain();
            }
            if watched[0].revents & libc::POLLIN != 0 {
                self.accept()?;
            }
        }
    }

    /// Turn every queued SIGINT or SIGTERM into the `quit` behaviour (spec
    /// "Quit"), reading until the signalfd has no signal left.
    fn on_signals(&self, signals: Option<&SignalFd>) -> io::Result<()> {
        let Some(signals) = signals else {
            return Ok(());
        };
        while let Some(signo) = signals.next()? {
            if shutdown_signal(signo) {
                self.lifetime.quit();
            }
        }
        Ok(())
    }

    /// Accept every connection that is waiting, each on its own thread, and
    /// stop at `WouldBlock` so the loop can wait for the next event (D12).
    fn accept(&self) -> io::Result<()> {
        loop {
            match self.socket.listener().accept() {
                Ok((stream, _address)) => self.serve(stream),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    /// Start the thread that serves one accepted connection. A thread that
    /// cannot start is reported and the daemon keeps serving the others.
    fn serve(&self, stream: net::UnixStream) {
        let lifetime = Arc::clone(&self.lifetime);
        let wait = lifetime.wait();
        let spawned = thread::Builder::new()
            .name("pinwin-ipc".to_owned())
            .spawn(move || {
                if let Err(error) = connection::serve(stream, &lifetime, wait) {
                    eprintln!("pinwin: an ipc connection ended: {error}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("pinwin: could not serve an ipc connection: {error}");
        }
    }
}
