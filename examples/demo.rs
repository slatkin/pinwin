//! Dev-only driver for the [`pinwin::Panel`](pinwin::panel::Panel) API
//! (port-to-rust D9): the port of `demo/main.c`. Built only by
//! `cargo build --examples`, never installed — the example twin of the old
//! `zig build demo`.
//!
//! It makes its own pty pair (`forkpty`), forks a canned child on the slave
//! side and sets the child's terminal environment, then drives the panel API
//! from this thread: a canned startup layout, a live apply, a rejected
//! layout, an animated and a plain width toggle, a covering toggle, and a
//! drop to stop. Fork and
//! exec are fine here because this is a program; only the library must not.
//!
//! Commands on the demo's own stdin (one per line):
//!   <enter>  toggle side/width and apply live (resize/re-dock)
//!   e        animated width toggle 40 <-> 120 cols, same side (200 ms)
//!   p        the same toggle through the plain snap apply
//!   c        animated cover toggle: pushing 40 cols vs covering 120 cols,
//!            same side (200 ms) — the reservation holds, tiles stay put
//!   b        apply a rejected layout: expect `InvalidLayout`, host lives
//!   q        stop the panel and exit
//! Run `exit` inside the panel to watch a pty hangup leave the host alone.
//!
//! `DEMO_KEYBOARD=<mode>` selects the panel's keyboard mode, fixed at start:
//! `on-demand` (the default — click-to-focus, so other windows keep the
//! keyboard and the terminal that launched the demo keeps its stdin
//! commands), `exclusive` (the C demo's `PINWIN_KEYBOARD_EXCLUSIVE` path,
//! where the panel owns the keyboard outright) or `none`.
//!
//! `DEMO_DENSE=1` replaces the shell child with a dense stand-in: a full
//! 120+ column text grid, kitty images on screen and light periodic traffic,
//! retransmitting the images after every `SIGWINCH` like a real TUI host
//! does. Image bytes come from a system icon PNG, as in the C demo.

use std::fmt::Write as _;
use std::io::{BufRead as _, Write as _};
use std::num::NonZeroU16;
use std::os::fd::RawFd;
use std::os::raw::c_char;
use std::os::raw::c_void;
use std::os::unix::process::CommandExt;
use std::sync::atomic::Ordering;
use std::time::Duration;

use pinwin::layout::{Accent, Coverage, Keyboard, Layout, Side};
use pinwin::panel::{Panel, Startup};

const DEMO_COLS: u16 = 40;
/// The wide end of the plain width and cover toggles, in columns.
const DEMO_WIDE_COLS: u16 = 120;
const DEMO_GUTTER: i32 = 12;
/// The default animated duration, `pinwin.h`'s `PINWIN_ANIM_DEFAULT_MS`.
const DEMO_ANIM_MS: u32 = 200;
/// How long the canned layout settles before the live re-dock, as the C's
/// `SETTLE_US`.
const SETTLE: Duration = Duration::from_micros(1_500_000);
/// The second argument the demo re-execs itself with for the dense child.
const DENSE_CHILD_ARG: &str = "--dense-child";
/// The dense child's image source (a system icon PNG, as in the C demo).
const DENSE_IMAGE: &str = "/usr/share/icons/hicolor/512x512/apps/com.mitchellh.ghostty.png";

/// The dense child's `SIGWINCH` latch, the `volatile sig_atomic_t` of the C.
static DENSE_RESIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// `demo/main.c`'s `canned_layout`: the demo layout with `DEMO_GUTTER` as the
/// right gutter and no other reservation.
fn canned_layout(side: Side, cols: u16) -> Layout {
    Layout::new(
        side,
        NonZeroU16::new(cols).expect("demo columns are non-zero"),
        0,
        0,
        0,
        DEMO_GUTTER,
    )
}

/// The optional numeric argument of the demo: 1..=65535 columns (the layout
/// type's range, D6). Anything else is ignored, like the C's `strtol` guard.
fn parse_cols(arg: &str) -> Option<u16> {
    arg.parse::<u16>().ok().filter(|cols| *cols >= 1)
}

