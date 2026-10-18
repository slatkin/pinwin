//! The daemon (replace-terminal-with-app-host D12, D15): the one Unix
//! socket that serves every panel of a Wayland display, the panel handles and
//! the registry that routes a request to the panel it names.
//!
//! The path is `$XDG_RUNTIME_DIR/pinwin/<display>.sock`, where `<display>` is
//! the base name of the display's socket (spec "Daemon socket"). [`bind`]
//! creates the `pinwin` directory with mode 0700, binds first so a daemon that
//! starts at the same time cannot unlink a live socket, replaces the file a
//! killed daemon left behind, and refuses a path that a live daemon answers on
//! (spec "One daemon for each Wayland display"). The returned [`DaemonSocket`]
//! removes the file when it drops, so every daemon exit path cleans up (spec
//! "Daemon socket").
//!
//! The path and the bind take the runtime directory and the display as
//! parameters, so the tests bind into a scratch runtime directory without
//! touching the process environment (`port-to-rust` D10); only
//! [`socket_path_from_env`] and [`DaemonSocket::bind_from_env`] read
//! `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`.
//!
//! A client's socket path has one more source (D1): [`client_socket_path`]
//! uses `PINWIN_SOCKET` when it is set and not empty, with a relative path
//! refused, and falls back to the display's path. [`DaemonSocket::
//! bind_from_env`] never reads that variable, so `pinwin daemon` binds the
//! display's socket as before (spec "Client socket lookup").
//!
//! The panel handles live in [`panel`]: a command channel, an eventfd that
//! wakes the panel's poll loop and a bounded wait for the answer. [`registry`]
//! maps a panel name to its handle and routes a decoded request, so one
//! panel that does not answer never stops another or the daemon (spec "The
//! daemon survives bad input"). [`events`] is the bus that carries a panel's
//! events to its subscribers, [`connection`] serves one IPC connection,
//! including the `subscribe` stream and the `create` request that starts the
//! panel a `pinwin run` asks for (spec "Run an app in a panel", D2), and
//! [`lifetime`] holds the accept loop, the shutdown trigger, `quit`,
//! `reload` and the SIGINT and SIGTERM handling (spec "Daemon lifetime
//! preference", "Quit", "Reload").

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

pub mod connection;
pub mod events;
pub mod lifetime;
pub mod panel;
pub mod ready;
pub mod registry;
pub mod spawn;

#[cfg(test)]
mod tests;

/// The display the Wayland client library connects with when
/// `WAYLAND_DISPLAY` is unset, so a daemon started without it names the
/// socket of the display it will actually use.
const DEFAULT_DISPLAY: &str = "wayland-0";

/// Lock a mutex the daemon's threads share, ignoring poisoning: a panic in
/// another thread must not stop the daemon or another panel (spec "The
/// daemon survives bad input", `port-to-rust` D5), and the registry and the
/// command channel keep their own invariant across a panic because no thread
/// panics while holding either.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The directory inside the runtime directory that holds the daemon sockets.
const SOCKET_DIR: &str = "pinwin";

/// The extension of a daemon socket file (spec "Daemon socket").
const SOCKET_SUFFIX: &str = ".sock";

/// Why no daemon socket path could be composed (spec "Daemon socket", "No
/// runtime directory").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathError {
    /// `XDG_RUNTIME_DIR` is unset or empty.
    NoRuntimeDir,
    /// `WAYLAND_DISPLAY` is set but names no socket: empty, `.`, `..`, or a
    /// path with no file name. The variant carries the name as it would
    /// print, so the message can point at it.
    BadDisplay(String),
    /// `PINWIN_SOCKET` is set and is not an absolute path (D1).
    RelativeSocket,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRuntimeDir => f.write_str(
                "XDG_RUNTIME_DIR: expected the runtime directory of the Wayland session",
            ),
            Self::BadDisplay(raw) => write!(
                f,
                "WAYLAND_DISPLAY: expected the name of the display's socket, got '{raw}'"
            ),
            Self::RelativeSocket => {
                f.write_str("PINWIN_SOCKET: expected an absolute path to the daemon's socket")
            }
        }
    }
}

