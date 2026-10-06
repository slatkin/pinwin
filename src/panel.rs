//! The public `Panel` API (port-to-rust D4, D6, D7): the host calls from its
//! own thread; each panel runs on its own Wayland panel thread (the
//! `wayland_side` module) that the start spawns and the drop ends
//! (replace-gtk-with-wayland D2).
//!
//! The shape follows `src/pinwin_api.c`: start validates its arguments before
//! any thread work, then hands the startup to the panel thread and waits for
//! a handshake that completes when the panel is on screen with live metrics.
//! Applies and focus requests post a command on the thread's calloop channel
//! and wait up to five seconds for the reply (a wedged thread is `Internal`,
//! never a hang). Drop posts a teardown, waits for its bounded reply and
//! returns; the thread then ends on its own (D2).
//!
//! Panics never cross the API (D5): every public entry point runs under the
//! shared [`crate::guard`], and the poisoned check comes before the
//! "panel thread ended" check, so a panic reports `Internal`, never
//! `NotRunning`.
//!
//! The tests here are display-free (D10): the error and outcome mappings, the
//! handshake and reply channel logic, the single-instance guard and the
//! poisoned-over-not-running precedence run without a display; the display
//! tests are `#[ignore]`d.

mod error;
mod handshake;
mod startup;
pub mod wayland_side;

pub use error::PinwinError;
pub use startup::Startup;

use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::activation::ActivationToken;
use crate::guard::{Poisoned, guard, guard_always};
use crate::layout::{Keyboard, Layout};

use handshake::{Handshake, wait_for_start};
use wayland_side::{PanelThread, StartCommand, spawn_panel_thread};

/// The longest animated apply duration, clamped like `pinwin_api.c`'s
/// `PINWIN_ANIM_MAX_MS` (`pinwin.h`: "the duration SHALL be clamped").
pub const ANIMATION_MAX_MS: u32 = 1000;

/// The single-instance state: at most one panel per process (the spec's
/// "Library Panel API"). `Starting` covers the start handshake window, so two
/// concurrent starts cannot both spawn a panel thread.
enum Phase {
    Idle,
    Starting,
    Running,
}

struct Instance {
    phase: Phase,
}

static INSTANCE: Mutex<Instance> = Mutex::new(Instance { phase: Phase::Idle });

/// The display-free handle state (D10): the panel's one shared D5
/// latch — the same flag every glue closure of this panel guards against —
/// whether the panel thread is still live, and the keyboard mode fixed at
/// start time (a focus request short-circuits on it before posting, D10).
/// The thread holds a clone of the `Arc` and clears `live` when the panel
/// ends on its own, so an apply or a focus request on a dead panel reports
/// `NotRunning` without posting anything.
#[derive(Debug)]
pub(crate) struct Inner {
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
    /// The panel thread the start spawned; the applies, the focus requests
    /// and the drop's teardown post through it (D2).
    thread: PanelThread,
}

impl Panel {
    /// Start a panel: put it on screen and return once it is there or has
    /// failed.
    ///
    /// The fd is validated before any thread work (`InvalidFd`, D7). A start
    /// while another handle is alive fails with `AlreadyRunning` and leaves
    /// the running panel unchanged. `NoDisplay` when no Wayland display or
    /// wlr-layer-shell is available — nothing opened.
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
        // built inside and handed to the panel thread.
        let latch = Poisoned::new();
        guard(&latch, || Self::start_inner(startup))
            .map_err(|_| PinwinError::Internal)
            .and_then(std::convert::identity)
    }

    fn start_inner(startup: Startup) -> Result<Panel, PinwinError> {
        // Pure argument checks, no thread work (pinwin_api.c's startup_valid
        // minus the classes the argument types make unrepresentable, D6/D7).
        if !fd_is_open(startup.fd) {
            return Err(PinwinError::InvalidFd);
        }

        let poisoned = Poisoned::new();
        let inner = Arc::new(Inner {
            poisoned: poisoned.clone(),
            live: AtomicBool::new(true),
            keyboard: startup.keyboard,
        });

        // Claim the single-instance slot before touching the Wayland side, so
        // two concurrent starts cannot both reach the handshake. The guard is
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
            poisoned,
            handshake: Handshake::new(reply_tx),
            inner: Arc::clone(&inner),
            startup,
        };

        // One panel thread per start (D2). A failed OS spawn is the internal
        // path; a failed connection or bind reports through the handshake.
        let Ok(thread) = spawn_panel_thread(None, command) else {
            release_phase();
            return Err(PinwinError::Internal);
        };

        // Wait for the panel thread to go live (the handshake), like
        // pinwin_start's cond wait. The thread always reports.
        let outcome = wait_for_start(&reply_rx);

        {
            let mut instance = INSTANCE.lock().expect("pinwin instance lock");
            instance.phase = if outcome.is_ok() {
                Phase::Running
            } else {
                Phase::Idle
            }
        };
        outcome.map(|()| Panel { inner, thread })
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
    /// same left and right gutters) and the duration is non-zero, the
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
        apply_via_inner(&self.inner, layout, duration_ms, |layout, duration_ms| {
            self.thread.apply(layout, duration_ms)
        })
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
    /// Until the panel thread replaces the GTK thread (rows 8.1 and 8.2 of
    /// the `replace-gtk-with-wayland` change), the request is accepted and
    /// does nothing: no activation request reaches the compositor, so the
    /// panel does not get the keyboard this way.
    ///
    /// The token is the one-use permission a compositor gives a program it
    /// launches; the library owns no transport for the request, the host
    /// decides how a request and its token reach it. Taken by value: the
    /// request posts the token to the panel thread, which hands it to the
    /// compositor (replace-gtk-with-wayland D4).
    pub fn request_focus(&self, token: ActivationToken) -> Result<(), PinwinError> {
        request_focus_via_inner(&self.inner, token, |token| self.thread.request_focus(token))
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
    post: impl FnOnce(Layout, u32) -> Result<(), PinwinError>,
) -> Result<(), PinwinError> {
    guard(&inner.poisoned, || {
        post_apply(inner, layout, duration_ms, post)
    })
    .map_err(|_| PinwinError::Internal)
    .and_then(std::convert::identity)
}