/// The dense child's `SIGWINCH` handler: set the repaint latch, nothing else
/// (async-signal-safe). Ports `dense_on_winch`.
extern "C" fn dense_on_winch(_sig: std::os::raw::c_int) {
    DENSE_RESIZED.store(true, Ordering::Release);
}

/// RFC 4648 base64 (standard alphabet, padded) for the kitty payload: the
/// local replacement for the `glib::base64_encode` the GTK path used. The
/// RFC 4648 test vectors are asserted in the demo's test module.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |&byte| u32::from(byte));
        let b2 = chunk.get(2).map_or(0, |&byte| u32::from(byte));
        let n = (b0 << 16) | (b1 << 8) | b2;
        let quad = [
            TABLE[num_traits::cast::<u32, usize>((n >> 18) & 0x3f).expect("six bits")],
            TABLE[num_traits::cast::<u32, usize>((n >> 12) & 0x3f).expect("six bits")],
            if chunk.len() > 1 {
                TABLE[num_traits::cast::<u32, usize>((n >> 6) & 0x3f).expect("six bits")]
            } else {
                b'='
            },
            if chunk.len() > 2 {
                TABLE[num_traits::cast::<u32, usize>(n & 0x3f).expect("six bits")]
            } else {
                b'='
            },
        ];
        out.push_str(std::str::from_utf8(&quad).expect("table bytes are ascii"));
    }
    out
}

/// The dense child's kitty image transmission: the PNG base64-encoded into
/// four placements of distinct image ids, one per quadrant — a ~480 KB kitty
/// burst per repaint, the same order as the real host's art re-encode.
/// `q=1` so parse errors come back on stdin, where [`dump_stdin`] shows
/// them.
fn dense_send_image() {
    // The C capped its fixed buffer at 128 KiB; keep the cap so the burst
    // size stays the one the demo was tuned against.
    let raw = match std::fs::read(DENSE_IMAGE) {
        Ok(raw) => raw,
        Err(_) => return,
    };
    let raw = &raw[..raw.len().min(1 << 17)];
    let b64 = base64_encode(raw);

    let mut out = String::new();
    for id in 1..=4u32 {
        let slot = id - 1;
        let _ = write!(
            out,
            "\x1b[{};{}H\x1b_Gf=24,a=T,i={id},q=1,c=20,r=20;{}\x1b\\",
            2 + slot / 2 * 22,
            2 + slot % 2 * 25,
            b64
        );
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
}

/// The dense child drains its own stdin (the panel side of the pty) so kitty
/// responses (`q=1` errors) reach the demo log instead of sitting unread:
/// non-blocking read, dumped to stderr. Ports `dump_stdin`.
fn dump_stdin() {
    // SAFETY: `F_GETFL`/`F_SETFL` on our own stdin descriptor, restored after
    // the drain; `read` on our own descriptor with a writable buffer.
    unsafe {
        let flags = libc::fcntl(0, libc::F_GETFL);
        if flags < 0 || libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return;
        }
        let mut buf = [0u8; 256];
        loop {
            let n = libc::read(0, buf.as_mut_ptr().cast::<c_void>(), buf.len());
            if n > 0 {
                eprint!("child got {n} bytes: ");
                let _ = std::io::stderr().write_all(&buf[..n as usize]);
                eprintln!();
            } else {
                if n < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::WouldBlock {
                        eprintln!("child stdin: {error}");
                    }
                }
                break;
            }
        }
        libc::fcntl(0, libc::F_SETFL, flags);
    }
}

