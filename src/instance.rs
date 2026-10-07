//! The instance socket: the validated instance name, the socket path under
//! `$XDG_RUNTIME_DIR/pinwin`, the duplicate check, stale-file removal and
//! bind that all happen before any surface opens, the listener that parses
//! `toggle\n` and answers `ok\n` or `error\n` until the host's child ends,
//! and the client that asks a running host to toggle its panel
//! (`keyboard-focus-request`'s socket plumbing, the toggle protocol of
//! `replace-gtk-with-wayland` D4; the module moved into the library with
//! `serve-instance-socket` D7, so a library host can serve the same socket).
//! The transport is the standard library's Unix sockets only, no new
//! dependencies; every socket wait is bounded, so one peer cannot hang the
//! listener or the client.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Read, Write};
use std::ops::Deref;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{fs, os::unix::net};

use crate::guard::{Poisoned, guard_default};

/// A validated instance name (`PINWIN_NAME`, `--toggle [name]`): 1..=64
/// characters from `[A-Za-z0-9_-]`. The newtype keeps the socket path safe
/// (no separators, no `.` or `..`) and inside the 108-byte `sun_path` limit
/// of a socket address (keyboard-focus-request design).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceName(String);

impl InstanceName {
    /// The name used when none is given: `default`.
    #[must_use]
    pub fn default_instance() -> InstanceName {
        InstanceName("default".to_owned())
    }

    /// Validate `raw`, reporting a failure as the full `pinwin:` exit-2
    /// message under `context` (`PINWIN_NAME` or `--toggle`).
    pub fn parse(context: &str, raw: &str) -> Result<InstanceName, String> {
        let valid = (1..=64).contains(&raw.len())
            && raw
                .bytes()
                .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'));
        if valid {
            Ok(InstanceName(raw.to_owned()))
        } else {
            Err(format!(
                "pinwin: {context}: expected a name of 1..=64 characters from \
                 [A-Za-z0-9_-], got '{raw}'"
            ))
        }
    }
}

impl fmt::Display for InstanceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Deref for InstanceName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

/// Why the socket for an instance could not be taken.
#[derive(Debug)]
pub enum BindError {
    /// A live listener answered the connect: another host already owns the
    /// name on this display.
    Duplicate,
    /// The socket could not be set up.
    Failed(io::Error),
}

/// The socket path for the instance: `$XDG_RUNTIME_DIR/pinwin/
/// $WAYLAND_DISPLAY-<name>.sock`. The runtime directory is used as given,
/// with no lossy conversion; a missing or empty `XDG_RUNTIME_DIR` is an
/// exit-2 error. `WAYLAND_DISPLAY` unset falls back to `wayland-0`, the
/// same default the Wayland client library connects with; a set value must
/// be valid UTF-8 and hold no `/`, so the path cannot escape
/// `$XDG_RUNTIME_DIR/pinwin` and distinct displays cannot collapse onto one
/// socket.
pub fn socket_path(
    runtime_dir: Option<&OsStr>,
    display: Option<&OsStr>,
    name: &InstanceName,
) -> Result<PathBuf, String> {
    let Some(runtime_dir) = runtime_dir.filter(|dir| !dir.as_bytes().is_empty()) else {
        return Err(
            "pinwin: XDG_RUNTIME_DIR: expected the runtime directory of the Wayland session"
                .to_owned(),
        );
    };
    let display = match display {
        None => "wayland-0".to_owned(),
        Some(raw) => {
            let valid = raw
                .to_str()
                .filter(|text| !text.is_empty() && !text.contains('/'));
            let Some(display) = valid else {
                return Err(format!(
                    "pinwin: WAYLAND_DISPLAY: expected a UTF-8 display name without '/', \
                     got '{}'",
                    raw.to_string_lossy()
                ));
            };
            display.to_owned()
        }
    };
    Ok(Path::new(runtime_dir)
        .join("pinwin")
        .join(format!("{display}-{name}.sock")))
}

