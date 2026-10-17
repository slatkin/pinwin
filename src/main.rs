//! The `pinwin` binary.
//!
//! The daemon, its IPC client, the config file and the option parser arrive
//! with the panel-control rows of replace-terminal-with-app-host; until they
//! do, the binary reports the usage error and exits 2, so a half-built
//! checkout cannot pretend to run.

use std::process::ExitCode;

// The geometry core lands before the rows that apply it: the option parser
// (row 4.3) and the reserve surface (row 6.4) are its first users, and row
// 6.4 removes this allow once the binary uses it (D15).
#[allow(
    dead_code,
    reason = "rows 4.3 and 6.4 wire geometry in; row 6.4 removes this"
)]
mod geometry;

fn main() -> ExitCode {
    eprintln!("usage: pinwin <command> [options]");
    ExitCode::from(2)
}
