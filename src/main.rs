//! The `pinwin` binary (replace-terminal-with-app-host D2, D12, D15): the
//! subcommand dispatch, the daemon in the foreground and the client
//! subcommands that send one request each (spec "One daemon for each Wayland
//! display", "Command-line client", "Command-line exit codes").
//!
//! `pinwin daemon` binds the display's socket and runs the accept loop until
//! a `quit` request or SIGINT/SIGTERM ends it (D12). The client subcommands
//! build their [`ipc::Request`], wait for the daemon's reply and print it:
//! a success exits 0, a daemon error or no daemon exits 1, and a command line
//! the parser refuses exits 2 (spec "Command-line exit codes"). `subscribe`
//! stays connected and prints each event line (spec "Command-line client").
//!
//! `pinwin run` asks the daemon for a new panel, sets the display the reply
//! names and replaces itself with the app (D2). When no daemon answers, it
//! starts one in the background and waits a bounded time for it (row 7.2,
//! spec "Run starts the daemon"); row 7.3 adds the panel lifetime from the
//! app's pidfd and its connection count.
//!
//! The client's socket path comes from `PINWIN_SOCKET` when it is set, else
//! from the display (D1). A `pinwin run` whose path came from the variable
//! never starts a daemon there (D3).

use std::ffi::OsString;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use crate::cli::Command;
use crate::config::Config;
use crate::daemon::lifetime::{Daemon, Lifetime, SignalFd};
use crate::daemon::{DaemonSocket, SocketError, Source};
use crate::guard::{Poisoned, guard};
use crate::ipc::{Answer, CallError, ErrorKind, Request, Subscription};
use crate::run::Exec;

mod cli;
mod config;
mod daemon;
mod fd;
mod geometry;
mod guard;
mod ipc;
mod name;
mod proxy;
mod run;
mod tray;

/// The usage line a refused command line ends with.
const USAGE: &str = "usage: pinwin <command> [options]";

/// The exit code of an invalid config file (spec "Config file": the daemon
/// prints a message that names the file and line, and exits 2).
const INVALID_CONFIG: u8 = 2;

/// The exit code of a `pinwin run` whose command cannot be executed (spec
/// "Command not found": pinwin prints a message, exits 127, and the panel
/// ends).
const COMMAND_NOT_FOUND: u8 = 127;

/// The exit code of a daemon that cannot start (spec "Command-line exit
/// codes": a daemon error exits 1).
const DAEMON_FAILURE: u8 = 1;

/// Why `pinwin daemon` did not start (spec "Run starts the daemon"): the
/// message the daemon prints and reports to a `pinwin run` that started it,
/// and the exit code the caller uses.
struct Failure {
    code: u8,
    message: String,
    /// Another daemon already serves the display, so a `pinwin run` that
    /// started this one must use the winner (spec "Two first runs").
    lost_race: bool,
}

impl Failure {
    /// A start that reports `message` and exits with `code`.
    fn fatal(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            lost_race: false,
        }
    }

    /// A start that lost the bind race: the caller reports readiness and
    /// exits 0 without changing the winner (spec "Two first runs").
    fn lost_race(message: impl Into<String>) -> Self {
        Self {
            code: 0,
            message: message.into(),
            lost_race: true,
        }
    }
}

fn main() -> ExitCode {
    let poisoned = Poisoned::new();
    let Ok(code) = guard(&poisoned, dispatch) else {
        eprintln!("pinwin: a panic stopped the command");
        return ExitCode::FAILURE;
    };
    code
}

/// Parse the command line and run the named command. The whole body runs
/// under [`main`]'s D5 guard, so a panic in a subcommand reports an internal
/// failure instead of unwinding out of the process.
fn dispatch() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match cli::parse_args(&args) {
        Ok(Command::Run(run)) => run_app(&run),
        Ok(Command::Daemon) => daemon(),
        Ok(Command::Client(request)) => client(&request),
        Err(error) => {
            eprintln!("{error}");
            eprintln!("{USAGE}");
            ExitCode::from(cli::UsageError::EXIT_CODE)
        }
    }
}

