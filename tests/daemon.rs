//! The harness that compiles the daemon socket's tests
//! (replace-terminal-with-app-host row 5.2).
//!
//! `src/daemon.rs` holds the one socket that serves a Wayland display's
//! panels (spec "Daemon socket", "One daemon for each Wayland display"); the
//! registry, the event bus and the daemon's lifetime arrive with rows 5.3 to
//! 5.5, and `src/main.rs` compiles the daemon once it dispatches `pinwin
//! daemon`. Until then this file includes the module, so `cargo nextest run`
//! exercises the path rule, the stale-file replacement, the live-daemon
//! refusal, the two displays and the socket file's mode without a daemon.

#[path = "../src/daemon.rs"]
pub mod daemon;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use daemon::{PathError, SocketError, socket_path};

/// A unique scratch directory under the system temp dir, standing in for
/// `XDG_RUNTIME_DIR`: the tests never touch the process environment and
/// never collide (nextest runs each test in its own process; a plain
/// `cargo test` shares one).
fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pinwin-daemon-test-{}-{}-{}",
        std::process::id(),
        tag,
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).expect("temp dir");
    dir
}

/// The socket path is `$XDG_RUNTIME_DIR/pinwin/<display>.sock`, with the
/// base name of the display's socket and the `wayland-0` fallback the
/// Wayland client library connects with (spec "Daemon socket").
#[test]
fn the_socket_path_is_named_for_the_display() {
    let os = OsStr::new;
    assert_eq!(
        socket_path(Some(os("/run/user/1000")), Some(os("wayland-1"))),
        Ok(PathBuf::from("/run/user/1000/pinwin/wayland-1.sock"))
    );
    assert_eq!(
        socket_path(Some(os("/run/user/1000")), None),
        Ok(PathBuf::from("/run/user/1000/pinwin/wayland-0.sock"))
    );
    // `WAYLAND_DISPLAY` may be an absolute path; only its base name joins
    // the runtime directory, so the socket stays inside `pinwin/`.
    assert_eq!(
        socket_path(
            Some(os("/run/user/1000")),
            Some(os("/tmp/way-land/wayland-2"))
        ),
        Ok(PathBuf::from("/run/user/1000/pinwin/wayland-2.sock"))
    );

    // A display that names no socket is a typed path error.
    for raw in [os(""), os("/"), os("..")] {
        let error = socket_path(Some(os("/run/user/1000")), Some(raw)).expect_err("bad display");
        assert_eq!(
            error,
            PathError::BadDisplay(raw.to_string_lossy().into_owned()),
            "raw = {raw:?}"
        );
    }
}

/// With no `XDG_RUNTIME_DIR` there is no socket path, so the daemon exits 1
/// and creates nothing (spec "No runtime directory").
#[test]
fn no_runtime_directory_is_a_path_error_and_creates_nothing() {
    let dir = temp_dir("no-runtime");
    for runtime_dir in [None, Some(OsStr::new(""))] {
        let error = socket_path(runtime_dir, Some(OsStr::new("wayland-1")))
            .expect_err("no runtime directory");
        assert_eq!(error, PathError::NoRuntimeDir);
        assert_eq!(
            error.to_string(),
            "XDG_RUNTIME_DIR: expected the runtime directory of the Wayland session"
        );
    }
    assert!(
        fs::read_dir(&dir).expect("read dir").next().is_none(),
        "nothing is created"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A socket file a killed daemon left behind is removed and bound again
/// (spec "Stale socket file").
#[test]
fn a_stale_socket_file_is_replaced() {
    let dir = temp_dir("stale");
    let path = dir.join("pinwin").join("wayland-1.sock");
    fs::create_dir(dir.join("pinwin")).expect("pinwin dir");

    // A SIGKILL-ed daemon leaves the socket file but nobody answers it.
    drop(net::UnixListener::bind(&path).expect("stale socket file"));
    net::UnixStream::connect(&path).expect_err("nothing listens on the stale file");

    let socket = daemon::bind(&path).expect("stale socket replaced");
    net::UnixStream::connect(&path).expect("the new socket is live");
    drop(socket);

    // A plain regular file refuses the connect the same way.
    fs::write(&path, b"not a socket").expect("regular file");
    drop(daemon::bind(&path).expect("regular file replaced"));
    assert!(!path.exists(), "the replaced socket's file is gone");
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A live daemon keeps its socket: the second daemon reports the live owner
/// and leaves the file untouched (spec "Second daemon").
#[test]
fn a_live_daemon_is_not_replaced() {
    let dir = temp_dir("live");
    let path = dir.join("pinwin").join("wayland-1.sock");

    let first = daemon::bind(&path).expect("first bind");
    assert!(
        matches!(daemon::bind(&path), Err(SocketError::Live)),
        "the second daemon is refused"
    );
    net::UnixStream::connect(&path).expect("the first daemon's socket still answers");
    drop(first);

    // The first daemon's exit leaves the display free again.
    drop(daemon::bind(&path).expect("the display is free again"));
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// Two displays get two sockets under the same runtime directory, and each
/// answers only on its own path (spec "Two displays").
#[test]
fn two_displays_get_two_sockets() {
    let dir = temp_dir("two-displays");
    let one = socket_path(Some(dir.as_os_str()), Some(OsStr::new("wayland-1"))).expect("path");
    let two = socket_path(Some(dir.as_os_str()), Some(OsStr::new("wayland-2"))).expect("path");
    assert_ne!(one, two);

    let first = daemon::bind(&one).expect("wayland-1");
    let second = daemon::bind(&two).expect("wayland-2");
    net::UnixStream::connect(&one).expect("wayland-1 answers");
    net::UnixStream::connect(&two).expect("wayland-2 answers");
    drop((first, second));
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The socket file is readable and writable only by its owner, inside a
/// directory that nobody else can enter (spec "Daemon socket").
#[test]
fn the_socket_file_is_private() {
    let dir = temp_dir("perm");
    let path = dir.join("pinwin").join("wayland-1.sock");

    let socket = daemon::bind(&path).expect("bind");
    let mode = fs::metadata(&path)
        .expect("stat socket")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the socket file is 0600");
    let dir_mode = fs::metadata(path.parent().expect("parent"))
        .expect("stat dir")
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700, "the pinwin dir is 0700");
    drop(socket);

    // A rebind sets the mode again, so the umask cannot loosen it.
    let socket = daemon::bind(&path).expect("second bind");
    let mode = fs::metadata(&path)
        .expect("stat socket")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the rebound socket file is 0600");
    drop(socket);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// Dropping the daemon's socket removes its file, so every exit path after
/// the bind cleans up (spec "Daemon socket": "When the daemon exits, it SHALL
/// remove its socket file").
#[test]
fn a_dropped_socket_removes_its_file() {
    let dir = temp_dir("drop");
    let path = dir.join("pinwin").join("wayland-1.sock");

    let socket = daemon::bind(&path).expect("bind");
    assert!(path.exists(), "the bind created the socket file");
    net::UnixStream::connect(&path).expect("the bind is live");
    drop(socket);
    assert!(!path.exists(), "the dropped socket's file is gone");

    // The display is free again, and the second socket cleans up the same
    // way.
    drop(daemon::bind(&path).expect("the display is free again"));
    assert!(!path.exists(), "the second socket's file is gone too");
    fs::remove_dir_all(&dir).expect("cleanup");
}
