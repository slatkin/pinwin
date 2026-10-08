//! The `wayland_side` module's tests, split from `wayland_side.rs` to keep
//! the module at its line budget (the split is mechanical; every test stays
//! in place, reached through `use super::*`).

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
    Startup::new(
        -1,
        Layout::new(
            Side::Left,
            NonZeroU16::new(40).expect("test columns"),
            0,
            0,
            0,
            0,
        ),
        Keyboard::OnDemand,
        None,
    )
}

/// A live handle state like a started panel's, for the thread-side
/// tests.
fn live_inner() -> Arc<Inner> {
    Arc::new(Inner {
        poisoned: GuardPoisoned::new(),
        live: AtomicBool::new(true),
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

/// A show on a dead thread handle reports `NotRunning` without posting
/// (the spec's dead-panel scenario): the
/// shared post skips the reply path, so the call does not wait.
#[test]
fn a_show_on_a_dead_thread_handle_is_not_running() {
    let (commands, _receiver) = channel::channel::<PanelCommand>();
    let thread = PanelThread {
        commands,
        inner: Arc::new(Inner {
            poisoned: GuardPoisoned::new(),
            live: AtomicBool::new(false),
        }),
    };
    let started = std::time::Instant::now();
    assert_eq!(thread.show(), Err(PinwinError::NotRunning));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "the dead-panel show does not wait"
    );
}

/// A panel thread handle over a fake command server (a plain calloop
/// loop answering each command through its bounded reply): an apply and
/// a toggle post through the real command channel and map their replies,
/// and a teardown posts, answers and ends the loop — the wiring
/// `Panel::apply_layout`, `toggle` and `Drop` depend on, without
/// a display (D10).
#[test]
fn a_panel_thread_handle_posts_apply_toggle_and_teardown_to_a_loop() {
    use calloop::channel::Event;

    let (commands, receiver) = channel::channel::<PanelCommand>();
    let thread = PanelThread {
        commands,
        inner: live_inner(),
    };

    // The fake server: answer each command like the real handlers do
    // (an apply with its publish outcome, a toggle and a teardown with
    // `()`), then stop once the teardown passed.
    let server = std::thread::spawn(move || {
        let mut event_loop = calloop::EventLoop::<Cell<bool>>::try_new().expect("the fake loop");
        event_loop
            .handle()
            .insert_source(receiver, |event, (), done: &mut Cell<bool>| {
                let Event::Msg(command) = event else {
                    done.set(true);
                    return;
                };
                match command {
                    PanelCommand::Apply { reply, .. } => {
                        let _ = reply.send(PublishOutcome::Applied);
                    }
                    // The toggle and the show both answer `()` here.
                    PanelCommand::Toggle { reply } | PanelCommand::Show { reply } => {
                        let _ = reply.send(());
                    }
                    PanelCommand::Teardown { reply } => {
                        let _ = reply.send(());
                        done.set(true);
                    }
                }
            })
            .expect("insert the command source");
        let mut done = Cell::new(false);
        while !done.get() {
            if event_loop
                .dispatch(Some(std::time::Duration::from_secs(5)), &mut done)
                .is_err()
            {
                break;
            }
        }
    });

    assert_eq!(thread.apply(startup().layout(), 0), Ok(()));
    assert_eq!(thread.toggle(), Ok(()), "the toggle posts and answers");
    assert_eq!(thread.show(), Ok(()), "the show posts and answers");
    thread.teardown();
    server.join().expect("the fake server thread");
}
