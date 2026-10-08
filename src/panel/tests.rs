//! The `Panel` handle's display-free tests (D10): the error and outcome
//! mappings, the handshake and reply logic, the single-instance guard and
//! the poisoned-over-not-running precedence, driven against the inner handle
//! and fake posts without a display.

use super::*;
use crate::instance::{bind_at, send_to};
use crate::layout::{Keyboard, Side};
use std::cell::Cell;
use std::io::Write as _;
use std::num::NonZeroU16;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
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
    Startup::new(fd, layout(), Keyboard::OnDemand, None)
}

/// A unique scratch directory with the socket path inside it, like the
/// `instance` tests' scratch directories, so these tests never touch the
/// process environment and never share a parent with anything else (D10):
/// the bind chmods the socket's parent to 0700, so that parent must be
/// the test's own directory.
fn instance_socket_path(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pinwin-panel-test-{}-{}-{}",
        std::process::id(),
        tag,
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).expect("scratch dir");
    dir.join("wayland-0-pinwin-test.sock")
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
    };
    assert_eq!(
        apply_via_inner(&inner, layout(), 0, |_, _| Err(PinwinError::Internal)),
        Err(PinwinError::Internal)
    );
    // The toggle follows the same precedence: a latched handle reports
    // `Internal`, never `NotRunning`.
    assert_eq!(
        show_or_toggle_via_inner(&inner, || Err(PinwinError::Internal)),
        Err(PinwinError::Internal)
    );
    // The show shares that posted path (serve-instance-socket D6), so it
    // keeps the precedence too.
    assert_eq!(
        show_or_toggle_via_inner(&inner, || Err(PinwinError::Internal)),
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

/// A toggle on a handle whose panel is no longer live reports
/// `NotRunning` without blocking: the post is skipped, the reply path is
/// not entered (the spec's dead-panel toggle scenario,
/// `replace-gtk-with-wayland` D4).
#[test]
fn a_toggle_on_a_dead_panel_is_not_running_without_blocking() {
    let inner = Inner {
        poisoned: Poisoned::new(),
        live: AtomicBool::new(false),
    };
    let started = std::time::Instant::now();
    assert_eq!(
        show_or_toggle_via_inner(&inner, || Err(PinwinError::Internal)),
        Err(PinwinError::NotRunning)
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the dead-panel toggle does not wait"
    );
}

/// A show on a handle whose panel is no longer live reports
/// `NotRunning` without blocking (the spec's dead-panel scenario): the
/// post is skipped through the same
/// shared path as the toggle's, so the reply path is not entered.
#[test]
fn a_show_on_a_dead_panel_is_not_running_without_blocking() {
    let inner = Inner {
        poisoned: Poisoned::new(),
        live: AtomicBool::new(false),
    };
    let started = std::time::Instant::now();
    assert_eq!(
        show_or_toggle_via_inner(&inner, || Err(PinwinError::Internal)),
        Err(PinwinError::NotRunning)
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the dead-panel show does not wait"
    );
}

/// A live handle's toggle posts to the panel thread
/// (`replace-gtk-with-wayland` D4): show and hide are not focus requests,
/// so every keyboard mode posts.
#[test]
fn a_live_toggle_posts_to_the_panel_thread() {
    let inner = Inner {
        poisoned: Poisoned::new(),
        live: AtomicBool::new(true),
    };
    let posted = Cell::new(0);
    let result = show_or_toggle_via_inner(&inner, || {
        posted.set(posted.get() + 1);
        Ok(())
    });
    assert_eq!(result, Ok(()));
    assert_eq!(posted.get(), 1, "the toggle posted once");
}

/// A live handle's show posts to the panel thread through the same shared
/// posted path as the toggle (serve-instance-socket D6).
#[test]
fn a_live_show_posts_to_the_panel_thread() {
    let inner = Inner {
        poisoned: Poisoned::new(),
        live: AtomicBool::new(true),
    };
    let posted = Cell::new(0);
    let result = show_or_toggle_via_inner(&inner, || {
        posted.set(posted.get() + 1);
        Ok(())
    });
    assert_eq!(result, Ok(()));
    assert_eq!(posted.get(), 1, "the show posted once");
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
    let panel =
        Panel::start(startup(file.as_raw_fd())).expect("the panel starts in a layer-shell session");
    panel.apply_layout(layout()).unwrap();
    panel.apply_layout_animated(layout(), 200).unwrap();
    drop(panel);
    // A later start MAY succeed; whether it does is compositor
    // behaviour, so only the drop returning is asserted here.
}

/// A start that fails `InvalidFd` with a bound instance socket leaves no
/// socket file (the spec's failed-start
/// scenario): the owned socket drops with the failed start, and its drop
/// removes the path, so the name is free again at once. Display-free: the
/// fd check runs before any thread work (D7).
#[test]
fn a_failed_start_with_a_bound_socket_leaves_no_socket_file() {
    let path = instance_socket_path("invalid-fd");
    let socket = bind_at(&path).expect("the bind");
    let started = Panel::start(startup(-1).with_instance(socket));
    assert!(matches!(started, Err(PinwinError::InvalidFd)));
    assert!(!path.exists(), "the socket file is gone");
}

/// A full lifecycle with a bound instance socket in a real compositor
/// session (`serve-instance-socket` D2): the started panel serves
/// `toggle` and `show` requests on the socket through the client call,
/// the drop removes the socket file, and the drop returns within the
/// teardown bound — the detached listener is never joined. Ignored
/// because it needs a Wayland compositor with wlr-layer-shell and opens
/// real surfaces on the panel thread (D2).
#[test]
#[ignore = "needs a Wayland compositor with wlr-layer-shell; opens real surfaces"]
fn a_serving_panel_answers_requests_and_drops_without_waiting() {
    let path = instance_socket_path("serve");
    let socket = bind_at(&path).expect("the bind");
    let file = std::fs::File::open("/dev/null").expect("/dev/null");
    let panel = Panel::start(startup(file.as_raw_fd()).with_instance(socket))
        .expect("the panel starts in a layer-shell session");

    // The toggle hides the panel, the show shows it again; both answer
    // `ok` through the detached listener thread (D2).
    send_to(&path, Request::Toggle).expect("the toggle");
    send_to(&path, Request::Show).expect("the show");

    let started = std::time::Instant::now();
    drop(panel);
    assert!(
        started.elapsed() < handshake::APPLY_WAIT,
        "the drop does not wait past the teardown bound"
    );
    assert!(!path.exists(), "the socket file is gone");
}

/// The half-request variant of the drop contract (`serve-instance-socket`
/// D2): a client connection with half a request line keeps the
/// detached listener blocked in its bounded read, and the drop still
/// returns long before that read bound — a joining drop would wait out
/// the listener's 500 ms read, so 250 ms discriminates. Ignored because
/// it needs a Wayland compositor with wlr-layer-shell (D2).
#[test]
#[ignore = "needs a Wayland compositor with wlr-layer-shell; opens real surfaces"]
fn a_half_written_request_does_not_hold_the_drop() {
    let path = instance_socket_path("half-request");
    let socket = bind_at(&path).expect("the bind");
    let file = std::fs::File::open("/dev/null").expect("/dev/null");
    let panel = Panel::start(startup(file.as_raw_fd()).with_instance(socket))
        .expect("the panel starts in a layer-shell session");

    // Half a request line, the connection left open: the listener accepts
    // within one accept poll and then blocks in its bounded read (D2).
    // The write lands just before the drop.
    let mut client = UnixStream::connect(&path).expect("the connection");
    client.write_all(b"tog").expect("the half line");
    // One accept poll (100 ms) plus margin, so the listener is inside its
    // read when the drop happens.
    std::thread::sleep(Duration::from_millis(200));

    let started = std::time::Instant::now();
    drop(panel);
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "the drop does not wait out the listener's read bound"
    );
    assert!(!path.exists(), "the socket file is gone");
}
