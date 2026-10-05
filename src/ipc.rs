//! The command-line IPC identity, socket setup, listener and `--focus`
//! client for the `pinwin` program (change `keyboard-focus-request`): the
//! validated instance name, the socket path under `$XDG_RUNTIME_DIR/pinwin`,
//! the duplicate check, stale-file removal and bind that all happen before
//! any surface opens, the listener that answers `focus\n` with `ok\n` or
//! `error\n` until the host's child ends, and the client that asks a running
//! host for focus. The transport is the standard library's Unix sockets
//! only, no new dependencies; every socket wait is bounded, so one peer
//! cannot hang the listener or the client.

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

use pinwin::guard::{Poisoned, guard_default};

/// A validated instance name (`PINWIN_NAME`, `--focus [name]`): 1..=64
/// characters from `[A-Za-z0-9_-]`. The newtype keeps the socket path safe
/// (no separators, no `.` or `..`) and inside the 108-byte `sun_path` limit
/// of a socket address (keyboard-focus-request design).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceName(String);

impl InstanceName {
    /// The name used when none is given: `default`.
    pub fn default_instance() -> InstanceName {
        InstanceName("default".to_owned())
    }

    /// Validate `raw`, reporting a failure as the full `pinwin:` exit-2
    /// message under `context` (`PINWIN_NAME` or `--focus`).
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
    let dir = path.parent().expect("the socket path has a parent");
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
    // relying on the socket type std happens to create.
    // SAFETY: `fd` is the listener's own descriptor and `F_SETFD` only sets
    // its close-on-exec flag.
    let _ = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    Ok(listener)
}

/// The request line of the focus protocol.
const REQUEST: &[u8] = b"focus\n";
/// The listener's reply for an accepted request.
const REPLY_OK: &[u8] = b"ok\n";
/// The listener's reply for a failed or unknown request.
const REPLY_ERROR: &[u8] = b"error\n";
/// The largest request the protocol reads: one short line. Anything longer
/// is not a request this protocol defines; the read stops there.
const MAX_REQUEST: usize = 64;
/// How long one side may wait on the other: a peer that stalls is dropped
/// after this, so one connection cannot hang the listener or the client.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);
/// How often the accept loop wakes to notice the shutdown flag.
const ACCEPT_POLL: Duration = Duration::from_millis(100);

/// Answer focus requests on `listener` until `shutdown` is set: each
/// connection sends one request line; `focus\n` runs `request_focus` and is
/// answered `ok\n`, or `error\n` when it fails or the request is unknown.
/// Every connection's read and write are bounded in size and time, so one
/// client cannot hang the listener; the loop ends within one accept poll of
/// `shutdown` — the host sets it after its child exits.
///
/// The body runs through the D5 guard with its own latch: a panic in the
/// listener ends the loop quietly instead of unwinding out of the thread.
pub(crate) fn serve_focus_requests<E>(
    listener: &net::UnixListener,
    request_focus: &(impl Fn() -> Result<(), E> + Sync),
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
            let reply = match read_bounded(&mut stream) {
                Some(request) if request == REQUEST => match request_focus() {
                    Ok(()) => REPLY_OK,
                    Err(_) => REPLY_ERROR,
                },
                Some(_) | None => REPLY_ERROR,
            };
            write_bounded(&mut stream, reply);
        }
    });
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
pub(crate) fn remove_socket_file(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!("pinwin: {}: {error}", path.display());
    }
}

/// What asking a host for focus ended in.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FocusError {
    /// The environment did not provide a usable socket path (exit 2, the
    /// same class as a bad name).
    Environment(String),
    /// No host answered `ok`: no host at all, a refusing host, or no valid
    /// reply in time (exit 1).
    NotAnswered(String),
}

/// Ask the host that owns `name`'s socket for focus: resolve the socket path
/// from the environment (a failure is [`FocusError::Environment`]), connect,
/// send `focus\n`, and wait a bounded time for the reply. `ok\n` is [`Ok`];
/// everything else — a refused connect, `error\n`, a timeout, an invalid
/// reply — is [`FocusError::NotAnswered`] with the message to print.
pub(crate) fn request_focus_from_host(
    runtime_dir: Option<&OsStr>,
    display: Option<&OsStr>,
    name: &InstanceName,
) -> Result<(), FocusError> {
    let path = socket_path(runtime_dir, display, name).map_err(FocusError::Environment)?;
    focus_client(&path)
}

