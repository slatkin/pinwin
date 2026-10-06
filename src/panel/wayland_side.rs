//! The Wayland side of the `Panel` lifecycle (replace-gtk-with-wayland D2):
//! one thread per start, owning the Wayland connection and the calloop loop,
//! where the GTK side is a parked process-lifetime thread ([`super::gtk_side`]).
//!
//! [`spawn_panel_thread`] opens the connection from the display name (or the
//! environment when none is given), answers a socket that is missing or does
//! not speak Wayland with `NoDisplay` through the start handshake, and then
//! runs the calloop loop: apply, focus and teardown commands arrive on a
//! calloop channel, each answered through the bounded replies of
//! [`super::handshake`]. A teardown ends the loop and the thread ends on its
//! own, like the drop contract promises; a closed compositor connection
//! marks the panel dead (D2), so later calls report `NotRunning`.
//!
//! The thread serves exactly one panel, so unlike the parked GTK thread its
//! commands carry no panel id: a command can only reach the thread that
//! started for it, and the single-instance guard keeps a second panel out.
//!
//! The bound session — the sctk handlers, the two layer surfaces of row 3.1
//! and the grid sizing of row 3.3 — lives in the submodules beside this
//! lifecycle plumbing: [`state`] holds the dispatch state, [`surfaces`] the
//! surface geometry, [`sizing`] the pure size decisions, [`apply`] the layout
//! apply (row 3.5), [`commands`] the command-channel handling,
//! [`watchdog`] the startup watchdog, [`tween`] the width tween's driver
//! (row 6.1), [`crop`] the tween's crop plan and wide buffer (row 6.2),
//! [`tween_draw`] the tween's holder and frame glue (row 6.2) and
//! [`renderer`] the thread's font setup and renderer — the one owner of
//! the draw passes, metrics, theme and focus state that row 8.1's later
//! dispatches wire the frames and the tween's wide draw to (row 8.1).
//! [`frame_log`] the tween's frame log and its presentation-time source
//! (row 6.3). [`present`] is the frames' present step (dispatch D4c) —
//! the pure planner, the frame input and the executor the configure path
//! and the loop's repaint hook run; like [`renderer`] it is `pub` while
//! dispatch D5 has not moved `Panel::start` onto the thread, because a
//! `pub(crate)` entry with no caller is dead code under `-D warnings` and
//! no lint suppression is permitted. Until row
//! 8.1 switches `Panel::start` over, nothing in the crate calls
//! [`spawn_panel_thread`]: the entry point is `pub` so it stays reachable (a
//! `pub(crate)` entry with no caller is dead code under `-D warnings`, and
//! no lint suppression is permitted). Rows 3.5 and 7.2 grow the publish and
//! activation paths into the command handlers.
//!
//! Panics never cross back into calloop or the compositor (D5): the whole
//! thread body and every callback the loop runs go through the shared
//! [`crate::guard`] helpers, latching the panel's one shared poisoned flag —
//! the same rule [`super::gtk_side`] applies to its GTK closures.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use calloop::channel::{self, Channel};
use calloop::timer::Timer;
use calloop_wayland_source::WaylandSource;
use wayland_client::Connection;

use crate::activation::ActivationToken;
use crate::guard::{Poisoned, guard, guard_always};
use crate::layout::Layout;
use crate::pty::{Pty, attach_calloop};
use crate::surfaces::PublishOutcome;

use super::Inner;
use super::PinwinError;
use super::Startup;
use super::handshake::{APPLY_WAIT, Handshake, StartOutcome, wait_for_apply, wait_for_focus};

pub(crate) mod activation;
pub(crate) mod apply;
pub mod buffers;
pub mod commands;
pub mod crop;
pub(crate) mod frame_log;
pub(crate) mod glue;
pub mod present;
pub mod renderer;
pub mod seat;
pub mod sizing;
pub(crate) mod state;
mod surfaces;
pub mod tween;
pub(crate) mod tween_draw;
pub(crate) mod watchdog;

use state::{BindFailure, PanelState};

