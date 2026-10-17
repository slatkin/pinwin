//! The app `pinwin run` replaces itself with (replace-terminal-with-app-host
//! D2, spec "Run an app in a panel"): the program, its arguments and the
//! environment the app keeps, with `WAYLAND_DISPLAY` set to the new panel's
//! socket.
//!
//! Building the spec is a pure step, so its tests check what the app receives
//! without running it (`port-to-rust` D10): the program and the arguments
//! unchanged, `WAYLAND_DISPLAY` set to the panel's display, the working
//! folder and every other variable inherited. Only [`Exec::exec`] touches the
//! process, with `exec`, so the app's process ID is the one `pinwin run`
//! reported to the daemon (D2, D3).

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::Command;

/// The app's name for the daemon's report (row 7.4, D3, spec "Hand-off app
/// is reported"): the file name of the program `pinwin run` executes, so
/// `/usr/bin/notes` reports `notes`. A program whose path has no file name
/// is reported as the caller wrote it.
#[must_use]
pub fn app_name(program: &OsStr) -> String {
    Path::new(program)
        .file_name()
        .unwrap_or(program)
        .to_string_lossy()
        .into_owned()
}

/// The process `pinwin run` becomes (spec "Run an app in a panel"): the app's
/// program, its arguments and the environment it inherits, with
/// `WAYLAND_DISPLAY` set to the panel's display.
#[derive(Debug)]
pub struct Exec {
    program: OsString,
    args: Vec<OsString>,
    display: String,
}

impl Exec {
    /// The exec spec of `command`, whose first element is the program (the
    /// parser never accepts an empty command, spec "No command"). `display`
    /// is the panel's display from the daemon's `create` reply (D2).
    pub fn new(command: &[OsString], display: impl Into<String>) -> io::Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no command to run"))?;
        Ok(Self {
            program: program.clone(),
            args: args.to_vec(),
            display: display.into(),
        })
    }

    /// Replace this process with the app (D2): `exec` keeps the process ID
    /// the daemon's pidfd watches (D3), so the panel follows the app, and the
    /// app inherits the caller's working folder, environment, terminal and
    /// file descriptors. The returned error means `exec` failed, so the
    /// caller still runs and reports it (spec "Command not found").
    pub fn exec(self) -> io::Error {
        use std::os::unix::process::CommandExt as _;
        self.into_command().exec()
    }

    /// The command the app runs as: its program, its arguments and its
    /// environment. A test checks the hand-over through this, so no test
    /// replaces the test process.
    fn into_command(self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        // `Command` inherits the working folder and the environment, so only
        // the panel's display is set here: every other variable stays the
        // caller's (spec "Run an app in a panel").
        command.env("WAYLAND_DISPLAY", &self.display);
        command
    }
}

// The path is explicit because the tests include this file the way
// `src/cli.rs` includes `cli/tests.rs`, and a module loaded that way searches
// its children beside the included file rather than beside `src/run.rs`.
#[cfg(test)]
#[path = "run/tests.rs"]
mod tests;
