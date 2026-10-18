//! The daemon-lifetime tests of replace-terminal-with-app-host row 5.5
//! (spec "Daemon lifetime preference", "Quit", "Reload"): the daemon exits
//! with its last panel or stays resident, `quit` ends every panel and the
//! daemon, and `reload` applies a valid file and keeps the previous config
//! for an invalid one.
//!
//! Each test binds a real socket under a scratch runtime directory and runs
//! [`Daemon::run`] on a thread, so the client talks to the daemon through the
//! wire format with no Wayland display. The panels are stand-in threads
//! (D12). Shutdown is the lifetime's own trigger, so no test sends a real
//! signal: only `shutdown_signal`'s mapping is tested, and the accept loop's
//! SIGINT and SIGTERM handling is live code (D12).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use super::{WAIT, answering_panel, defaults_status, temp_dir};
use crate::config::{Config, PanelOptions};
use crate::daemon;
use crate::daemon::lifetime::{Daemon, Lifetime, SignalFd, no_window_message, shutdown_signal};
use crate::geometry::Size;
use crate::ipc::{self, ErrorKind, Request};
use crate::name::PanelName;

/// The config file's name inside a test's scratch directory.
const CONFIG_FILE: &str = "config.toml";

/// A daemon whose accept loop runs on `dir`'s socket, with its config file at
/// `dir/config.toml` and a short panel wait.
struct Started {
    path: PathBuf,
    lifetime: Arc<Lifetime>,
    loop_thread: JoinHandle<io::Result<()>>,
}

/// Bind the socket, construct the daemon and start its accept loop.
fn start(dir: &Path, config: Config) -> Started {
    let path = daemon::socket_path(Some(dir.as_os_str()), Some(OsStr::new("wayland-1")))
        .expect("a socket path");
    let socket = daemon::bind(&path).expect("the daemon socket");
    let lifetime =
        Arc::new(Lifetime::new(dir.join(CONFIG_FILE), config, WAIT).expect("a daemon lifetime"));
    let daemon = Daemon::new(socket, Arc::clone(&lifetime));
    let loop_thread = thread::spawn(move || daemon.run(None));
    Started {
        path,
        lifetime,
        loop_thread,
    }
}

/// The panel name the reload tests use.
fn notes() -> PanelName {
    PanelName::parse("notes").expect("notes is a panel name")
}

/// Ask the daemon for a `status` reply and return its JSON object: the
/// client's parsed reply does not decode the panel list, so a test that needs
/// the panels reads the line the daemon wrote.
fn status(path: &Path, panel: Option<PanelName>) -> serde_json::Value {
    let mut stream = UnixStream::connect(path).expect("the daemon socket");
    let request = Request::Status { panel };
    ipc::write_line(&mut stream, request.to_line().as_bytes(), WAIT)
        .expect("the status request is sent");
    let line = ipc::read_line(&mut stream, ipc::MAX_LINE, WAIT).expect("the reply line arrives");
    serde_json::from_slice(&line).expect("a reply object")
}