/// The dense child's full-grid repaint: header/footer bars with reverse
/// video, a body of dense varied text (the renderer pays per cell) and
/// background-colour spans, like a real TUI's cards — then the kitty images.
/// Ports `dense_draw_screen`.
fn dense_draw_screen() {
    let mut ws = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `ws` is a writable winsize for the ioctl on our stdout (the
    // slave pty).
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return;
    }
    let (rows, cols) = (i32::from(ws.ws_row), i32::from(ws.ws_col));
    if rows < 1 || cols < 1 {
        return;
    }

    let mut frame = String::with_capacity(rows as usize * cols as usize * 12);
    frame.push_str("\x1b[?25l\x1b[H\x1b[2J");
    for r in 0..rows {
        let _ = write!(frame, "\x1b[{};1H", r + 1);
        for c in 0..cols {
            if r == 0 || r == rows - 1 {
                if c == 0 {
                    frame.push_str("\x1b[7m");
                }
                frame.push(if c % 2 == 0 {
                    ' '
                } else if r == 0 {
                    '-'
                } else {
                    '='
                });
                if c == cols - 1 {
                    frame.push_str("\x1b[27m");
                }
            } else if (c / 9) % 2 == 0 {
                let _ = write!(frame, "\x1b[48;5;{}m", (r * 7 + c / 9) % 200 + 16);
                frame.push((b'a' + ((r * 31 + c * 7) % 26) as u8) as char);
            } else {
                frame.push_str("\x1b[49m");
                frame.push((b'0' + ((r + c) % 10) as u8) as char);
            }
        }
        frame.push_str("\x1b[49m");
    }
    {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(frame.as_bytes());
        let _ = stdout.flush();
    }
    dense_send_image();
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\x1b[{rows};1H");
    let _ = stdout.flush();
}

/// The dense child's clock row: light periodic traffic so the pty is not
/// idle, saved/restored around the cursor position like the C.
fn dense_clock_row(now: libc::time_t) {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `now` is a plain `time_t` value and `tm` is our writable
    // struct; `strftime` writes only inside our bounded buffer, whose format
    // string is a NUL-terminated literal.
    unsafe {
        libc::localtime_r(&now, &mut tm);
        let mut buf = [0u8; 64];
        const FMT: &[u8] = b"\x1b[s%H:%M:%S\x1b[u\0";
        let n = libc::strftime(
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len(),
            FMT.as_ptr().cast::<c_char>(),
            &tm,
        );
        let clock = &buf[..n as usize];
        let mut stdout = std::io::stdout().lock();
        let _ = write!(stdout, "\x1b[1;1H\x1b[7m ");
        let _ = stdout.write_all(clock);
        let _ = write!(stdout, " \x1b[27m");
        let _ = stdout.flush();
    }
}

/// The dense child's main loop: repaint on `SIGWINCH`, one clock row per
/// second otherwise, drain stdin, sleep. Ports `dense_child_main`.
fn dense_child_main() {
    // SAFETY: installing our own `SIGWINCH` disposition before any thread
    // exists; the handler only stores to an atomic.
    unsafe {
        libc::signal(
            libc::SIGWINCH,
            dense_on_winch as *const () as libc::sighandler_t,
        );
    }
    let mut last: libc::time_t = 0;
    loop {
        // SAFETY: `time` takes no argument.
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        if DENSE_RESIZED.swap(false, Ordering::AcqRel) {
            dense_draw_screen();
        } else if now != last {
            last = now;
            dense_clock_row(now);
        }
        dump_stdin();
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The canned child on the slave side: the tester's shell, told it is a
/// colour terminal (the library sets no child environment, the spec's
/// host-owned pty requirement). With `DEMO_DENSE=1` the child is the dense
/// stand-in instead, re-exec'd from this same binary. Ports `spawn_child`.
fn spawn_child(dense_mode: bool) -> Result<RawFd, std::io::Error> {
    // Edition 2024: `set_var` is unsafe, and this runs before the fork.
    // SAFETY: single-threaded at this point, so no other thread can observe
    // the environment mid-write.
    unsafe {
        std::env::set_var("TERM", "xterm-256color");
        std::env::set_var("COLORTERM", "truecolor");
    }

    let mut master: libc::c_int = -1;
    // SAFETY: `forkpty` writes the master fd through our pointer and leaves
    // the terminal attributes and winsize untouched (`null`); the child
    // branch below never returns.
    let pid = unsafe {
        libc::forkpty(
            &mut master,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if pid < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if pid == 0 {
        // Rust's std sets SIGPIPE to SIG_IGN at startup and the disposition
        // survives execve; restore the default so the shell and the dense
        // child die on a closed pipe as demo/main.c's SIG_DFL child did.
        // SAFETY: a plain disposition change in the forked child, before any
        // thread exists and before exec.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL as libc::sighandler_t);
        }
        if dense_mode {
            // Re-exec self in dense-child mode: no shell, no session.
            let _ = std::process::Command::new("/proc/self/exe")
                .arg(DENSE_CHILD_ARG)
                .exec();
        } else {
            let shell = std::env::var_os("SHELL")
                .filter(|shell| !shell.is_empty())
                .unwrap_or_else(|| std::ffi::OsString::from("/bin/sh"));
            let _ = std::process::Command::new(shell).exec();
        }
        // `exec` only returns when it failed.
        std::process::exit(127);
    }
    Ok(master)
}

/// The live plain apply and its report line, the C's `apply`.
fn apply(panel: &Panel, side: Side, cols: u16) {
    let layout = canned_layout(side, cols);
    let result = panel.apply_layout(layout);
    println!("apply_layout(side={side:?}, cols={cols}) = {result:?}");
}

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(DENSE_CHILD_ARG) {
        dense_child_main();
        return;
    }
    let dense_mode = std::env::var_os("DEMO_DENSE").is_some();
    // Default to click-to-focus so the launching terminal keeps the keyboard
    // and its stdin commands; `DEMO_KEYBOARD` opts into the other modes.
    let keyboard = match std::env::var_os("DEMO_KEYBOARD") {
        None => Keyboard::OnDemand,
        Some(value) => {
            let value = value.to_string_lossy();
            match Keyboard::parse(&value) {
                Some(keyboard) => keyboard,
                None => {
                    eprintln!(
                        "pinwin-demo: unknown DEMO_KEYBOARD={value} \
                         (want on-demand, exclusive or none)"
                    );
                    std::process::exit(2);
                }
            }
        }
    };
    let mut cols = DEMO_COLS;
    if let Some(arg) = args.next()
        && let Some(parsed) = parse_cols(&arg)
    {
        cols = parsed;
    }

    // The child's exit is its own business; reap it silently.
    // SAFETY: a plain disposition change, no handler state.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
    }

    let master = match spawn_child(dense_mode) {
        Ok(master) => master,
        Err(error) => {
            eprintln!("forkpty: {error}");
            std::process::exit(1);
        }
    };

    let side = Side::Left;
    let startup = Startup {
        fd: master,
        layout: canned_layout(side, cols),
        keyboard,
        accent: Some(Accent::new(
            [0xda, 0xbc, 0x7f],
            NonZeroU16::new(1).expect("accent width 1"),
        )),
    };
    match Panel::start(startup) {
        Ok(panel) => {
            println!("pinwin_start = Ok(())");
            run_commands(panel, cols)
        }
        Err(error) => {
            eprintln!("pinwin-demo: pinwin_start failed ({error:?})");
            std::process::exit(1);
        }
    }
}

/// What one stdin command asks the panel to do, so the command loop's
/// single match builds the layout, the report line and the apply call
/// together and a new command needs one edit here plus one arm there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DemoAction {
    /// `e`: the animated width toggle.
    Animated,
    /// `p`: the same toggle through the plain snap apply.
    Plain,
    /// `c`: the cover toggle.
    CoverToggle,
    /// `<enter>` (or anything else): the side/width re-dock.
    ReDock,
}

/// The demo's tracked layout state: the layout actually applied — side,
/// columns and coverage — so a later `c` flips the coverage that is really
/// on screen even after `e`, `p` or `<enter>` re-applied a pushing layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DemoLayout {
    side: Side,
    cols: u16,
    coverage: Coverage,
}

