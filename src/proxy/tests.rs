//! The proxy's tests (replace-terminal-with-app-host row 6.1, spec "Panic in
//! one panel"): a panic in one panel's loop ends only that panel's handle.
//!
//! The loop's own steps need a wl-proxy `State`, which needs a compositor, so
//! these tests drive what runs after the loop: the panic guard and the end
//! report. The panels are stand-in threads, as in `src/daemon/tests.rs` (D12), so
//! no test connects to a compositor and no test touches the user's display.

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use super::guarded;
use crate::config::Config;
use crate::daemon::lifetime::Lifetime;
use crate::daemon::panel::{AppId, Command, EventFd, PanelHandle, PanelReply};
use crate::ipc::{ErrorKind, Request};
use crate::name::PanelName;

/// The bound every wait in these tests uses.
const WAIT: Duration = Duration::from_millis(500);

/// A daemon lifetime for a test: the default config, a path no test reads and
/// a short wait. It opens one eventfd and touches no display (D12).
fn lifetime() -> Arc<Lifetime> {
    let path = std::env::temp_dir().join("pinwin-proxy-test-config.toml");
    let config = Config::parse(&path, "").expect("the built-in defaults");
    Arc::new(Lifetime::new(path, config, WAIT).expect("a daemon lifetime"))
}

/// A panel thread in the panel's place (D12): it answers every command with
/// success, so a test sees whether the daemon can still route to it.
fn stand_in(name: &str, pid: u32) -> (PanelName, PanelHandle, thread::JoinHandle<()>) {
    let (commands, incoming) = mpsc::channel::<Command>();
    let wake = Arc::new(EventFd::new().expect("an eventfd"));
    let watch = Arc::clone(&wake);
    let thread = thread::spawn(move || {
        while let Ok(command) = incoming.recv() {
            watch.drain().expect("the panel clears its wake");
            command.answer(PanelReply::Done);
        }
    });
    let name = PanelName::parse(name).expect("a panel name");
    (
        name,
        PanelHandle::new(pid, commands, wake, AppId::default()),
        thread,
    )
}

/// A panic in one panel's loop ends only that panel's handle: its name leaves
/// the registry, the other panel still answers and the daemon stays up (spec
/// "Panic in one panel", D5).
#[test]
fn a_panic_ends_only_the_panicking_panel() {
    let lifetime = lifetime();
    let (panicking, panicking_handle, panicking_thread) = stand_in("panicking", 11);
    let (other, other_handle, other_thread) = stand_in("other", 12);
    lifetime
        .add_panel(panicking.clone(), panicking_handle)
        .expect("the name is free");
    lifetime
        .add_panel(other.clone(), other_handle)
        .expect("the name is free");

    guarded(&lifetime, &panicking, || panic!("the panel loop panicked"));

    // The panicking panel's name left the registry, so a request for it is
    // `no_panel` (spec "Name is free again").
    let reply = lifetime
        .registry()
        .route(
            &Request::Show {
                panel: panicking.clone(),
            },
            WAIT,
        )
        .expect("the registry answers a panel request");
    assert_eq!(
        reply.as_error().expect("an error reply").kind(),
        ErrorKind::NoPanel,
        "the panicking panel is gone"
    );

    // The other panel still answers, so the daemon serves its requests.
    let reply = lifetime
        .registry()
        .route(
            &Request::Show {
                panel: other.clone(),
            },
            WAIT,
        )
        .expect("the registry answers a panel request");
    assert!(reply.is_ok(), "the other panel still answers: {reply:?}");

    // The daemon stays up: one panel is still registered, so the last panel
    // has not ended and no shutdown was triggered (spec "Daemon lifetime
    // preference").
    assert!(!lifetime.is_shutdown(), "the daemon stays up");
    assert!(!lifetime.registry().is_empty(), "the other panel remains");

    drop(lifetime);
    panicking_thread
        .join()
        .expect("the panicking panel's stand-in ends");
    other_thread
        .join()
        .expect("the other panel's stand-in ends");
}