/// `exit_when_idle` true: the last panel's end exits the daemon, which
/// removes its socket file (spec "Exit with the last panel"). A daemon that
/// has not had a panel yet stays for its first one (spec "Daemon lifetime
/// preference").
#[test]
fn the_daemon_exits_with_the_last_panel() {
    let dir = temp_dir("exit-with-last-panel");
    let config = Config::parse(&dir.join(CONFIG_FILE), "[daemon]\nexit_when_idle = true\n")
        .expect("a valid config");
    let started = start(&dir, config);

    // No panel has ended yet, so the idle daemon answers and stays.
    let reply = status(&started.path, None);
    assert_eq!(reply["ok"], true, "the idle daemon answers: {reply}");
    assert_eq!(
        reply["panels"].as_array().expect("panels").len(),
        0,
        "no panel exists yet: {reply}"
    );

    let (name, handle, panel) = answering_panel("notes", 11, defaults_status());
    started
        .lifetime
        .add_panel(name.clone(), handle)
        .expect("notes is free");
    assert!(
        started.lifetime.panel_ended(&name, None),
        "the panel was there"
    );

    started
        .loop_thread
        .join()
        .expect("the loop thread ends")
        .expect("the daemon exits cleanly");
    panel.join().expect("the panel thread ends");
    assert!(
        !started.path.exists(),
        "the exited daemon removed its socket file"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// `exit_when_idle` false: the last panel's end leaves the daemon resident,
/// it answers `status` with no panels, and `quit` ends it (spec "Stay
/// resident").
#[test]
fn the_daemon_stays_resident() {
    let dir = temp_dir("stay-resident");
    let config = Config::parse(&dir.join(CONFIG_FILE), "[daemon]\nexit_when_idle = false\n")
        .expect("a valid config");
    let started = start(&dir, config);

    let (name, handle, panel) = answering_panel("notes", 11, defaults_status());
    started
        .lifetime
        .add_panel(name.clone(), handle)
        .expect("notes is free");
    assert!(
        started.lifetime.panel_ended(&name, None),
        "the panel was there"
    );
    panel.join().expect("the panel thread ends");

    let reply = status(&started.path, None);
    assert_eq!(reply["ok"], true, "the resident daemon answers: {reply}");
    assert_eq!(
        reply["pid"].as_u64(),
        Some(u64::from(std::process::id())),
        "the reply names the daemon: {reply}"
    );
    assert_eq!(
        reply["panels"].as_array().expect("panels").len(),
        0,
        "the last panel ended and the daemon kept running: {reply}"
    );

    let answer = ipc::call(&started.path, &Request::Quit).expect("quit is answered");
    assert!(answer.reply().is_ok(), "the quit reply is a success");
    started
        .loop_thread
        .join()
        .expect("the loop thread ends")
        .expect("the daemon exits cleanly");
    assert!(
        !started.path.exists(),
        "the quit daemon removed its socket file"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// `quit` with two panels gets a success reply, ends both panels and then the
/// daemon, which removes its socket file (spec "Quit with panels").
#[test]
fn quit_ends_every_panel_and_the_daemon() {
    let dir = temp_dir("quit-with-panels");
    let started = start(&dir, Config::default());

    let (first, first_handle, first_panel) = answering_panel("notes", 11, defaults_status());
    let (second, second_handle, second_panel) = answering_panel("chat", 22, defaults_status());
    started
        .lifetime
        .add_panel(first, first_handle)
        .expect("notes is free");
    started
        .lifetime
        .add_panel(second, second_handle)
        .expect("chat is free");

    let answer = ipc::call(&started.path, &Request::Quit).expect("the client gets its reply");
    assert!(answer.reply().is_ok(), "the quit reply is a success");

    started
        .loop_thread
        .join()
        .expect("the loop thread ends")
        .expect("the daemon exits cleanly");
    first_panel.join().expect("the first panel's thread ends");
    second_panel.join().expect("the second panel's thread ends");
    assert!(
        !started.path.exists(),
        "the quit daemon removed its socket file"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A valid reload is a success, existing panels keep their settings, and the
/// next panel and the daemon preferences take the new file (spec "Reload a
/// valid file").
#[test]
fn a_valid_reload_changes_only_new_panels() {
    let dir = temp_dir("reload-valid");
    let config_path = dir.join(CONFIG_FILE);
    fs::write(&config_path, "[panel.defaults]\nsize = \"30%\"\n").expect("the config file");
    let config = Config::load(&config_path).expect("a valid config");
    let started = start(&dir, config);

    let (name, handle, panel) = answering_panel("notes", 11, defaults_status());
    started
        .lifetime
        .add_panel(name.clone(), handle)
        .expect("notes is free");
    let before = status(&started.path, Some(name.clone()));
    assert_eq!(
        before["panels"][0]["size"].as_str(),
        Some("25%"),
        "the stand-in panel reports the built-in size: {before}"
    );

    fs::write(
        &config_path,
        "[daemon]\nexit_when_idle = false\n\n[panel.defaults]\nsize = \"40%\"\n",
    )
    .expect("the changed config file");
    let answer = ipc::call(&started.path, &Request::Reload).expect("the reload is answered");
    assert!(answer.reply().is_ok(), "the valid file reloaded");

    // The existing panel is untouched, while a panel that starts now and the
    // daemon preference take the new file.
    let after = status(&started.path, Some(name.clone()));
    assert_eq!(
        after["panels"][0]["size"], before["panels"][0]["size"],
        "an existing panel keeps its size"
    );
    assert_eq!(
        started
            .lifetime
            .panel_settings(&name, &PanelOptions::default())
            .size(),
        Size::percent(40).expect("40 percent"),
        "a panel that starts now takes the reloaded size"
    );
    assert!(
        !started.lifetime.daemon_settings().exit_when_idle(),
        "the reloaded daemon preference applies"
    );

    let answer = ipc::call(&started.path, &Request::Quit).expect("quit is answered");
    assert!(answer.reply().is_ok(), "the quit reply is a success");
    started
        .loop_thread
        .join()
        .expect("the loop thread ends")
        .expect("the daemon exits cleanly");
    panel.join().expect("the panel thread ends");
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// An invalid reload replies with kind `invalid_config` naming the file and
/// the line, and the daemon keeps the previous config (spec "Reload an
/// invalid file").
#[test]
fn an_invalid_reload_keeps_the_previous_config() {
    let dir = temp_dir("reload-invalid");
    let config_path = dir.join(CONFIG_FILE);
    fs::write(&config_path, "[panel.defaults]\nsize = \"30%\"\n").expect("the config file");
    let config = Config::load(&config_path).expect("a valid config");
    let started = start(&dir, config);

    fs::write(&config_path, "[panel.defaults]\nsize = \"wide\"\n").expect("the broken file");
    let answer = ipc::call(&started.path, &Request::Reload).expect("the reload is answered");
    let error = answer
        .reply()
        .as_error()
        .expect("the invalid file is refused");
    assert_eq!(error.kind(), ErrorKind::InvalidConfig);
    assert!(
        error
            .message()
            .contains(config_path.to_str().expect("the scratch path is UTF-8")),
        "the message names the file: {}",
        error.message()
    );
    assert!(
        error.message().contains(":2:"),
        "the message names the line: {}",
        error.message()
    );

    assert_eq!(
        started
            .lifetime
            .panel_settings(&notes(), &PanelOptions::default())
            .size(),
        Size::percent(30).expect("30 percent"),
        "the daemon kept the previous config"
    );

    let answer = ipc::call(&started.path, &Request::Quit).expect("quit is answered");
    assert!(answer.reply().is_ok(), "the quit reply is a success");
    started
        .loop_thread
        .join()
        .expect("the loop thread ends")
        .expect("the daemon exits cleanly");
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The report's warning names the app and the panel with the spec's
/// wording (spec "Hand-off app is reported", row 7.4).
#[test]
fn the_no_window_report_names_the_app_and_the_panel() {
    assert_eq!(
        no_window_message("myeditor", &notes()),
        "myeditor exited without opening a window in panel notes. It may have handed its \
         window to a running copy, which pinwin cannot dock."
    );
}

/// SIGINT and SIGTERM mean the same as `quit`, and no other signal does
/// (spec "Quit"). The accept loop reads the shutdown trigger rather than a
/// signal, so only the signal-to-trigger mapping is checked here.
#[test]
fn sigint_and_sigterm_mean_quit() {
    assert!(shutdown_signal(libc::SIGINT));
    assert!(shutdown_signal(libc::SIGTERM));
    assert!(!shutdown_signal(libc::SIGHUP));
    assert!(!shutdown_signal(libc::SIGUSR1));
}

/// The signalfd reads a signal the thread blocks and keeps pending, so the
/// test process never takes it (spec "Quit").
#[test]
fn the_signal_fd_reads_one_queued_signal() {
    thread::spawn(|| {
        let signals = SignalFd::open().expect("a signalfd");
        // SAFETY: `pthread_kill` only sends the signal that `open` blocked
        // to this thread, which reads it through the signalfd.
        let sent = unsafe { libc::pthread_kill(libc::pthread_self(), libc::SIGTERM) };
        assert_eq!(sent, 0, "the signal is queued for this thread");
        assert_eq!(
            signals.next().expect("the signalfd read"),
            Some(libc::SIGTERM)
        );
        assert_eq!(signals.next().expect("the signalfd read"), None);
    })
    .join()
    .expect("the signal thread finishes");
}