/// The unguarded body of [`apply_via_inner`]: the ended check, then the
/// bounded posted apply (`post` carries the token to the panel thread, D2).
fn post_apply(
    inner: &Inner,
    layout: Layout,
    duration_ms: u32,
    post: impl FnOnce(Layout, u32) -> Result<(), PinwinError>,
) -> Result<(), PinwinError> {
    // Not running → `NotRunning` without blocking (pinwin_api.c's order).
    if !inner.live.load(Ordering::Relaxed) {
        return Err(PinwinError::NotRunning);
    }
    post(layout, duration_ms)
}

/// A focus request through the display-free inner handle (D10): the same
/// order as [`apply_via_inner`] — the poisoned check first (D5: a panic
/// reports `Internal`, never `NotRunning`), then the ended check — then the
/// mode short-circuit and the request itself.
pub(crate) fn request_focus_via_inner(
    inner: &Inner,
    token: ActivationToken,
    post: impl FnOnce(ActivationToken) -> Result<(), PinwinError>,
) -> Result<(), PinwinError> {
    match guard(&inner.poisoned, || post_focus(inner, token, post)) {
        Ok(result) => result,
        Err(_) => Err(PinwinError::Internal),
    }
}

/// The unguarded body of [`request_focus_via_inner`]: the ended check, the
/// mode short-circuit, then the request itself.
fn post_focus(
    inner: &Inner,
    token: ActivationToken,
    post: impl FnOnce(ActivationToken) -> Result<(), PinwinError>,
) -> Result<(), PinwinError> {
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
    // The token posts to the panel thread, which makes the xdg-activation
    // request for the panel surface (replace-gtk-with-wayland D4).
    post(token)
}

impl Drop for Panel {
    fn drop(&mut self) {
        // D5: the teardown runs even on a latched flag and never unwinds; the
        // thread ends on its own after the teardown, and the drop never joins
        // it (D2).
        let _ = guard_always(&self.inner.poisoned, || {
            teardown_inner(&self.inner, || self.thread.teardown());
        });
    }
}