/// A command the host posts to a running panel thread (D2), the wayland twin
/// of the GTK side's dispatched glue: an apply, a focus request or a
/// teardown, each carrying its own bounded reply channel from
/// [`super::handshake`].
#[derive(Debug)]
pub(crate) enum PanelCommand {
    /// Apply a layout (animated when `duration_ms` is non-zero), answered
    /// with a [`PublishOutcome`].
    Apply {
        /// The layout to publish.
        layout: Layout,
        /// The animated apply's duration, 0 for a snap.
        duration_ms: u32,
        /// The bounded reply the host waits on.
        reply: mpsc::SyncSender<PublishOutcome>,
    },
    /// Request keyboard focus by activating the panel surface with `token`
    /// (row 7.2), answered with `()` (replace-gtk-with-wayland D4: the
    /// compositor's choice is invisible to the client, so the reply only
    /// says the request was made).
    Focus {
        /// The token the request passes to the compositor.
        token: ActivationToken,
        /// The bounded reply the host waits on.
        reply: mpsc::SyncSender<()>,
    },
    /// Tear the panel down and end the thread, answered with `()`.
    Teardown {
        /// The bounded reply the host waits on.
        reply: mpsc::SyncSender<()>,
    },
}

/// Everything one panel thread needs to run its panel (D2). Built on the
/// host thread and moved into [`spawn_panel_thread`]. The fields are
/// `pub(crate)` because the thread, not the host, owns the panel state: a
/// host outside the crate can only pass a command it was handed, never
/// assemble one.
pub struct StartCommand {
    /// The panel's one shared D5 latch.
    pub(crate) poisoned: Poisoned,
    /// The start handshake the thread reports through.
    pub(crate) handshake: Handshake,
    /// The handle state the dead mapping writes to.
    pub(crate) inner: Arc<Inner>,
    /// The startup payload: the host-owned pty fd, the startup layout the
    /// surfaces apply (row 3.1) and the keyboard mode they map with (row
    /// 3.4). The thread attaches the fd to its pty and its read source
    /// (row 8.1); the host keeps the child's process lifetime.
    pub(crate) startup: Startup,
}

impl std::fmt::Debug for StartCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The handshake is a one-shot channel pair, not printable state.
        f.debug_struct("StartCommand")
            .field("poisoned", &self.poisoned)
            .field("startup", &self.startup)
            .finish_non_exhaustive()
    }
}

/// The host-side handle of one running panel thread (D2): the sender of the
/// command channel plus the handle state the posted commands check. Dropping
/// the handle closes the command channel, which ends the thread; the panel's
/// teardown goes through [`PanelThread::teardown`].
///
/// Not `Clone`: one handle per panel, like [`Panel`](super::Panel).
#[derive(Debug)]
pub struct PanelThread {
    commands: channel::Sender<PanelCommand>,
    inner: Arc<Inner>,
}

impl PanelThread {
    /// Apply a layout on the panel thread: the same order as the handle's
    /// apply — the poisoned check first (D5: a panic reports `Internal`,
    /// never `NotRunning`), then the ended check, then the bounded reply.
    ///
    /// # Errors
    /// `NotRunning` on a dead panel, `InvalidLayout` on a rejected layout,
    /// `Internal` on a caught panic, a wedged thread or a thread that ended
    /// between the live check and the post (the closed reply reads the same
    /// way the wedged GTK side does).
    pub fn apply(&self, layout: Layout, duration_ms: u32) -> Result<(), PinwinError> {
        match guard(&self.inner.poisoned, || {
            self.post_apply(layout, duration_ms)
        }) {
            Ok(result) => result,
            Err(_) => Err(PinwinError::Internal),
        }
    }

    /// The unguarded body of [`PanelThread::apply`].
    fn post_apply(&self, layout: Layout, duration_ms: u32) -> Result<(), PinwinError> {
        // Not running → `NotRunning` without blocking (the dead-panel order
        // of `pinwin_api.c`).
        if !self.inner.live.load(Ordering::Relaxed) {
            return Err(PinwinError::NotRunning);
        }
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let _ = self.commands.send(PanelCommand::Apply {
            layout,
            duration_ms,
            reply: reply_tx,
        });
        wait_for_apply(&reply_rx, APPLY_WAIT)
    }

    /// Post a keyboard-focus request and wait for its bounded reply. The
    /// keyboard-mode short-circuit stays with the host's handle: the mode is
    /// fixed at start time and recorded on the handle state, so the host
    /// decides whether anything is posted at all. On the thread, the token
    /// reaches the compositor as an xdg-activation request for the panel
    /// surface (row 7.2, D4) — or as a no-op when the mode is not
    /// `on-demand` or the compositor offers no xdg-activation.
    ///
    /// # Errors
    /// `NotRunning` on a dead panel, `Internal` on a caught panic or a
    /// wedged or ended thread.
    pub fn request_focus(&self, token: &ActivationToken) -> Result<(), PinwinError> {
        match guard(&self.inner.poisoned, || self.post_focus(token)) {
            Ok(result) => result,
            Err(_) => Err(PinwinError::Internal),
        }
    }