impl DemoLayout {
    /// The next state and the action for one stdin command: `e`/`p` the width
    /// toggle and `c` the cover toggle on the same side, anything else (an
    /// empty line included) the `<enter>` side/width re-dock. Every command
    /// applies a plain pushing layout except `c` flipping into covering, so
    /// the coverage always tracks what the panel is actually in.
    fn step(self, cmd: Option<char>) -> (DemoAction, Self) {
        let action = match cmd {
            Some('e') => DemoAction::Animated,
            Some('p') => DemoAction::Plain,
            Some('c') => DemoAction::CoverToggle,
            _ => DemoAction::ReDock,
        };
        let toggled = |wide| {
            if self.cols == DEMO_COLS {
                wide
            } else {
                DEMO_COLS
            }
        };
        let state = match action {
            DemoAction::Animated | DemoAction::Plain => Self {
                cols: toggled(DEMO_WIDE_COLS),
                coverage: Coverage::Push,
                ..self
            },
            DemoAction::CoverToggle => {
                let coverage = match self.coverage {
                    Coverage::Push => Coverage::Cover,
                    Coverage::Cover => Coverage::Push,
                };
                Self {
                    cols: match coverage {
                        Coverage::Cover => DEMO_WIDE_COLS,
                        Coverage::Push => DEMO_COLS,
                    },
                    coverage,
                    ..self
                }
            }
            DemoAction::ReDock => Self {
                side: match self.side {
                    Side::Left => Side::Right,
                    Side::Right => Side::Left,
                },
                cols: toggled(DEMO_COLS + 8),
                coverage: Coverage::Push,
            },
        };
        (action, state)
    }

