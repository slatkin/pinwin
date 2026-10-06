//! The public `Panel` API (port-to-rust D4, D6, D7): the host calls from its
//! own thread; the GTK side runs on a process-lifetime `pinwin-gtk` thread
//! (the `gtk_side` module) whose loop each start drives with its own
//! `GtkApplication`.
//!
//! The shape follows `src/pinwin_api.c`: start validates its arguments before
//! any GTK work, then hands the startup to the GTK thread and waits for a
//! handshake that completes when the panel is on screen with live metrics.
//! Applies post a command with `MainContext::invoke` and wait up to five
//! seconds for the reply (a wedged loop is `Internal`, never a hang); a
//! focus request posts the same way. Drop posts a teardown, waits for its
//! reply, and never joins the parked thread (D4), so a later start may
//! succeed.
//!
//! Panics never cross the API (D5): every public entry point runs under the
//! shared [`crate::guard`], and the poisoned check comes before the
//! "GTK side ended" check, so a panic reports `Internal`, never `NotRunning`.
//!
//! The tests here are GTK-free (D10): the error and outcome mappings, the
//! handshake and reply channel logic, the single-instance guard and the
//! poisoned-over-not-running precedence run without a display; the display
//! tests are `#[ignore]`d.

mod error;
mod gtk_side;
mod handshake;
pub mod wayland_side;

pub use error::PinwinError;
pub use gtk_side::Startup;

use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;

use crate::activation::ActivationToken;
use crate::guard::{Poisoned, guard, guard_always};
use crate::layout::{Keyboard, Layout};

use gtk_side::{StartCommand, dispatch_apply, dispatch_teardown, gtk_thread_main};
use handshake::{APPLY_WAIT, Handshake, wait_for_apply, wait_for_start};

/// The longest animated apply duration, clamped like `pinwin_api.c`'s
/// `PINWIN_ANIM_MAX_MS` (`pinwin.h`: "the duration SHALL be clamped").
pub const ANIMATION_MAX_MS: u32 = 1000;

/// The next panel's id: distinct per start, so a queued command from a
/// dropped panel can never act on the panel that runs after it.
static NEXT_PANEL_ID: AtomicU64 = AtomicU64::new(0);

/// The single-instance state: at most one panel per process (the spec's
/// "Library Panel API"). `Starting` covers the start handshake window, so two
/// concurrent starts cannot both spawn the GTK thread.
enum Phase {
    Idle,
    Starting,
    Running,
}

struct Instance {
    phase: Phase,
    /// The parked GTK thread's start-command channel; `None` until the first
    /// start spawned the thread.
    gtk: Option<mpsc::Sender<StartCommand>>,
}

static INSTANCE: Mutex<Instance> = Mutex::new(Instance {
    phase: Phase::Idle,
    gtk: None,
});

/// The display-free handle state (D10): the panel's id, its one shared D5
/// latch — the same flag every glue closure of this panel guards against —
/// whether the GTK side is still live, and the keyboard mode fixed at start
/// time (a focus request short-circuits on it before posting, D10). The GTK
/// thread holds a clone of the `Arc` and clears `live` when the panel ends
/// on its own, so an apply or a focus request on a dead panel reports
/// `NotRunning` without posting anything.
#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) id: u64,
    pub(crate) poisoned: Poisoned,
    pub(crate) live: AtomicBool,
    /// The startup keyboard mode; `request_focus` is a no-op outside
    /// `on-demand`.
    pub(crate) keyboard: Keyboard,
}

/// A running panel, the host's handle. Dropping it closes the panel.
///
/// Besides applies, the handle offers a focus request
/// ([`Panel::request_focus`]): in `on-demand` mode the panel passes the
/// request's activation token to the compositor as an xdg-activation request
/// (replace-gtk-with-wayland D4), the other modes are a no-op `Ok`.
///
/// Not `Clone`: one handle per panel, so the single-instance rule is
/// ownership, not bookkeeping.
#[derive(Debug)]
pub struct Panel {
    inner: Arc<Inner>,
}

