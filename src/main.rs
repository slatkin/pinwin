//! The `pinwin` program: runs a command (default `$SHELL`) in a layer-shell
//! panel docked to the left edge, until the command exits.
//!
//! ```text
//! pinwin [--] [command...]
//! COLS=60 GUTTER=8 PINWIN_KEYBOARD=on-demand|exclusive|none
//! PINWIN_ACCENT=on|off PINWIN_ACCENT_COLOR=#RRGGBB PINWIN_ACCENT_WIDTH=2
//! PINWIN_NAME=default
//! pinwin htop
//! pinwin --toggle [name]   # asks the named running panel to show or hide
//!                          # itself, for a niri key binding
//! ```
//!
//! A thin host over the library API ([`pinwin::panel`]), the port of
//! `host/main.c` (port-to-rust D8): it owns the pty, the child's environment
//! and the process lifetime; the library owns the panel. The pure parts live
//! in testable modules — argument parsing ([`cli`]), environment parsing
//! ([`settings`]) and the toggle-socket identity ([`pinwin::instance`]); the
//! process parts (`forkpty`, signals, waiting) stay in [`run`] and
//! [`host_panel`].

use std::env;
use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pinwin::layout::{Layout, Side};
use pinwin::panel::{Panel, PinwinError, Startup};

mod cli;
mod settings;

use cli::{Mode, default_command, parse_args};
use pinwin::instance::{
    BindError, InstanceName, SocketFile, ToggleError, bind_instance_socket, serve_toggle_requests,
    socket_path_from_env, toggle_client,
};
use settings::{Settings, Zone, read_settings};

/// The child's process id, read by the signal handler; zero means "no child
/// yet". `signal(2)` handlers may only touch async-signal-safe state, which
/// an atomic store/load is.
static CHILD_PID: AtomicI32 = AtomicI32::new(0);

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
        unsafe { libc::kill(pid, libc::SIGHUP) };
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
        let rc = unsafe { libc::waitpid(pid, &raw mut status, 0) };
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
    unsafe { libc::kill(pid, libc::SIGHUP) };
    wait_for_child(pid);
}

/// The command ready for `execvp`: the argv as `CString`s, with the pointer
/// array built, so everything allocation-based happens in the parent before
/// the fork.
struct ChildCommand {
    argv: Vec<CString>,
    /// `argv`'s pointers plus the terminating null, as `execvp` wants them.
    /// The pointers target the `argv` buffers, which are never moved after
    /// this is built.
    pointers: Vec<*const libc::c_char>,
}

/// Build the child's exec arguments in the parent, before the fork, so the
/// child between `forkpty` and `exec` only calls `signal`/`execvp`/`_exit`.
/// An interior NUL in any argument is reported with the program's name here —
/// the caller exits 127 without any child, matching the status and message
/// the child produced before (`host/main.c`'s exec path).
fn build_child_command(command: &[OsString]) -> Result<ChildCommand, String> {
    let mut argv = Vec::with_capacity(command.len());
    for arg in command {
        let Ok(cstring) = CString::new(arg.as_bytes()) else {
            return Err(format!(
                "pinwin: {}: argument has an interior NUL",
                command[0].to_string_lossy()
            ));
        };
        argv.push(cstring);
    }
    let mut pointers: Vec<*const libc::c_char> = argv.iter().map(|arg| arg.as_ptr()).collect();
    pointers.push(std::ptr::null());
    Ok(ChildCommand { argv, pointers })
}

