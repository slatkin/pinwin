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

use std::ffi::OsString;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use crate::cli::{ClientCommand, Command};
use crate::config::Config;
use crate::daemon::DaemonSocket;
use crate::daemon::SocketError;
use crate::daemon::lifetime::{Daemon, Lifetime, SignalFd};
use crate::ipc::{Answer, CallError, ErrorKind, Request, Subscription};
use crate::run::Exec;

pub mod cli;
pub mod config;
pub mod daemon;

// The geometry core is applied by the proxy from row 6.4 (D9): the apply
// path of row 6.6 (`src/proxy/apply.rs`) reads `Margins::new` and the
// `Geometry::across` and `Geometry::gutters` accessors, and the width
// tween (row 10.1) runs `HeldGap::reserve`, `HeldGap::tweens_with_panel` and
// `HeldGap::edge`. Row 12.x makes the module private again once every part
// has a caller in the binary (D15); until then it stays public at this crate
// root so no waiting part is reported dead, and nothing outside the binary
// can name them.
pub mod geometry;

// The parser, the config file and the daemon are complete before the rows
// that call every part of them (D15), so their modules are public at this
// crate root: a private module would report the parts rows 6.x to 12.x use as
// dead code. Nothing outside the binary can name them, and the test
// harnesses include the same files as public modules for the same reason.
pub mod ipc;

// The proxy's panic guard (port-to-rust D5) is compiled now that the panel
// thread uses `guard`; `guard_always` and `guard_default` wait for the rows
// that register the trampolines they protect (the event callbacks of rows
// 8.x and the ring of row 9.1), so the module stays public at this crate
// root until those callers land and nothing outside the binary can name it.
pub mod guard;

// The panel proxy lands before the rows that wire it in: row 7.1 calls
// `proxy::spawn` from the `create` request, and that row makes this module
// private once every part of it has a caller in the binary (D15). Until then
// a private module would report the API that row calls as dead code.
pub mod proxy;

mod name;

// The app `pinwin run` becomes (row 7.1, D2): the exec spec's program,
// arguments and environment are pure, so their test needs no display.
mod run;

/// The usage line a refused command line ends with.
const USAGE: &str = "usage: pinwin <command> [options]";

/// The exit code of an invalid config file (spec "Config file": the daemon
/// prints a message that names the file and line, and exits 2).
const INVALID_CONFIG: u8 = 2;

/// The exit code of a `pinwin run` whose command cannot be executed (spec
/// "Command not found": pinwin prints a message, exits 127, and the panel
/// ends).
const COMMAND_NOT_FOUND: u8 = 127;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match cli::parse_args(&args) {
        Ok(Command::Run(run)) => run_app(&run),
        Ok(Command::Daemon) => daemon(),
        Ok(Command::Client(command)) => client(command),
        Err(error) => {
            eprintln!("{error}");
            eprintln!("{USAGE}");
            ExitCode::from(error.exit_code())
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
    let path = match daemon::socket_path_from_env() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    };
    let request = Request::Create {
        panel: run.name().clone(),
        pid: std::process::id(),
        app: run::app_name(&run.command()[0]),
        options: run.options().clone(),
    };
    let answer = match create_panel(&path, &request) {
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
    let exec = match Exec::new(run.command(), display) {
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
/// a message and exit 1 instead of hanging.
fn create_panel(path: &Path, request: &Request) -> Result<Answer, String> {
    match ipc::call(path, request) {
        Ok(answer) => Ok(answer),
        Err(CallError::Connect { .. }) => {
            daemon::spawn::start_and_wait(path).map_err(|error| error.to_string())?;
            ipc::call(path, request).map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Run the daemon in the foreground (spec "One daemon for each Wayland
/// display", D12). The config file is read first, so an invalid file exits 2
/// before the daemon takes the display's socket and creates anything (spec
/// "Invalid config file").
fn daemon() -> ExitCode {
    let Some(config_path) = config::config_path_from_env() else {
        eprintln!("pinwin: XDG_CONFIG_HOME and HOME are unset: no config file path");
        return ExitCode::FAILURE;
    };
    let config = match Config::load(&config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::from(INVALID_CONFIG);
        }
    };
    // D12: the compositor must offer layer-shell before the daemon takes the
    // display's socket, so a compositor that lacks it leaves no daemon and no
    // panel behind (spec "Wayland layer-shell is required", "No
    // layer-shell").
    if let Err(error) = crate::proxy::check::check_layer_shell() {
        eprintln!("pinwin: {error}");
        return ExitCode::FAILURE;
    }
    let socket = match DaemonSocket::bind_from_env() {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("pinwin: {error}");
            // A live daemon keeps its display: the second start reports it
            // and exits 0 without changing it (spec "Second daemon"). Every
            // other bind failure (no runtime directory, an unusable display,
            // an I/O error) exits 1 and creates nothing (spec "No runtime
            // directory").
            return if matches!(error, SocketError::Live) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };
    // D12: SIGINT and SIGTERM are blocked and read through one signalfd on
    // the thread that runs the accept loop, before it starts a connection or
    // panel thread, so those threads inherit the mask and no signal handler
    // races them.
    let signals = match SignalFd::open() {
        Ok(signals) => signals,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    };
    let lifetime = match Lifetime::new(config_path, config, ipc::REPLY_TIMEOUT) {
        Ok(lifetime) => lifetime,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The socket file is removed by the socket's drop when the loop returns
    // (spec "Daemon socket").
    match Daemon::new(socket, Arc::new(lifetime)).run(Some(&signals)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pinwin: the daemon ended: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Send one client subcommand to the daemon and print its answer (spec
/// "Command-line client", "Command-line exit codes").
fn client(command: ClientCommand) -> ExitCode {
    if matches!(command, ClientCommand::Subscribe) {
        return subscribe();
    }
    // `pinwin status` prints the reply's JSON; the other successes print
    // nothing (spec "Command-line exit codes").
    let status = matches!(command, ClientCommand::Status { .. });
    let request = Request::from(command);
    let path = match daemon::socket_path_from_env() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    };
    let answer = match ipc::call(&path, &request) {
        Ok(answer) => answer,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(error) = answer.reply().as_error() {
        // The daemon's own message, so a script sees why the request failed
        // (spec "Unknown panel").
        eprintln!("pinwin: {}", error.message());
        return ExitCode::FAILURE;
    }
    if status && let Err(error) = write_stdout(answer.line()) {
        eprintln!("pinwin: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Print each line of the daemon's event stream until the daemon closes it
/// or the reader goes away (spec "Event stream", "Command-line client").
fn subscribe() -> ExitCode {
    let path = match daemon::socket_path_from_env() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut subscription = match Subscription::connect(&path) {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    };
    match subscription.reply() {
        Ok(reply) if reply.is_ok() => {}
        Ok(reply) => {
            if let Some(error) = reply.as_error() {
                eprintln!("pinwin: {}", error.message());
            }
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("pinwin: {error}");
            return ExitCode::FAILURE;
        }
    }
    loop {
        match subscription.next_event() {
            Ok(Some(line)) => {
                // A closed pipe (`pinwin subscribe | head -1`) ends the
                // stream: nobody is left to print the events for.
                if write_stdout(&line).is_err() {
                    return ExitCode::SUCCESS;
                }
            }
            Ok(None) => return ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("pinwin: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}

/// Write one line to stdout and flush it, so a reply or an event reaches the
/// caller as it arrives.
fn write_stdout(line: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(line)?;
    stdout.flush()
}