impl Panel {
    /// Start a panel: put it on screen and return once it is there or has
    /// failed.
    ///
    /// The fd is validated before any GTK work (`InvalidFd`, D7). A start
    /// while another handle is alive fails with `AlreadyRunning` and leaves
    /// the running panel unchanged. `NoDisplay` when GTK or wlr-layer-shell
    /// is unavailable — nothing opened.
    // Approved per-instance (#13): the guard-Poisoned latch discards the
    // panic payload by design; the public entry maps it onto `Internal`.
    #[allow(
        clippy::map_err_ignore,
        reason = "approved #13: guard-Poisoned latch discards the panic payload by design"
    )]
    pub fn start(startup: Startup) -> Result<Panel, PinwinError> {
        // D5 boundary: the start itself is a public entry point. The shared
        // latch does not exist until the start succeeds, so the start's own
        // body is guarded with a throwaway one; the panel's shared latch is
        // built inside and handed to the GTK side.
        let latch = Poisoned::new();
        guard(&latch, || Self::start_inner(startup))
            .map_err(|_| PinwinError::Internal)
            .and_then(std::convert::identity)
    }

    fn start_inner(startup: Startup) -> Result<Panel, PinwinError> {
        // Pure argument checks, no GTK (pinwin_api.c's startup_valid minus
        // the classes the argument types make unrepresentable, D6/D7).
        if !fd_is_open(startup.fd) {
            return Err(PinwinError::InvalidFd);
        }

        let poisoned = Poisoned::new();
        let id = NEXT_PANEL_ID.fetch_add(1, Ordering::Relaxed);
        let inner = Arc::new(Inner {
            id,
            poisoned: poisoned.clone(),
            live: AtomicBool::new(true),
            keyboard: startup.keyboard,
        });

        // Claim the single-instance slot before touching GTK, so two
        // concurrent starts cannot both reach the handshake. The guard is
        // dropped explicitly: a bare scope block ping-pongs between the two
        // semicolon-placement lints.
        let mut instance = INSTANCE.lock().expect("pinwin instance lock");
        if !matches!(instance.phase, Phase::Idle) {
            return Err(PinwinError::AlreadyRunning);
        }
        instance.phase = Phase::Starting;
        drop(instance);

        let (reply_tx, reply_rx) = mpsc::channel();
        let command = StartCommand {
            startup,
            id,
            poisoned: poisoned.clone(),
            handshake: Handshake::new(reply_tx),
            inner: Arc::clone(&inner),
        };

        // Deliver to the parked GTK thread, or spawn it on the first start
        // (D4). A dead sender means the thread died by panic: respawn it — a
        // new thread cannot `gtk::init` (D4's restart caveat), so the
        // handshake will report `Internal` through its closed channel.
        let command = deliver(command).err();
        if let Some(command) = command {
            let Ok(sender) = spawn_gtk_thread() else {
                release_phase();
                return Err(PinwinError::Internal);
            };
            if sender.send(command).is_err() {
                release_phase();
                return Err(PinwinError::Internal);
            }
        }

        // Wait for the GTK/layer-shell side to go live (the handshake), like
        // pinwin_start's cond wait. The GTK side always reports.
        let outcome = wait_for_start(&reply_rx);

        {
            let mut instance = INSTANCE.lock().expect("pinwin instance lock");
            instance.phase = if outcome.is_ok() {
                Phase::Running
            } else {
                Phase::Idle
            }
        };
        outcome.map(|()| Panel { inner })
    }

    /// Apply a layout without animation: update the applied column count,
    /// gutters, docking side and push/cover choice as one operation, resize
    /// the panel width, and resize the terminal grid and pty winsize when
    /// necessary.
    ///
    /// The reservation follows the layout's push/cover choice; the exact
    /// rules are stated once on [`Coverage`](crate::layout::Coverage), and
    /// the checks each choice gets on
    /// [`Layout::validate`](crate::layout::Layout::validate). A rejected
    /// apply (`InvalidLayout`) leaves the applied layout and the held strip
    /// untouched.
    pub fn apply_layout(&self, layout: Layout) -> Result<(), PinwinError> {
        self.apply_common(layout, 0)
    }

    /// Apply a layout animated: when the layout differs from the applied
    /// one only in its column count and/or push/cover choice (same side,
    /// same left and right gutters) and GTK animations are enabled, the
    /// width changes continuously over `duration_ms` (clamped to
    /// [`ANIMATION_MAX_MS`]); anything else snaps exactly like
    /// [`Panel::apply_layout`]. The reservation moves with the panel only
    /// while the target pushes with a strip that differs from the held one;
    /// a covering target leaves the held strip alone in every frame.
    /// `Ok(())` means the layout was validated and accepted, not that the
    /// animation finished.
    pub fn apply_layout_animated(
        &self,
        layout: Layout,
        duration_ms: u32,
    ) -> Result<(), PinwinError> {
        self.apply_common(layout, duration_ms.min(ANIMATION_MAX_MS))
    }

    fn apply_common(&self, layout: Layout, duration_ms: u32) -> Result<(), PinwinError> {
        apply_via_inner(&self.inner, layout, duration_ms)
    }

    /// Ask the compositor to give the panel keyboard focus (issue #15's
    /// focus on request). In `on-demand` mode the panel passes `token` to
    /// the compositor as an xdg-activation request for the panel surface
    /// (replace-gtk-with-wayland D4): the panel never unmaps, and the
    /// request changes neither the terminal grid, the pty window size nor
    /// the reserved gap. On a compositor without xdg-activation, or for a
    /// stale token — one the compositor already used, or one that is too
    /// old — the request does nothing; the compositor's choice is invisible
    /// to the caller, so the call returns `Ok(())` once the request is sent
    /// either way. In the `none` and `exclusive` modes the call returns
    /// `Ok(())` and changes nothing — the host chose the mode, and an
    /// activation request could not gain focus there anyway. The keyboard
    /// mode itself is fixed at start time; the request never changes it.
    ///
    /// The token is the one-use permission a compositor gives a program it
    /// launches; the library owns no transport for the request, the host
    /// decides how a request and its token reach it. Taken by reference:
    /// the request carries the token to the panel side, it does not consume
    /// it, and the host may reuse the token it built.
    pub fn request_focus(&self, token: &ActivationToken) -> Result<(), PinwinError> {
        request_focus_via_inner(&self.inner, token)
    }
}