/// Take the socket at `path` for this instance, before any surface opens:
///
/// 1. The parent directory is created with mode 0700.
/// 2. Bind first: whoever wins the bind owns the name. Connecting first and
///    unlinking on a failed connect would let a racing second host unlink
///    the first host's live socket.
/// 3. On `EADDRINUSE` the path's owner is probed with a connect. A live
///    owner answers, and the caller exits 2 ([`BindError::Duplicate`]).
///    Only a dead owner — connection refused, or the file already gone —
///    is unlinked and bound again, once. Any other connect error stands
///    ([`BindError::Failed`]), so a permission or resource failure never
///    gets something else's path deleted.
pub fn bind_instance_socket(path: &Path) -> Result<net::UnixListener, BindError> {
    // `socket_path` always composes a parent, but a hand-built path without
    // one is a caller error, not a panic.
    let Some(dir) = path.parent() else {
        return Err(BindError::Failed(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the socket path has no parent directory",
        )));
    };
    if let Err(error) = fs::create_dir(dir)
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Err(BindError::Failed(error));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(BindError::Failed)?;
    match bind_fresh(path) {
        Ok(listener) => return Ok(listener),
        // The path is taken; find out whether its owner is still alive.
        Err(error) if error.raw_os_error() == Some(libc::EADDRINUSE) => {}
        Err(error) => return Err(BindError::Failed(error)),
    }
    match net::UnixStream::connect(path) {
        Ok(_live) => Err(BindError::Duplicate),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ECONNREFUSED | libc::ENOENT)
            ) =>
        {
            // A dead owner left the file (or it vanished between the bind
            // and the probe); replace it once. The rebind failing with
            // EADDRINUSE again is the final duplicate check
            // (keyboard-focus-request design).
            if let Err(error) = fs::remove_file(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                return Err(BindError::Failed(error));
            }
            bind_fresh(path).map_err(BindError::Failed)
        }
        Err(error) => Err(BindError::Failed(error)),
    }
}

/// Bind the socket and keep the exec'd command from inheriting it.
fn bind_fresh(path: &Path) -> Result<net::UnixListener, io::Error> {
    let listener = net::UnixListener::bind(path)?;
    // The forked child execs the host's command, and the command must never
    // inherit the listener; set the close-on-exec flag explicitly instead of
    // relying on the socket type std happens to create. A failed `fcntl` is
    // an error, not a tolerated one: an inheritable listener would keep the
    // name claimed after the host exits.
    // SAFETY: `fd` is the listener's own descriptor and `F_SETFD` only sets
    // its close-on-exec flag.
    if unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(listener)
}

/// The request line: the whole protocol is the one word (`replace-gtk-with-wayland`
/// D4); there is no argument, so the line is `toggle\n` exactly.
const REQUEST: &[u8] = b"toggle\n";
/// The listener's reply for an accepted request.
const REPLY_OK: &[u8] = b"ok\n";
/// The listener's reply for a failed or unknown request.
const REPLY_ERROR: &[u8] = b"error\n";
/// The largest request the protocol reads: the request line plus a
/// tolerance, so an oversized or malformed line is refused instead of
/// buffered without bound. Anything longer than this is not a request this
/// protocol defines; the read stops there and the request is refused.
const MAX_REQUEST: usize = 64;
/// How long one side may wait on the other: a peer that stalls is dropped
/// after this, so one connection cannot hang the listener or the client.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);
/// How often the accept loop wakes to notice the shutdown flag.
const ACCEPT_POLL: Duration = Duration::from_millis(100);

/// Answer toggle requests on `listener` until `shutdown` is set: each
/// connection sends one request line; exactly `toggle\n` runs `toggle` and
/// is answered `ok\n`, or `error\n` when it fails. Every other line — any
/// other command, an argument, or a line over [`MAX_REQUEST`] — is answered
/// `error\n` without calling `toggle`. Every connection's read and write are
/// bounded in size and time, so one client cannot hang the listener; the
/// loop ends within one accept poll of `shutdown` — the host sets it after
/// its child exits.
///
/// The body runs through the D5 guard with its own latch: a panic in the
/// listener ends the loop quietly instead of unwinding out of the thread.
pub fn serve_toggle_requests<E>(
    listener: &net::UnixListener,
    toggle: &(impl Fn() -> Result<(), E> + Sync),
    shutdown: &AtomicBool,
) {
    let poisoned = Poisoned::new();
    guard_default(&poisoned, (), || {
        // Poll instead of blocking in `accept`, so the shutdown flag is
        // noticed without a second mechanism between connections.
        while !shutdown.load(Ordering::Relaxed) {
            if !poll_ready(listener.as_raw_fd(), libc::POLLIN, ACCEPT_POLL) {
                continue;
            }
            let Ok((mut stream, _)) = listener.accept() else {
                continue;
            };
            let reply = match read_bounded(&mut stream).as_deref() {
                // The peer must still be there before the toggle runs: a
                // toggle is a non-idempotent flip, so a request from a
                // client that already gave up — its 500 ms wait ran out
                // while this loop was busy in an earlier toggle — must not
                // flip the panel later, when the loop reaches the queued
                // request. A fully closed peer shows up as a hang-up; a
                // half-closed one that still reads the reply is kept.
                Some(line) if line == REQUEST && !peer_gone(&stream) => match toggle() {
                    Ok(()) => REPLY_OK,
                    Err(_) => REPLY_ERROR,
                },
                _ => REPLY_ERROR,
            };
            write_bounded(&mut stream, reply);
        }
    });
}

