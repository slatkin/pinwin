//! The `pinwin` binary's command-line IPC end to end, without a display
//! (`keyboard-focus-request`'s socket plumbing, the toggle protocol of
//! `replace-gtk-with-wayland` D4). The tests redirect the socket path into
//! a private runtime directory, so they never touch the real session's
//! sockets:
//!
//! - After `pinwin true` exits, the socket file the host bound is gone. The
//!   panel cannot start here (the temporary runtime directory has no
//!   Wayland socket), which still covers the row: the host removes the file
//!   on the shutdown path it takes, whatever the child's outcome.
//! - `pinwin --toggle notes` and `pinwin --show notes` with no instance
//!   answer exit 1 with a `pinwin:` message on stderr.
//! - `pinwin --toggle a/b` and `pinwin --show a/b` print a message naming
//!   the option and exit 2 without contacting any instance.

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

/// With no instance listening on the name, the client reports the request
/// as not answered and exits 1: the socket path resolved (the runtime
/// directory is set), so this is the no-host status, not the environment
/// error's exit 2.
#[test]
fn the_toggle_client_without_an_instance_exits_1() {
    let dir = temp_dir("no-instance");

    let output = {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pinwin"));
        display_free_env(&mut command, &dir);
        command
            .arg("--toggle")
            .arg("notes")
            .output()
            .expect("run pinwin")
    };

    assert_eq!(output.status.code(), Some(1), "no instance, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pinwin:") && stderr.contains("no pinwin is listening"),
        "the failure is reported on stderr: {stderr}"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The show client behaves like the toggle one with no instance listening:
/// the request is not answered, exit 1.
#[test]
fn the_show_client_without_an_instance_exits_1() {
    let dir = temp_dir("no-instance-show");

    let output = {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pinwin"));
        display_free_env(&mut command, &dir);
        command
            .arg("--show")
            .arg("notes")
            .output()
            .expect("run pinwin")
    };

    assert_eq!(output.status.code(), Some(1), "no instance, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pinwin:") && stderr.contains("no pinwin is listening"),
        "the failure is reported on stderr: {stderr}"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// An invalid name after `--toggle` exits 2 naming the option, before any
/// socket path is resolved and without contacting any instance.
#[test]
fn the_toggle_client_rejects_an_invalid_name_with_exit_2() {
    let dir = temp_dir("bad-name");

    let output = {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pinwin"));
        display_free_env(&mut command, &dir);
        command
            .arg("--toggle")
            .arg("a/b")
            .output()
            .expect("run pinwin")
    };

    assert_eq!(output.status.code(), Some(2), "invalid name, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pinwin: --toggle:"),
        "the rejection names the option: {stderr}"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The same for `--show`: an invalid name exits 2 naming the option.
#[test]
fn the_show_client_rejects_an_invalid_name_with_exit_2() {
    let dir = temp_dir("bad-name-show");

    let output = {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pinwin"));
        display_free_env(&mut command, &dir);
        command
            .arg("--show")
            .arg("a/b")
            .output()
            .expect("run pinwin")
    };

    assert_eq!(output.status.code(), Some(2), "invalid name, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pinwin: --show:"),
        "the rejection names the option: {stderr}"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}
