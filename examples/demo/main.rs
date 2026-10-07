//! Dev-only driver for the [`pinwin::Panel`](pinwin::panel::Panel) API
//! (port-to-rust D9): the port of `demo/main.c`. Built only by
//! `cargo build --examples`, never installed — the example twin of the old
//! `zig build demo`.
//!
//! It makes its own pty pair (`forkpty`), forks a canned child on the slave
//! side and sets the child's terminal environment, then drives the panel API
//! from this thread: a canned startup layout, a live apply, a rejected
//! layout, an animated and a plain width toggle, a covering toggle, vertical
//! insets, a negative gutter, a hide/show toggle, and a drop to stop. Fork
//! and exec are fine here because this is a program; only the library must
//! not.
//!
//! Commands on the demo's own stdin (one per line):
//!   <enter>  toggle side/width and apply live (resize/re-dock)
//!   e        animated width toggle 40 <-> 120 cols, same side (200 ms);
//!            press it again mid-tween to interrupt the animation
//!   p        the same toggle through the plain snap apply
//!   c        animated cover toggle: pushing 40 cols vs covering 120 cols,
//!            same side (200 ms) — the reservation holds, tiles stay put
//!   t        hide/show toggle (`Panel::toggle`): hide unmaps the panel and
//!            releases the reservation, show draws the grid again; apply a
//!            layout while hidden to see it come back at the show
//!   i        vertical-inset toggle: top and bottom gutters 0 <-> 40
//!   g        right-gutter toggle: 12 <-> -24, so the negative gutter moves
//!            the panel's edge past the output edge
//!   b        apply a rejected layout: expect `InvalidLayout`, host lives
//!   q        stop the panel and exit
//! Run `exit` inside the panel to watch a pty hangup leave the host alone.
//!
//! `DEMO_KEYBOARD=<mode>` selects the panel's keyboard mode, fixed at start:
//! `on-demand` (the default — click-to-focus, so other windows keep the
//! keyboard and the terminal that launched the demo keeps its stdin
//! commands), `exclusive` (the C demo's `PINWIN_KEYBOARD_EXCLUSIVE` path,
//! where the panel owns the keyboard outright) or `none`. `t` toggles in
//! every mode, so `DEMO_KEYBOARD=none` shows the hidden panel too.
//!
//! `DEMO_ZONE=reserve|overlay` mirrors `PINWIN_ZONE`: `reserve` (the
//! default) starts pushing, so tiles sit beside the panel and a `t` toggle
//! moves them; `overlay` starts covering, so nothing is reserved and a
//! toggle moves no window. `c` flips the coverage either way mid-run.
//!
//! `DEMO_ACCENT=on|off` mirrors `PINWIN_ACCENT`: the default accent is on
//! (the niri focus-ring colour); `off` starts with no accent, so focus
//! shows no highlight.
//!
//! The environment reaches the library as it does for the host, so
//! `PINWIN_FRAMELOG=1` records one summary line per tween from the `e` and
//! `c` commands.
//!
//! `DEMO_DENSE=1` replaces the shell child with a dense stand-in: a full
//! 120+ column text grid, kitty images on screen and light periodic traffic,
//! retransmitting the images after every `SIGWINCH` like a real TUI host
//! does. Its code lives in the `dense_child` module.

use std::io::BufRead as _;
use std::num::NonZeroU16;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::time::Duration;

use pinwin::layout::{Accent, Coverage, Keyboard, Layout, Side};
use pinwin::panel::{Panel, Startup};

mod dense_child;