    /// The unguarded body of [`PanelThread::request_focus`].
    fn post_focus(&self, token: &ActivationToken) -> Result<(), PinwinError> {
        if !self.inner.live.load(Ordering::Relaxed) {
            return Err(PinwinError::NotRunning);
        }
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let _ = self.commands.send(PanelCommand::Focus {
            token: token.clone(),
            reply: reply_tx,
        });
        wait_for_focus(&reply_rx, APPLY_WAIT)
    }

    /// Post the teardown and wait for its bounded reply. The thread then
    /// ends on its own; the handle is never joined (D2). Whether the reply
    /// arrives is the caller's business — the drop path waits with the same
    /// bound and ignores the outcome.
    pub fn teardown(&self) {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let _ = self
            .commands
            .send(PanelCommand::Teardown { reply: reply_tx });
        let _ = reply_rx.recv_timeout(APPLY_WAIT);
    }
}

/// Spawn the panel thread (D2): one thread per start, owning the Wayland
/// connection, the calloop loop and the panel's surfaces. The thread reports
/// the start handshake's outcome — `NoDisplay` when the named socket (or the
/// environment's default) is missing or does not speak Wayland — and then
/// serves commands until a teardown or a closed connection ends it.
///
/// `display_name` names the Wayland socket: `None` follows the environment
/// (`WAYLAND_SOCKET`, then `WAYLAND_DISPLAY` in `XDG_RUNTIME_DIR`), a
/// relative name is a socket inside `XDG_RUNTIME_DIR`, an absolute name is
/// the socket path itself.
///
/// # Errors
/// Only the OS-level thread spawn fails here; a failed connection is
/// reported through the start handshake, not this result.
pub fn spawn_panel_thread(
    display_name: Option<&str>,
    start: StartCommand,
) -> std::io::Result<PanelThread> {
    let (commands, receiver) = channel::channel::<PanelCommand>();
    let inner = Arc::clone(&start.inner);
    let display = display_name.map(str::to_owned);
    std::thread::Builder::new()
        .name("pinwin-wayland".to_owned())
        .spawn(move || thread_main(display, start, receiver))?;
    Ok(PanelThread { commands, inner })
}

/// The panel thread's entry (D5 boundary): a panic anywhere in the thread
/// body is caught by the thread's own latch, and whatever ended the thread
/// — teardown, a closed connection or a caught panic — leaves the handle
/// dead and a still-pending start handshake failed, instead of hanging the
/// waiting host. A caught panic reports `Internal` (D5) and latches the
/// shared flag, so later host calls report `Internal`, never `NotRunning`.
fn thread_main(display_name: Option<String>, start: StartCommand, commands: Channel<PanelCommand>) {
    let StartCommand {
        poisoned,
        handshake,
        inner,
        startup,
    } = start;
    let latch = Poisoned::new();
    let ended = guard_always(&latch, || {
        run_thread(
            display_name,
            &poisoned,
            &handshake,
            Arc::clone(&inner),
            commands,
            startup,
        );
    });
    // The panel is gone either way (D2): the handle stops posting, and a
    // start handshake that never completed fails like the GTK side's
    // loop-returned path — the panel never went live. Both are idempotent
    // on the paths that already did the same.
    inner.live.store(false, Ordering::Relaxed);
    report_thread_end(&poisoned, &handshake, &ended);
}

/// Map how the thread body ended onto the start handshake and the shared
/// latch (D5): a clean end is the loop-returned path — `NoDisplay`, the
/// panel never went live (or the handshake was already resolved, which the
/// one-shot report drops). A caught panic is `Internal`, and the shared
/// flag is latched so later host calls report `Internal`, never
/// `NotRunning`.
fn report_thread_end(poisoned: &Poisoned, handshake: &Handshake, ended: &Result<(), Poisoned>) {
    if ended.is_err() {
        poisoned.latch();
        handshake.report(StartOutcome::Internal);
    } else {
        handshake.report(StartOutcome::NoDisplay);
    }
}

