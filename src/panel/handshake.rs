//! The start handshake and the bounded apply reply (port-to-rust D4/D5): the
//! small channel layer between the host thread and the panel side,
//! display-free so tests can drive every mapping and timeout outcome without
//! a Wayland connection.
//!
//! Mirrors `src/pinwin_api.c`: only the first start result counts (the
//! post-loop call cannot undo a successful start), an apply waits up to five
//! seconds for its reply and reports `Internal` on a timeout, and a panel
//! side that died mid-handshake closes the reply channel, which the waiting
//! host reports as `Internal` instead of hanging.

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::surfaces::PublishOutcome;

use super::error::PinwinError;

/// How long an apply — and a drop's teardown — waits for the panel side's
/// reply
/// (`pinwin_api.c`'s `APPLY_WAIT_TIMEOUT_US`). A wedged loop must not block
/// the host thread forever (D5).
pub(crate) const APPLY_WAIT: Duration = Duration::from_secs(5);

/// The start handshake's outcome, sent once from the panel thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartOutcome {
    /// The panel is on screen with live metrics.
    Started,
    /// The Wayland connection or a required global is unavailable (or the
    /// loop returned before the panel went live): `PinwinError::NoDisplay`.
    NoDisplay,
    /// The panel side failed unexpectedly (a caught panic during startup):
    /// `PinwinError::Internal`.
    Internal,
}

/// The one-shot start handshake: exactly one report reaches the waiting host,
/// like `pinwin_api.c`'s `g_api_start_done` latch. Cloned into every
/// panel-side closure that can complete the handshake (the start-result
/// hook, the activate path, the startup watchdog and the loop-returned
/// cleanup).
#[derive(Clone)]
pub(crate) struct Handshake(Arc<Mutex<Option<mpsc::Sender<StartOutcome>>>>);

impl Handshake {
    /// A handshake waiting for its first report.
    pub fn new(sender: mpsc::Sender<StartOutcome>) -> Self {
        Handshake(Arc::new(Mutex::new(Some(sender))))
    }

    /// Complete the handshake. Only the first report is delivered; later ones
    /// are dropped, so a cleanup path cannot undo a completed start.
    pub fn report(&self, outcome: StartOutcome) {
        if let Some(sender) = self.0.lock().expect("handshake lock").take() {
            let _ = sender.send(outcome);
        }
    }

    /// Whether the handshake was completed (successfully or not).
    pub fn resolved(&self) -> bool {
        self.0.lock().expect("handshake lock").is_none()
    }
}

/// Map an apply's `PublishOutcome` onto the `Panel` API's result (a panel
/// without live metrics is a lifecycle state, not a layout verdict).
pub(crate) fn map_outcome(outcome: PublishOutcome) -> Result<(), PinwinError> {
    match outcome {
        PublishOutcome::Applied => Ok(()),
        PublishOutcome::NotLive => Err(PinwinError::NotRunning),
        PublishOutcome::InvalidLayout => Err(PinwinError::InvalidLayout),
    }
}

/// Map a start handshake outcome onto the `Panel` API's result.
pub(crate) fn map_start(outcome: StartOutcome) -> Result<(), PinwinError> {
    match outcome {
        StartOutcome::Started => Ok(()),
        StartOutcome::NoDisplay => Err(PinwinError::NoDisplay),
        StartOutcome::Internal => Err(PinwinError::Internal),
    }
}

/// Wait for a reply that carries no outcome — a focus request's or a
/// toggle's — within `timeout`, like [`wait_for_apply`]: a timeout and a
/// closed channel are both `Internal`. The reply says only that the panel
/// side acted (a focus request was made, a toggle's hide or show committed;
/// replace-gtk-with-wayland D4 — the compositor's choice is invisible to
/// the client, and the lifecycle states short-circuit before the post).
pub(crate) fn wait_for_unit(
    receiver: &mpsc::Receiver<()>,
    timeout: Duration,
) -> Result<(), PinwinError> {
    match receiver.recv_timeout(timeout) {
        Ok(()) => Ok(()),
        Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
            Err(PinwinError::Internal)
        }
    }
}

