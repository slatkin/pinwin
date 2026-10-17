//! The harness that compiles the daemon's tests
//! (replace-terminal-with-app-host rows 5.2 to 5.5).
//!
//! `src/daemon.rs` holds the one socket that serves a Wayland display's
//! panels (spec "Daemon socket", "One daemon for each Wayland display") and,
//! with `src/daemon/panel.rs`, `src/daemon/registry.rs`,
//! `src/daemon/events.rs`, `src/daemon/connection.rs` and
//! `src/daemon/lifetime.rs`, the panel handles, the routing of a request to a
//! panel, the event bus, one IPC connection and the daemon's lifetime
//! (D12). `src/main.rs` compiles the daemon once it dispatches
//! `pinwin daemon`. Until then this file includes the modules, so `cargo
//! nextest run` exercises the path rule, the stale-file replacement, the
//! live-daemon refusal, the two displays, the socket file's mode, the name
//! rules, the status reply and the routing, with a test thread in the panel's
//! place, all without a daemon. The event-stream tests live in
//! `src/daemon/events/tests.rs`, which `src/daemon/events.rs` includes under
//! `cfg(test)`, and the lifetime tests in `tests/daemon/lifetime.rs`, included
//! as a child module below.
//!
//! The modules are `pub` so the dead-code lint does not report the parts of
//! the included modules these tests do not reach.

#[path = "../src/geometry.rs"]
pub mod geometry;

#[path = "../src/name.rs"]
pub mod name;

#[path = "../src/config.rs"]
pub mod config;

#[path = "../src/cli.rs"]
pub mod cli;

#[path = "../src/ipc.rs"]
pub mod ipc;

#[path = "../src/daemon.rs"]
pub mod daemon;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use config::{Config, Keyboard, Layer, PanelOptions, PanelSettings};
use daemon::panel::{Command, EventFd, PanelHandle, PanelReply, PanelRequest};
use daemon::registry::Registry;
use daemon::{PathError, SocketError, socket_path};
use geometry::{Edge, ReserveMode, Size};
use ipc::{ErrorKind, ErrorReply, PanelStatus, Reply, Request};
use name::PanelName;

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

/// How long the routing tests wait for a panel that answers at once.
const WAIT: Duration = Duration::from_millis(500);

/// The panel name `notes`.
fn notes() -> PanelName {
    PanelName::parse("notes").expect("notes is a panel name")
}

/// The status a stand-in panel reports: the built-in defaults, shown, no
/// window, 640 pixels wide (spec "Status", "Built-in defaults").
fn defaults_status() -> PanelStatus {
    PanelStatus::new(PanelSettings::built_in(), true, false, 640)
}

/// A stand-in for a panel thread (D12): it takes each command off the
/// channel, clears the eventfd that delivered it and answers what the test
/// says, so routing runs against a plain thread with no proxy.
fn panel_thread(
    name: &str,
    pid: u32,
    answer: impl Fn(&PanelRequest) -> PanelReply + Send + 'static,
) -> (PanelName, PanelHandle, JoinHandle<()>) {
    let (commands, incoming) = mpsc::channel::<Command>();
    let wake = Arc::new(EventFd::new().expect("an eventfd"));
    let watch = Arc::clone(&wake);
    let thread = thread::spawn(move || {
        while let Ok(command) = incoming.recv() {
            watch.drain().expect("the panel clears its wake");
            let reply = answer(command.request());
            command.answer(reply);
        }
    });
    let name = PanelName::parse(name).expect("a panel name");
    (name, PanelHandle::new(pid, commands, wake), thread)
}

/// A panel that reports `status` when the daemon asks for it and carries
/// every other command out.
fn answering_panel(
    name: &str,
    pid: u32,
    status: PanelStatus,
) -> (PanelName, PanelHandle, JoinHandle<()>) {
    panel_thread(name, pid, move |request| match request {
        PanelRequest::Status => PanelReply::Status(status.clone()),
        _ => PanelReply::Done,
    })
}

/// A panel thread that takes each command and never answers, so the daemon
/// has to give up on its own bounded wait (spec "The daemon survives bad
/// input").
fn silent_panel(name: &str) -> (PanelName, PanelHandle, JoinHandle<()>) {
    let (commands, incoming) = mpsc::channel::<Command>();
    let wake = Arc::new(EventFd::new().expect("an eventfd"));
    let watch = Arc::clone(&wake);
    let thread = thread::spawn(move || {
        // Keeping the command keeps its reply channel open, so the daemon
        // waits out its timeout instead of seeing an ended panel.
        let mut held = Vec::new();
        while let Ok(command) = incoming.recv() {
            watch.drain().expect("the panel clears its wake");
            held.push(command);
        }
    });
    let name = PanelName::parse(name).expect("a panel name");
    (name, PanelHandle::new(0, commands, wake), thread)
}

