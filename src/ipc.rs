//! The command-line IPC identity and socket setup for the `pinwin` program
//! (change `keyboard-focus-request`): the validated instance name, the socket
//! path under `$XDG_RUNTIME_DIR/pinwin`, and the duplicate check, stale-file
//! removal and bind that all happen before any surface opens.
//!
//! The listener thread and the `--focus` client are later rows of the change;
//! the transport is the standard library's Unix sockets only, no new
//! dependencies.

use std::fmt;
use std::io;
use std::ops::Deref;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::{fs, os::unix::net};

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
/// $WAYLAND_DISPLAY-<name>.sock`. A missing or empty `XDG_RUNTIME_DIR` is an
/// exit-2 error. `WAYLAND_DISPLAY` falls back to `wayland-0`, the same
/// default the Wayland client library connects with.
pub fn socket_path(
    runtime_dir: Option<&str>,
    display: Option<&str>,
    name: &InstanceName,
) -> Result<PathBuf, String> {
    let Some(runtime_dir) = runtime_dir.filter(|dir| !dir.is_empty()) else {
        return Err(
            "pinwin: XDG_RUNTIME_DIR: expected the runtime directory of the Wayland session"
                .to_owned(),
        );
    };
    let display = display
        .filter(|display| !display.is_empty())
        .unwrap_or("wayland-0");
    Ok(Path::new(runtime_dir)
        .join("pinwin")
        .join(format!("{display}-{name}.sock")))
}

/// Take the socket at `path` for this instance, before any surface opens:
///
/// 1. A successful connect means a live host owns the name: [`BindError::Duplicate`].
/// 2. Otherwise the parent directory is created with mode 0700.
/// 3. Any file still on the path is stale (a SIGKILL-ed host leaves its
///    socket file, a regular file is stale too) and is removed.
/// 4. The bind is the final duplicate check: two hosts starting in the same
///    instant can both pass the connect, and one of the two binds then fails
///    (keyboard-focus-request design).
pub fn bind_instance_socket(path: &Path) -> Result<net::UnixListener, BindError> {
    if net::UnixStream::connect(path).is_ok() {
        return Err(BindError::Duplicate);
    }
    let dir = path.parent().expect("the socket path has a parent");
    if let Err(error) = fs::create_dir(dir)
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Err(BindError::Failed(error));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(BindError::Failed)?;
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(BindError::Failed(error));
    }
    let listener = net::UnixListener::bind(path).map_err(BindError::Failed)?;
    // The forked child execs the host's command, and the command must never
    // inherit the listener; set the close-on-exec flag explicitly instead of
    // relying on the socket type std happens to create.
    // SAFETY: `fd` is the listener's own descriptor and `F_SETFD` only sets
    // its close-on-exec flag.
    let _ = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    /// with the `wayland-0` fallback for an unset display.
    #[test]
    fn socket_path_composes_the_runtime_path() {
        let name = InstanceName::parse("x", "notes").expect("valid");
        assert_eq!(
            socket_path(Some("/run/user/1000"), Some("wayland-1"), &name),
            Ok(PathBuf::from("/run/user/1000/pinwin/wayland-1-notes.sock"))
        );
        assert_eq!(
            socket_path(Some("/run/user/1000"), None, &name),
            Ok(PathBuf::from("/run/user/1000/pinwin/wayland-0-notes.sock"))
        );
        assert_eq!(
            socket_path(Some("/run/user/1000"), Some(""), &name),
            Ok(PathBuf::from("/run/user/1000/pinwin/wayland-0-notes.sock"))
        );
        // A missing runtime directory is an exit-2 error.
        socket_path(None, Some("wayland-0"), &name).expect_err("no runtime directory");
        socket_path(Some(""), Some("wayland-0"), &name).expect_err("empty runtime directory");
    }

    /// A stale file on the socket path — the socket file of a SIGKILL-ed
    /// host, or any regular file — is removed and bound again, and the new
    /// listener answers connects.
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

        // Also a plain regular file on the path is stale, not fatal.
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
}