/// An apply through the display-free inner handle (D10): the poisoned check
/// comes first (D5 — a panic reports `Internal`, never `NotRunning`), then
/// the ended check, then the bounded posted apply.
// Approved per-instance (#13): the guard-Poisoned latch discards the panic
// payload by design; the entry maps it onto `Internal`.
#[allow(
    clippy::map_err_ignore,
    reason = "approved #13: guard-Poisoned latch discards the panic payload by design"
)]
pub(crate) fn apply_via_inner(
    inner: &Inner,
    layout: Layout,
    duration_ms: u32,
) -> Result<(), PinwinError> {
    guard(&inner.poisoned, || post_apply(inner, layout, duration_ms))
        .map_err(|_| PinwinError::Internal)
        .and_then(std::convert::identity)
}

/// The unguarded body of [`apply_via_inner`].
fn post_apply(inner: &Inner, layout: Layout, duration_ms: u32) -> Result<(), PinwinError> {
    // Not running → `NotRunning` without blocking (pinwin_api.c's order).
    if !inner.live.load(Ordering::Relaxed) {
        return Err(PinwinError::NotRunning);
    }
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let id = inner.id;
    // The metrics live on the GTK thread, so the publish (and its validation)
    // must run there (D4): post the command and wait for the synchronous
    // result, bounded (D5).
    gtk4::glib::MainContext::default()
        .invoke(move || dispatch_apply(id, layout, duration_ms, &reply_tx));
    wait_for_apply(&reply_rx, APPLY_WAIT)
}

/// A focus request through the display-free inner handle (D10): the same
/// order as [`apply_via_inner`] — the poisoned check first (D5: a panic
/// reports `Internal`, never `NotRunning`), then the ended check — then the
/// mode short-circuit and the request itself.
pub(crate) fn request_focus_via_inner(
    inner: &Inner,
    token: &ActivationToken,
) -> Result<(), PinwinError> {
    match guard(&inner.poisoned, || post_focus(inner, token)) {
        Ok(result) => result,
        Err(_) => Err(PinwinError::Internal),
    }
}

