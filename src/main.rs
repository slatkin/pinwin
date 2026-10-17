//! The `pinwin` binary.
//!
//! The daemon, its IPC client, the config file and the option parser arrive
//! with the panel-control rows of replace-terminal-with-app-host; until they
//! do, the binary reports the usage error and exits 2, so a half-built
//! checkout cannot pretend to run.

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("usage: pinwin <command> [options]");
    ExitCode::from(2)
}