/// Whether the peer has fully closed its end: an immediate `poll` reports
/// `POLLHUP` when the peer's whole socket is gone. A half-close — a peer
/// that only stopped writing and is still reading the reply, the shape of
/// a piped-in `socat` client — shows up as a readable end-of-file
/// instead, and keeps the callback. No data yet or a failed poll keeps
/// the peer: only a definite hang-up skips it.
fn peer_gone(stream: &net::UnixStream) -> bool {
    let mut fds = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: 0,
        revents: 0,
    };
    // SAFETY: `fds` is one valid pollfd entry for the duration of the
    // call, and the zero timeout makes the wait non-blocking. With
    // `events` empty only the always-reported hang-up and error bits can
    // come back.
    let ready = unsafe { libc::poll(std::ptr::addr_of_mut!(fds), 1, 0) };
    ready > 0 && fds.revents & libc::POLLHUP != 0
}

/// Wait until `fd` reports `events` (`POLLIN`/`POLLOUT`), at most `timeout`.
/// False when the wait expired or poll failed.
fn poll_ready(fd: RawFd, events: libc::c_short, timeout: Duration) -> bool {
    let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    let mut fds = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: `fds` is one valid pollfd entry for the duration of the call.
    let ready = unsafe { libc::poll(std::ptr::addr_of_mut!(fds), 1, timeout_ms) };
    ready > 0 && fds.revents & events != 0
}

/// Read one request/reply line from `stream`, bounded: the stream is
/// switched to non-blocking and every wait polls the descriptor, so a peer
/// that stalls or floods cannot hold the reader past the timeout. Returns
/// the bytes read when a line ended (newline), the peer closed, or the size
/// limit was hit; `None` when nothing arrived in time.
fn read_bounded(stream: &mut net::UnixStream) -> Option<Vec<u8>> {
    stream.set_nonblocking(true).ok()?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut line = Vec::new();
    let mut chunk = [0u8; MAX_REQUEST];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                line.extend_from_slice(&chunk[..n]);
                if line.ends_with(b"\n") || line.len() > MAX_REQUEST {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline
                    || !poll_ready(stream.as_raw_fd(), libc::POLLIN, HANDSHAKE_TIMEOUT)
                {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    Some(line)
}

/// Write `reply` to `stream`, bounded like [`read_bounded`]: the write waits
/// for writability at most the timeout and gives up rather than blocking on
/// a peer that never reads.
fn write_bounded(stream: &mut net::UnixStream, reply: &[u8]) {
    let _ = stream.set_nonblocking(true);
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut written = 0;
    while written < reply.len() {
        match stream.write(&reply[written..]) {
            Ok(0) => return,
            Ok(n) => written += n,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline
                    || !poll_ready(stream.as_raw_fd(), libc::POLLOUT, HANDSHAKE_TIMEOUT)
                {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// Remove the instance's socket file after the host ends. The listener this
/// module bound always created the file at that path (the bind replaced any
/// stale file first), so the removal only has to skip an already-gone file.
fn remove_socket_file(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!("pinwin: {}: {error}", path.display());
    }
}

/// The instance's socket file; dropping it removes the file, so every exit
/// path after the bind cleans up.
#[derive(Debug)]
pub struct SocketFile(PathBuf);

impl SocketFile {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        remove_socket_file(&self.0);
    }
}

/// [`socket_path`] from the process environment: `XDG_RUNTIME_DIR` is used as
/// given, with no lossy conversion.
pub fn socket_path_from_env(name: &InstanceName) -> Result<PathBuf, String> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
    let display = std::env::var_os("WAYLAND_DISPLAY");
    socket_path(runtime_dir.as_deref(), display.as_deref(), name)
}

/// What asking a host to toggle ended in.
#[derive(Debug, PartialEq, Eq)]
pub enum ToggleError {
    /// The environment did not provide a usable socket path (exit 2, the
    /// same class as a bad name).
    Environment(String),
    /// No host answered `ok`: no host at all, a refusing host, a failing
    /// host, or no valid reply in time (exit 1).
    NotAnswered(String),
}

/// The exchange itself against the socket at `path`: connect, send
/// `toggle\n`, and wait a bounded time for the reply. `ok\n` is [`Ok`];
/// everything else — a refused connect, `error\n`, a timeout, an invalid
/// reply — is [`ToggleError::NotAnswered`] with the message to print.
pub fn toggle_client(path: &Path) -> Result<(), ToggleError> {
    let mut stream = net::UnixStream::connect(path).map_err(|error| {
        ToggleError::NotAnswered(format!(
            "pinwin: no pinwin is listening on {} ({error})",
            path.display()
        ))
    })?;
    write_bounded(&mut stream, REQUEST);
    match read_bounded(&mut stream) {
        Some(reply) if reply == REPLY_OK => Ok(()),
        Some(reply) if reply == REPLY_ERROR => Err(ToggleError::NotAnswered(
            "pinwin: the panel did not toggle (the request failed on the host)".to_owned(),
        )),
        _ => Err(ToggleError::NotAnswered(format!(
            "pinwin: no valid answer came from {} in time",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests;