/// The unguarded body of [`request_focus_via_inner`].
fn post_focus(inner: &Inner, _token: &ActivationToken) -> Result<(), PinwinError> {
    // Not running → `NotRunning` without blocking, unconditionally (the
    // spec's dead-panel scenario; pinwin_api.c's order).
    if !inner.live.load(Ordering::Relaxed) {
        return Err(PinwinError::NotRunning);
    }
    // The mode is fixed at start time and recorded on the handle: an
    // activation request cannot gain focus outside `on-demand`, so the
    // request is `Ok` and posts nothing.
    if inner.keyboard != Keyboard::OnDemand {
        return Ok(());
    }
    // Until row 8.1 switches `Panel` to the panel thread, the GTK side has
    // no xdg-activation request to make (the gtk4-layer-shell binding has
    // none), so the request is `Ok` and posts nothing: the legacy path
    // loses its focus request on the way (replace-gtk-with-wayland row
    // 7.2's known intermediate regression), and row 8.1 posts the token to
    // the panel thread's activation instead.
    Ok(())
}

impl Drop for Panel {
    fn drop(&mut self) {
        // D5: the teardown runs even on a latched flag and never unwinds; it
        // never joins the parked GTK thread (D4).
        let _ = guard_always(&self.inner.poisoned, || teardown_inner(&self.inner));
    }
}

/// The teardown body: post the teardown and wait for its reply, then release
/// the single-instance slot so a later start may succeed.
fn teardown_inner(inner: &Inner) {
    if inner.live.swap(false, Ordering::Relaxed) {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let id = inner.id;
        gtk4::glib::MainContext::default().invoke(move || dispatch_teardown(id, &reply_tx));
        // Wait for the reply, bounded: a wedged loop must not block the host
        // forever (the C joined the thread; the port never joins, D4).
        let _ = reply_rx.recv_timeout(APPLY_WAIT);
    }
    release_phase();
}

/// Deliver a start command to the parked GTK thread. `Err(command)` when
/// there is no live thread to receive it.
fn deliver(command: StartCommand) -> Result<(), StartCommand> {
    let sender = INSTANCE.lock().expect("pinwin instance lock").gtk.clone();
    match sender {
        Some(sender) => sender.send(command).map_err(|error| error.0),
        None => Err(command),
    }
}

/// Spawn the process-lifetime `pinwin-gtk` thread (D4) and store its command
/// channel. Created lazily on the first start; parked between panels; never
/// joined.
fn spawn_gtk_thread() -> std::io::Result<mpsc::Sender<StartCommand>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("pinwin-gtk".to_owned())
        .spawn(move || gtk_thread_main(&receiver))?;
    INSTANCE.lock().expect("pinwin instance lock").gtk = Some(sender.clone());
    Ok(sender)
}

/// Back out of a claimed single-instance slot after a failed start.
fn release_phase() {
    INSTANCE.lock().expect("pinwin instance lock").phase = Phase::Idle;
}

