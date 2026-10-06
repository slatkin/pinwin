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
//! Until row 8.1 switches `Panel::start` over, nothing in the crate calls
//! [`spawn_panel_thread`]: the entry point is `pub` so it stays reachable (a
//! `pub(crate)` entry with no caller is dead code under `-D warnings`, and
//! no lint suppression is permitted). Rows 3.1–3.5 grow the layer surfaces
//! into the command handlers and add the startup payload to
//! [`StartCommand`].
//!
//! Panics never cross back into calloop or the compositor (D5): the whole
//! thread body and every callback the loop runs go through the shared
//! [`crate::guard`] helpers, latching the panel's one shared poisoned flag —
//! the same rule [`super::gtk_side`] applies to its GTK closures.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use calloop::channel::{self, Channel, Event};
use calloop_wayland_source::WaylandSource;
use wayland_client::Connection;

use crate::guard::{Poisoned, guard, guard_always};
use crate::layout::Layout;
use crate::surfaces::PublishOutcome;

use super::Inner;
use super::PinwinError;
use super::handshake::{
    APPLY_WAIT, FocusOutcome, Handshake, StartOutcome, wait_for_apply, wait_for_focus,
};

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
    /// Request keyboard focus, answered with a [`FocusOutcome`].
    Focus {
        /// The bounded reply the host waits on.
        reply: mpsc::SyncSender<FocusOutcome>,
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
}

