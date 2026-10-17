//! The `pinwin` binary's command-line IPC end to end (replace-terminal-with-
//! app-host row 5.6, spec "Command-line exit codes").
//!
//! Every test runs the real binary with a private runtime directory as
//! `XDG_RUNTIME_DIR` and a made-up `WAYLAND_DISPLAY`, set on the child only,
//! so no test touches the user's runtime directory, socket or config file.
//! The daemon tests start `pinwin daemon` in the same scratch directory and
//! wait, bounded, until its socket answers before they send a request.
//!
//! The daemon checks for wlr-layer-shell before it binds its socket (row 6.1,
//! spec "Wayland layer-shell is required"), so each test that starts a daemon
//! runs a stand-in compositor on the made-up display. The stand-in is a
//! wl-proxy `State` without an upstream connection: it announces
//! `zwlr_layer_shell_v1` and answers the daemon's roundtrip, and it never
//! touches the user's compositor.

use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use wl_proxy::baseline::Baseline;
use wl_proxy::client::ClientHandler;
use wl_proxy::object::ConcreteObject;
use wl_proxy::protocols::wayland::wl_callback::WlCallback;
use wl_proxy::protocols::wayland::wl_display::{WlDisplay, WlDisplayHandler};
use wl_proxy::protocols::wayland::wl_registry::WlRegistry;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wl_proxy::state::State;

/// The display the tests use: a made-up name whose socket the stand-in
/// compositor binds, so no test touches the user's display.
const DISPLAY: &str = "wayland-test";