/// The wake is an eventfd: every command adds one to its count and the
/// panel's loop clears the count in one read (D12).
#[test]
fn the_wake_counts_and_clears() {
    let wake = EventFd::new().expect("an eventfd");
    assert_eq!(wake.drain().expect("drain"), 0, "a fresh wake is clear");
    wake.notify().expect("notify");
    wake.notify().expect("notify");
    assert_eq!(wake.drain().expect("drain"), 2, "both commands woke it");
    assert_eq!(
        wake.drain().expect("drain"),
        0,
        "the drain cleared the count"
    );
}

/// A command for a name no panel has is `no_panel`, and `status` for that
/// name is the same kind (spec "Unknown panel").
#[test]
fn an_unknown_panel_is_no_panel() {
    let registry = Registry::new();
    let reply = registry
        .route(&Request::Show { panel: notes() }, WAIT)
        .expect("the registry answers a panel request");
    let error = reply.as_error().expect("an error reply");
    assert_eq!(error.kind(), ErrorKind::NoPanel);
    assert!(
        error.message().contains("notes"),
        "the message names the panel: {}",
        error.message()
    );

    let reply = registry
        .route(
            &Request::Status {
                panel: Some(notes()),
            },
            WAIT,
        )
        .expect("the registry answers a status request");
    assert_eq!(
        reply.as_error().map(ErrorReply::kind),
        Some(ErrorKind::NoPanel)
    );
}

/// A name a panel already uses is `duplicate_name`, and the panel that has
/// it stays as it was (spec "Duplicate name").
#[test]
fn a_duplicate_name_is_refused_and_the_panel_is_unchanged() {
    let registry = Registry::new();
    let (name, handle, thread) = answering_panel("notes", 4242, defaults_status());
    registry
        .add(name.clone(), handle)
        .expect("the name is free");

    let (_, taken, taken_thread) = answering_panel("notes", 4343, defaults_status());
    let error = registry
        .add(name.clone(), taken)
        .expect_err("the name is taken");
    assert_eq!(error.kind(), ErrorKind::DuplicateName);
    assert!(
        error.message().contains("notes"),
        "the message names the panel: {}",
        error.message()
    );

    // The panel that has the name is unchanged: one entry, its own pid.
    let reply = registry
        .route(&Request::Status { panel: Some(name) }, WAIT)
        .expect("the registry answers a status request");
    let status = reply.as_status().expect("a status reply");
    assert_eq!(status.panels().len(), 1);
    assert_eq!(status.panels()[0].pid(), 4242);

    drop(registry);
    thread.join().expect("the panel thread ends");
    taken_thread.join().expect("the refused panel thread ends");
}

/// When a panel ends and leaves the registry its name is free again, and a
/// new panel takes it (spec "Name is free again").
#[test]
fn a_name_is_free_again_when_its_panel_ends() {
    let registry = Registry::new();
    let (name, handle, ended) = answering_panel("notes", 11, defaults_status());
    registry
        .add(name.clone(), handle)
        .expect("the name is free");
    assert!(
        registry.remove(&name),
        "the ended panel was in the registry"
    );
    assert!(!registry.remove(&name), "a name is removed once");
    ended.join().expect("the ended panel thread ends");

    let (again, handle, thread) = answering_panel("notes", 22, defaults_status());
    registry
        .add(again.clone(), handle)
        .expect("the name is free again");
    let reply = registry
        .route(&Request::Status { panel: Some(name) }, WAIT)
        .expect("the registry answers a status request");
    let status = reply.as_status().expect("a status reply");
    assert_eq!(status.panels().len(), 1);
    assert_eq!(status.panels()[0].pid(), 22, "the new panel has the name");

    drop(registry);
    thread.join().expect("the new panel thread ends");
}

/// `status` without a panel reports the daemon's process ID and every panel
/// with the fields the spec names, in start order (spec "Status of all
/// panels").
#[test]
fn status_lists_every_panel_in_start_order() {
    let registry = Registry::new();
    let chat_name = PanelName::parse("chat").expect("chat is a panel name");
    let chat_settings = Config::parse(
        Path::new("config.toml"),
        "[panel.chat]\nedge = 'right'\nsize = '30%'\n",
    )
    .expect("a valid config")
    .panel(&chat_name, &PanelOptions::default());

    let (notes_name, notes_handle, notes_thread) = answering_panel("notes", 11, defaults_status());
    let (chat, chat_handle, chat_thread) = answering_panel(
        "chat",
        22,
        PanelStatus::new(chat_settings, false, true, 500),
    );
    registry
        .add(notes_name, notes_handle)
        .expect("notes is free");
    registry.add(chat, chat_handle).expect("chat is free");

    let reply = registry
        .route(&Request::Status { panel: None }, WAIT)
        .expect("the registry answers a status request");
    let status = reply.as_status().expect("a status reply");
    assert_eq!(status.pid(), std::process::id());
    // `chat` sorts before `notes` by name, so this also checks the start
    // order rather than the registry map's key order.
    let names: Vec<&str> = status
        .panels()
        .iter()
        .map(|panel| panel.name().as_str())
        .collect();
    assert_eq!(names, ["notes", "chat"]);

    let notes = &status.panels()[0];
    assert_eq!(notes.pid(), 11);
    assert!(notes.shown());
    assert!(!notes.has_window());
    assert_eq!(notes.edge(), Edge::Left);
    assert_eq!(notes.size(), Size::percent(25).expect("25 percent"));
    assert_eq!(notes.width(), 640);
    assert_eq!(notes.reserve(), ReserveMode::Push);
    assert_eq!(notes.layer(), Layer::Overlay);
    assert_eq!(notes.keyboard(), Keyboard::OnDemand);
    assert_eq!(notes.output(), None);

    let chat = &status.panels()[1];
    assert_eq!(chat.pid(), 22);
    assert!(!chat.shown());
    assert!(chat.has_window());
    assert_eq!(chat.edge(), Edge::Right);
    assert_eq!(chat.size(), Size::percent(30).expect("30 percent"));
    assert_eq!(chat.width(), 500);

    drop(registry);
    notes_thread.join().expect("the notes thread ends");
    chat_thread.join().expect("the chat thread ends");
}