impl std::fmt::Debug for StartCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The handshake is a one-shot channel pair, not printable state.
        f.debug_struct("StartCommand")
            .field("poisoned", &self.poisoned)
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
    /// decides whether anything is posted at all.
    ///
    /// # Errors
    /// `NotRunning` on a dead panel, `Internal` on a caught panic or a
    /// wedged or ended thread.
    pub fn request_focus(&self) -> Result<(), PinwinError> {
        match guard(&self.inner.poisoned, || self.post_focus()) {
            Ok(result) => result,
            Err(_) => Err(PinwinError::Internal),
        }
    }

    /// The unguarded body of [`PanelThread::request_focus`].
    fn post_focus(&self) -> Result<(), PinwinError> {
        if !self.inner.live.load(Ordering::Relaxed) {
            return Err(PinwinError::NotRunning);
        }
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let _ = self.commands.send(PanelCommand::Focus { reply: reply_tx });
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
    } = start;
    let latch = Poisoned::new();
    let ended = guard_always(&latch, || {
        run_thread(
            display_name,
            &poisoned,
            &handshake,
            Arc::clone(&inner),
            commands,
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

/// The thread body once the start command is unpacked: connect, then run the
/// loop. A connection that fails to open (a missing socket, a socket that is
/// not a Wayland server) is `NoDisplay` through the handshake.
fn run_thread(
    display_name: Option<String>,
    poisoned: &Poisoned,
    handshake: &Handshake,
    inner: Arc<Inner>,
    commands: Channel<PanelCommand>,
) {
    let Ok(connection) = connect(display_name) else {
        handshake.report(StartOutcome::NoDisplay);
        return;
    };
    // The first round trip proves the socket speaks Wayland; without it a
    // dead endpoint would pass the connect and hang the loop below.
    if connection.roundtrip().is_err() {
        handshake.report(StartOutcome::NoDisplay);
        return;
    }

    let mut state = PanelState {
        poisoned: poisoned.clone(),
        handshake: handshake.clone(),
        done: false,
        inner,
    };
    if run_loop(&connection, &mut state, commands).is_err() {
        // The wayland source surfaces a closed connection (and any other
        // fatal loop error) as a dispatch error: the compositor is gone, so
        // the panel is dead (D2).
        state.connection_closed();
    }
}

/// The live panel's state on the thread side: the one shared D5 latch, the
/// start handshake, the loop-end flag and the handle state the dead mapping
/// writes to.
struct PanelState {
    poisoned: Poisoned,
    handshake: Handshake,
    /// Set by the teardown command (or a closed command channel): end the
    /// loop and let the thread return.
    done: bool,
    inner: Arc<Inner>,
}

impl PanelState {
    /// A closed compositor connection maps the panel onto the dead state
    /// (D2): the handle stops posting (`NotRunning` without blocking), and a
    /// still-pending start handshake fails — the panel never went live, the
    /// same mapping the GTK side's loop-returned path reports.
    fn connection_closed(&mut self) {
        self.inner.live.store(false, Ordering::Relaxed);
        self.handshake.report(StartOutcome::NoDisplay);
    }
}

/// The calloop loop (D2): the command channel and the connection's event
/// queue as sources, dispatched until a teardown (or a closed command
/// channel) ends the loop, or a dispatch error — a closed compositor
/// connection — fails it.
fn run_loop(
    connection: &Connection,
    state: &mut PanelState,
    commands: Channel<PanelCommand>,
) -> Result<(), calloop::Error> {
    let mut event_loop = calloop::EventLoop::<PanelState>::try_new()?;
    let handle = event_loop.handle();

    // The connection's event queue: the default queue the wayland source
    // dispatches. No protocol objects are bound to it yet (rows 3.1+ bind
    // the globals here); wl_display's own events need no handler.
    let queue = connection.new_event_queue::<PanelState>();
    // The wayland source reads and dispatches the connection: a closed
    // compositor connection surfaces as an `Err` from the `dispatch` below
    // (D2), and the source flushes outgoing requests before every sleep.
    WaylandSource::new(connection.clone(), queue)
        .insert(handle.clone())
        .map_err(calloop::Error::from)?;

    handle.insert_source(commands, |event, (), state| {
        on_command_event(state, event);
    })?;

    loop {
        if state.done {
            return Ok(());
        }
        // `None`: park until the command channel or the connection wakes the
        // loop, like the parked GTK thread's main context.
        event_loop.dispatch(None, state)?;
    }
}

/// The command channel's callback (D5 boundary): a callback is a boundary
/// like a GTK closure, so ordinary commands run under the shared [`guard`]
/// and a latched panel runs no more glue code. The stop relays — a teardown
/// and a closed command channel — must run even on a latched flag (D5):
/// skipping them would drop the teardown's reply (the host's drop waits the
/// whole reply bound) and never end the loop, leaking the thread and its
/// Wayland connection.
fn on_command_event(state: &mut PanelState, event: Event<PanelCommand>) {
    // The latch is cloned first: the guard's borrow and the command
    // handling must not alias the same `PanelState`.
    let poisoned = state.poisoned.clone();
    match event {
        // The teardown is a stop relay, not ordinary glue (D5):
        // `guard_always` runs it even on a latched flag.
        Event::Msg(PanelCommand::Teardown { reply }) => {
            let _ = guard_always(&poisoned, || {
                handle_command(state, PanelCommand::Teardown { reply });
            });
        }
        Event::Msg(command) => {
            let _ = guard(&poisoned, || handle_command(state, command));
        }
        // Every sender is gone (the host dropped the handle without a
        // teardown): end the thread the same way a teardown does. A plain
        // store cannot panic, so it needs no guard at all.
        Event::Closed => state.done = true,
    }
}

/// One posted command (D2): the guarded arms mirror the GTK side's
/// dispatched glue, each answering through the bounded reply the command
/// carried.
fn handle_command(state: &mut PanelState, command: PanelCommand) {
    match command {
        PanelCommand::Apply {
            layout,
            duration_ms,
            reply,
        } => {
            let _ = reply.send(apply_without_surfaces(layout, duration_ms));
        }
        PanelCommand::Focus { reply } => {
            let _ = reply.send(focus_without_surfaces());
        }
        PanelCommand::Teardown { reply } => {
            // A stop relay, not ordinary glue (D5): the reply and the loop's
            // end must happen even on a latched flag, or a dead panel
            // strands the host's drop for the whole reply bound.
            state.done = true;
            let scratch = Poisoned::new();
            let _ = guard_always(&scratch, || {
                let _ = reply.send(());
            });
        }
    }
}

/// The apply answer while the thread has no layer surfaces yet (rows
/// 3.1–3.5 replace this with the guarded publish of the surfaces, the shape
/// of `gtk_side::dispatch_apply`): a connected thread has no live panel, so
/// the answer is the same not-live lifecycle state the GTK side reports for
/// a panel without metrics, through the same bounded reply.
fn apply_without_surfaces(_layout: Layout, _duration_ms: u32) -> PublishOutcome {
    PublishOutcome::NotLive
}

/// The focus answer while the thread has no layer surfaces yet (rows 3.4 and
/// 7.2 replace this with the xdg-activation request): the same not-live
/// lifecycle state as [`apply_without_surfaces`].
fn focus_without_surfaces() -> FocusOutcome {
    FocusOutcome::NotLive
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
    use super::super::gtk_side::Startup;
    use super::super::handshake::wait_for_start;
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::layout::{Keyboard, Side};
    use std::num::NonZeroU16;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    /// A startup for the tests; the thread does not touch the pty fd until
    /// the surfaces exist, so a placeholder fd is fine here.
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
    /// tests. (The startup payload arrives with the surfaces in rows 3.1+.)
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
        };
        let _thread =
            spawn_panel_thread(Some("pinwin-test-no-such-socket"), command).expect("the thread");
        assert_eq!(wait_for_start(&rx), Err(PinwinError::NoDisplay));
    }

    /// A closed compositor connection maps the panel onto the dead state
    /// (D2): the handle stops posting, and a still-pending start handshake
    /// fails with `NoDisplay`.
    #[test]
    fn a_closed_connection_marks_the_panel_dead() {
        let (tx, rx) = mpsc::channel();
        let mut state = PanelState {
            poisoned: Poisoned::new(),
            handshake: Handshake::new(tx),
            done: false,
            inner: live_inner(),
        };
        state.connection_closed();
        assert!(
            !state.inner.live.load(Ordering::Relaxed),
            "the panel is dead"
        );
        assert_eq!(
            wait_for_start(&rx),
            Err(PinwinError::NoDisplay),
            "a still-pending start fails"
        );
    }

    /// A closed connection after the start handshake was resolved marks the
    /// panel dead and adds no second report: the first one is the only one.
    #[test]
    fn a_closed_connection_after_a_resolved_start_reports_nothing_new() {
        let (tx, rx) = mpsc::channel();
        let handshake = Handshake::new(tx);
        handshake.report(StartOutcome::Started);
        let mut state = PanelState {
            poisoned: Poisoned::new(),
            handshake,
            done: false,
            inner: live_inner(),
        };
        state.connection_closed();
        assert!(
            !state.inner.live.load(Ordering::Relaxed),
            "the panel is dead"
        );
        assert_eq!(
            rx.try_recv().expect("the first report"),
            StartOutcome::Started
        );
        assert!(
            rx.try_recv().is_err(),
            "the closed connection adds no second report"
        );
    }

    /// An apply posted to a thread without surfaces is answered `NotLive`
    /// through the same bounded reply (`NotRunning`), never a hang.
    #[test]
    fn an_apply_without_surfaces_is_not_running() {
        let (tx, _rx) = mpsc::channel();
        let mut state = PanelState {
            poisoned: Poisoned::new(),
            handshake: Handshake::new(tx),
            done: false,
            inner: live_inner(),
        };
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        handle_command(
            &mut state,
            PanelCommand::Apply {
                layout: startup().layout,
                duration_ms: 0,
                reply: reply_tx,
            },
        );
        assert_eq!(
            wait_for_apply(&reply_rx, Duration::from_secs(1)),
            Err(PinwinError::NotRunning)
        );
        assert!(!state.done, "an apply does not end the thread");
    }

    /// A focus request posted to a thread without surfaces is answered
    /// `NotLive` through the same bounded reply (`NotRunning`).
    #[test]
    fn a_focus_request_without_surfaces_is_not_running() {
        let (tx, _rx) = mpsc::channel();
        let mut state = PanelState {
            poisoned: Poisoned::new(),
            handshake: Handshake::new(tx),
            done: false,
            inner: live_inner(),
        };
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        handle_command(&mut state, PanelCommand::Focus { reply: reply_tx });
        assert_eq!(
            wait_for_focus(&reply_rx, Duration::from_secs(1)),
            Err(PinwinError::NotRunning)
        );
    }

    /// A teardown command ends the loop state and answers through the
    /// bounded reply, even on a latched flag (D5's stop relay): the host's
    /// drop must not strand. The test drives the real callback path
    /// ([`on_command_event`], outer guard included) — a `handle_command`
    /// call alone would skip the short-circuiting outer guard that hid this
    /// relay once.
    #[test]
    fn a_teardown_command_ends_the_thread_state() {
        let (tx, _rx) = mpsc::channel();
        let mut state = PanelState {
            poisoned: Poisoned::latched(),
            handshake: Handshake::new(tx),
            done: false,
            inner: live_inner(),
        };
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        on_command_event(
            &mut state,
            Event::Msg(PanelCommand::Teardown { reply: reply_tx }),
        );
        assert!(state.done, "the loop ends");
        assert_eq!(
            reply_rx.recv_timeout(Duration::from_secs(1)),
            Ok(()),
            "the teardown replies even on a latched flag"
        );
    }

    /// A closed command channel ends the loop even on a latched flag (D5's
    /// stop relay): a swallowed `Closed` would leak the thread and its
    /// Wayland connection after a plain handle drop.
    #[test]
    fn a_closed_command_channel_ends_the_thread_even_when_latched() {
        let (tx, _rx) = mpsc::channel();
        let mut state = PanelState {
            poisoned: Poisoned::latched(),
            handshake: Handshake::new(tx),
            done: false,
            inner: live_inner(),
        };
        on_command_event(&mut state, Event::Closed);
        assert!(state.done, "the closed channel ends the loop");
    }

    /// An ordinary command stays under the short-circuiting guard (D5): on
    /// a latched flag it never runs and its reply channel closes empty,
    /// while the loop keeps going.
    #[test]
    fn a_latched_flag_drops_an_ordinary_command() {
        let (tx, _rx) = mpsc::channel();
        let mut state = PanelState {
            poisoned: Poisoned::latched(),
            handshake: Handshake::new(tx),
            done: false,
            inner: live_inner(),
        };
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        on_command_event(
            &mut state,
            Event::Msg(PanelCommand::Apply {
                layout: startup().layout,
                duration_ms: 0,
                reply: reply_tx,
            }),
        );
        assert!(!state.done, "an ordinary command does not end the thread");
        assert_eq!(
            reply_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Disconnected),
            "the guarded command never ran"
        );
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