const DEMO_COLS: u16 = 40;
/// The wide end of the plain width and cover toggles, in columns.
const DEMO_WIDE_COLS: u16 = 120;
const DEMO_GUTTER: i32 = 12;
/// The default animated duration, `pinwin.h`'s `PINWIN_ANIM_DEFAULT_MS`.
const DEMO_ANIM_MS: u32 = 200;
/// The `i` key's inset: the top and bottom gutters the toggle applies.
const DEMO_INSET: i32 = 40;
/// The `g` key's negative gutter: it moves the panel's edge past the output
/// edge (the layout type allows negative gutters).
const DEMO_NEG_GUTTER: i32 = -24;
/// How long the canned layout settles before the live re-dock, as the C's
/// `SETTLE_US`.
const SETTLE: Duration = Duration::from_millis(1500);
/// The second argument the demo re-execs itself with for the dense child.
const DENSE_CHILD_ARG: &str = "--dense-child";

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
    };

    let mut master: libc::c_int = -1;
    // SAFETY: `forkpty` writes the master fd through our pointer and leaves
    // the terminal attributes and winsize untouched (`null`); the child
    // branch below never returns.
    let pid = unsafe {
        libc::forkpty(
            &raw mut master,
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
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
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

/// Apply the tracked state's own layout and report the result: every apply
/// the demo makes derives from the tracked `DemoLayout`, so the coverage and
/// gutters the panel is really in are never rebuilt from side and columns
/// alone.
fn apply_state(panel: &Panel, state: DemoLayout) {
    let result = panel.apply_layout(state.layout());
    println!(
        "apply_layout(side={:?}, cols={}) = {result:?}",
        state.side, state.cols
    );
}

/// The tracked state the demo starts from, matching the startup layout: the
/// reserve zone starts pushing on the re-dock side, the overlay zone keeps
/// the startup's covering layout on its own side (a covering side switch is
/// not a move the panel makes).
fn initial_state(cols: u16, overlay: bool) -> DemoLayout {
    if overlay {
        DemoLayout {
            side: Side::Left,
            cols,
            coverage: Coverage::Cover,
            inset: 0,
            gutter: DEMO_GUTTER,
        }
    } else {
        DemoLayout {
            side: Side::Right,
            cols,
            coverage: Coverage::Push,
            inset: 0,
            gutter: DEMO_GUTTER,
        }
    }
}

/// The demo's startup accent, the niri focus-ring colour at width 1: the
/// same accent the host's `PINWIN_ACCENT` default uses.
fn default_accent() -> Accent {
    Accent::new(
        [0xda, 0xbc, 0x7f],
        NonZeroU16::new(1).expect("accent width 1"),
    )
}

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(DENSE_CHILD_ARG) {
        dense_child::dense_child_main();
        return;
    }
    let dense_mode = std::env::var_os("DEMO_DENSE").is_some();
    // `DEMO_ZONE` mirrors `PINWIN_ZONE`: `overlay` starts covering, so
    // nothing is reserved and a `t` toggle moves no window (the same
    // mapping `main.rs` applies to the host's starting layout).
    let overlay = match std::env::var_os("DEMO_ZONE").as_deref() {
        None => false,
        Some(value) => match value.to_string_lossy().as_ref() {
            "" | "reserve" => false,
            "overlay" => true,
            other => {
                eprintln!("pinwin-demo: unknown DEMO_ZONE={other} (want reserve or overlay)");
                std::process::exit(2);
            }
        },
    };
    // `DEMO_ACCENT` mirrors `PINWIN_ACCENT`: `off` starts with no accent, so
    // focus shows no highlight.
    let accent = match std::env::var_os("DEMO_ACCENT").as_deref() {
        None => Some(default_accent()),
        Some(value) => match value.to_string_lossy().as_ref() {
            "" | "on" => Some(default_accent()),
            "off" => None,
            other => {
                eprintln!("pinwin-demo: unknown DEMO_ACCENT={other} (want on or off)");
                std::process::exit(2);
            }
        },
    };
    // Default to click-to-focus so the launching terminal keeps the keyboard
    // and its stdin commands; `DEMO_KEYBOARD` opts into the other modes.
    let keyboard = match std::env::var_os("DEMO_KEYBOARD") {
        None => Keyboard::OnDemand,
        Some(value) => {
            let value = value.to_string_lossy();
            let Some(keyboard) = Keyboard::parse(&value) else {
                eprintln!(
                    "pinwin-demo: unknown DEMO_KEYBOARD={value} \
                     (want on-demand, exclusive or none)"
                );
                std::process::exit(2);
            };
            keyboard
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
    };

    let master = match spawn_child(dense_mode) {
        Ok(master) => master,
        Err(error) => {
            eprintln!("forkpty: {error}");
            std::process::exit(1);
        }
    };

    let side = Side::Left;
    let mut startup_layout = canned_layout(side, cols);
    if overlay {
        startup_layout = startup_layout.covering();
    }
    let startup = Startup {
        fd: master,
        layout: startup_layout,
        keyboard,
        accent,
    };
    match Panel::start(startup) {
        Ok(panel) => {
            println!("pinwin_start = Ok(())");
            run_commands(panel, cols, overlay);
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
    /// `i`: the vertical-inset toggle.
    InsetToggle,
    /// `g`: the right-gutter toggle (into the negative gutter).
    GutterToggle,
    /// `<enter>` (or anything else): the side/width re-dock.
    ReDock,
}

/// The demo's tracked layout state: the layout actually applied — side,
/// columns, coverage and the top/bottom and right gutters — so a later `c`
/// flips the coverage that is really on screen even after `e`, `p` or
/// `<enter>` re-applied a pushing layout, and `i`/`g` carry the gutters the
/// panel is really in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DemoLayout {
    side: Side,
    cols: u16,
    coverage: Coverage,
    /// The top and bottom gutters the `i` toggle flips.
    inset: i32,
    /// The right gutter the `g` toggle flips.
    gutter: i32,
}

impl DemoLayout {
    /// The next state and the action for one stdin command: `e`/`p` the width
    /// toggle, `c` the cover toggle, `i` the inset toggle and `g` the gutter
    /// toggle on the same side, anything else (an empty line included) the
    /// `<enter>` side/width re-dock. Every command applies a plain pushing
    /// layout except `c` flipping into covering, so the coverage always
    /// tracks what the panel is actually in. The `t` hide/show toggle is not
    /// a layout change, so it never reaches this state machine.
    fn step(self, cmd: Option<char>) -> (DemoAction, Self) {
        let action = match cmd {
            Some('e') => DemoAction::Animated,
            Some('p') => DemoAction::Plain,
            Some('c') => DemoAction::CoverToggle,
            Some('i') => DemoAction::InsetToggle,
            Some('g') => DemoAction::GutterToggle,
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
            DemoAction::InsetToggle => Self {
                inset: if self.inset == 0 { DEMO_INSET } else { 0 },
                ..self
            },
            DemoAction::GutterToggle => Self {
                gutter: if self.gutter == DEMO_GUTTER {
                    DEMO_NEG_GUTTER
                } else {
                    DEMO_GUTTER
                },
                ..self
            },
            DemoAction::ReDock => Self {
                side: match self.side {
                    Side::Left => Side::Right,
                    Side::Right => Side::Left,
                },
                cols: toggled(DEMO_COLS + 8),
                coverage: Coverage::Push,
                ..self
            },
        };
        (action, state)
    }

    /// The layout this state applies, covering only when the state says so
    /// and carrying the state's top/bottom and right gutters.
    fn layout(self) -> Layout {
        let layout = Layout::new(
            self.side,
            NonZeroU16::new(self.cols).expect("demo columns are non-zero"),
            self.inset,
            self.inset,
            0,
            self.gutter,
        );
        if self.coverage == Coverage::Cover {
            layout.covering()
        } else {
            layout
        }
    }
}

/// The command loop after a successful start: the C demo's settle, re-dock
/// and stdin command handling, ending with the stop (drop).
fn run_commands(panel: Panel, cols: u16, overlay: bool) {
    // Let the canned layout dock, then re-dock live so the change is visible.
    // With `DEMO_ZONE=overlay` the start covered, so the tracked state stays
    // on the startup side covering — a covering side switch is not a move
    // the panel makes, and the overlay run keeps nothing reserved until `c`
    // flips it.
    std::thread::sleep(SETTLE);
    let mut state = initial_state(cols, overlay);
    apply_state(&panel, state);

    println!(
        "commands, typed in THIS terminal (not in the panel):\n          \
         <enter> toggle side/width, 'c' cover toggle, 'i' vertical insets,\n          \
         'g' negative gutter, 't' hide/show, 'b' rejected layout,\n          \
         'q' quit; run `exit` in the panel to see a pty hangup survive."
    );

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        match line.chars().next() {
            Some('q') => break,
            Some('t') => {
                // The hide/show toggle (row 9.1): hide unmaps the panel and
                // releases the reservation, show draws the grid again. While
                // hidden, the other keys' applies validate and store the
                // layout, and the next `t` shows it. Like the applies, the
                // result is printed as-is — a dead panel thread reports
                // `NotRunning` here the same way.
                let result = panel.toggle();
                println!("toggle = {result:?}");
            }
            Some('b') => {
                // The C applied cols 0; zero columns are unrepresentable in
                // `Layout` now (D6), so the demo's rejected layout is one the
                // monitor cannot hold instead: the full column range, built
                // on the tracked state's side and gutters and pushing — the
                // rejection is the width, not the coverage.
                let bad = Layout::new(
                    state.side,
                    NonZeroU16::new(u16::MAX).expect("the full column range is non-zero"),
                    state.inset,
                    state.inset,
                    0,
                    state.gutter,
                );
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
                    DemoAction::InsetToggle => (
                        format!("inset toggle(top/bottom={})", state.inset),
                        panel.apply_layout(layout),
                    ),
                    DemoAction::GutterToggle => (
                        format!("gutter toggle(right={})", state.gutter),
                        panel.apply_layout(layout),
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
            inset: 0,
            gutter: DEMO_GUTTER,
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
    fn the_inset_and_gutter_toggles_carry_into_the_layout() {
        let start = DemoLayout {
            side: Side::Left,
            cols: DEMO_COLS,
            coverage: Coverage::Push,
            inset: 0,
            gutter: DEMO_GUTTER,
        };
        // `i` puts the 40 px top and bottom insets in; the right gutter and
        // the rest of the state stay put.
        let (action, state) = start.step(Some('i'));
        assert_eq!(action, DemoAction::InsetToggle);
        let layout = state.layout();
        assert_eq!(layout.top(), DEMO_INSET);
        assert_eq!(layout.bottom(), DEMO_INSET);
        assert_eq!(layout.right(), DEMO_GUTTER);
        assert_eq!(layout.cols().get(), DEMO_COLS);
        // A second `i` is back to the flush start.
        let (_, state) = state.step(Some('i'));
        assert_eq!(state.layout().top(), 0);
        // `g` puts the negative right gutter in, moving the panel's edge
        // past the output edge; the insets stay where they were.
        let (_, state) = start.step(Some('g'));
        let layout = state.layout();
        assert_eq!(layout.right(), DEMO_NEG_GUTTER);
        assert_eq!(layout.top(), 0);
        assert_eq!(layout.bottom(), 0);
        // And back.
        let (_, state) = state.step(Some('g'));
        assert_eq!(state.layout().right(), DEMO_GUTTER);
    }

    /// The post-settle apply uses the tracked state's layout, so the zone
    /// the start chose survives it: reserve stays pushing, overlay stays
    /// covering with exactly the startup's covering layout.
    #[test]
    fn the_post_settle_apply_matches_the_startup_zone() {
        let startup = canned_layout(Side::Left, DEMO_COLS);
        // Reserve starts pushing, on the re-dock side.
        let reserve = initial_state(DEMO_COLS, false);
        assert_eq!(reserve.layout().coverage(), Coverage::Push);
        assert_eq!(reserve.layout().side(), Side::Right);
        assert_eq!(reserve.layout().right(), DEMO_GUTTER);
        // Overlay keeps the startup's covering layout on its own side, so
        // the post-settle apply cannot flip it back to pushing.
        let overlay = initial_state(DEMO_COLS, true);
        assert_eq!(overlay.layout(), startup.covering());
        assert_eq!(overlay.layout().coverage(), Coverage::Cover);
        assert_eq!(overlay.layout().side(), Side::Left);
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
