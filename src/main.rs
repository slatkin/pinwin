//! The `pinwin` program: runs a command (default `$SHELL`) in a layer-shell
//! panel docked to the left edge, until the command exits.
//!
//! ```text
//! pinwin [--] [command...]
//! COLS=60 GUTTER=8 PINWIN_KEYBOARD=on-demand|exclusive|none
//! PINWIN_ACCENT=on|off PINWIN_ACCENT_COLOR=#RRGGBB PINWIN_ACCENT_WIDTH=2
//! PINWIN_NAME=default
//! pinwin htop
//! pinwin --focus [name]   # asks for keyboard focus with the activation
//!                         # token in $XDG_ACTIVATION_TOKEN, which the
//!                         # compositor sets when a key binding spawns it;
//!                         # without it, exit 2
//! ```
//!
// TEMPORARY (replace-gtk-with-wayland 8.1)
//! Until the GTK libraries are unlinked (rows 8.1 and 8.2), the program
//! cannot see the compositor's token: the GTK libraries consume
//! `XDG_ACTIVATION_TOKEN` before `main` runs, so `--focus` exits 2.
//!
//! A thin host over the library API ([`pinwin::panel`]), the port of
//! `host/main.c` (port-to-rust D8): it owns the pty, the child's environment
//! and the process lifetime; the library owns the panel. The pure parts live
//! in testable modules — argument parsing ([`cli`]), environment parsing
//! ([`settings`]) and the focus-socket identity ([`ipc`]); the process parts
//! (`forkpty`, signals, waiting) stay in [`run`].

use std::env;
use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pinwin::activation::ActivationToken;
use pinwin::layout::{Layout, Side};
use pinwin::panel::{Panel, PinwinError, Startup};

mod cli;
mod ipc;
mod settings;

use cli::{Mode, default_command, parse_args};
use ipc::{BindError, FocusError, InstanceName};
use settings::read_settings;

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
    unsafe {
        libc::execvp(child.argv[0].as_ptr(), child.pointers.as_ptr());
    }
    // execvp only returns on failure.
    let error = io::Error::last_os_error();
    eprintln!("pinwin: {}: {error}", child.argv[0].to_string_lossy());
    // SAFETY: `_exit` never runs atexit handlers or unwinds, which matters in
    // this forked child.
    unsafe { libc::_exit(127) }
}

