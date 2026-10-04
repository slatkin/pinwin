//! The `pinwin` program: runs a command (default `$SHELL`) in a layer-shell
//! panel docked to the left edge, until the command exits.
//!
//! ```text
//! pinwin [--] [command...]
//! COLS=60 GUTTER=8 PINWIN_KEYBOARD=on-demand|exclusive|none
//! PINWIN_ACCENT=on|off PINWIN_ACCENT_COLOR=#RRGGBB PINWIN_ACCENT_WIDTH=2
//! pinwin htop
//! ```
//!
//! A thin host over the library API ([`pinwin::panel`]), the port of
//! `host/main.c` (port-to-rust D8): it owns the pty, the child's environment
//! and the process lifetime; the library owns the panel. The pure parts —
//! argument and environment parsing, the child exit-status mapping — live in
//! testable functions below; the process parts (`forkpty`, signals, waiting)
//! stay in [`run`].

use std::env;
use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::num::NonZeroU16;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicI32, Ordering};

use pinwin::layout::{Accent, Keyboard, Layout, Side};
use pinwin::panel::{Panel, PinwinError, Startup};

/// The default column count and gutter (`host/main.c`'s fallbacks).
const DEFAULT_COLS: i64 = 40;
const DEFAULT_GUTTER: i64 = 0;

/// The default accent colour, matching the niri focus ring so the panel's
/// highlight reads like the tiling one (`host/main.c`'s `accent_color`).
const DEFAULT_ACCENT_RGB: [u8; 3] = [0xda, 0xbc, 0x7f];

/// The shell used when `$SHELL` is unset or empty.
const FALLBACK_SHELL: &str = "/bin/sh";

/// The child's process id, read by the signal handler; zero means "no child
/// yet". `signal(2)` handlers may only touch async-signal-safe state, which
/// an atomic store/load is.
static CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// The host settings parsed from the environment, in the library's argument
/// types (`port-to-rust` D6: the invalid classes are unrepresentable, so the
/// only rejections left are the env strings themselves).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings {
    cols: NonZeroU16,
    /// The right gutter; the C host docks left with `left = 0`.
    right: i32,
    keyboard: Keyboard,
    accent: Option<Accent>,
}

/// One environment variable as a whole number in `min..=max`; unset or empty
/// is the fallback. `get` is the accessor so tests can run without touching
/// the process environment.
fn env_number(
    name: &str,
    raw: Option<&str>,
    fallback: i64,
    min: i64,
    max: i64,
) -> Result<i64, String> {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return Ok(fallback);
    };
    // `strtol`-shaped: an optional sign, then digits, then end of string. A
    // value that does not fit in `i64` cannot be in range either.
    let parsed = raw.parse::<i64>();
    match parsed {
        Ok(value) if (min..=max).contains(&value) => Ok(value),
        _ => Err(format!(
            "pinwin: {name}: expected a number from {min} to {max}, got '{raw}'"
        )),
    }
}

/// `PINWIN_KEYBOARD`: `on-demand` (the default), `exclusive` or `none`.
fn keyboard_mode(raw: Option<&str>) -> Result<Keyboard, String> {
    match raw.filter(|raw| !raw.is_empty()) {
        None => Ok(Keyboard::OnDemand),
        Some("on-demand") => Ok(Keyboard::OnDemand),
        Some("exclusive") => Ok(Keyboard::Exclusive),
        Some("none") => Ok(Keyboard::None),
        Some(raw) => Err(format!(
            "pinwin: PINWIN_KEYBOARD: expected on-demand, exclusive or none, got '{raw}'"
        )),
    }
}

/// `PINWIN_ACCENT`: `on` (the default) or `off`.
fn accent_enabled(raw: Option<&str>) -> Result<bool, String> {
    match raw.filter(|raw| !raw.is_empty()) {
        None => Ok(true),
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        Some(raw) => Err(format!(
            "pinwin: PINWIN_ACCENT: expected on or off, got '{raw}'"
        )),
    }
}

/// `PINWIN_ACCENT_COLOR`: `#RRGGBB` or `RRGGBB`, six hex digits; unset or
/// empty is the default colour.
fn accent_color(raw: Option<&str>) -> Result<[u8; 3], String> {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return Ok(DEFAULT_ACCENT_RGB);
    };
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    let parsed = match (hex.len(), u32::from_str_radix(hex, 16)) {
        (6, Ok(value)) if hex.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            Ok([(value >> 16) as u8, (value >> 8) as u8, value as u8])
        }
        _ => Err(()),
    };
    parsed.map_err(|_| format!("pinwin: PINWIN_ACCENT_COLOR: expected #RRGGBB, got '{raw}'"))
}