/// `status` with a panel reports that panel only (spec "Status of one
/// panel").
#[test]
fn status_of_one_panel_reports_that_panel_only() {
    let registry = Registry::new();
    let (notes_name, notes_handle, notes_thread) = answering_panel("notes", 11, defaults_status());
    let (chat, chat_handle, chat_thread) = answering_panel("chat", 22, defaults_status());
    registry
        .add(notes_name, notes_handle)
        .expect("notes is free");
    registry.add(chat, chat_handle).expect("chat is free");

    let reply = registry
        .route(
            &Request::Status {
                panel: Some(notes()),
            },
            WAIT,
        )
        .expect("the registry answers a status request");
    let status = reply.as_status().expect("a status reply");
    assert_eq!(status.pid(), std::process::id());
    assert_eq!(status.panels().len(), 1);
    assert_eq!(status.panels()[0].name(), &notes());

    drop(registry);
    notes_thread.join().expect("the notes thread ends");
    chat_thread.join().expect("the chat thread ends");
}

/// A panel that does not answer makes the request fail with error kind
/// `internal`, after the bounded wait and not the production timeout (spec
/// "The daemon survives bad input").
#[test]
fn a_panel_that_does_not_answer_fails_with_internal() {
    let registry = Registry::new();
    let (name, handle, thread) = silent_panel("notes");
    registry.add(name.clone(), handle).expect("notes is free");

    let started = Instant::now();
    let reply = registry
        .route(&Request::Toggle { panel: name }, Duration::from_millis(50))
        .expect("the registry answers a panel request");
    let error = reply.as_error().expect("an error reply");
    assert_eq!(error.kind(), ErrorKind::Internal);
    assert!(
        error.message().contains("notes"),
        "the message names the panel: {}",
        error.message()
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the daemon gave up after {:?}",
        started.elapsed()
    );

    drop(registry);
    thread.join().expect("the silent panel thread ends");
}

/// A panel's own refusal keeps its error kind, so a layout the output cannot
/// hold reaches the client as `invalid_layout` (spec "Too large for the
/// output").
#[test]
fn a_panel_refusal_keeps_its_kind() {
    let registry = Registry::new();
    let (name, handle, thread) = panel_thread("notes", 7, |_| {
        PanelReply::Refused(ErrorReply::new(
            ErrorKind::InvalidLayout,
            "the output cannot hold it",
        ))
    });
    registry.add(name.clone(), handle).expect("notes is free");

    let reply = registry
        .route(
            &Request::SetSize {
                panel: name,
                size: Size::pixels(4000).expect("4000 pixels"),
                ms: Some(0),
            },
            WAIT,
        )
        .expect("the registry answers a panel request");
    assert_eq!(
        reply.as_error().map(ErrorReply::kind),
        Some(ErrorKind::InvalidLayout)
    );

    drop(registry);
    thread.join().expect("the panel thread ends");
}

/// `subscribe`, `quit` and `reload` are the daemon's own requests, so the
/// registry leaves them to the event bus, the lifetime and the config reload
/// (D12, rows 5.4 to 5.5).
#[test]
fn the_daemon_answers_its_own_requests() {
    let registry = Registry::new();
    for request in [Request::Subscribe, Request::Quit, Request::Reload] {
        assert!(
            registry.route(&request, WAIT).is_none(),
            "{request:?} is not a panel request"
        );
    }
    // A request for a panel still gets its reply, so the two paths differ.
    assert!(
        matches!(
            registry.route(&Request::Show { panel: notes() }, WAIT),
            Some(Reply::Error(_))
        ),
        "a panel request is routed"
    );
}

// The lifetime's tests are a child module so this harness stays well under
// the 800-line limit; `#[path]` keeps them in `tests/daemon/` beside it.
#[path = "daemon/lifetime.rs"]
mod lifetime;