/// Wait for the start handshake. Unbounded, like `pinwin_start`'s cond wait:
/// the panel side always reports (the start-result hook, the startup watchdog
/// or the loop-returned cleanup), and a panel thread that died closes the
/// channel, which becomes `Internal` instead of a hang.
pub(crate) fn wait_for_start(receiver: &mpsc::Receiver<StartOutcome>) -> Result<(), PinwinError> {
    match receiver.recv() {
        Ok(outcome) => map_start(outcome),
        Err(_) => Err(PinwinError::Internal),
    }
}

/// Wait for an apply's reply within [`APPLY_WAIT`] (or a caller-supplied
/// bound, for tests). A timeout and a closed channel are both `Internal`: the
/// wedged and the dead panel side are unexpected failures, not layout verdicts.
pub(crate) fn wait_for_apply(
    receiver: &mpsc::Receiver<PublishOutcome>,
    timeout: Duration,
) -> Result<(), PinwinError> {
    match receiver.recv_timeout(timeout) {
        Ok(outcome) => map_outcome(outcome),
        Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
            Err(PinwinError::Internal)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// Only the first report reaches the waiting host; later reports (the
    /// loop-returned cleanup after a successful start) are dropped.
    #[test]
    fn the_handshake_delivers_only_the_first_report() {
        let (tx, rx) = mpsc::channel();
        let handshake = Handshake::new(tx);

        assert!(!handshake.resolved());
        handshake.report(StartOutcome::Started);
        assert!(handshake.resolved());

        handshake.report(StartOutcome::Internal);
        handshake.report(StartOutcome::NoDisplay);

        assert_eq!(rx.recv().expect("first report"), StartOutcome::Started);
        // The channel is empty: the later reports were dropped.
        rx.recv_timeout(Duration::from_millis(10)).unwrap_err();
    }

    /// A panel thread that died without reporting closes the channel: the
    /// waiting start becomes `Internal`, never a hang.
    #[test]
    fn a_dropped_handshake_sender_is_internal() {
        let (tx, rx) = mpsc::channel::<StartOutcome>();
        drop(tx);
        assert_eq!(wait_for_start(&rx), Err(PinwinError::Internal));
    }

    /// The start mappings cover every outcome.
    #[test]
    fn start_outcomes_map_onto_the_api_errors() {
        assert_eq!(map_start(StartOutcome::Started), Ok(()));
        assert_eq!(
            map_start(StartOutcome::NoDisplay),
            Err(PinwinError::NoDisplay)
        );
        assert_eq!(
            map_start(StartOutcome::Internal),
            Err(PinwinError::Internal)
        );
    }

    /// The apply mappings cover every publish outcome.
    #[test]
    fn publish_outcomes_map_onto_the_api_errors() {
        assert_eq!(map_outcome(PublishOutcome::Applied), Ok(()));
        assert_eq!(
            map_outcome(PublishOutcome::NotLive),
            Err(PinwinError::NotRunning)
        );
        assert_eq!(
            map_outcome(PublishOutcome::InvalidLayout),
            Err(PinwinError::InvalidLayout)
        );
    }

    /// A reply that arrives in time maps onto its outcome.
    #[test]
    fn an_apply_reply_in_time_maps_normally() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(PublishOutcome::Applied).expect("send");
        assert_eq!(wait_for_apply(&rx, Duration::from_secs(1)), Ok(()));
    }

    /// A reply that never arrives within the bound is `Internal`, and the
    /// waiting thread is not blocked past it.
    #[test]
    fn an_apply_timeout_is_internal() {
        let (tx, rx) = mpsc::sync_channel::<PublishOutcome>(1);
        // The sender stays alive (a wedged-but-alive panel side) in another
        // thread, so the only outcome is the timeout.
        let holder = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            drop(tx);
        });
        let started = std::time::Instant::now();
        assert_eq!(
            wait_for_apply(&rx, Duration::from_millis(50)),
            Err(PinwinError::Internal)
        );
        assert!(started.elapsed() < Duration::from_millis(250), "bounded");
        holder.join().expect("holder thread");
    }

    /// A panel side that died mid-apply closes the reply channel: `Internal`,
    /// immediately rather than after the bound.
    #[test]
    fn a_disconnected_apply_reply_is_internal() {
        let (tx, rx) = mpsc::sync_channel::<PublishOutcome>(1);
        drop(tx);
        assert_eq!(
            wait_for_apply(&rx, Duration::from_secs(5)),
            Err(PinwinError::Internal)
        );
    }
}