/// Restore the default signal dispositions the Rust runtime changed, so the
/// exec'd command keeps the default contract `host/main.c`'s child had from
/// its plain C fork: std sets `SIGPIPE` to `SIG_IGN` at startup and an
/// ignored disposition survives `execve`, so it must go back to `SIG_DFL`.
/// The `SIGINT`/`SIGTERM` handlers are installed after the fork and `exec`
/// resets caught dispositions, so they need no reset here.
fn child_signal_setup() {
    // SAFETY: a plain signal number, `SIG_DFL` a valid disposition.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Replace this (child) process with the command, exiting 127 when the exec
/// fails — the child never returns from here. Only async-signal-safe calls
/// happen here: the arguments and pointer array were built in the parent.
fn child_exec(child: &ChildCommand) -> ! {
    child_signal_setup();
    // SAFETY: the pointers are NUL-terminated C strings and the array is
    // null-terminated; on success this call never returns.
    unsafe { libc::execvp(child.argv[0].as_ptr(), child.pointers.as_ptr()) };
    // execvp only returns on failure.
    let error = io::Error::last_os_error();
    eprintln!("pinwin: {}: {error}", child.argv[0].to_string_lossy());
    // SAFETY: `_exit` never runs atexit handlers or unwinds, which matters in
    // this forked child.
    unsafe { libc::_exit(127) }
}

/// The `--toggle` client side (replace-gtk-with-wayland D4): ask the host
/// that owns the name's socket to toggle its panel and report the reply.
/// The client reads no environment besides the display — the request itself
/// carries nothing. `ok` exits 0, a missing or failing host exits 1, and an
/// environment error — an unusable socket path — exits 2 like the other
/// environment errors.
fn run_toggle_client(name: Option<InstanceName>) -> i32 {
    let name = name.unwrap_or_else(InstanceName::default_instance);
    let result = socket_path_from_env(&name)
        .map_err(ToggleError::Environment)
        .and_then(|path| toggle_client(&path));
    match result {
        Ok(()) => 0,
        Err(ToggleError::Environment(message)) => {
            eprintln!("{message}");
            2
        }
        Err(ToggleError::NotAnswered(message)) => {
            eprintln!("{message}");
            1
        }
    }
}

/// The whole program; returns the exit status.
fn run() -> i32 {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let mode = match parse_args(&args) {
        Ok(mode) => mode,
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
    // The toggle client asks the host that owns the name's socket to
    // toggle its panel and reports the reply (replace-gtk-with-wayland
    // D4); it reads no environment besides the display.
    let command = match mode {
        Mode::Toggle { name } => return run_toggle_client(name),
        Mode::Host { command } => command,
    };
    let command = if command.is_empty() {
        default_command()
    } else {
        command
    };
    host_panel(&settings, &command)
}

/// Host the panel over the command's pty (the `Mode::Host` path): bind the
/// instance's focus socket, fork the command onto a new pty, start the panel
/// on the master and serve focus requests until the child exits. Returns the
/// host's exit status.
fn host_panel(settings: &Settings, command: &[OsString]) -> i32 {
    // The focus socket must be ours before any surface opens: a live host
    // with the same name on this display makes this start exit 2 instead
    // (keyboard-focus-request design).
    let (listener, socket_file) = match bind_focus_socket(&settings.name) {
        Ok(listener) => listener,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };

    // The layout the C host built: docked left, top/bottom/left gutters zero,
    // `COLS` columns wide, `GUTTER` on the right. `PINWIN_ZONE=overlay` opts
    // into a covering start, so nothing is ever reserved and a toggle moves
    // no window (replace-gtk-with-wayland D4); the library needs no new code
    // for it, because a covering start already reserves nothing.
    let layout = Layout::new(Side::Left, settings.cols, 0, 0, 0, settings.right);
    let layout = match settings.zone {
        Zone::Reserve => layout,
        Zone::Overlay => layout.covering(),
    };

    // The library sets no child environment: say we are a colour terminal
    // before the fork, so the child inherits it.
    // SAFETY: single-threaded at this point (the fork precedes any thread we
    // or the library spawn), and the names and values contain no NUL.
    unsafe {
        env::set_var("TERM", "xterm-256color");
        env::set_var("COLORTERM", "truecolor");
    };

    // Build the child's exec arguments before the fork, so the child between
    // `forkpty` and `exec` only calls `signal`/`execvp`/`_exit`; an interior
    // NUL is reported here and exits 127 without any child.
    let child = match build_child_command(command) {
        Ok(child) => child,
        Err(message) => {
            eprintln!("{message}");
            return 127;
        }
    };

    // Fork the child onto a new pty; the parent keeps the master fd.
    let mut master: libc::c_int = -1;
    // SAFETY: `master` is writable; the termios and winsize arguments are
    // null, keeping the child's defaults.
    let pid = unsafe {
        libc::forkpty(
            &raw mut master,
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
        child_exec(&child);
    }
    CHILD_PID.store(pid, Ordering::Relaxed);

    // Forward SIGINT/SIGTERM to the child as SIGHUP (installed in the parent
    // only, after the fork, like the C).
    // SAFETY: `on_term` is async-signal-safe and the libc signal handler
    // signature matches.
    unsafe {
        libc::signal(libc::SIGINT, on_term as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
    };

    // Start the panel on the pty master. On failure the child is hung up and
    // reaped and the host exits 1.
    let panel = match Panel::start(Startup::new(
        master.as_raw_fd(),
        layout,
        settings.keyboard,
        settings.accent,
    )) {
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
    let status = serve_toggle_until_exit(&panel, &listener, socket_file, pid);
    // Dropping the handle closes the panel (the C's `pinwin_stop`).
    drop(panel);
    child_exit_status(status)
}

/// Serve toggle requests on the bound socket while the child runs
/// (replace-gtk-with-wayland D4): a scoped thread borrows the panel — the
/// request is bounded like `Panel::apply_layout` — and ends within one
/// accept poll of the shutdown flag set once the child exits. The socket
/// file goes with it: the host removes the file it created. Returns the
/// child's raw wait status.
fn serve_toggle_until_exit(
    panel: &Panel,
    listener: &net::UnixListener,
    socket_file: SocketFile,
    pid: i32,
) -> i32 {
    let shutdown = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            serve_toggle_requests(listener, &|| panel.toggle(), &shutdown);
        });
        let status = wait_for_child(pid);
        shutdown.store(true, Ordering::Relaxed);
        drop(socket_file);
        status
    })
}

/// Bind the focus socket for this instance, before any surface opens: a live
/// host with the same name on this display makes the start exit 2 instead
/// (keyboard-focus-request design). Returns the listener together with the
/// socket file's path, which the host removes after its child exits. Errors
/// carry the full `pinwin:` message.
fn bind_focus_socket(name: &InstanceName) -> Result<(net::UnixListener, SocketFile), String> {
    let socket_path = socket_path_from_env(name)?;
    match bind_instance_socket(&socket_path) {
        Ok(listener) => Ok((listener, SocketFile::new(socket_path))),
        Err(error) => Err(match error {
            BindError::Duplicate => {
                format!(
                    "pinwin: another pinwin already owns {}",
                    socket_path.display()
                )
            }
            BindError::Failed(error) => format!("pinwin: {}: {error}", socket_path.display()),
        }),
    }
}

fn main() {
    std::process::exit(run());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

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

    /// The `--toggle` client maps the outcomes: a name with no listener is
    /// a not-answered request, exit 1. The invalid-name and socket-path
    /// environment classes are covered by the `InstanceName::parse` and
    /// `socket_path` contracts in `instance.rs`; here the no-host path holds.
    /// The name has no listener, and the test never touches the process
    /// environment (the display variables it resolves are inherited, which
    /// only changes the message, not the status).
    #[test]
    fn the_toggle_client_without_a_listener_exits_1() {
        let name = Some(InstanceName::parse("--toggle", "no-such-instance").expect("valid"));
        assert_eq!(run_toggle_client(name), 1);
    }

    /// The exec arguments are the C strings of the command, with the pointer
    /// array terminated, and an interior NUL is reported naming the program.
    #[test]
    fn build_child_command_builds_the_exec_arguments() {
        let os = |slice: &[&str]| -> Vec<OsString> {
            slice
                .iter()
                .map(OsStr::new)
                .map(OsStr::to_os_string)
                .collect()
        };

        let child = build_child_command(&os(&["htop", "-d", "10"])).expect("no NUL");
        assert_eq!(child.argv.len(), 3);
        assert_eq!(child.argv[0].to_bytes(), b"htop");
        assert_eq!(child.argv[2].to_bytes(), b"10");
        // The pointer array mirrors `argv` and ends on a null.
        assert_eq!(child.pointers.len(), 4);
        assert_eq!(child.pointers[0], child.argv[0].as_ptr());
        assert_eq!(child.pointers[2], child.argv[2].as_ptr());
        assert!(child.pointers[3].is_null());

        // An interior NUL anywhere names the program, not the offending
        // argument, like the child's message did.
        let command = os(&["htop"]);
        let mut nul = OsString::from("-d");
        nul.push(OsStr::from_bytes(&[b'a', 0, b'b']));
        let mut command = command;
        command.push(nul);
        let Err(error) = build_child_command(&command) else {
            panic!("expected the interior-NUL error")
        };
        assert_eq!(error, "pinwin: htop: argument has an interior NUL");
    }

    /// The child's signal reset: `signal` reports the previous disposition,
    /// so after the reset `SIGPIPE`'s previous handler is `SIG_DFL` — not the
    /// `SIG_IGN` the Rust runtime installed at startup.
    #[test]
    fn child_signal_setup_restores_the_sigpipe_default() {
        child_signal_setup();
        // SAFETY: a plain signal number; the ignore is restored below so the
        // rest of the test process keeps std's contract.
        let previous = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
        assert_eq!(previous, libc::SIG_DFL);
    }

    /// The reset reaches the exec'd program, through the real `child_exec`
    /// path (`std::process::Command` resets `SIGPIPE` in its own children, so
    /// it cannot show the inheritance). Fork, dup the reporting pipe onto
    /// stdout, then either run [`child_exec`] itself or — the control, as
    /// `child_exec` was before the fix — the same `execve` without the
    /// signal reset, and read the `SigIgn` mask of the exec'd `cat` back
    /// through the pipe: the Rust runtime ignores `SIGPIPE` (bit
    /// `SIGPIPE - 1`), the ignore would survive the exec, and the reset puts
    /// the default back. Linux-only (`/proc`, `SigIgn`), like the panel.
    #[test]
    fn the_execed_child_sees_the_default_sigpipe_disposition() {
        use std::path::Path;

        /// Drain the reporting pipe's read end, retrying an interrupted
        /// read and panicking on any other failure.
        fn drain_read_end(read_end: libc::c_int) -> Vec<u8> {
            let mut output = Vec::new();
            loop {
                let mut chunk = [0u8; 512];
                // SAFETY: `read_end` is the open read end and `chunk` is
                // writable.
                let n = unsafe { libc::read(read_end, chunk.as_mut_ptr().cast(), chunk.len()) };
                if n == 0 {
                    return output;
                }
                if n < 0 {
                    let error = io::Error::last_os_error();
                    match error.raw_os_error() {
                        Some(libc::EINTR) => continue,
                        _ => panic!("read: {error}"),
                    }
                }
                output.extend_from_slice(&chunk[..n.cast_unsigned()]);
            }
        }

        fn sigpipe_ignored(reset: bool) -> bool {
            // `cat` installs no signal handlers, so its `SigIgn` mask is
            // exactly what the exec delivered. Everything is built in the
            // parent; the absolute program path also keeps `execvp` off its
            // PATH search in the forked child.
            let program_path = ["/usr/bin/cat", "/bin/cat"]
                .into_iter()
                .find(|path| Path::new(path).exists())
                .expect("cat at a known absolute path");
            let child_command = build_child_command(&[
                OsString::from(program_path),
                OsString::from("/proc/self/status"),
            ])
            .expect("no NUL");
            let program = CString::new(program_path).expect("no NUL");
            let arg0 = CString::new("cat").expect("no NUL");
            let file = CString::new("/proc/self/status").expect("no NUL");

            let mut fds = [0; 2];
            // SAFETY: `fds` is writable.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
            // SAFETY: fork in a threaded test process; the child between the
            // fork and the exec only calls async-signal-safe functions —
            // close, signal, dup2, exec, _exit — exactly like `child_exec`.
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0, "fork: {}", io::Error::last_os_error());
            if pid == 0 {
                // SAFETY: single-threaded forked child between fork and exec;
                // the read end is this child's to close and stdout is the
                // write end's destination.
                unsafe {
                    libc::close(fds[0]);
                    libc::dup2(fds[1], libc::STDOUT_FILENO)
                };
                if reset {
                    // The real child path, reset and exec included; it
                    // never returns.
                    child_exec(&child_command);
                }
                // The control: the same exec without the reset; it never
                // returns.
                // SAFETY: the C strings are NUL-terminated, the pointer
                // array null-terminated, and `execve` never returns here.
                unsafe {
                    let execve_argv = [arg0.as_ptr(), file.as_ptr(), std::ptr::null()];
                    libc::execve(program.as_ptr(), execve_argv.as_ptr(), std::ptr::null());
                    libc::_exit(127);
                }
            }
            // SAFETY: each end is closed by its own process only.
            unsafe { libc::close(fds[1]) };
            let output = drain_read_end(fds[0]);
            let mut status: i32 = 0;
            // SAFETY: `pid` is this test's child, `status` writable.
            let rc = unsafe { libc::waitpid(pid, &raw mut status, 0) };
            assert_eq!(rc, pid, "waitpid");
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "cat failed: {:?}",
                String::from_utf8_lossy(&output)
            );
            let text = String::from_utf8(output).expect("utf-8");
            let line = text
                .lines()
                .find(|line| line.starts_with("SigIgn:"))
                .expect("a SigIgn line");
            let mask =
                u64::from_str_radix(line.split_whitespace().nth(1).expect("the mask field"), 16)
                    .expect("a hex mask");
            // Signals are numbered from one, so `SIGPIPE` is bit 12.
            mask & (1u64 << (libc::SIGPIPE - 1)) != 0
        }

        // The runtime's ignore survives the exec without the reset.
        assert!(
            sigpipe_ignored(false),
            "SIGPIPE should be ignored without the reset"
        );
        // The child's reset puts the default back before the exec.
        assert!(
            !sigpipe_ignored(true),
            "SIGPIPE should be default after the reset"
        );
    }
}