/// How long a test waits for the daemon's socket to answer, and for the
/// daemon to end after `quit`.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a test waits for one `pinwin` client command to exit: longer than
/// the client's own two-second reply bound, and short enough that a wedged
/// client fails its test instead of blocking the suite.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Run one `pinwin` client to exit with a bounded wait and collect its
/// output. A child that outlives the bound is killed and reaped before the
/// test fails, so a wedged client can neither block the suite nor outlive
/// it.
fn client_output(mut command: Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run pinwin");
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    loop {
        if child.try_wait().expect("the client's status").is_some() {
            return child.wait_with_output().expect("the client's output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("pinwin did not exit within {CLIENT_TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// The daemon socket of the scratch directory (spec "Daemon socket").
fn socket_path(dir: &Path) -> PathBuf {
    dir.join("pinwin").join(format!("{DISPLAY}.sock"))
}

/// A `pinwin daemon` child and the stand-in compositor its layer-shell check
/// connects to. Dropping it kills the child in every path; the compositor
/// thread ends with the test process.
struct Daemon {
    child: Child,
    path: PathBuf,
    /// Kept for the whole test, so the daemon's check finds a compositor
    /// (row 6.1).
    _compositor: FakeCompositor,
}

impl Daemon {
    /// Start `pinwin daemon` in `dir` and wait, bounded, for its socket. The
    /// stand-in compositor listens on the test's display first, so the check
    /// passes before the daemon binds (spec "Wayland layer-shell is
    /// required").
    fn start(dir: &Path) -> Self {
        let compositor = FakeCompositor::start(dir);
        let child = pinwin(dir, &["daemon"])
            .stdout(Stdio::null())
            .spawn()
            .expect("start pinwin daemon");
        let mut daemon = Self {
            child,
            path: socket_path(dir),
            _compositor: compositor,
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

/// A stand-in compositor on the test's display socket. The daemon checks for
/// wlr-layer-shell at start (row 6.1, spec "Wayland layer-shell is required"),
/// so the tests serve one that announces it instead of connecting to the
/// user's compositor.
struct FakeCompositor {
    _thread: thread::JoinHandle<()>,
}

impl FakeCompositor {
    /// Listen on `dir`'s display socket and answer every connection.
    fn start(dir: &Path) -> Self {
        let listener =
            UnixListener::bind(dir.join(DISPLAY)).expect("the stand-in compositor socket");
        let thread = thread::spawn(move || {
            // One thread and one `State` for each connection, as the real
            // proxy serves one upstream connection per panel (D1).
            while let Ok((stream, _)) = listener.accept() {
                thread::spawn(move || answer_as_compositor(stream.into()));
            }
        });
        Self { _thread: thread }
    }
}

/// Answer one proxy connection as a compositor that implements layer-shell:
/// `get_registry` announces `zwlr_layer_shell_v1` and `sync` ends the
/// roundtrip (`src/proxy/check.rs`).
fn answer_as_compositor(socket: OwnedFd) {
    let state = State::builder(Baseline::V6)
        .without_server()
        .build()
        .expect("a stand-in compositor state");
    state.set_default_forward_to_server(false);
    let _destructor = state.create_destructor();
    let client = state
        .add_client(&Rc::new(socket))
        .expect("the daemon's connection");
    client.display().set_handler(FakeDisplay);
    client.set_handler(FakeClient {
        state: Rc::clone(&state),
    });
    while state.is_not_destroyed() {
        if state.dispatch_blocking().is_err() {
            return;
        }
    }
}

/// The stand-in compositor's display: it answers the two requests the
/// daemon's check sends (spec "Layer-shell is required").
struct FakeDisplay;

impl WlDisplayHandler for FakeDisplay {
    fn handle_get_registry(&mut self, _slf: &Rc<WlDisplay>, registry: &Rc<WlRegistry>) {
        // Version 4 is the first with `keyboard_interactivity` `on_demand`,
        // which row 8.3 uses.
        registry.send_global(1, ZwlrLayerShellV1::INTERFACE, 4);
    }

    fn handle_sync(&mut self, _slf: &Rc<WlDisplay>, callback: &Rc<WlCallback>) {
        callback.send_done(0);
    }
}

/// The stand-in compositor ends when its connection closes, so its thread
/// does not stay blocked in `dispatch` (D1).
struct FakeClient {
    state: Rc<State>,
}

impl ClientHandler for FakeClient {
    fn disconnected(self: Box<Self>) {
        self.state.destroy();
    }
}

/// With no daemon for the display, a client prints the daemon's absence and
/// exits 1 (spec "No daemon").
#[test]
fn a_client_with_no_daemon_exits_1() {
    let dir = temp_dir("no-daemon");
    let output = client_output(pinwin(&dir, &["toggle", "notes"]));
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
    let output = client_output(pinwin(&dir, &["show", "nope"]));
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
    let output = client_output(pinwin(&dir, &["frobnicate"]));
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
    let output = client_output(pinwin(&dir, &["status"]));
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
    let output = client_output(pinwin(&dir, &["quit"]));
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

/// The two `pinwin run` children of the "Two first runs" test and the
/// daemon they start. Dropping it asks the daemon to `quit`, bounded, and
/// then kills and reaps both children, so no daemon and no app outlives the
/// test whatever it asserted.
struct FirstRuns {
    dir: PathBuf,
    children: Vec<Child>,
}

impl FirstRuns {
    fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
            children: Vec::new(),
        }
    }

    /// Start `pinwin run` with `name` and the harmless `sleep` app, so the
    /// panel stays until the test ends it.
    fn start(&mut self, name: &str) {
        let child = pinwin(&self.dir, &["run", "--name", name, "--", "sleep", "30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start pinwin run");
        self.children.push(child);
    }

    /// Ask the daemon to `quit` and wait, bounded, for its socket file to go.
    fn quit(&self) {
        let path = socket_path(&self.dir);
        if !path.exists() {
            return;
        }
        let mut command = pinwin(&self.dir, &["quit"]);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let Ok(mut child) = command.spawn() else {
            return;
        };
        let deadline = Instant::now() + CLIENT_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        let deadline = Instant::now() + CLIENT_TIMEOUT;
        while path.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for FirstRuns {
    fn drop(&mut self) {
        self.quit();
        for mut child in self.children.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The panel names of a status reply, in the daemon's start order.
fn panel_names(reply: &serde_json::Value) -> Vec<String> {
    reply["panels"].as_array().map_or_else(Vec::new, |panels| {
        panels
            .iter()
            .filter_map(|panel| panel["name"].as_str().map(str::to_owned))
            .collect()
    })
}

/// Wait, bounded, until the daemon answers `status` with every one of
/// `names`, and return that reply.
fn wait_for_panels(dir: &Path, names: &[&str]) -> serde_json::Value {
    let deadline = Instant::now() + DAEMON_TIMEOUT;
    loop {
        let output = client_output(pinwin(dir, &["status"]));
        if output.status.code() == Some(0)
            && let Ok(reply) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        {
            let panels = panel_names(&reply);
            if names
                .iter()
                .all(|name| panels.iter().any(|panel| panel == name))
            {
                return reply;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the daemon did not own {names:?} within {DAEMON_TIMEOUT:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// Two `pinwin run` calls that start at the same time with no daemon leave
/// one daemon that owns both panels (spec "Run starts the daemon": "Two
/// first runs", D12). The daemon's socket bind decides the race; the loser's
/// spawned daemon exits 0 and the loser's `run` waits for the winner.
#[test]
fn two_first_runs_leave_one_daemon() {
    let dir = temp_dir("two-first-runs");
    let _compositor = FakeCompositor::start(&dir);
    let mut runs = FirstRuns::new(&dir);
    runs.start("first");
    runs.start("second");
    let reply = wait_for_panels(&dir, &["first", "second"]);
    let pid = reply["pid"].as_u64().expect("the daemon names itself");
    assert_eq!(
        panel_names(&reply).len(),
        2,
        "the one daemon owns both panels: {reply}"
    );
    // The same daemon answers again and still owns both panels, so no second
    // daemon replaced it.
    let again = client_output(pinwin(&dir, &["status"]));
    assert_eq!(again.status.code(), Some(0), "status, status");
    let again: serde_json::Value =
        serde_json::from_slice(&again.stdout).expect("the reply is one JSON object");
    assert_eq!(
        again["pid"].as_u64(),
        Some(pid),
        "the daemon that answers is the first one: {again}"
    );
    assert_eq!(
        panel_names(&again).len(),
        2,
        "it still owns both panels: {again}"
    );
    drop(runs);
    assert!(
        !socket_path(&dir).exists(),
        "the daemon removed its socket file"
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
    let output = client_output(pinwin(&dir, &["daemon"]));
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
