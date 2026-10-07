//! The public `Panel` API (port-to-rust D4, D6, D7): the host calls from its
//! own thread; each panel runs on its own Wayland panel thread (the
//! `wayland_side` module) that the start spawns and the drop ends
//! (replace-gtk-with-wayland D2).
//!
//! The shape follows `src/pinwin_api.c`: start validates its arguments before
//! any thread work, then hands the startup to the panel thread and waits for
//! a handshake that completes when the panel is on screen with live metrics.
//! Applies, toggles, shows and the teardown post a command on the thread's calloop
//! channel
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

use crate::guard::{Poisoned, guard, guard_always};
use crate::layout::Layout;

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
/// and whether the panel thread is still live. The thread holds a clone of
/// the `Arc` and clears `live` when the panel ends on its own, so an apply
/// or a toggle or a show on a dead panel reports `NotRunning` without posting
/// anything.
#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) poisoned: Poisoned,
    pub(crate) live: AtomicBool,
}

/// A running panel, the host's handle. Dropping it closes the panel.
///
/// Besides applies, the handle offers the show/hide toggle
/// ([`Panel::toggle`]) — it hides a shown panel and shows a hidden one
/// (replace-gtk-with-wayland D4) — and a show ([`Panel::show`]): it shows a
/// hidden panel and leaves a shown one unchanged (serve-instance-socket D6).
///
/// Not `Clone`: one handle per panel, so the single-instance rule is
/// ownership, not bookkeeping.
#[derive(Debug)]
pub struct Panel {
    inner: Arc<Inner>,
    /// The panel thread the start spawned; the applies, the toggle, the
    /// show and the drop's teardown post through it (D2).
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
        if !fd_is_open(startup.fd()) {
            return Err(PinwinError::InvalidFd);
        }

        let poisoned = Poisoned::new();
        let inner = Arc::new(Inner {
            poisoned: poisoned.clone(),
            live: AtomicBool::new(true),
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

    /// Toggle the panel (row 9.1, replace-gtk-with-wayland D4): hide a
    /// shown panel, show a hidden one. The panel is shown at start. Hiding
    /// unmaps the panel surface and releases the held reservation, and a
    /// width animation in progress ends at its target layout first; the
    /// terminal and the pty keep running while the panel is hidden. Showing
    /// maps the panel again — in `on-demand` mode with `on-demand` keyboard
    /// interactivity, so a compositor that focuses a newly mapped surface
    /// gives it the keyboard without a click — draws the current grid and
    /// restores the held reservation. Neither direction resizes the terminal
    /// grid or the pty window size on its own; the one exception is a hide
    /// that ends a running width animation, whose deferred grid resize lands
    /// at the target — the animation's own end state. While hidden, an apply
    /// validates and stores the layout with no animation; the next show
    /// uses it.
    ///
    /// `Ok(())` means the panel thread committed the hide's null buffer or
    /// the show's commit without a buffer; the rest of a show follows the
    /// configure, after the reply. All keyboard modes toggle: the host chose
    /// the mode, and show and hide are not focus requests.
    pub fn toggle(&self) -> Result<(), PinwinError> {
        show_or_toggle_via_inner(&self.inner, || self.thread.toggle())
    }

    /// Show the panel (serve-instance-socket row 3.1, D6): show a hidden
    /// panel; a shown panel is left unchanged — the call returns `Ok`, no
    /// surface is unmapped or remapped and the reservation does not change
    /// (the spec's "Show a shown panel" scenario). A hidden panel maps like
    /// the show half of a toggle, and the reply is that show's commit
    /// without a buffer; the rest of a show follows the configure, after
    /// the reply. The panel is shown at start.
    ///
    /// The same bounded contract as the toggle: the call posts to the panel
    /// thread through the shared posted path and never blocks indefinitely.
    ///
    /// # Errors
    /// `NotRunning` on a dead panel (without blocking), `Internal` on a
    /// caught panic or a wedged or ended thread.
    pub fn show(&self) -> Result<(), PinwinError> {
        show_or_toggle_via_inner(&self.inner, || self.thread.show())
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

/// A posted show or toggle through the display-free inner handle (D10):
/// the same order as [`apply_via_inner`] — the poisoned check first (D5: a
/// panic reports `Internal`, never `NotRunning`), then the ended check —
/// then the posted command itself. One helper for both, so the two cannot
/// drift (serve-instance-socket D6). No mode short-circuit: every keyboard
/// mode toggles, and a show on a shown panel is answered on the thread.
pub(crate) fn show_or_toggle_via_inner(
    inner: &Inner,
    post: impl FnOnce() -> Result<(), PinwinError>,
) -> Result<(), PinwinError> {
    match guard(&inner.poisoned, || post_show_or_toggle(inner, post)) {
        Ok(result) => result,
        Err(_) => Err(PinwinError::Internal),
    }
}

/// The unguarded body of [`show_or_toggle_via_inner`]: the ended check, then
/// the posted command itself.
fn post_show_or_toggle(
    inner: &Inner,
    post: impl FnOnce() -> Result<(), PinwinError>,
) -> Result<(), PinwinError> {
    // Not running → `NotRunning` without blocking, unconditionally (the
    // spec's dead-panel scenario; pinwin_api.c's order).
    if !inner.live.load(Ordering::Relaxed) {
        return Err(PinwinError::NotRunning);
    }
    post()
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
mod tests;