/// The thread body once the start command is unpacked: connect, measure
/// the cell from the thread's own font, build the terminal, the renderer
/// and the pty, then bind the session and run the loop. A connection that
/// fails to open (a missing socket, a socket that is not a Wayland server)
/// is `NoDisplay` through the handshake; so is a bind whose required
/// globals are missing (D1), and an unresolvable font is `Internal` (the
/// display may be fine, the environment is not).
fn run_thread(
    display_name: Option<String>,
    poisoned: &Poisoned,
    handshake: &Handshake,
    inner: Arc<Inner>,
    commands: Channel<PanelCommand>,
    startup: Startup,
) {
    let Ok(connection) = connect(display_name) else {
        handshake.report(StartOutcome::NoDisplay);
        return;
    };
    // The registry enumeration does its own round trip, so a socket that
    // does not speak Wayland fails here instead of hanging the loop below.
    let Ok((globals, queue)) =
        wayland_client::globals::registry_queue_init::<PanelState>(&connection)
    else {
        handshake.report(StartOutcome::NoDisplay);
        return;
    };

    // The thread measures its own cell (row 8.1): the font loads before the
    // surfaces are created, and its cell is what the sizing and the surfaces
    // use from the first configure (D3).
    let Ok(setup) = glue::font_start() else {
        // Unresolvable font: not the "no display" case — the environment
        // is what failed, so the internal path reports it.
        handshake.report(StartOutcome::Internal);
        return;
    };
    let Some(cell) = setup.cell() else {
        // Unreachable: the measured pitch is a whole positive pixel count.
        handshake.report(StartOutcome::Internal);
        return;
    };
    let Ok(renderer) = glue::thread_renderer(startup.accent, setup) else {
        // Unreachable: the same positive pitch always yields metrics.
        handshake.report(StartOutcome::Internal);
        return;
    };
    let (terminal, repaint, pty) = glue::byte_path(poisoned.clone(), startup.fd, cell);

    let mut state = PanelState::headless(handshake.clone(), poisoned.clone(), inner, startup, cell);
    state.terminal = Some(Rc::clone(&terminal));
    state.repaint = Rc::clone(&repaint);
    state.render = Some(tween_draw::TweenRender { terminal, renderer });
    // The bind runs under the panel's shared latch: a panic in it latches
    // and reports `Internal` (D5), a missing required global reports
    // `NoDisplay` (D1).
    match guard(poisoned, || state.bind(&globals, &queue.handle())) {
        Ok(Ok(())) => {}
        Ok(Err(BindFailure::NoDisplay)) => {
            handshake.report(StartOutcome::NoDisplay);
            return;
        }
        Ok(Err(BindFailure::Internal)) | Err(_) => {
            handshake.report(StartOutcome::Internal);
            return;
        }
    }
    // The renderer's scale: the session's resolved scale (the preferred
    // scale arrives with the surfaces' events, dispatched only from the
    // loop on, so nothing can have moved it before here).
    state.sync_renderer_scale();

    if run_loop(&connection, &mut state, queue, commands, &pty).is_err() {
        // The wayland source surfaces a closed connection (and any other
        // fatal loop error) as a dispatch error: the compositor is gone, so
        // the panel is dead (D2).
        state.connection_closed();
    }
}

