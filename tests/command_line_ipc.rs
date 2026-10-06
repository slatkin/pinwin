//! The `pinwin` binary's command-line IPC end to end, without a display
//! (`keyboard-focus-request` rows 3.4 and 3.5). Both tests redirect the
//! socket path into a private runtime directory, so they never touch the
//! real session's sockets:
//!
//! - After `pinwin true` exits, the socket file the host bound is gone. The
//!   panel cannot start here (the temporary runtime directory has no
//!   Wayland socket), which still covers the row: the host removes the file
//!   on the shutdown path it takes, whatever the child's outcome.
//! - `pinwin --focus notes` with no host prints a `pinwin:` message on
//!   stderr and exits 1.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A unique scratch directory under the system temp dir, so the tests never
/// collide (nextest runs each test in its own process; a plain `cargo test`
/// shares one).
fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pinwin-bin-ipc-test-{}-{}-{}",
        std::process::id(),
        tag,
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).expect("temp dir");
    dir
}

/// The environment for a display-free run: the runtime directory is the
/// scratch dir (no Wayland socket in it) and the display names a file that
/// does not exist, so GTK cannot open a display anywhere.
fn display_free_env(command: &mut Command, dir: &PathBuf) {
    command
        .env("XDG_RUNTIME_DIR", dir)
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env_remove("DISPLAY");
}

#[test]
fn the_socket_file_is_gone_after_pinwin_true_exits() {
    let dir = temp_dir("host");
    let socket_file = dir.join("pinwin").join("wayland-test-default.sock");

    let output = {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pinwin"));
        display_free_env(&mut command, &dir);
        command.arg("true").output().expect("run pinwin")
    };

    // The child exited 0, but the host exits 1: the panel cannot start
    // without a display, and the socket file it bound is removed anyway —
    // on this shutdown path as on the panel's own.
    assert_eq!(output.status.code(), Some(1), "no display, status");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot start the panel"),
        "the failure is reported: {:?}",
        output.stderr
    );
    assert!(!socket_file.exists(), "the socket file is gone");
    fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn the_focus_client_prints_a_message_and_exits_1_without_a_host() {
    let dir = temp_dir("focus");

    let output = {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pinwin"));
        display_free_env(&mut command, &dir);
        command
            .arg("--focus")
            .arg("notes")
            .output()
            .expect("run pinwin")
    };

    assert_eq!(output.status.code(), Some(1), "no host, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("pinwin:"), "message on stderr: {stderr}");
    assert!(stderr.contains("notes"), "the name is in the message");
    fs::remove_dir_all(&dir).expect("cleanup");
}