/// The `--focus` client side (keyboard-focus-request row 3.5, the token
/// protocol of replace-gtk-with-wayland D4): build the activation token
/// from `get` — `XDG_ACTIVATION_TOKEN` in [`run`] — and only then ask the
/// host that owns the name's socket for focus and report the reply. A
/// missing or invalid token exits 2 without contacting any instance; `ok`
/// exits 0, a missing or failing host exits 1, and an environment error —
/// an unusable socket path — exits 2 like the other environment errors.
fn run_focus_client(name: Option<InstanceName>, get: impl Fn(&str) -> Option<String>) -> i32 {
    // The token comes first: without one there is nothing to ask for, and
    // no instance is contacted (the spec's "No token" scenario).
    let token = match settings::read_focus_token(get) {
        Ok(token) => token,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    let name = name.unwrap_or_else(InstanceName::default_instance);
    let result = ipc::socket_path_from_env(&name)
        .map_err(FocusError::Environment)
        .and_then(|path| ipc::focus_client(&path, &token));
    match result {
        Ok(()) => 0,
        Err(FocusError::Environment(message)) => {
            eprintln!("{message}");
            2
        }
        Err(FocusError::NotAnswered(message)) => {
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
    // The focus client builds its token from the environment before it
    // contacts any instance (replace-gtk-with-wayland D4), then asks the
    // host that owns the name's socket for focus and reports the reply
    // (keyboard-focus-request row 3.5).
    let command = match mode {
        Mode::Focus { name } => {
            return run_focus_client(name, |name| {
                env::var_os(name).map(|value| value.to_string_lossy().into_owned())
            });
        }
        Mode::Host { command } => command,
    };
    let command = if command.is_empty() {
        default_command()
    } else {
        command
    };

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

    // Build the child's exec arguments before the fork, so the child between
    // `forkpty` and `exec` only calls `signal`/`execvp`/`_exit`; an interior
    // NUL is reported here and exits 127 without any child.
    let child = match build_child_command(&command) {
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
        child_exec(&child);
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

    // The listener answers focus requests while the child runs (row 3.4):
    // a scoped thread borrows the panel — the request is bounded like
    // `Panel::apply_layout` — and ends within one accept
    // poll of the shutdown flag the main thread sets once the child exits.
    // The socket file goes with it: the host removes the file it created.
    // The request line carries the client's activation token, which goes
    // to the panel's xdg-activation request (replace-gtk-with-wayland D4).
    let shutdown = AtomicBool::new(false);
    let status = std::thread::scope(|scope| {
        scope.spawn(|| {
            ipc::serve_focus_requests(
                &listener,
                &|token: &ActivationToken| panel.request_focus(token.clone()),
                &shutdown,
            );
        });
        let status = wait_for_child(pid);
        shutdown.store(true, Ordering::Relaxed);
        drop(socket_file);
        status
    });
    // Dropping the handle closes the panel (the C's `pinwin_stop`).
    drop(panel);
    child_exit_status(status)
}

/// Bind the focus socket for this instance, before any surface opens: a live
/// host with the same name on this display makes the start exit 2 instead
/// (keyboard-focus-request design). Returns the listener together with the
/// socket file's path, which the host removes after its child exits. Errors
/// carry the full `pinwin:` message.
fn bind_focus_socket(name: &InstanceName) -> Result<(net::UnixListener, ipc::SocketFile), String> {
    let socket_path = ipc::socket_path_from_env(name)?;
    match ipc::bind_instance_socket(&socket_path) {
        Ok(listener) => Ok((listener, ipc::SocketFile::new(socket_path))),
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

    /// The `--focus` client reads the token before it contacts any
    /// instance: a missing token exits 2 even when no instance answers (a
    /// connect would exit 1), an invalid token exits 2, and a valid token
    /// with no listener exits 1. The environment is injected, so the test
    /// never touches the process environment; the name has no listener.
    #[test]
    fn the_focus_client_reads_the_token_before_it_connects() {
        let name = Some(InstanceName::parse("--focus", "no-such-instance").expect("valid"));
        let none = |_: &str| None;

        // No token: exit 2, no instance contacted (that would be exit 1).
        assert_eq!(run_focus_client(name.clone(), none), 2);

        // An invalid token: exit 2 as well.
        let spaced = |_: &str| Some("niri spawn".to_owned());
        assert_eq!(run_focus_client(name.clone(), spaced), 2);

        // A valid token with no listener on the name: the request is not
        // answered, exit 1.
        let valid = |_: &str| Some("niri-spawn:pinwin-172839".to_owned());
        assert_eq!(run_focus_client(name, valid), 1);
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

            let mut fds = [0 as libc::c_int; 2];
            // SAFETY: `fds` is writable.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
            // SAFETY: fork in a threaded test process; the child between the
            // fork and the exec only calls async-signal-safe functions —
            // close, signal, dup2, exec, _exit — exactly like `child_exec`.
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0, "fork: {}", io::Error::last_os_error());
            if pid == 0 {
                unsafe {
                    libc::close(fds[0]);
                    libc::dup2(fds[1], libc::STDOUT_FILENO);
                }
                if reset {
                    // The real child path, reset and exec included; it
                    // never returns.
                    child_exec(&child_command);
                }
                // The control: the same exec without the reset; it never
                // returns.
                unsafe {
                    let argv = [arg0.as_ptr(), file.as_ptr(), std::ptr::null()];
                    libc::execve(program.as_ptr(), argv.as_ptr(), std::ptr::null());
                    libc::_exit(127);
                }
            }
            // SAFETY: each end is closed by its own process only.
            unsafe {
                libc::close(fds[1]);
            }
            let mut output = Vec::new();
            loop {
                let mut chunk = [0u8; 512];
                // SAFETY: `fds[0]` is the read end, `chunk` is writable.
                let n = unsafe { libc::read(fds[0], chunk.as_mut_ptr().cast(), chunk.len()) };
                if n == 0 {
                    break;
                }
                if n < 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    panic!("read: {error}");
                }
                output.extend_from_slice(&chunk[..n as usize]);
            }
            // SAFETY: the read end is drained.
            unsafe {
                libc::close(fds[0]);
            }
            let mut status: i32 = 0;
            // SAFETY: `pid` is this test's child, `status` writable.
            assert_eq!(
                unsafe { libc::waitpid(pid, &mut status, 0) },
                pid,
                "waitpid"
            );
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