/// `pinwin run` (spec "Run an app in a panel", D2): ask the daemon for a new
/// panel, then replace this process with the app on the display the reply
/// names. No daemon exits 1 like any other client; a name another panel uses
/// exits 2 without running the command (spec "Panel names"); a command that
/// cannot be executed prints the error and exits 127 (spec "Command not
/// found").
fn run_app(run: &cli::Run) -> ExitCode {
    let (path, source) = match daemon::client_socket_path() {
        Ok((path, source)) => (path, source),
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The app's `PINWIN_SOCKET` must work from any working folder, so a
    // relative runtime directory becomes an absolute path here (D2).
    let path = match absolute_socket_path(path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("pinwin: could not resolve the daemon socket path: {error}");
            return ExitCode::FAILURE;
        }
    };
    let request = Request::Create {
        panel: run.name().clone(),
        pid: std::process::id(),
        app: run::app_name(&run.command()[0]),
        options: run.options().clone(),
    };
    let answer = match create_panel(&path, source, &request) {
        Ok(answer) => answer,
        Err(message) => {
            eprintln!("pinwin: {message}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(error) = answer.reply().as_error() {
        // A duplicate name is the caller's to fix, as an invalid name is, so
        // it exits 2 like the parser's own refusal (spec "Panel names").
        eprintln!("pinwin: {}", error.message());
        return ExitCode::from(match error.kind() {
            ErrorKind::DuplicateName => cli::UsageError::EXIT_CODE,
            _ => 1,
        });
    }
    let Some(display) = answer.reply().display() else {
        eprintln!("pinwin: the daemon's reply named no display for the new panel");
        return ExitCode::FAILURE;
    };
    let exec = match Exec::new(run.command(), display, run.name().as_str(), &path) {
        Ok(exec) => exec,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::from(COMMAND_NOT_FOUND);
        }
    };
    let program = run.command()[0].to_string_lossy();
    let error = exec.exec();
    eprintln!("pinwin: could not run '{program}': {error}");
    ExitCode::from(COMMAND_NOT_FOUND)
}

/// Ask the daemon for a new panel, starting one in the background when none
/// answers (spec "Run starts the daemon", D12). The wait for the started
/// daemon is bounded, so a daemon that cannot start makes `pinwin run` print
/// a message and exit 1 instead of hanging. A path from `PINWIN_SOCKET`
/// starts no daemon (D3): the variable names a daemon that should already
/// own the panel's display, so starting another would be the wrong one.
fn create_panel(path: &Path, source: Source, request: &Request) -> Result<Answer, String> {
    match ipc::call(path, request) {
        Ok(answer) => Ok(answer),
        Err(CallError::Connect { .. }) => {
            if source == Source::Variable {
                return Err(format!(
                    "no pinwin daemon answers at the PINWIN_SOCKET path {}",
                    path.display()
                ));
            }
            daemon::spawn::start_and_wait(path).map_err(|error| error.to_string())?;
            ipc::call(path, request).map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// The socket path the app receives in `PINWIN_SOCKET` (D2): absolute, so
/// the app may change its working folder before it runs another `pinwin`.
fn absolute_socket_path(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir().map(|folder| folder.join(path))
}

/// Run the daemon in the foreground (spec "One daemon for each Wayland
/// display", D12). The readiness pipe a `pinwin run` gave this daemon is
/// adopted before any work can fail, so every start result reaches that
/// caller; a manual daemon has no pipe and behaves as before (spec "Run
/// starts the daemon").
fn daemon() -> ExitCode {
    let ready = daemon::ready::ReadyPipe::from_env(
        std::env::var_os(daemon::ready::READY_FD_ENV).as_deref(),
    );
    match start() {
        Ok((daemon, signals)) => {
            // Readiness comes after the lifetime and its tray, just before
            // the accept loop: the socket's listen backlog already holds a
            // caller's connection, and a failure in the signalfd, the
            // lifetime or the tray still reports as a start failure (D12).
            if let Some(pipe) = ready {
                pipe.report_ready();
            }
            // The socket file is removed by the socket's drop when the loop
            // returns (spec "Daemon socket").
            match daemon.run(Some(&signals)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("pinwin: the daemon ended: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(failure) => {
            eprintln!("pinwin: {}", failure.message);
            if let Some(pipe) = ready {
                if failure.lost_race {
                    pipe.report_ready();
                } else {
                    pipe.report_failure(&failure.message);
                }
            }
            ExitCode::from(failure.code)
        }
    }
}

/// Set the daemon up and return it with its signalfd, or the reason it
/// cannot start (spec "Run starts the daemon"). The config file is read
/// first, so an invalid file exits 2 before the daemon takes the display's
/// socket and creates anything (spec "Invalid config file").
fn start() -> Result<(Daemon, SignalFd), Failure> {
    let config_path = config::config_path_from_env().ok_or_else(|| {
        Failure::fatal(
            DAEMON_FAILURE,
            "XDG_CONFIG_HOME and HOME are unset: no config file path",
        )
    })?;
    let config = Config::load(&config_path)
        .map_err(|error| Failure::fatal(INVALID_CONFIG, error.to_string()))?;
    // D12: the compositor must offer layer-shell before the daemon takes the
    // display's socket, so a compositor that lacks it leaves no daemon and no
    // panel behind (spec "Wayland layer-shell is required", "No
    // layer-shell").
    crate::proxy::check::check_layer_shell()
        .map_err(|error| Failure::fatal(DAEMON_FAILURE, error.to_string()))?;
    // A live daemon keeps its display: the second start reports it and exits
    // 0 without changing it (spec "Second daemon"). Every other bind failure
    // (no runtime directory, an unusable display, an I/O error) exits 1 and
    // creates nothing (spec "No runtime directory").
    let socket = DaemonSocket::bind_from_env().map_err(|error| match error {
        SocketError::Live => Failure::lost_race(error.to_string()),
        _ => Failure::fatal(DAEMON_FAILURE, error.to_string()),
    })?;
    // D12: SIGINT and SIGTERM are blocked and read through one signalfd on
    // the thread that runs the accept loop, before it starts a connection or
    // panel thread, so those threads inherit the mask and no signal handler
    // races them.
    let signals =
        SignalFd::open().map_err(|error| Failure::fatal(DAEMON_FAILURE, error.to_string()))?;
    let lifetime = Lifetime::new(config_path, config, ipc::REPLY_TIMEOUT)
        .map_err(|error| Failure::fatal(DAEMON_FAILURE, error.to_string()))?;
    // The tray icon shows only while panels exist (row 11.1, D14), so the
    // daemon enables it before any panel can start.
    let lifetime = Arc::new(lifetime);
    lifetime.enable_tray();
    Ok((Daemon::new(socket, lifetime), signals))
}

/// Send one client subcommand to the daemon and print its answer (spec
/// "Command-line client", "Command-line exit codes").
fn client(request: &Request) -> ExitCode {
    if matches!(request, Request::Subscribe) {
        return finish(subscribe());
    }
    // `pinwin status` prints the reply's JSON; the other successes print
    // nothing (spec "Command-line exit codes").
    let status = matches!(request, Request::Status { .. });
    finish(send(request, status))
}

/// The exit code of a command's result: a failure prints `pinwin: ` and its
/// message to stderr.
fn finish(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("pinwin: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Send `request` and, for `status`, print the reply.
fn send(request: &Request, status: bool) -> Result<(), String> {
    let (path, _source) = daemon::client_socket_path().map_err(|error| error.to_string())?;
    let answer = ipc::call(&path, request).map_err(|error| error.to_string())?;
    if let Some(error) = answer.reply().as_error() {
        // The daemon's own message, so a script sees why the request failed
        // (spec "Unknown panel").
        return Err(error.message().to_string());
    }
    if status {
        write_stdout(answer.line()).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Print each line of the daemon's event stream until the daemon closes it
/// or the reader goes away (spec "Event stream", "Command-line client").
fn subscribe() -> Result<(), String> {
    let (path, _source) = daemon::client_socket_path().map_err(|error| error.to_string())?;
    let mut subscription = Subscription::connect(&path).map_err(|error| error.to_string())?;
    let reply = subscription.reply().map_err(|error| error.to_string())?;
    if let Some(error) = reply.as_error() {
        return Err(error.message().to_string());
    }
    if !reply.is_ok() {
        return Err("the daemon refused the subscription".to_owned());
    }
    while let Some(line) = subscription
        .next_event()
        .map_err(|error| error.to_string())?
    {
        // A closed pipe (`pinwin subscribe | head -1`) ends the stream:
        // nobody is left to print the events for.
        if write_stdout(&line).is_err() {
            break;
        }
    }
    Ok(())
}

/// Write one line to stdout and flush it, so a reply or an event reaches the
/// caller as it arrives.
fn write_stdout(line: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(line)?;
    stdout.flush()
}
