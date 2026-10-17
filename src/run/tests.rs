//! The exec spec's hand-over (replace-terminal-with-app-host row 7.1, spec
//! "Run an app in a panel"): the program, its arguments, the panel's
//! `WAYLAND_DISPLAY` and the working folder the app keeps.
//!
//! The tests build the spec and inspect its `Command`, so they never replace
//! the test process with an app.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt as _;

use super::{Exec, app_name};

/// The program and its arguments reach the app unchanged, including one that
/// is not UTF-8 (spec "Run an app in a panel").
#[test]
fn the_program_and_its_arguments_are_handed_over_unchanged() {
    let raw = OsString::from_vec(vec![b'-', b'-', 0xff]);
    let command = [
        OsString::from("/usr/bin/myeditor"),
        OsString::from("~/notes.txt"),
        raw.clone(),
    ];
    let built = Exec::new(&command, "wayland-1")
        .expect("a command")
        .into_command();
    assert_eq!(built.get_program(), OsStr::new("/usr/bin/myeditor"));
    let args: Vec<&OsStr> = built.get_args().collect();
    assert_eq!(args, [OsStr::new("~/notes.txt"), raw.as_os_str()]);
}

/// The app gets the panel's `WAYLAND_DISPLAY`, and its working folder stays
/// the caller's: only the display is set, so every other variable and the
/// directory are inherited (spec "Run an app in a panel").
#[test]
fn the_panel_display_is_set_and_the_working_folder_is_kept() {
    let command = [OsString::from("myeditor")];
    let built = Exec::new(&command, "wayland-1")
        .expect("a command")
        .into_command();
    // `None` means the command inherits the caller's working folder.
    assert_eq!(built.get_current_dir(), None);
    let envs: Vec<(&OsStr, Option<&OsStr>)> = built.get_envs().collect();
    assert_eq!(
        envs,
        [(OsStr::new("WAYLAND_DISPLAY"), Some(OsStr::new("wayland-1")))]
    );
}

/// A command with no program is not an exec spec, so `pinwin run` reports it
/// as its own failure rather than replacing itself (spec "No command").
#[test]
fn an_empty_command_is_not_an_exec_spec() {
    let error = Exec::new(&[], "wayland-1").expect_err("no command");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

/// The report's app name is the program's file name (row 7.4, spec
/// "Hand-off app is reported"): directories are stripped, and a program
/// with no file name is reported as written.
#[test]
fn the_app_name_is_the_program_file_name() {
    assert_eq!(app_name(OsStr::new("/usr/bin/myeditor")), "myeditor");
    assert_eq!(app_name(OsStr::new("myeditor")), "myeditor");
    assert_eq!(app_name(OsStr::new("./my-editor")), "my-editor");
    assert_eq!(app_name(OsStr::new("/")), "/");
}
