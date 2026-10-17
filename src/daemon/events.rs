//! The daemon's event bus (replace-terminal-with-app-host D12, spec "Event
//! stream"): one bounded queue for each subscriber and the events a panel
//! publishes.
//!
//! A subscriber's connection thread takes the events from its queue and
//! writes each as one JSON line (D13), after the success reply to its
//! `subscribe` request. [`Bus::publish`] never waits: it offers the event to
//! every queue with `try_send`, and a queue that is full closes that
//! subscriber (D12: "each subscriber has a bounded queue, and a full queue
//! closes that subscriber"). A client that stops reading therefore holds
//! only its own queue and connection thread, never the daemon, a panel or
//! another client (spec "Slow subscriber", "Clients cannot block the
//! daemon").
//!
//! The queue depth is a parameter, so a test fills a queue in a few events
//! instead of the production depth (`port-to-rust` D10).

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use super::locked;
use crate::name::PanelName;

/// The queue depth of one subscriber (D12): deep enough for a burst of panel
/// events, shallow enough that a client which stops reading holds only this
/// many events before the bus closes it.
pub const QUEUE_DEPTH: usize = 64;

/// One event of the stream (spec "Event stream"): a panel was added,
/// removed, shown or hidden, or its geometry changed. Every event names the
/// panel, which is the object `panel` of its JSON line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// A panel was added.
    Added { panel: PanelName },
    /// A panel ended.
    Removed { panel: PanelName },
    /// A panel was shown.
    Shown { panel: PanelName },
    /// A panel was hidden.
    Hidden { panel: PanelName },
    /// A panel's geometry changed.
    GeometryChanged { panel: PanelName },
}

impl Event {
    /// The line the daemon writes to a subscriber: the JSON object plus its
    /// newline (D13).
    ///
    /// # Panics
    ///
    /// Only if the `Serialize` impl fails, which no wire type here can do.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("an event is serializable");
        line.push('\n');
        line
    }
}

/// The daemon's event bus (D12). Cloning a bus shares it, so the daemon's
/// connection threads and the panels publish through the same queues.
#[derive(Clone, Debug)]
pub struct Bus {
    queue_depth: usize,
    inner: Arc<Mutex<Subscribers>>,
}

/// Every subscriber's queue under one lock, keyed by the id that removes it
/// again when its subscription drops or its queue fills.
#[derive(Debug, Default)]
struct Subscribers {
    next_id: u64,
    queues: BTreeMap<u64, SyncSender<Event>>,
}

impl Bus {
    /// A bus whose subscribers each queue [`QUEUE_DEPTH`] events.
    #[must_use]
    pub fn new() -> Self {
        Self::with_queue_depth(QUEUE_DEPTH)
    }

    /// A bus with a queue of `queue_depth` events for each subscriber. A
    /// depth of zero closes a subscriber that is not already waiting for an
    /// event on its next publish.
    #[must_use]
    pub fn with_queue_depth(queue_depth: usize) -> Self {
        Self {
            queue_depth,
            inner: Arc::new(Mutex::new(Subscribers::default())),
        }
    }

    /// Add a subscriber and return the receiving end of its queue.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        let (queue, events) = mpsc::sync_channel(self.queue_depth);
        let mut subscribers = locked(&self.inner);
        let id = subscribers.next_id;
        subscribers.next_id += 1;
        subscribers.queues.insert(id, queue);
        Subscription {
            id,
            events,
            inner: Arc::clone(&self.inner),
        }
    }

    /// Give `event` to every subscriber whose queue has room. A queue that
    /// is full closes its subscriber (D12): its sender is dropped, so the
    /// connection thread finishes the events it has and ends. Publishing
    /// never waits, so a subscriber that stopped reading cannot hold the
    /// daemon (spec "Slow subscriber").
    pub fn publish(&self, event: &Event) {
        let mut subscribers = locked(&self.inner);
        let mut closed = Vec::new();
        for (id, queue) in &subscribers.queues {
            match queue.try_send(event.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => closed.push(*id),
            }
        }
        for id in closed {
            subscribers.queues.remove(&id);
        }
    }

    /// How many subscribers the bus has, so a test can check that a closed
    /// connection freed its queue.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        locked(&self.inner).queues.len()
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

/// One subscriber's receiving end. Dropping it removes the queue from the
/// bus, so a closed connection stops collecting events.
#[derive(Debug)]
pub struct Subscription {
    id: u64,
    events: Receiver<Event>,
    inner: Arc<Mutex<Subscribers>>,
}

impl Subscription {
    /// Wait at most `timeout` for the next event: an event, the timeout, or
    /// the bus closing this queue because it fell behind.
    pub fn next(&self, timeout: Duration) -> Result<Event, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        locked(&self.inner).queues.remove(&self.id);
    }
}

// The path is explicit because `tests/daemon.rs` includes this file with
// `#[path]`, and a module loaded that way searches its own children in the
// included file's directory rather than beside it (as in `src/ipc.rs`).
#[cfg(test)]
#[path = "events/tests.rs"]
mod tests;