    /// The layout this state applies, covering only when the state says so.
    fn layout(self) -> Layout {
        let layout = canned_layout(self.side, self.cols);
        if self.coverage == Coverage::Cover {
            layout.covering()
        } else {
            layout
        }
    }
}

/// The command loop after a successful start: the C demo's settle, re-dock
/// and stdin command handling, ending with the stop (drop).
fn run_commands(panel: Panel, cols: u16) {
    // Let the canned layout dock, then re-dock live so the change is visible.
    std::thread::sleep(SETTLE);
    let mut state = DemoLayout {
        side: Side::Right,
        cols,
        coverage: Coverage::Push,
    };
    apply(&panel, state.side, state.cols);

    println!(
        "commands, typed in THIS terminal (not in the panel):\n          \
         <enter> toggle side/width, 'c' cover toggle, 'b' rejected layout,\n          \
         'q' quit; run `exit` in the panel to see a pty hangup survive."
    );

    for line in std::io::stdin().lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        match line.chars().next() {
            Some('q') => break,
            Some('b') => {
                // The C applied cols 0; zero columns are unrepresentable in
                // `Layout` now (D6), so the demo's rejected layout is one the
                // monitor cannot hold instead: the full column range.
                let bad = canned_layout(state.side, u16::MAX);
                let result = panel.apply_layout(bad);
                println!("apply_layout(invalid) = {result:?} (want Err(InvalidLayout))");
            }
            cmd => {
                let (action, next) = state.step(cmd);
                state = next;
                let layout = state.layout();
                let (label, result) = match action {
                    DemoAction::Animated => (
                        format!("animated(cols={})", state.cols),
                        panel.apply_layout_animated(layout, DEMO_ANIM_MS),
                    ),
                    DemoAction::Plain => (
                        format!("plain(cols={})", state.cols),
                        panel.apply_layout(layout),
                    ),
                    DemoAction::CoverToggle => (
                        format!(
                            "cover toggle(cols={}, covering={})",
                            state.cols,
                            state.coverage == Coverage::Cover
                        ),
                        panel.apply_layout_animated(layout, DEMO_ANIM_MS),
                    ),
                    DemoAction::ReDock => (
                        format!("apply_layout(side={:?}, cols={})", state.side, state.cols),
                        panel.apply_layout(layout),
                    ),
                };
                println!("{label} = {result:?}");
            }
        }
    }

    // `pinwin_stop`: dropping the handle closes the panel.
    drop(panel);
    println!("pinwin_stop done");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The encoder's RFC 4648 test vectors (the empty string through all
    /// three padding shapes plus the full alphabet prefix).
    #[test]
    fn base64_encode_matches_the_rfc_4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn canned_layout_reserves_only_the_right_gutter() {
        for side in [Side::Left, Side::Right] {
            let layout = canned_layout(side, DEMO_COLS);
            assert_eq!(layout.side(), side);
            assert_eq!(layout.cols().get(), DEMO_COLS);
            assert_eq!(layout.top(), 0);
            assert_eq!(layout.bottom(), 0);
            assert_eq!(layout.left(), 0);
            assert_eq!(layout.right(), DEMO_GUTTER);
        }
    }

    #[test]
    fn the_cover_toggle_is_pushing_narrow_and_covering_wide() {
        let narrow = canned_layout(Side::Left, DEMO_COLS);
        assert_eq!(narrow.coverage(), Coverage::Push);
        let wide = canned_layout(Side::Left, DEMO_WIDE_COLS).covering();
        assert_eq!(wide.coverage(), Coverage::Cover);
        assert_eq!(wide.cols().get(), DEMO_WIDE_COLS);
        // Same side and gutters, so the toggle animates and the held strip
        // stays put.
        assert_eq!(wide.side(), narrow.side());
        assert_eq!(wide.left(), narrow.left());
        assert_eq!(wide.right(), narrow.right());
    }

    #[test]
    fn the_cover_toggle_tracks_the_layout_actually_applied() {
        // Start pushing 40, as after the re-dock.
        let start = DemoLayout {
            side: Side::Right,
            cols: DEMO_COLS,
            coverage: Coverage::Push,
        };
        // `c` covers 120.
        let (action, state) = start.step(Some('c'));
        assert_eq!(action, DemoAction::CoverToggle);
        let layout = state.layout();
        assert_eq!(layout.coverage(), Coverage::Cover);
        assert_eq!(layout.cols().get(), DEMO_WIDE_COLS);
        // `e` re-applies a pushing width toggle, so the coverage is pushing
        // now even though the cover toggle had just covered.
        let (action, state) = state.step(Some('e'));
        assert_eq!(action, DemoAction::Animated);
        let layout = state.layout();
        assert_eq!(layout.coverage(), Coverage::Push);
        assert_eq!(layout.cols().get(), DEMO_COLS);
        // `p` the same, at the wide end but still pushing.
        let (action, state) = state.step(Some('p'));
        assert_eq!(action, DemoAction::Plain);
        let layout = state.layout();
        assert_eq!(layout.coverage(), Coverage::Push);
        assert_eq!(layout.cols().get(), DEMO_WIDE_COLS);
        // So this `c` flips into covering; the old desynced flag would have
        // flipped away from a covering state that was no longer there and
        // re-applied the same pushing layout as a visible no-op.
        let (action, state) = state.step(Some('c'));
        assert_eq!(action, DemoAction::CoverToggle);
        let layout = state.layout();
        assert_eq!(layout.coverage(), Coverage::Cover);
        assert_eq!(layout.cols().get(), DEMO_WIDE_COLS);
        // And `<enter>` re-docks pushing again.
        let (action, state) = state.step(None);
        assert_eq!(action, DemoAction::ReDock);
        let layout = state.layout();
        assert_eq!(layout.coverage(), Coverage::Push);
        assert_eq!(layout.cols().get(), DEMO_COLS);
        assert_eq!(layout.side(), Side::Left);
    }

    #[test]
    fn parse_cols_accepts_only_the_layout_range() {
        assert_eq!(parse_cols("40"), Some(40));
        assert_eq!(parse_cols("65535"), Some(65535));
        assert_eq!(parse_cols("0"), None);
        assert_eq!(parse_cols("-1"), None);
        assert_eq!(parse_cols("65536"), None);
        assert_eq!(parse_cols("junk"), None);
        assert_eq!(parse_cols(""), None);
    }
}