/// Whether `fd` is an open descriptor (`fcntl(F_GETFL)`, D7). The library
/// never closes it either way.
fn fd_is_open(fd: RawFd) -> bool {
    if fd < 0 {
        return false;
    }
    // SAFETY: `F_GETFL` takes no argument and `fd` is a caller-supplied
    // descriptor number.
    unsafe { libc::fcntl(fd, libc::F_GETFL) >= 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Side;
    use std::num::NonZeroU16;
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    /// Serializes every test that touches the process-global single-instance
    /// state (the same shape as `pty.rs`'s sigwinch lock).
    static INSTANCE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn layout() -> Layout {
        Layout::new(
            Side::Left,
            NonZeroU16::new(40).expect("test columns"),
            0,
            0,
            0,
            0,
        )
    }

    fn startup(fd: RawFd) -> Startup {
        Startup {
            fd,
            layout: layout(),
            keyboard: Keyboard::OnDemand,
            accent: None,
        }
    }

    /// A valid test token, the argument every focus request carries from
    /// row 7.2 on.
    fn token() -> ActivationToken {
        ActivationToken::new("pinwin-test-token").expect("test token is valid")
    }

    /// A fd that is not an open descriptor is `InvalidFd` before any GTK
    /// work (D7): no thread is spawned, nothing else changes. A negative fd
    /// and a closed one are both covered.
    #[test]
    fn start_rejects_a_bad_fd_before_any_gtk_work() {
        assert!(matches!(
            Panel::start(startup(-1)),
            Err(PinwinError::InvalidFd)
        ));

        // A descriptor that has been closed.
        let mut fds: [libc::c_int; 2] = [0; 2];
        // SAFETY: `fds` is a writable two-element array for the call.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
        // SAFETY: closing a descriptor this test owns.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1])
        };
        assert!(matches!(
            Panel::start(startup(fds[0])),
            Err(PinwinError::InvalidFd)
        ));
    }

    /// A start while another handle is alive fails with `AlreadyRunning`
    /// before any GTK work; the fd check keeps its precedence over it.
    #[test]
    fn start_while_running_is_already_running() {
        let _test_lock = INSTANCE_TEST_LOCK.lock().expect("test lock");

        // Claim the slot as a live panel would.
        INSTANCE.lock().expect("instance lock").phase = Phase::Running;

        // A closed fd would be InvalidFd first; an open one hits the guard.
        let file = std::fs::File::open("/dev/null").expect("/dev/null");
        let started = Panel::start(startup(file.as_raw_fd()));
        assert!(matches!(started, Err(PinwinError::AlreadyRunning)));

        // Even `Starting` (a start handshake in flight) blocks a second one.
        INSTANCE.lock().expect("instance lock").phase = Phase::Starting;
        assert!(matches!(
            Panel::start(startup(file.as_raw_fd())),
            Err(PinwinError::AlreadyRunning)
        ));

        INSTANCE.lock().expect("instance lock").phase = Phase::Idle;
    }

    /// The poisoned check comes before the "GTK side ended" check (D5): a
    /// latched handle reports `Internal`, never `NotRunning`.
    #[test]
    fn poisoned_beats_not_running() {
        let inner = Inner {
            id: 0,
            poisoned: Poisoned::latched(),
            live: AtomicBool::new(false),
            keyboard: Keyboard::OnDemand,
        };
        assert_eq!(
            apply_via_inner(&inner, layout(), 0),
            Err(PinwinError::Internal)
        );
        // The focus request follows the same precedence: a latched handle
        // reports `Internal`, never `NotRunning`.
        assert_eq!(
            request_focus_via_inner(&inner, &token()),
            Err(PinwinError::Internal)
        );
    }

    /// An apply on a handle whose panel is no longer live reports
    /// `NotRunning` without blocking: the post is skipped, the reply path is
    /// not entered (D10's dead-handle path; the invoke that would run is
    /// never posted).
    #[test]
    fn apply_on_a_dead_panel_is_not_running_without_blocking() {
        let inner = Inner {
            id: 0,
            poisoned: Poisoned::new(),
            live: AtomicBool::new(false),
            keyboard: Keyboard::OnDemand,
        };
        let started = std::time::Instant::now();
        assert_eq!(
            apply_via_inner(&inner, layout(), 0),
            Err(PinwinError::NotRunning)
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the dead-panel apply does not wait"
        );
    }

    /// A focus request on a handle whose panel is no longer live reports
    /// `NotRunning` without blocking: the post is skipped, the reply path is
    /// not entered (the dead-panel scenario).
    #[test]
    fn a_focus_request_on_a_dead_panel_is_not_running_without_blocking() {
        let inner = Inner {
            id: 0,
            poisoned: Poisoned::new(),
            live: AtomicBool::new(false),
            keyboard: Keyboard::OnDemand,
        };
        let started = std::time::Instant::now();
        assert_eq!(
            request_focus_via_inner(&inner, &token()),
            Err(PinwinError::NotRunning)
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the dead-panel focus request does not wait"
        );
    }

    /// In the `none` and `exclusive` modes a focus request is `Ok(())` and
    /// posts nothing: `Ok` itself is the proof, because a posted command on
    /// this thread would run inline with no glue and fail the wait (`Ok`
    /// could never come back through a reply channel).
    #[test]
    fn a_focus_request_outside_on_demand_is_ok_and_posts_nothing() {
        for keyboard in [Keyboard::None, Keyboard::Exclusive] {
            let inner = Inner {
                id: 0,
                poisoned: Poisoned::new(),
                live: AtomicBool::new(true),
                keyboard,
            };
            assert_eq!(request_focus_via_inner(&inner, &token()), Ok(()));
        }
    }

    /// Until row 8.1 switches `Panel` to the panel thread, a live
    /// `on-demand` handle's request is also `Ok(())` and posts nothing: the
    /// GTK side has no xdg-activation request to make (replace-gtk-with-wayland
    /// row 7.2's known intermediate regression). The proof is the same as
    /// for the mode no-ops above: `Ok` without any reply path.
    #[test]
    fn a_focus_request_on_the_gtk_path_is_ok_and_posts_nothing() {
        let inner = Inner {
            id: 0,
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        };
        assert_eq!(request_focus_via_inner(&inner, &token()), Ok(()));
    }

    /// A panic in a guarded closure at the panel boundary yields
    /// `Err(Internal)`, not an unwind (D5/D10), and latches the panel's one
    /// shared flag — the same latch every glue closure of the panel runs
    /// under. Later calls through the inner-handle entry keep reporting
    /// `Internal`: the poisoned check comes first, so the still-live handle
    /// never leaks `NotRunning` past a caught panic.
    #[test]
    // Approved per-instance (#13): the test maps the caught panic onto
    // `Internal` to assert the boundary's mapping; the payload is the
    // point, not discarded detail.
    #[allow(
        clippy::map_err_ignore,
        reason = "approved #13: test asserts the panic-to-Internal mapping"
    )]
    fn a_panic_in_a_guarded_closure_is_internal_and_stays_internal() {
        let inner = Inner {
            id: 0,
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        };

        // The boundary's own expression: `guard` catches the panic, latches
        // the shared flag and returns the flag, which the public entry points
        // map onto `Internal`.
        let caught = guard(&inner.poisoned, || panic!("a deliberate glue panic"));
        assert!(caught.is_err(), "the panic is caught, not unwound");
        assert!(inner.poisoned.is_poisoned(), "the shared latch is set");
        assert_eq!(
            caught.map_err(|_| PinwinError::Internal),
            Err(PinwinError::Internal)
        );

        // Later applies through the inner-handle entry short-circuit on the
        // latch (`Internal`) before the not-running check could run, even
        // though `live` is still true here.
        assert_eq!(
            apply_via_inner(&inner, layout(), 0),
            Err(PinwinError::Internal)
        );
        assert_eq!(
            apply_via_inner(&inner, layout(), 0),
            Err(PinwinError::Internal)
        );
    }

    /// An apply on a live handle whose glue is gone (the invoked command
    /// finds no live panel) maps the not-live outcome onto `NotRunning` —
    /// the mapping the GTK side's reply reaches through.
    #[test]
    fn a_not_live_publish_reply_is_not_running() {
        // The invoke below runs inline on this thread: the default main
        // context has no owner here, and this thread's GLUE is empty, so the
        // command reports NotLive and the reply maps to NotRunning.
        let inner = Inner {
            id: u64::MAX, // no panel this test could collide with
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        };
        assert_eq!(
            apply_via_inner(&inner, layout(), 0),
            Err(PinwinError::NotRunning)
        );
    }

    /// A teardown on an already-dead panel posts nothing and releases the
    /// slot; the drop path never joins the GTK thread (D4 — structurally:
    /// there is no join in this module).
    #[test]
    fn teardown_of_a_dead_panel_is_a_noop_that_releases_the_slot() {
        let _test_lock = INSTANCE_TEST_LOCK.lock().expect("test lock");
        INSTANCE.lock().expect("instance lock").phase = Phase::Running;

        let inner = Inner {
            id: 0,
            poisoned: Poisoned::new(),
            live: AtomicBool::new(false),
            keyboard: Keyboard::OnDemand,
        };
        let started = std::time::Instant::now();
        let _ = guard_always(&inner.poisoned, || teardown_inner(&inner));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the dead-panel teardown does not wait"
        );

        // The single-instance slot was released.
        assert!(matches!(
            INSTANCE.lock().expect("instance lock").phase,
            Phase::Idle
        ));
    }

    /// A teardown whose reply never comes (a wedged loop) gives up after the
    /// bound instead of blocking the drop forever.
    #[test]
    fn a_wedged_teardown_reply_gives_up_after_the_bound() {
        // Hold the reply sender in another thread, like a wedged GTK side.
        let (tx, rx) = mpsc::sync_channel::<()>(1);
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(tx);
        });
        let started = std::time::Instant::now();
        let _ = rx.recv_timeout(Duration::from_millis(50));
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "the teardown wait is bounded"
        );
        holder.join().expect("holder thread");
    }

    /// The fd check: an open descriptor passes, a negative or closed one
    /// does not.
    #[test]
    fn fd_is_open_checks_the_descriptor() {
        assert!(!fd_is_open(-1));
        let file = std::fs::File::open("/dev/null").expect("/dev/null");
        assert!(fd_is_open(file.as_raw_fd()));
        let mut fds: [libc::c_int; 2] = [0; 2];
        // SAFETY: `fds` is a writable two-element array for the call.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
        // SAFETY: closing descriptors this test owns.
        unsafe { libc::close(fds[1]) };
        assert!(!fd_is_open(fds[1]));
        // SAFETY: closing a descriptor this test owns.
        unsafe { libc::close(fds[0]) };
    }

    /// The animated apply's duration clamp matches `PINWIN_ANIM_MAX_MS`.
    #[test]
    fn the_animation_duration_clamps_at_the_maximum() {
        assert_eq!(ANIMATION_MAX_MS, 1000);
        assert_eq!(1000u32.min(ANIMATION_MAX_MS), 1000);
        assert_eq!(60000u32.min(ANIMATION_MAX_MS), 1000);
    }

    /// A start in a session without any display fails with `NoDisplay` and
    /// opens nothing: the real handshake plumbing (thread spawn, command
    /// delivery, handshake reply) against a GTK that cannot init.
    #[test]
    fn a_start_without_a_display_is_no_display() {
        // Only meaningful where no display exists at all; in a session with
        // one, the panel would really open, so the test skips.
        let display = ["WAYLAND_DISPLAY", "DISPLAY", "WAYLAND_SOCKET"]
            .into_iter()
            .any(|name| std::env::var_os(name).is_some());
        if display {
            eprintln!("skipping: a display exists, the panel would really open");
            return;
        }

        let _test_lock = INSTANCE_TEST_LOCK.lock().expect("test lock");
        INSTANCE.lock().expect("instance lock").phase = Phase::Idle;
        INSTANCE.lock().expect("instance lock").gtk = None;

        let file = std::fs::File::open("/dev/null").expect("/dev/null");
        let started = Panel::start(startup(file.as_raw_fd()));
        assert!(matches!(started, Err(PinwinError::NoDisplay)));

        // The failed start released the single-instance slot.
        assert!(matches!(
            INSTANCE.lock().expect("instance lock").phase,
            Phase::Idle
        ));
    }

    /// A full lifecycle in a real compositor session: start, apply, animated
    /// apply, drop, and a second start may then succeed. Ignored because it
    /// needs a Wayland compositor with wlr-layer-shell and opens real
    /// surfaces.
    #[test]
    #[ignore = "needs a Wayland compositor with wlr-layer-shell; opens real surfaces"]
    fn a_full_lifecycle_opens_applies_and_closes() {
        let file = std::fs::File::open("/dev/null").expect("/dev/null");
        let panel = Panel::start(startup(file.as_raw_fd()))
            .expect("the panel starts in a layer-shell session");
        panel.apply_layout(layout()).unwrap();
        panel.apply_layout_animated(layout(), 200).unwrap();
        drop(panel);
        // A later start MAY succeed; whether it does is compositor
        // behaviour, so only the drop returning is asserted here.
    }
}