/// Read the whole environment contract into [`Settings`]. `get` returns the
/// raw value of a variable (`None` when unset); the errors carry the same
/// messages the C host printed, and the caller exits 2 with them before any
/// surface opens.
fn read_settings(get: impl Fn(&str) -> Option<String>) -> Result<Settings, String> {
    let cols = env_number("COLS", get("COLS").as_deref(), DEFAULT_COLS, 1, 65535)?;
    let right = env_number("GUTTER", get("GUTTER").as_deref(), DEFAULT_GUTTER, 0, 65535)?;
    let keyboard = keyboard_mode(get("PINWIN_KEYBOARD").as_deref())?;
    // `PINWIN_ACCENT_WIDTH` is read only when the accent is on.
    let accent = if accent_enabled(get("PINWIN_ACCENT").as_deref())? {
        let rgb = accent_color(get("PINWIN_ACCENT_COLOR").as_deref())?;
        let width = env_number(
            "PINWIN_ACCENT_WIDTH",
            get("PINWIN_ACCENT_WIDTH").as_deref(),
            1,
            1,
            65535,
        )?;
        Some(Accent::new(
            rgb,
            NonZeroU16::new(width as u16).expect("width in 1..=65535"),
        ))
    } else {
        None
    };
    Ok(Settings {
        cols: NonZeroU16::new(cols as u16).expect("cols in 1..=65535"),
        right: right as i32,
        keyboard,
        accent,
    })
}

/// The command to run, from the host's arguments: `--` ends option parsing
/// and an unknown `--`-prefixed option is rejected; anything else is the
/// command. An empty result means "use the default shell".
fn parse_command(args: &[OsString]) -> Result<Vec<OsString>, String> {
    let mut rest = args;
    if let Some(first) = rest.first() {
        if first.as_bytes() == b"--" {
            rest = &rest[1..];
        } else if first.as_bytes().starts_with(b"--") {
            return Err(format!(
                "pinwin: unknown option '{}'",
                first.to_string_lossy()
            ));
        }
    }
    Ok(rest.to_vec())
}

/// The default command when the host gave none: `$SHELL`, else `/bin/sh`.
fn default_command() -> Vec<OsString> {
    let shell = env::var_os("SHELL").filter(|shell| !shell.is_empty());
    vec![shell.unwrap_or_else(|| OsStr::new(FALLBACK_SHELL).to_os_string())]
}

/// The host's exit status for a finished child: its own exit status, or 1
/// when it did not exit normally (a signal, a stop). Ports `host/main.c`'s
/// `WIFEXITED(status) ? WEXITSTATUS(status) : 1`.
fn child_exit_status(status: i32) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else {
        1
    }
}

/// SIGINT and SIGTERM are forwarded to the child as SIGHUP (D8): the child
/// owns the foreground terminal session, and SIGHUP is what its shell and
/// children expect on a hangup. The handler only loads and kills —
/// async-signal-safe — and `waitpid` happens back in [`run`].
extern "C" fn on_term(_sig: i32) {
    let pid = CHILD_PID.load(Ordering::Relaxed);
    if pid > 0 {
        // SAFETY: `pid` is this process's child, `SIGHUP` a plain signal.
        unsafe {
            libc::kill(pid, libc::SIGHUP);
        }
    }
}

/// Wait for the child to finish, retrying an interrupted wait; returns the
/// raw wait status, or 0 (an exited child with status 0) when `waitpid`
/// failed otherwise — `host/main.c` starts `status` at zero and ignores a
/// failed wait the same way.
fn wait_for_child(pid: i32) -> i32 {
    let mut status: i32 = 0;
    loop {
        // SAFETY: `pid` is this process's child and `status` is writable.
        let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
        if rc == pid {
            return status;
        }
        if rc < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            eprintln!("pinwin: waitpid: {error}");
            return status;
        }
    }
}

/// Hang the child up and wait for it: the failure and shutdown path shared by
/// the start failure (`host/main.c` kills and waits before exiting 1).
fn hang_up_child_and_wait(pid: i32) {
    // SAFETY: `pid` is this process's child, `SIGHUP` a plain signal.
    unsafe {
        libc::kill(pid, libc::SIGHUP);
    }
    wait_for_child(pid);
}