/// The calloop loop (D2): the command channel and the connection's event
/// queue as sources, dispatched until a teardown (or a closed command
/// channel) ends the loop, or a dispatch error — a closed compositor
/// connection — fails it.
fn run_loop(
    connection: &Connection,
    state: &mut PanelState,
    queue: wayland_client::EventQueue<PanelState>,
    commands: Channel<PanelCommand>,
    pty: &Pty,
) -> Result<(), calloop::Error> {
    let mut event_loop = calloop::EventLoop::<PanelState>::try_new()?;
    let handle = event_loop.handle();

    // The connection's event queue from the registry enumeration: the
    // default queue the wayland source dispatches, carrying the globals'
    // events and every surface, output and layer event the session's
    // handlers answer.
    let queue = WaylandSource::new(connection.clone(), queue)
        .insert(handle.clone())
        .map_err(calloop::Error::from)?;

    handle.insert_source(commands, |event, (), state| {
        commands::on_command_event(state, event);
    })?;

    // The startup watchdog ([`watchdog`]), two timers where the GTK side's
    // one repeating `timeout_add_local` folds both duties together: the
    // repeating 200 ms poll reports `Internal` only on a latched shared
    // flag (a panic in the startup path) and never fails a slow start,
    // because start resolution depends on compositor events after the bind
    // (D3) that a busy or cold compositor can delay past any short bound;
    // the one-shot hard deadline at the apply reply's wedge bound fails a
    // start that never completes — a compositor that never sends the panel
    // output's xdg-output logical size — with `NoDisplay`. Both fires tear
    // the panel down the way the GTK side's watchdog closes the glue and
    // quits the loop.
    handle.insert_source(
        Timer::from_duration(watchdog::START_WATCHDOG),
        |_, &mut (), state| watchdog::on_poll_tick(state),
    )?;
    handle.insert_source(
        Timer::from_duration(watchdog::DEADLINE),
        |_, &mut (), state| watchdog::on_deadline_tick(state),
    )?;

    // The pty read source (row 8.1): the fd the thread's `Pty` holds, its
    // bytes fed into the shared terminal. The drain's tween budget reads
    // the tween driver's own flag, and a hangup or a teardown retires the
    // shared fd slot, so later writes are no-ops while the descriptor stays
    // open for the host (D7). A failed attach degrades to no read source —
    // the library never exits over an environment failure (port-to-rust
    // D3), the same degradation the GTK path's `let _ = attach` has; the
    // writes and the winsize pushes keep working, only the reads are gone.
    let tween_flag = state.tween.tween_flag();
    let mut pty_source = state.render.as_ref().and_then(|render| {
        let terminal = Rc::clone(&render.terminal);
        let (fd, fd_slot, pty_poisoned) = pty.read_source();
        // `.ok()`: a failed attach degrades to no read source (port-to-rust
        // D3), the GTK path's `let _ = attach` — the writes and the winsize
        // pushes keep working, only the reads are gone.
        attach_calloop(
            handle.clone(),
            fd,
            fd_slot,
            tween_flag,
            pty_poisoned,
            move |data| terminal.borrow_mut().push_pty_data(data),
        )
        .ok()
    });
    // No terminal (unreachable on a production thread, which builds it
    // before the bind): nothing to feed, and no source to attach.

    loop {
        if state.done {
            // The teardown removed the surfaces; the pty read source goes
            // with them (row 8.1, the GTK teardown's order: surfaces first,
            // then the `Pty::detach` twin), retiring the fd slot. A source
            // that already removed itself on hangup leaves a stale request
            // here, which dropping is harmless.
            if let Some(source) = pty_source.take() {
                source.remove();
            }
            let _ = queue;
            return Ok(());
        }
        if let Err(error) = event_loop.dispatch(None, state) {
            // The wayland source surfaces a closed connection (and any other
            // fatal loop error) as a dispatch error: the compositor is gone,
            // so the panel is dead (D2). The read source goes with the loop;
            // removing it first retires the fd slot like a teardown does.
            if let Some(source) = pty_source.take() {
                source.remove();
            }
            return Err(error);
        }
        // The repaint request the terminal's callbacks latched (row 8.1):
        // the draw step reads and clears it here once the pool buffers and
        // the present path exist (dispatch D4c). Nothing draws yet.
        let _repaint = state.take_repaint_request();
        // The tween's watchdog (row 6.1, [`tween`]): the timer lives only
        // while a tween runs — armed here after each dispatch at the
        // driver's pending deadline, replaced when a retarget moves the
        // deadline, and dropped by its own fire once the tween is gone or
        // expired. A begin inside a dispatch leaves the deadline pending,
        // which the next iteration arms.
        tween::arm_watchdog(&handle, state);
    }
}

/// Open the panel thread's own connection (D1/D2): the socket the
/// environment names when `display_name` is `None`, else the named socket.
/// A name that does not resolve to a socket, or a socket that is not a
/// Wayland server, is the spec's "no display" case.
fn connect(display_name: Option<String>) -> Result<Connection, wayland_client::ConnectError> {
    match display_name {
        None => Connection::connect_to_env(),
        // The named branch never consults `WAYLAND_SOCKET`, so a spawn with
        // an explicit name is testable in any environment.
        Some(name) => {
            let Some(path) = socket_path(&name) else {
                return Err(wayland_client::ConnectError::NoCompositor);
            };
            let Ok(stream) = UnixStream::connect(path) else {
                return Err(wayland_client::ConnectError::NoCompositor);
            };
            Connection::from_socket(stream)
        }
    }
}

