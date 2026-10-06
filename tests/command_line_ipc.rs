//! The `pinwin` binary's command-line IPC end to end, without a display
//! (`keyboard-focus-request` rows 3.4 and 3.5). Both tests redirect the
//! socket path into a private runtime directory, so they never touch the
//! real session's sockets:
//!
//! - After `pinwin true` exits, the socket file the host bound is gone. The
//!   panel cannot start here (the temporary runtime directory has no
//!   Wayland socket), which still covers the row: the host removes the file
//!   on the shutdown path it takes, whatever the child's outcome.
//! - `pinwin --focus notes` with no `XDG_ACTIVATION_TOKEN` prints a
//!   `pinwin:` message on stderr naming the variable and exits 2 without
//!   contacting any instance (replace-gtk-with-wayland D4).
//!
//! The with-token exit-1 path has no binary-level test here on purpose:
//! while the binary still links GTK (removed later in
//! `replace-gtk-with-wayland`), GTK's own startup constructor consumes
//! `XDG_ACTIVATION_TOKEN` before `main` runs, so a spawned `pinwin --focus`
//! cannot see a token at all. That path is covered in `src/main.rs`'s unit
//! test with the injected environment; it goes live once GTK is unlinked.

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
/// does not exist, so the panel cannot open a display anywhere.
fn display_free_env(command: &mut Command, dir: &PathBuf) {
    command
        .env("XDG_RUNTIME_DIR", dir)
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env_remove("DISPLAY")
        .env_remove("XDG_ACTIVATION_TOKEN");
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

/// Without `XDG_ACTIVATION_TOKEN` the client exits 2 naming the variable,
/// before it contacts any instance: the reply it never got is the no-host
/// exit 1, so the two statuses tell the order apart.
#[test]
fn the_focus_client_prints_a_message_and_exits_2_without_a_token() {
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

    assert_eq!(output.status.code(), Some(2), "no token, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pinwin:") && stderr.contains("XDG_ACTIVATION_TOKEN"),
        "the variable is named on stderr: {stderr}"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}