/// Replace this (child) process with the command, exiting 127 when the exec
/// fails — the child never returns from here. Only async-signal-safe calls
/// happen between the fork and the exec.
fn child_exec(command: &[OsString]) -> ! {
    let program = match CString::new(command[0].as_bytes()) {
        Ok(program) => program,
        Err(_) => {
            eprintln!(
                "pinwin: {}: argument has an interior NUL",
                command[0].to_string_lossy()
            );
            // SAFETY: `_exit` never runs atexit handlers or unwinds.
            unsafe { libc::_exit(127) }
        }
    };
    let argv: Vec<CString> = command
        .iter()
        .map(|arg| CString::new(arg.as_bytes()))
        .collect::<Result<_, _>>()
        .unwrap_or_else(|_| {
            eprintln!(
                "pinwin: {}: argument has an interior NUL",
                command[0].to_string_lossy()
            );
            // SAFETY: `_exit` never runs atexit handlers or unwinds.
            unsafe { libc::_exit(127) }
        });
    let mut pointers: Vec<*const libc::c_char> = argv.iter().map(|arg| arg.as_ptr()).collect();
    pointers.push(std::ptr::null());
    // SAFETY: `program` and `pointers` are NUL-terminated C strings and the
    // array is null-terminated; on success this call never returns.
    unsafe {
        libc::execvp(program.as_ptr(), pointers.as_ptr());
    }
    // execvp only returns on failure.
    let error = io::Error::last_os_error();
    eprintln!("pinwin: {}: {error}", command[0].to_string_lossy());
    // SAFETY: `_exit` never runs atexit handlers or unwinds, which matters in
    // this forked child.
    unsafe { libc::_exit(127) }
}