impl std::error::Error for PathError {}

/// Why the daemon socket could not be taken (spec "One daemon for each
/// Wayland display", "Daemon socket").
#[derive(Debug)]
pub enum SocketError {
    /// A live daemon answered the connect: this display already has its
    /// daemon, which must stay unchanged.
    Live,
    /// The environment gave no usable socket path. Only the
    /// environment-reading bind can end here; a bind that takes the path
    /// already has it.
    Path(PathError),
    /// The socket could not be set up.
    Io(io::Error),
}

impl fmt::Display for SocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live => f.write_str("another pinwin daemon already serves this display"),
            Self::Path(error) => fmt::Display::fmt(error, f),
            Self::Io(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for SocketError {}

/// The daemon socket path for a display (spec "Daemon socket"):
/// `$XDG_RUNTIME_DIR/pinwin/<display>.sock`. The runtime directory is used as
/// given, with no lossy conversion; a missing or empty `XDG_RUNTIME_DIR` is a
/// [`PathError::NoRuntimeDir`]. Only the base name of the display's socket
/// joins the runtime directory, so an absolute `WAYLAND_DISPLAY` — which the
/// Wayland client library also accepts — cannot move the socket out of
/// `pinwin/`; a display that names no socket is a
/// [`PathError::BadDisplay`]. An unset `WAYLAND_DISPLAY` falls back to
/// [`DEFAULT_DISPLAY`].
pub fn socket_path(
    runtime_dir: Option<&OsStr>,
    display: Option<&OsStr>,
) -> Result<PathBuf, PathError> {
    let Some(runtime_dir) = runtime_dir.filter(|dir| !dir.as_bytes().is_empty()) else {
        return Err(PathError::NoRuntimeDir);
    };
    let name = match display {
        None => OsString::from(DEFAULT_DISPLAY),
        Some(raw) => match Path::new(raw).file_name() {
            Some(name) => name.to_owned(),
            None => {
                return Err(PathError::BadDisplay(raw.to_string_lossy().into_owned()));
            }
        },
    };
    let mut file = name;
    file.push(SOCKET_SUFFIX);
    Ok(Path::new(runtime_dir).join(SOCKET_DIR).join(file))
}

/// [`socket_path`] from the process environment.
pub fn socket_path_from_env() -> Result<PathBuf, PathError> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
    let display = std::env::var_os("WAYLAND_DISPLAY");
    socket_path(runtime_dir.as_deref(), display.as_deref())
}

/// Where a client's daemon socket path came from (D1): a `pinwin run` uses
/// the tag to decide whether it may start a daemon on that path (D3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `PINWIN_SOCKET` named the path.
    Variable,
    /// The path came from `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`.
    Display,
}

/// The socket path a client uses (spec "Client socket lookup", D1):
/// `PINWIN_SOCKET` when it is set and not empty, else the display's path.
/// The tag says which source won, so only [`Source::Display`] may start a
/// daemon (D3).
///
/// # Errors
///
/// [`PathError::RelativeSocket`] when `PINWIN_SOCKET` is set to a relative
/// path, as no daemon socket can be found from it, and the display path's
/// own errors when the variable is unset or empty.
pub fn client_socket_path() -> Result<(PathBuf, Source), PathError> {
    let socket = std::env::var_os("PINWIN_SOCKET");
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
    let display = std::env::var_os("WAYLAND_DISPLAY");
    client_socket_path_from(
        socket.as_deref(),
        runtime_dir.as_deref(),
        display.as_deref(),
    )
}

/// [`client_socket_path`] with its environment values as parameters, so the
/// tests never change the process environment (`port-to-rust` D10). An empty
/// `PINWIN_SOCKET` is the same as an unset one (spec "Client socket
/// lookup").
pub fn client_socket_path_from(
    socket: Option<&OsStr>,
    runtime_dir: Option<&OsStr>,
    display: Option<&OsStr>,
) -> Result<(PathBuf, Source), PathError> {
    let Some(socket) = socket.filter(|path| !path.as_bytes().is_empty()) else {
        return socket_path(runtime_dir, display).map(|path| (path, Source::Display));
    };
    let path = PathBuf::from(socket);
    if !path.is_absolute() {
        return Err(PathError::RelativeSocket);
    }
    Ok((path, Source::Variable))
}

