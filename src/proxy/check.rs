//! The layer-shell check the daemon runs before it takes the display's socket
//! (replace-terminal-with-app-host D12, spec "Wayland layer-shell is
//! required"): pinwin runs only on a compositor that implements
//! wlr-layer-shell, so the daemon connects to the compositor once, reads its
//! global list and exits 1 with a message when the compositor does not answer
//! or does not announce `zwlr_layer_shell_v1`. There is no fallback to a
//! normal window.
//!
//! The connection is a throwaway wl-proxy `State` with no client: the check
//! sends `wl_display.get_registry` and `wl_display.sync` upstream and reads the
//! globals in the roundtrip. The decision itself is [`has_layer_shell`], a
//! pure function of the global names, so `cargo nextest` checks it without a
//! compositor (`port-to-rust` D10); only the roundtrip needs a display.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

use wl_proxy::baseline::Baseline;
use wl_proxy::object::ObjectCoreApi;
use wl_proxy::protocols::ObjectInterface;
use wl_proxy::protocols::wayland::wl_callback::{WlCallback, WlCallbackHandler};
use wl_proxy::protocols::wayland::wl_registry::{WlRegistry, WlRegistryHandler};
use wl_proxy::state::{State, StateError};

/// The interface the compositor has to announce (spec "Wayland layer-shell is
/// required").
const LAYER_SHELL: &str = "zwlr_layer_shell_v1";

/// How long the check waits for the compositor's roundtrip. A compositor that
/// answers the connect but not the roundtrip is as unusable as a missing one,
/// and the daemon must not hang at start (D12).
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// Why the compositor cannot host panels (spec "Wayland layer-shell is
/// required").
#[derive(Debug)]
pub enum CheckError {
    /// The compositor connection could not be made: no Wayland display, a
    /// missing socket, or a display that is not a Wayland display.
    Connect(StateError),
    /// The connection broke while the check waited for the globals.
    Dispatch(StateError),
    /// The compositor did not answer the roundtrip within the wait.
    NoAnswer(Duration),
    /// The compositor answered and does not implement wlr-layer-shell.
    NoLayerShell,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(error) => write!(f, "no Wayland compositor answered: {error}"),
            Self::Dispatch(error) => {
                write!(f, "the compositor's answer could not be read: {error}")
            }
            Self::NoAnswer(wait) => write!(
                f,
                "the compositor did not answer within {} ms",
                wait.as_millis()
            ),
            Self::NoLayerShell => write!(
                f,
                "the compositor does not implement wlr-layer-shell ({LAYER_SHELL}), \
                 so pinwin does not run on it"
            ),
        }
    }
}

impl std::error::Error for CheckError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect(error) | Self::Dispatch(error) => Some(error),
            Self::NoAnswer(_) | Self::NoLayerShell => None,
        }
    }
}

/// Check the compositor before the daemon takes the display's socket (D12):
/// the daemon prints the message and exits 1 when this fails, so no daemon
/// and no panel exists on a compositor that cannot host one (spec "No
/// layer-shell").
pub fn check_layer_shell() -> Result<(), CheckError> {
    let state = State::builder(Baseline::V6)
        .build()
        .map_err(CheckError::Connect)?;
    let _destructor = state.create_destructor();
    let globals = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let answered = Rc::new(Cell::new(false));
    let display = state.display();
    let registry = display.create_child::<WlRegistry>();
    registry.set_handler(CollectGlobals {
        globals: Rc::clone(&globals),
    });
    display.send_get_registry(&registry);
    let callback = display.create_child::<WlCallback>();
    callback.set_handler(RoundtripDone {
        answered: Rc::clone(&answered),
    });
    display.send_sync(&callback);
    let deadline = Instant::now() + CHECK_TIMEOUT;
    while !answered.get() {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return Err(CheckError::NoAnswer(CHECK_TIMEOUT));
        };
        state.dispatch(Some(left)).map_err(CheckError::Dispatch)?;
    }
    if has_layer_shell(&globals.borrow()) {
        Ok(())
    } else {
        Err(CheckError::NoLayerShell)
    }
}

/// The check's decision (spec "No layer-shell"): a compositor that announced
/// `zwlr_layer_shell_v1` can host a panel, and every other global list cannot.
fn has_layer_shell(globals: &[&str]) -> bool {
    globals.contains(&LAYER_SHELL)
}

/// The handler of the check's registry: it keeps the interface name of every
/// global the compositor announces.
struct CollectGlobals {
    globals: Rc<RefCell<Vec<&'static str>>>,
}

impl WlRegistryHandler for CollectGlobals {
    fn handle_global(
        &mut self,
        _slf: &Rc<WlRegistry>,
        _name: u32,
        interface: ObjectInterface,
        _version: u32,
    ) {
        self.globals.borrow_mut().push(interface.name());
    }
}

/// The handler of the check's roundtrip: the compositor answered every request
/// the check sent before the sync.
struct RoundtripDone {
    answered: Rc<Cell<bool>>,
}

impl WlCallbackHandler for RoundtripDone {
    fn handle_done(&mut self, _slf: &Rc<WlCallback>, _callback_data: u32) {
        self.answered.set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision is a pure function of the global names: a compositor that
    /// announced `zwlr_layer_shell_v1` passes and every other list fails
    /// (spec "No layer-shell").
    #[test]
    fn the_decision_reads_the_global_list() {
        assert!(has_layer_shell(&["wl_compositor", "zwlr_layer_shell_v1"]));
        assert!(has_layer_shell(&["zwlr_layer_shell_v1"]));
        assert!(!has_layer_shell(&["wl_compositor", "wl_shm"]));
        assert!(!has_layer_shell(&[]));
    }

    /// The refusal names the missing interface, so the user learns which
    /// compositor support is missing (spec "No layer-shell": pinwin prints a
    /// message).
    #[test]
    fn the_missing_global_is_named_in_the_message() {
        let message = CheckError::NoLayerShell.to_string();
        assert!(
            message.contains("zwlr_layer_shell_v1"),
            "the message names the interface: {message}"
        );
        assert!(
            message.contains("wlr-layer-shell"),
            "the message names the feature: {message}"
        );
    }
}