/// Resolve a display name to a socket path, the same rules
/// `Connection::connect_to_env` applies to `WAYLAND_DISPLAY`: an absolute
/// name is a path, a relative one sits in `XDG_RUNTIME_DIR`, which must be
/// set and absolute.
fn socket_path(name: &str) -> Option<PathBuf> {
    let name = PathBuf::from(name);
    if name.as_os_str().is_empty() {
        return None;
    }
    if name.is_absolute() {
        return Some(name);
    }
    let mut path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    if !path.is_absolute() {
        return None;
    }
    path.push(name);
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::layout::{Keyboard, Side};
    use crate::panel::handshake::wait_for_start;
    use std::num::NonZeroU16;
    use std::sync::atomic::AtomicBool;

    /// A startup for the tests; the thread does not touch the pty fd until
    /// the terminal and the pty source move onto it, so a placeholder fd is
    /// fine here.
    fn startup() -> Startup {
        Startup {
            fd: -1,
            layout: Layout::new(
                Side::Left,
                NonZeroU16::new(40).expect("test columns"),
                0,
                0,
                0,
                0,
            ),
            keyboard: Keyboard::OnDemand,
            accent: None,
        }
    }

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<Inner> {
        Arc::new(Inner {
            id: 0,
            poisoned: GuardPoisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        })
    }

    /// A spawn whose display name names a socket that does not exist reports
    /// `NoDisplay` through the start handshake (D1: a failed connection is
    /// the spec's "no display" case), and the thread ends on its own.
    #[test]
    fn a_spawn_with_a_missing_socket_is_no_display() {
        let (tx, rx) = mpsc::channel();
        let command = StartCommand {
            poisoned: Poisoned::new(),
            handshake: Handshake::new(tx),
            inner: live_inner(),
            startup: startup(),
        };
        let _thread =
            spawn_panel_thread(Some("pinwin-test-no-such-socket"), command).expect("the thread");
        assert_eq!(wait_for_start(&rx), Err(PinwinError::NoDisplay));
    }

    /// A caught panic in the thread body reports `Internal` through the
    /// start handshake and latches the shared flag (D5): later host calls
    /// must read `Internal`, never `NotRunning`.
    #[test]
    fn a_caught_thread_panic_reports_internal_and_latches_the_shared_flag() {
        let (tx, rx) = mpsc::channel();
        let handshake = Handshake::new(tx);
        let shared = Poisoned::new();
        report_thread_end(&shared, &handshake, &Err(Poisoned::latched()));
        assert!(shared.is_poisoned(), "the shared latch is set");
        assert_eq!(wait_for_start(&rx), Err(PinwinError::Internal));
    }

    /// A clean thread end reports `NoDisplay` through the start handshake
    /// and leaves the shared flag alone: a connection failure is the spec's
    /// "no display" case, not a panic.
    #[test]
    fn a_clean_thread_end_reports_no_display_without_latching() {
        let (tx, rx) = mpsc::channel();
        let handshake = Handshake::new(tx);
        let shared = Poisoned::new();
        report_thread_end(&shared, &handshake, &Ok(()));
        assert!(!shared.is_poisoned(), "no panic, no latch");
        assert_eq!(wait_for_start(&rx), Err(PinwinError::NoDisplay));
    }

    /// The display-name resolution follows `connect_to_env`'s rules: a
    /// relative name sits in `XDG_RUNTIME_DIR`, an absolute name is the path
    /// itself, and an unset or relative `XDG_RUNTIME_DIR` fails a relative
    /// name.
    #[test]
    fn socket_path_follows_the_wayland_rules() {
        // An absolute name is used as the path itself, whatever the
        // environment says.
        let path = socket_path("/run/user/1000/pinwin-test").expect("absolute");
        assert_eq!(path, PathBuf::from("/run/user/1000/pinwin-test"));

        // A relative name resolves inside XDG_RUNTIME_DIR when it is set and
        // absolute. The variable is process state, so the test restores it.
        let runtime = std::env::var_os("XDG_RUNTIME_DIR");
        // SAFETY: env mutation races with concurrent readers of the same
        // variable; nextest runs each test in its own process.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000") };
        let path = socket_path("pinwin-test").expect("relative with runtime dir");
        assert_eq!(path, PathBuf::from("/run/user/1000/pinwin-test"));
        // SAFETY: restoring the variable this test read above.
        unsafe {
            match runtime {
                Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        };

        // An empty name is never a socket.
        assert!(socket_path("").is_none());
    }
}
