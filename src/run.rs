//! The app `pinwin run` replaces itself with (replace-terminal-with-app-host
//! D2, spec "Run an app in a panel"): the program, its arguments and the
//! environment the app keeps, with `WAYLAND_DISPLAY`, `PINWIN_PANEL` and
//! `PINWIN_SOCKET` set for the new panel.
//!
//! Building the spec is a pure step, so its tests check what the app receives
//! without running it (`port-to-rust` D10): the program and the arguments
//! unchanged, the panel's three variables set, the working folder and every
//! other variable inherited. Only [`Exec::exec`] touches the process, with
//! `exec`, so the app's process ID is the one `pinwin run` reported to the
//! daemon (D2, D3).

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

/// The process `pinwin run` becomes (spec "Run an app in a panel"): the
/// command that hands the app its program, its arguments and the environment
/// it inherits, with `WAYLAND_DISPLAY`, `PINWIN_PANEL` and `PINWIN_SOCKET`
/// set for the panel.
#[derive(Debug)]
pub struct Exec {
    command: Command,
}

impl Exec {
    /// The exec spec of `command`, whose first element is the program (the
    /// parser never accepts an empty command, spec "No command"). `display`
    /// is the panel's display from the daemon's `create` reply, `panel` its
    /// name and `socket` the absolute daemon socket path (D2). `Command::env`
    /// replaces inherited values, so a nested `pinwin run` gives its app its
    /// own panel's values (spec "Run an app in a panel": "Run from inside a
    /// panel").
    pub fn new(
        command: &[OsString],
        display: impl Into<String>,
        panel: &str,
        socket: &Path,
    ) -> io::Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no command to run"))?;
        let mut built = Command::new(program);
        built.args(args);
        // `Command` inherits the working folder and the environment, so only
        // the panel's three variables are set here: every other variable
        // stays the caller's (spec "Run an app in a panel").
        built.env("WAYLAND_DISPLAY", display.into());
        built.env("PINWIN_PANEL", panel);
        built.env("PINWIN_SOCKET", socket);
        Ok(Self { command: built })
    }

    /// Replace this process with the app (D2): `exec` keeps the process ID
    /// the daemon's pidfd watches (D3), so the panel follows the app, and the
    /// app inherits the caller's working folder, environment, terminal and
    /// file descriptors. The returned error means `exec` failed, so the
    /// caller still runs and reports it (spec "Command not found").
    pub fn exec(mut self) -> io::Error {
        use std::os::unix::process::CommandExt as _;
        self.command.exec()
    }
}

#[cfg(test)]
mod tests;