/// The teardown body: post the teardown and wait for its reply, then release
/// the single-instance slot so a later start may succeed. A dead panel (its
/// thread ended on its own) posts nothing — the teardown is a no-op that
/// still releases the slot.
fn teardown_inner(inner: &Inner, post_teardown: impl FnOnce()) {
    if inner.live.swap(false, Ordering::Relaxed) {
        post_teardown();
    }
    release_phase();
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
    use std::cell::Cell;
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

    /// A fd that is not an open descriptor is `InvalidFd` before any thread
    /// work (D7): no thread is spawned, nothing else changes. A negative fd
    /// and a closed one are both covered.
    #[test]
    fn start_rejects_a_bad_fd_before_any_thread_work() {
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
    /// before any thread work; the fd check keeps its precedence over it.
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

    /// The poisoned check comes before the "panel thread ended" check (D5):
    /// a latched handle reports `Internal`, never `NotRunning`. The post
    /// closures would fail the result if they ran, so `Internal` can only
    /// come from the latch.
    #[test]
    fn poisoned_beats_not_running() {
        let inner = Inner {
            poisoned: Poisoned::latched(),
            live: AtomicBool::new(false),
            keyboard: Keyboard::OnDemand,
        };
        assert_eq!(
            apply_via_inner(&inner, layout(), 0, |_, _| Err(PinwinError::Internal)),
            Err(PinwinError::Internal)
        );
        // The focus request follows the same precedence: a latched handle
        // reports `Internal`, never `NotRunning`.
        assert_eq!(
            request_focus_via_inner(&inner, token(), |_| Err(PinwinError::Internal)),
            Err(PinwinError::Internal)
        );
    }

    /// An apply on a handle whose panel is no longer live reports
    /// `NotRunning` without blocking: the post is skipped, the reply path is
    /// not entered (D10's dead-handle path). The post closure fails the
    /// result if it ran, so `NotRunning` can only come from the ended check.
    #[test]
    fn apply_on_a_dead_panel_is_not_running_without_blocking() {
        let inner = Inner {
            poisoned: Poisoned::new(),
            live: AtomicBool::new(false),
            keyboard: Keyboard::OnDemand,
        };
        let started = std::time::Instant::now();
        assert_eq!(
            apply_via_inner(&inner, layout(), 0, |_, _| Err(PinwinError::Internal)),
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
            poisoned: Poisoned::new(),
            live: AtomicBool::new(false),
            keyboard: Keyboard::OnDemand,
        };
        let started = std::time::Instant::now();
        assert_eq!(
            request_focus_via_inner(&inner, token(), |_| Err(PinwinError::Internal)),
            Err(PinwinError::NotRunning)
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the dead-panel focus request does not wait"
        );
    }

    /// In the `none` and `exclusive` modes a focus request is `Ok(())` and
    /// posts nothing: the post closure fails the result if it ran, so `Ok`
    /// can only come from the mode short-circuit.
    #[test]
    fn a_focus_request_outside_on_demand_is_ok_and_posts_nothing() {
        for keyboard in [Keyboard::None, Keyboard::Exclusive] {
            let inner = Inner {
                poisoned: Poisoned::new(),
                live: AtomicBool::new(true),
                keyboard,
            };
            assert_eq!(
                request_focus_via_inner(&inner, token(), |_| Err(PinwinError::Internal)),
                Ok(())
            );
        }
    }

    /// A live `on-demand` handle's request posts the token to the panel
    /// thread: the post closure receives the very token the caller built
    /// (replace-gtk-with-wayland D4).
    #[test]
    fn a_focus_request_on_demand_posts_the_token() {
        let inner = Inner {
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        };
        let posted = Cell::new(None);
        let result = request_focus_via_inner(&inner, token(), |token| {
            posted.set(Some(token));
            Ok(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(posted.take().as_ref(), Some(&token()));
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
        // latch (`Internal`) before the not-running check or the post could
        // run, even though `live` is still true here.
        assert_eq!(
            apply_via_inner(&inner, layout(), 0, |_, _| Err(PinwinError::Internal)),
            Err(PinwinError::Internal)
        );
        assert_eq!(
            apply_via_inner(&inner, layout(), 0, |_, _| Err(PinwinError::Internal)),
            Err(PinwinError::Internal)
        );
    }

    /// A teardown on an already-dead panel posts nothing and releases the
    /// slot; the drop path never joins the panel thread (D2 — structurally:
    /// there is no join in this module). The post closure panics if it ran,
    /// so a post would fail the test loudly.
    #[test]
    fn teardown_of_a_dead_panel_is_a_noop_that_releases_the_slot() {
        let _test_lock = INSTANCE_TEST_LOCK.lock().expect("test lock");
        INSTANCE.lock().expect("instance lock").phase = Phase::Running;

        let inner = Inner {
            poisoned: Poisoned::new(),
            live: AtomicBool::new(false),
            keyboard: Keyboard::OnDemand,
        };
        let started = std::time::Instant::now();
        let _ = guard_always(&inner.poisoned, || {
            teardown_inner(&inner, || panic!("a dead panel posts no teardown"));
        });
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

    /// A teardown on a live panel posts exactly once and releases the slot;
    /// the handle stops posting afterwards.
    #[test]
    fn a_live_teardown_posts_once_and_releases_the_slot() {
        let _test_lock = INSTANCE_TEST_LOCK.lock().expect("test lock");
        INSTANCE.lock().expect("instance lock").phase = Phase::Running;

        let inner = Inner {
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        };
        let posted = Cell::new(0);
        let _ = guard_always(&inner.poisoned, || {
            teardown_inner(&inner, || posted.set(posted.get() + 1));
        });
        assert_eq!(posted.get(), 1, "the teardown posted once");
        assert!(!inner.live.load(std::sync::atomic::Ordering::Relaxed));

        // The single-instance slot was released.
        assert!(matches!(
            INSTANCE.lock().expect("instance lock").phase,
            Phase::Idle
        ));
    }

    /// A teardown whose reply never comes (a wedged thread) gives up after
    /// the bound instead of blocking the drop forever.
    #[test]
    fn a_wedged_teardown_reply_gives_up_after_the_bound() {
        // Hold the reply sender in another thread, like a wedged panel thread.
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
    /// opens nothing: the real handshake plumbing (thread spawn, connection
    /// attempt, handshake reply) against a compositor that cannot be
    /// reached. The connection failure is fast, so the test never hangs.
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
    /// surfaces on the panel thread (D2).
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