/// Take the daemon socket at `path`, before any panel exists:
///
/// 1. The parent directory is created with mode 0700.
/// 2. Bind first: whoever wins the bind owns the display. Connecting first and
///    unlinking on a failed connect would let a daemon that starts at the same
///    time unlink the first daemon's live socket (D12 lets two starts race).
/// 3. On `EADDRINUSE` the path's owner is probed with a connect. A live owner
///    answers, and the caller exits 0 without changing it
///    ([`SocketError::Live`]). Only a dead owner — connection refused, or the
///    file already gone — is unlinked and bound again, once. Any other connect
///    error stands, so a permission or resource failure never gets another
///    daemon's path deleted.
/// 4. The socket file is readable and writable only by its owner, and a
///    failure there drops the new socket, which removes the file.
pub fn bind(path: &Path) -> Result<DaemonSocket, SocketError> {
    // `socket_path` always composes a parent, but a hand-built path without
    // one is a caller error, not a panic.
    let Some(dir) = path.parent() else {
        return Err(SocketError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the socket path has no parent directory",
        )));
    };
    if let Err(error) = fs::create_dir(dir)
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Err(SocketError::Io(error));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(SocketError::Io)?;
    let listener = match net::UnixListener::bind(path) {
        Ok(listener) => listener,
        // The path is taken; find out whether its owner is still alive.
        Err(error) if error.raw_os_error() == Some(libc::EADDRINUSE) => probe_and_replace(path)?,
        Err(error) => return Err(SocketError::Io(error)),
    };
    let socket = DaemonSocket {
        listener,
        _file: SocketFile(path.to_owned()),
    };
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(SocketError::Io)?;
    Ok(socket)
}

/// The owner probe of [`bind`]: a live owner is [`SocketError::Live`], a dead
/// one's file is removed and the path bound again once.
fn probe_and_replace(path: &Path) -> Result<net::UnixListener, SocketError> {
    match net::UnixStream::connect(path) {
        Ok(_live) => Err(SocketError::Live),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ECONNREFUSED | libc::ENOENT)
            ) =>
        {
            // A dead owner left the file (or it vanished between the bind
            // and the probe); replace it once. The rebind failing with
            // EADDRINUSE again is the final live-daemon check.
            if let Err(error) = fs::remove_file(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                return Err(SocketError::Io(error));
            }
            net::UnixListener::bind(path).map_err(SocketError::Io)
        }
        Err(error) => Err(SocketError::Io(error)),
    }
}

/// The daemon's bound socket: the listener plus the socket file its bind
/// created.
#[derive(Debug)]
pub struct DaemonSocket {
    listener: net::UnixListener,
    /// The socket file the bind created; its drop removes the path.
    _file: SocketFile,
}

impl DaemonSocket {
    /// Take the socket for this process's display from the environment: the
    /// path comes from `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`
    /// ([`socket_path_from_env`]), a missing runtime directory or an unusable
    /// display name is [`SocketError::Path`], and a live daemon on the path is
    /// [`SocketError::Live`].
    pub fn bind_from_env() -> Result<DaemonSocket, SocketError> {
        let path = socket_path_from_env().map_err(SocketError::Path)?;
        bind(&path)
    }

    /// The listener the daemon accepts its IPC connections on (D12).
    #[must_use]
    pub fn listener(&self) -> &net::UnixListener {
        &self.listener
    }
}

/// The daemon's socket file; dropping it removes the file, so every exit path
/// after the bind cleans up (spec "Daemon socket": "When the daemon exits, it
/// SHALL remove its socket file").
#[derive(Debug)]
struct SocketFile(PathBuf);

impl Drop for SocketFile {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("pinwin: {}: {error}", self.0.display());
        }
    }
}