/// The exchange itself against the socket at `path`: connect, send
/// `focus\n`, and wait a bounded time for the reply. `ok\n` is [`Ok`];
/// everything else — a refused connect, `error\n`, a timeout, an invalid
/// reply — is [`FocusError::NotAnswered`] with the message to print.
fn focus_client(path: &Path) -> Result<(), FocusError> {
    let mut stream = net::UnixStream::connect(path).map_err(|error| {
        FocusError::NotAnswered(format!(
            "pinwin: no pinwin is listening on {} ({error})",
            path.display()
        ))
    })?;
    write_bounded(&mut stream, REQUEST);
    match read_bounded(&mut stream) {
        Some(reply) if reply == REPLY_OK => Ok(()),
        Some(reply) if reply == REPLY_ERROR => Err(FocusError::NotAnswered(
            "pinwin: the panel could not take focus (the request failed on the host)".to_owned(),
        )),
        _ => Err(FocusError::NotAnswered(format!(
            "pinwin: no valid answer came from {} in time",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A unique scratch directory under the system temp dir, so the tests
    /// never touch the process environment and never collide (nextest runs
    /// each test in its own process; a plain `cargo test` shares one).
    fn temp_dir(tag: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pinwin-ipc-test-{}-{}-{}",
            std::process::id(),
            tag,
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).expect("temp dir");
        dir
    }

    /// A valid name parses, and the boundaries of the character set, the
    /// length range and the error message hold.
    #[test]
    fn the_instance_name_validates() {
        let name = InstanceName::parse("PINWIN_NAME", "notes").expect("valid");
        assert_eq!(&*name, "notes");
        assert_eq!(name.to_string(), "notes");

        // The whole allowed character set, and the 64-character boundary.
        InstanceName::parse("x", "A-z_09-").expect("the full allowed set");
        InstanceName::parse("x", &"a".repeat(64)).expect("64 characters");

        // Empty, too long, and characters outside the set.
        for raw in ["", &"a".repeat(65), "a/b", "a b", "a.b", "ä"] {
            let error = InstanceName::parse("PINWIN_NAME", raw).expect_err(raw);
            assert_eq!(
                error,
                format!(
                    "pinwin: PINWIN_NAME: expected a name of 1..=64 characters from \
                     [A-Za-z0-9_-], got '{raw}'"
                ),
                "raw = {raw:?}"
            );
        }
    }

    /// The name used when none is given is `default`.
    #[test]
    fn the_default_instance_name_is_default() {
        assert_eq!(&*InstanceName::default_instance(), "default");
    }

    /// The socket path is `$XDG_RUNTIME_DIR/pinwin/$WAYLAND_DISPLAY-<name>.sock`,
    /// with the `wayland-0` fallback for an unset display, and a set display
    /// that is valid UTF-8 and holds no `/`. The runtime directory is passed
    /// through as an `OsStr`, with no lossy conversion.
    #[test]
    fn socket_path_composes_the_runtime_path() {
        let name = InstanceName::parse("x", "notes").expect("valid");
        let os = OsStr::new;
        assert_eq!(
            socket_path(Some(os("/run/user/1000")), Some(os("wayland-1")), &name),
            Ok(PathBuf::from("/run/user/1000/pinwin/wayland-1-notes.sock"))
        );
        assert_eq!(
            socket_path(Some(os("/run/user/1000")), None, &name),
            Ok(PathBuf::from("/run/user/1000/pinwin/wayland-0-notes.sock"))
        );

        // An empty display, a display with a separator, and a non-UTF-8
        // display are all exit-2 errors: the path must stay inside
        // `$XDG_RUNTIME_DIR/pinwin` and distinct displays must stay distinct.
        for raw in [
            os(""),
            os("way/land"),
            os("/abs/path"),
            OsStr::from_bytes(&[b'w', 0xff]),
        ] {
            let error =
                socket_path(Some(os("/run/user/1000")), Some(raw), &name).expect_err("bad display");
            assert_eq!(
                error,
                format!(
                    "pinwin: WAYLAND_DISPLAY: expected a UTF-8 display name without '/', \
                     got '{}'",
                    raw.to_string_lossy()
                ),
                "raw = {:?}",
                raw
            );
        }

        // A missing or empty runtime directory is an exit-2 error.
        socket_path(None, Some(os("wayland-0")), &name).expect_err("no runtime directory");
        socket_path(Some(os("")), Some(os("wayland-0")), &name)
            .expect_err("empty runtime directory");
    }

    /// A stale file on the socket path is removed and bound again, and the
    /// new listener answers connects. On Linux both a SIGKILL-ed host's
    /// socket file and a plain regular file refuse the connect
    /// (`ECONNREFUSED`), so both count as a dead owner and are replaced.
    #[test]
    fn a_stale_file_on_the_socket_path_is_replaced() {
        let dir = temp_dir("stale");
        let path = dir.join("wayland-0-notes.sock");

        // The realistic stale case: a bound socket whose listener is gone.
        drop(net::UnixListener::bind(&path).expect("stale socket file"));
        net::UnixStream::connect(&path).expect_err("nothing listens on the stale file");

        let listener = bind_instance_socket(&path).expect("stale socket replaced");
        assert!(path.exists(), "the socket file is back");
        net::UnixStream::connect(&path).expect("the bind is live");
        drop(listener);

        // A plain regular file refuses the connect the same way.
        fs::remove_file(&path).expect("drop the leftover socket file");
        fs::write(&path, b"not a socket").expect("regular file");
        net::UnixStream::connect(&path).expect_err("a regular file is not a socket");
        let listener = bind_instance_socket(&path).expect("regular file replaced");
        drop(listener);
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// The parent directory is created with mode 0700, also when it already
    /// existed.
    #[test]
    fn the_socket_directory_is_private() {
        let dir = temp_dir("perm");
        let path = dir.join("pinwin").join("wayland-0-default.sock");

        // First bind creates `pinwin` inside the scratch dir.
        let listener = bind_instance_socket(&path).expect("first bind");
        drop(listener);
        let mode = fs::metadata(path.parent().expect("parent"))
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "the pinwin dir is 0700");

        // A second bind keeps the directory usable and private.
        let listener = bind_instance_socket(&path).expect("second bind");
        drop(listener);
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// A live listener on the path makes the bind fail as a duplicate: the
    /// second host of the same name must exit 2 before it opens a surface.
    #[test]
    fn a_live_listener_makes_the_bind_fail() {
        let dir = temp_dir("duplicate");
        let path = dir.join("wayland-0-default.sock");

        let first = bind_instance_socket(&path).expect("first bind");
        assert!(
            matches!(bind_instance_socket(&path), Err(BindError::Duplicate)),
            "the live listener is a duplicate"
        );
        drop(first);
        // After the first listener is gone the name is free again (its file
        // is now stale): a fresh host binds instead of failing.
        bind_instance_socket(&path).expect("the name is free again");
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// The bind-first order: a second host must not unlink the first host's
    /// live socket, even though both of their binds race on the same path.
    #[test]
    fn a_racing_second_host_leaves_the_live_socket_in_place() {
        let dir = temp_dir("race");
        let path = dir.join("wayland-0-default.sock");

        let first = bind_instance_socket(&path).expect("first bind");
        // The second host reports the duplicate without touching the file...
        assert!(matches!(
            bind_instance_socket(&path),
            Err(BindError::Duplicate)
        ));
        // ...and the first owner's socket still answers.
        net::UnixStream::connect(&path).expect("the live socket survived");
        drop(first);
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// Ask the stub behind a real bound socket over the real protocol: a
    /// succeeding `request_focus` is answered `ok`, a failing one `error`,
    /// and an unknown request too. A connection that sends nothing is
    /// dropped after the bounded wait (with the error reply) without
    /// stopping the next request, and the listener stops when the shutdown
    /// flag is set. The retries bound the test, so a wedged loop fails it
    /// instead of hanging it.
    #[test]
    fn the_listener_answers_the_protocol_and_stops_on_shutdown() {
        let dir = temp_dir("listener");
        let path = dir.join("wayland-0-default.sock");
        let listener = bind_instance_socket(&path).expect("bind");
        let shutdown = AtomicBool::new(false);
        let fail = AtomicBool::new(false);
        let stub = || {
            if fail.load(Ordering::Relaxed) {
                Err(())
            } else {
                Ok(())
            }
        };

        std::thread::scope(|scope| {
            // The thread reports its own exit through a flag the test polls
            // with a deadline, so a wedged loop fails the test instead of
            // hanging it.
            let stopped = std::sync::Arc::new(AtomicBool::new(false));
            let thread_stopped = std::sync::Arc::clone(&stopped);
            let listener_ref = &listener;
            let stub_ref = &stub;
            let shutdown_ref = &shutdown;
            scope.spawn(move || {
                serve_focus_requests(listener_ref, stub_ref, shutdown_ref);
                thread_stopped.store(true, Ordering::Relaxed);
            });

            // One full exchange; retried reads absorb the bounded waits the
            // stalled connection below puts into the accept loop.
            let ask = |request: &[u8], stream: &mut net::UnixStream| -> Vec<u8> {
                write_bounded(stream, request);
                let reply =
                    (0..20).find_map(|_| read_bounded(stream).filter(|reply| !reply.is_empty()));
                let Some(reply) = reply else {
                    panic!("no reply within the bounded retries");
                };
                reply
            };

            // `focus` against a succeeding stub is `ok`.
            let mut stream = net::UnixStream::connect(&path).expect("connect");
            assert_eq!(ask(REQUEST, &mut stream), REPLY_OK.to_vec());

            // A failing stub — the `request_focus` error case — is `error`.
            fail.store(true, Ordering::Relaxed);
            let mut stream = net::UnixStream::connect(&path).expect("connect");
            assert_eq!(ask(REQUEST, &mut stream), REPLY_ERROR.to_vec());

            // An unknown request is `error` too.
            let mut stream = net::UnixStream::connect(&path).expect("connect");
            assert_eq!(ask(b"hello\n", &mut stream), REPLY_ERROR.to_vec());

            // A connection that sends nothing is dropped after the bounded
            // wait and answered with the error reply — the listener does not
            // hang on it.
            let mut stalled = net::UnixStream::connect(&path).expect("connect");
            assert_eq!(ask(&[], &mut stalled), REPLY_ERROR.to_vec());

            // ...and the next request is still served normally.
            fail.store(false, Ordering::Relaxed);
            let mut stream = net::UnixStream::connect(&path).expect("connect");
            assert_eq!(ask(REQUEST, &mut stream), REPLY_OK.to_vec());

            // The shutdown flag ends the listener within a few accept polls.
            shutdown.store(true, Ordering::Relaxed);
            for _ in 0..100 {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert!(
                stopped.load(Ordering::Relaxed),
                "the listener stopped on the shutdown flag"
            );
        });
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// The client's exchange against a scripted host: the request line goes
    /// out as `focus\n`; `ok` succeeds, `error` and a garbage reply are
    /// reported as the request not being answered, a host that never replies
    /// is dropped after the bounded wait, and a stale path (connect
    /// refused) too.
    #[test]
    fn the_focus_client_reports_the_reply() {
        let dir = temp_dir("client");
        let path = dir.join("wayland-0-default.sock");

        // Bind a one-shot host that asserts the request line and sends
        // `reply`. Dropping a listener leaves its socket file behind (a
        // stale file), so each case unlinks it before it binds.
        let scripted = |reply: &[u8]| {
            let _ = fs::remove_file(&path);
            let host = net::UnixListener::bind(&path).expect("host bind");
            let reply = reply.to_vec();
            std::thread::spawn(move || {
                let (mut stream, _) = host.accept().expect("accept");
                let mut request = [0u8; 32];
                let mut read = 0;
                while read < request.len() && !request[..read].ends_with(b"\n") {
                    match stream.read(&mut request[read..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => read += n,
                    }
                }
                assert_eq!(&request[..read], b"focus\n", "the request line");
                let _ = stream.write_all(&reply);
            })
        };

        // `ok` succeeds.
        let host = scripted(REPLY_OK);
        assert_eq!(focus_client(&path), Ok(()));
        host.join().expect("host thread");

        // `error` reports the request as refused.
        let host = scripted(REPLY_ERROR);
        assert!(
            matches!(
                focus_client(&path),
                Err(FocusError::NotAnswered(message)) if message.contains("could not take focus")
            ),
            "an error reply is not answered"
        );
        host.join().expect("host thread");

        // A garbage reply is no valid answer.
        let host = scripted(b"nope\n");
        assert!(matches!(
            focus_client(&path),
            Err(FocusError::NotAnswered(message)) if message.contains("no valid answer")
        ));
        host.join().expect("host thread");

        // A host that accepts but never replies is dropped after the
        // bounded wait.
        let _ = fs::remove_file(&path);
        let host = net::UnixListener::bind(&path).expect("silent host bind");
        let silent = std::thread::spawn(move || {
            let _ = host.accept().expect("accept");
            std::thread::sleep(HANDSHAKE_TIMEOUT + std::time::Duration::from_millis(200));
        });
        assert!(
            matches!(
                focus_client(&path),
                Err(FocusError::NotAnswered(message)) if message.contains("no valid answer")
            ),
            "a silent host is not answered in time"
        );
        silent.join().expect("silent host thread");

        // No host at all: the file the last listener left behind is stale
        // (nothing answers the connect).
        assert!(
            matches!(
                focus_client(&path),
                Err(FocusError::NotAnswered(message)) if message.contains("no pinwin")
            ),
            "a stale path is not answered"
        );

        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