/// The whole program; returns the exit status.
fn run() -> i32 {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let command = match parse_command(&args) {
        Ok(command) if !command.is_empty() => command,
        Ok(_) => default_command(),
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    let settings = match read_settings(|name| {
        env::var_os(name).map(|value| value.to_string_lossy().into_owned())
    }) {
        Ok(settings) => settings,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };

    // The layout the C host built: docked left, top/bottom/left gutters zero,
    // `COLS` columns wide, `GUTTER` on the right.
    let layout = Layout::new(Side::Left, settings.cols, 0, 0, 0, settings.right);

    // The library sets no child environment: say we are a colour terminal
    // before the fork, so the child inherits it.
    // SAFETY: single-threaded at this point (the fork precedes any thread we
    // or the library spawn), and the names and values contain no NUL.
    unsafe {
        env::set_var("TERM", "xterm-256color");
        env::set_var("COLORTERM", "truecolor");
    }

    // Fork the child onto a new pty; the parent keeps the master fd.
    let mut master: libc::c_int = -1;
    // SAFETY: `master` is writable; the termios and winsize arguments are
    // null, keeping the child's defaults.
    let pid = unsafe {
        libc::forkpty(
            &mut master,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if pid < 0 {
        eprintln!("pinwin: forkpty: {}", io::Error::last_os_error());
        return 1;
    }
    if pid == 0 {
        child_exec(&command);
    }
    CHILD_PID.store(pid, Ordering::Relaxed);

    // Forward SIGINT/SIGTERM to the child as SIGHUP (installed in the parent
    // only, after the fork, like the C).
    // SAFETY: `on_term` is async-signal-safe and the libc signal handler
    // signature matches.
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_term as extern "C" fn(i32) as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            on_term as extern "C" fn(i32) as libc::sighandler_t,
        );
    }

    // Start the panel on the pty master. On failure the child is hung up and
    // reaped and the host exits 1.
    let panel = match Panel::start(Startup {
        fd: master.as_raw_fd(),
        layout,
        keyboard: settings.keyboard,
        accent: settings.accent,
    }) {
        Ok(panel) => panel,
        Err(error) => {
            let reason = match error {
                PinwinError::NoDisplay => "no Wayland display or wlr-layer-shell",
                _ => "error",
            };
            eprintln!("pinwin: cannot start the panel ({reason})");
            hang_up_child_and_wait(pid);
            return 1;
        }
    };

    let status = wait_for_child(pid);
    // Dropping the handle closes the panel (the C's `pinwin_stop`).
    drop(panel);
    child_exit_status(status)
}

fn main() {
    std::process::exit(run());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// `env_number` around its fallback, range and error message.
    #[test]
    fn env_number_parses_and_rejects() {
        assert_eq!(env_number("COLS", None, 40, 1, 65535), Ok(40));
        assert_eq!(env_number("COLS", Some(""), 40, 1, 65535), Ok(40));
        assert_eq!(env_number("COLS", Some("60"), 40, 1, 65535), Ok(60));
        assert_eq!(env_number("COLS", Some("+2"), 40, 1, 65535), Ok(2));
        assert_eq!(env_number("COLS", Some("1"), 40, 1, 65535), Ok(1));
        assert_eq!(env_number("COLS", Some("65535"), 40, 1, 65535), Ok(65535));

        for raw in [
            "0",
            "65536",
            "-1",
            "4o",
            "12x",
            " 4",
            "4 ",
            "0x10",
            "99999999999999999999",
        ] {
            let error = env_number("COLS", Some(raw), 40, 1, 65535).expect_err(raw);
            assert_eq!(
                error,
                format!("pinwin: COLS: expected a number from 1 to 65535, got '{raw}'"),
                "raw = {raw:?}"
            );
        }
    }

    /// `keyboard_mode` accepts the three modes with `on-demand` defaulting.
    #[test]
    fn keyboard_mode_parses_the_three_modes() {
        assert_eq!(keyboard_mode(None), Ok(Keyboard::OnDemand));
        assert_eq!(keyboard_mode(Some("")), Ok(Keyboard::OnDemand));
        assert_eq!(keyboard_mode(Some("on-demand")), Ok(Keyboard::OnDemand));
        assert_eq!(keyboard_mode(Some("exclusive")), Ok(Keyboard::Exclusive));
        assert_eq!(keyboard_mode(Some("none")), Ok(Keyboard::None));
        let error = keyboard_mode(Some("sometimes")).expect_err("invalid");
        assert_eq!(
            error,
            "pinwin: PINWIN_KEYBOARD: expected on-demand, exclusive or none, got 'sometimes'"
        );
    }

    /// `accent_enabled` accepts `on` (default) and `off`.
    #[test]
    fn accent_enabled_parses_on_and_off() {
        assert!(accent_enabled(None).expect("unset"));
        assert!(accent_enabled(Some("")).expect("empty"));
        assert!(accent_enabled(Some("on")).expect("on"));
        assert!(!accent_enabled(Some("off")).expect("off"));
        let error = accent_enabled(Some("1")).expect_err("invalid");
        assert_eq!(error, "pinwin: PINWIN_ACCENT: expected on or off, got '1'");
    }

    /// `accent_color` accepts `#RRGGBB` and `RRGGBB` and defaults.
    #[test]
    fn accent_color_parses_hex_triplets() {
        assert_eq!(accent_color(None), Ok(DEFAULT_ACCENT_RGB));
        assert_eq!(accent_color(Some("")), Ok(DEFAULT_ACCENT_RGB));
        assert_eq!(accent_color(Some("#dabc7f")), Ok([0xda, 0xbc, 0x7f]));
        assert_eq!(accent_color(Some("dabc7f")), Ok([0xda, 0xbc, 0x7f]));
        assert_eq!(accent_color(Some("#DABC7F")), Ok([0xda, 0xbc, 0x7f]));
        assert_eq!(accent_color(Some("#000000")), Ok([0, 0, 0]));

        for raw in ["#dabc7", "dabc7f0", "#zdbc7f", "#dabc7f ", "##dabc7", "-1"] {
            let error = accent_color(Some(raw)).expect_err(raw);
            assert_eq!(
                error,
                format!("pinwin: PINWIN_ACCENT_COLOR: expected #RRGGBB, got '{raw}'"),
                "raw = {raw:?}"
            );
        }
    }

    /// `read_settings` composes the variables into the library's types and
    /// reads `PINWIN_ACCENT_WIDTH` only when the accent is on.
    #[test]
    fn read_settings_builds_the_startup_arguments() {
        // A plain helper, so the map borrow stays a plain borrow.
        fn call(map: &HashMap<String, String>) -> Result<Settings, String> {
            read_settings(|name| map.get(name).cloned())
        }

        // All defaults.
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(
            call(&empty),
            Ok(Settings {
                cols: NonZeroU16::new(40).expect("40"),
                right: 0,
                keyboard: Keyboard::OnDemand,
                accent: Some(Accent::new(
                    DEFAULT_ACCENT_RGB,
                    NonZeroU16::new(1).expect("1")
                )),
            })
        );

        // Everything set; an invalid `PINWIN_ACCENT_WIDTH` is still read
        // because the accent is on.
        let map = HashMap::from([
            ("COLS".to_owned(), "120".to_owned()),
            ("GUTTER".to_owned(), "8".to_owned()),
            ("PINWIN_KEYBOARD".to_owned(), "exclusive".to_owned()),
            ("PINWIN_ACCENT_COLOR".to_owned(), "0a0b0c".to_owned()),
            ("PINWIN_ACCENT_WIDTH".to_owned(), "3".to_owned()),
        ]);
        assert_eq!(
            call(&map),
            Ok(Settings {
                cols: NonZeroU16::new(120).expect("120"),
                right: 8,
                keyboard: Keyboard::Exclusive,
                accent: Some(Accent::new(
                    [0x0a, 0x0b, 0x0c],
                    NonZeroU16::new(3).expect("3")
                )),
            })
        );

        // The accent off: no colour or width is read, even invalid ones.
        let map = HashMap::from([
            ("PINWIN_ACCENT".to_owned(), "off".to_owned()),
            ("PINWIN_ACCENT_COLOR".to_owned(), "nope".to_owned()),
            ("PINWIN_ACCENT_WIDTH".to_owned(), "0".to_owned()),
        ]);
        assert_eq!(
            call(&map),
            Ok(Settings {
                cols: NonZeroU16::new(40).expect("40"),
                right: 0,
                keyboard: Keyboard::OnDemand,
                accent: None,
            })
        );

        // A keyboard mode that the type cannot express is still an env error.
        let map = HashMap::from([("PINWIN_KEYBOARD".to_owned(), "sometimes".to_owned())]);
        assert!(call(&map).is_err());
    }

    /// `parse_command` mirrors `host/main.c`'s argv handling.
    #[test]
    fn parse_command_handles_the_option_separator() {
        let os = |slice: &[&str]| -> Vec<OsString> {
            slice
                .iter()
                .map(OsStr::new)
                .map(OsStr::to_os_string)
                .collect()
        };

        // No arguments, and `--` alone: the default shell.
        assert_eq!(parse_command(&os(&[])), Ok(vec![]));
        assert_eq!(parse_command(&os(&["--"])), Ok(vec![]));

        // A command passes through; `--` ends option parsing.
        assert_eq!(parse_command(&os(&["htop"])), Ok(os(&["htop"])));
        assert_eq!(
            parse_command(&os(&["--", "htop", "-x"])),
            Ok(os(&["htop", "-x"]))
        );

        // A `--`-prefixed unknown option is rejected, `--` after the first
        // argument is just a command argument.
        assert_eq!(
            parse_command(&os(&["--frobnicate"])),
            Err("pinwin: unknown option '--frobnicate'".to_owned())
        );
        assert_eq!(
            parse_command(&os(&["htop", "--help"])),
            Ok(os(&["htop", "--help"]))
        );
    }

    /// The default command is `$SHELL`, else `/bin/sh`.
    #[test]
    fn default_command_prefers_the_shell_variable() {
        // SAFETY: single-threaded test process; the variable is restored
        // before the test ends so the other tests see the real environment.
        unsafe {
            env::set_var("SHELL", "/bin/zsh");
        }
        assert_eq!(
            default_command(),
            vec![OsStr::new("/bin/zsh").to_os_string()]
        );
        unsafe {
            env::set_var("SHELL", "");
        }
        assert_eq!(
            default_command(),
            vec![OsStr::new(FALLBACK_SHELL).to_os_string()]
        );
        unsafe {
            env::remove_var("SHELL");
        }
        assert_eq!(
            default_command(),
            vec![OsStr::new(FALLBACK_SHELL).to_os_string()]
        );
    }

    /// The exit-status mapping: the child's status when it exited, 1 when it
    /// did not (killed by a signal). The raw statuses are built the way the
    /// kernel lays them out: exit code in the high byte.
    #[test]
    fn child_exit_status_maps_the_wait_status() {
        assert_eq!(child_exit_status(7 << 8), 7);
        assert_eq!(child_exit_status(0 << 8), 0);
        assert_eq!(child_exit_status(255 << 8), 255);
        // Killed by SIGHUP (signal 1), core dump bit unset.
        assert_eq!(child_exit_status(1), 1);
        // Killed by SIGKILL (signal 9).
        assert_eq!(child_exit_status(9), 1);
    }
}
