//! The `pinwin` binary's command-line IPC end to end (replace-terminal-with-
//! app-host row 5.6, spec "Command-line exit codes").
//!
//! Every test runs the real binary with a private runtime directory as
//! `XDG_RUNTIME_DIR` and a made-up `WAYLAND_DISPLAY`, set on the child only,
//! so no test touches the user's runtime directory, socket or config file.
//! The daemon tests start `pinwin daemon` in the same scratch directory and
//! wait, bounded, until its socket answers before they send a request. The
//! daemon needs no compositor until row 6.1 adds the layer-shell check.

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The display the tests use; its Wayland socket does not exist, because a
/// daemon serves its panels only once the proxy rows land.
const DISPLAY: &str = "wayland-test";

/// How long a test waits for the daemon's socket to answer, and for the
/// daemon to end after `quit`.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(10);

/// A unique scratch directory under the system temp dir, standing in for
/// `XDG_RUNTIME_DIR` and the config home: the tests never touch the process
/// environment and never collide (nextest runs each test in its own process;
/// a plain `cargo test` shares one).
fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pinwin-cli-test-{}-{}-{}",
        std::process::id(),
        tag,
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).expect("temp dir");
    dir
}

/// A `pinwin` command in the display-free environment: the scratch runtime
/// directory, the made-up display and a config home inside the scratch
/// directory, so the daemon reads no user config file.
fn pinwin(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pinwin"));
    command
        .env("XDG_RUNTIME_DIR", dir)
        .env("WAYLAND_DISPLAY", DISPLAY)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .args(args);
    command
}

/// The daemon socket of the scratch directory (spec "Daemon socket").
fn socket_path(dir: &Path) -> PathBuf {
    dir.join("pinwin").join(format!("{DISPLAY}.sock"))
}

/// A `pinwin daemon` child that a failing test cannot leave behind: dropping
/// it kills the child in every path.
struct Daemon {
    child: Child,
    path: PathBuf,
}

impl Daemon {
    /// Start `pinwin daemon` in `dir` and wait, bounded, for its socket.
    fn start(dir: &Path) -> Self {
        let child = pinwin(dir, &["daemon"])
            .stdout(Stdio::null())
            .spawn()
            .expect("start pinwin daemon");
        let mut daemon = Self {
            child,
            path: socket_path(dir),
        };
        daemon.wait_until_answering();
        daemon
    }

    /// Wait until the daemon socket accepts a connection, so a request
    /// cannot lose the race with the bind.
    fn wait_until_answering(&mut self) {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        while Instant::now() < deadline {
            if UnixStream::connect(&self.path).is_ok() {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("the daemon's status") {
                panic!("pinwin daemon exited before its socket answered: {status}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("pinwin daemon did not answer within {DAEMON_TIMEOUT:?}");
    }

    /// Wait, bounded, for the daemon to exit on its own.
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("the daemon's status") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "pinwin daemon did not exit within {DAEMON_TIMEOUT:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let running = self.child.try_wait().is_ok_and(|status| status.is_none());
        if running {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// With no daemon for the display, a client prints the daemon's absence and
/// exits 1 (spec "No daemon").
#[test]
fn a_client_with_no_daemon_exits_1() {
    let dir = temp_dir("no-daemon");
    let output = pinwin(&dir, &["toggle", "notes"])
        .output()
        .expect("run pinwin toggle");
    assert_eq!(output.status.code(), Some(1), "no daemon, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pinwin:") && stderr.contains("no daemon answered"),
        "the absence is reported on stderr: {stderr}"
    );
    assert!(output.stdout.is_empty(), "nothing is printed on stdout");
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A panel command for a name the daemon does not know prints the daemon's
/// message and exits 1 (spec "Unknown panel").
#[test]
fn an_unknown_panel_exits_1() {
    let dir = temp_dir("unknown-panel");
    let daemon = Daemon::start(&dir);
    let output = pinwin(&dir, &["show", "nope"])
        .output()
        .expect("run pinwin show");
    assert_eq!(output.status.code(), Some(1), "unknown panel, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pinwin:") && stderr.contains("no panel named 'nope'"),
        "the daemon's message is printed: {stderr}"
    );
    drop(daemon);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A command line the parser refuses prints a message and exits 2 (spec
/// "Usage error").
#[test]
fn a_usage_error_exits_2() {
    let dir = temp_dir("usage-error");
    let output = pinwin(&dir, &["frobnicate"])
        .output()
        .expect("run pinwin frobnicate");
    assert_eq!(output.status.code(), Some(2), "usage error, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pinwin:") && stderr.contains("frobnicate"),
        "the refusal names the subcommand: {stderr}"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// `pinwin status` prints the reply's JSON on stdout and exits 0, and a
/// daemon that never had a panel stays for it (spec "Command-line exit
/// codes", "Daemon lifetime preference").
#[test]
fn status_prints_the_reply_json() {
    let dir = temp_dir("status");
    let daemon = Daemon::start(&dir);
    let output = pinwin(&dir, &["status"])
        .output()
        .expect("run pinwin status");
    assert_eq!(output.status.code(), Some(0), "status, status");
    let reply: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the reply is one JSON object");
    assert_eq!(reply["ok"], true, "the daemon answers: {reply}");
    assert!(
        reply["pid"].as_u64().is_some(),
        "the reply names the daemon: {reply}"
    );
    assert_eq!(
        reply["panels"],
        serde_json::json!([]),
        "no panel exists: {reply}"
    );
    drop(daemon);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// `pinwin quit` gets its success reply, the daemon exits 0 and its socket
/// file is gone (spec "Quit", "Daemon socket").
#[test]
fn quit_ends_the_daemon_and_removes_its_socket() {
    let dir = temp_dir("quit");
    let mut daemon = Daemon::start(&dir);
    let output = pinwin(&dir, &["quit"]).output().expect("run pinwin quit");
    assert_eq!(output.status.code(), Some(0), "quit, status");
    let status = daemon.wait();
    assert_eq!(status.code(), Some(0), "the daemon exits 0 after quit");
    assert!(
        !daemon.path.exists(),
        "the daemon removed its socket file: {}",
        daemon.path.display()
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A daemon that starts with an invalid config file names the file and the
/// line, exits 2 and takes no socket (spec "Invalid config file").
#[test]
fn an_invalid_config_file_exits_2() {
    let dir = temp_dir("invalid-config");
    let config = dir.join("config").join("pinwin").join("config.toml");
    fs::create_dir_all(config.parent().expect("a config directory"))
        .expect("create the config dir");
    fs::write(&config, "[panel.defaults]\nedge = \"middle\"\n").expect("write the broken file");
    let output = pinwin(&dir, &["daemon"])
        .output()
        .expect("run pinwin daemon");
    assert_eq!(output.status.code(), Some(2), "invalid config, status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("config.toml:2:") && stderr.contains("middle"),
        "the message names the file and line: {stderr}"
    );
    assert!(
        !dir.join("pinwin").exists(),
        "the daemon took no socket for an invalid file"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}
