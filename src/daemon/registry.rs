//! The daemon's registry of panels and the routing of a decoded request to
//! the panel it names (replace-terminal-with-app-host D12).
//!
//! The registry maps a [`PanelName`] to its [`PanelHandle`] and keeps the
//! order the panels started in, so a status reply lists them in that order
//! (spec "Status"). A name is unique: adding a panel under a name another
//! panel uses is refused with `duplicate_name` and the existing panel stays
//! untouched (spec "Duplicate name"), and removing a panel frees its name
//! for the next one (spec "Name is free again").
//!
//! [`Registry::route`] takes a decoded [`Request`] and turns it into an
//! [`ipc::Reply`]: a panel command goes to its panel and the reply is the
//! panel's, a status asks each panel and assembles the entries, and the
//! requests the daemon answers itself (`subscribe`, `quit`, `reload`) return
//! [`None`], so the event bus, the lifetime and the config reload own them
//! (rows 5.4 and 5.5). A wait bounds every call, so a panel that does not
//! answer fails its request with error kind `internal` (spec "The daemon
//! survives bad input").

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::locked;
use super::panel::{CommandError, PanelHandle, PanelReply, PendingCall};
use crate::ipc::{ErrorKind, ErrorReply, PanelEntry, Reply, Request};
use crate::name::PanelName;

/// The daemon's panels in the order they started (D12). Every thread that
/// serves a client connection shares it, so one registry lives in the daemon
/// and each request is routed under its lock only long enough to find the
/// handle. A panel's push order is its start order, so removal keeps the
/// order of the remaining panels without a counter or a sort.
#[derive(Debug, Default)]
pub struct Registry {
    panels: Mutex<Vec<(PanelName, Arc<PanelHandle>)>>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `handle` under `name` (spec "Panel names"). A name another
    /// panel already uses is refused with `duplicate_name`, and that panel
    /// and its name stay unchanged (spec "Duplicate name").
    pub fn add(&self, name: PanelName, handle: PanelHandle) -> Result<(), ErrorReply> {
        let mut panels = locked(&self.panels);
        if panels.iter().any(|(existing, _)| existing == &name) {
            return Err(ErrorReply::new(
                ErrorKind::DuplicateName,
                format!("a panel named '{name}' already exists"),
            ));
        }
        panels.push((name, Arc::new(handle)));
        Ok(())
    }

    /// Drop the panel named `name` when it ends, so its name is free for the
    /// next panel (spec "Name is free again"); false when no panel had it.
    pub fn remove(&self, name: &PanelName) -> bool {
        let mut panels = locked(&self.panels);
        let before = panels.len();
        panels.retain(|(existing, _)| existing != name);
        panels.len() != before
    }

    /// Remove every panel, so a `quit` ends each panel thread by closing its
    /// command channel (spec "Quit").
    pub fn clear(&self) {
        locked(&self.panels).clear();
    }

    /// Whether no panel is registered, so the daemon's lifetime can tell
    /// when the last panel ended (spec "Daemon lifetime preference").
    #[must_use]
    pub fn is_empty(&self) -> bool {
        locked(&self.panels).is_empty()
    }

    /// The app id that names the tray icon (row 11.1, D14): the oldest
    /// panel's, or `None` when no panel exists or its app has not named
    /// itself yet. Only the oldest panel is read, because a later panel's
    /// app id never stands in for it.
    #[must_use]
    pub fn oldest_app_id(&self) -> Option<String> {
        locked(&self.panels)
            .first()
            .and_then(|(_, handle)| handle.app_id())
    }

    /// Route one decoded request (D12). `None` means the daemon answers it
    /// itself: `create` (a new panel, D2), `subscribe` (the event bus),
    /// `quit` (the lifetime) and `reload` (the config file) name no panel.
    #[must_use]
    pub fn route(&self, request: &Request, wait: Duration) -> Option<Reply> {
        Some(match request {
            Request::Status { panel } => self.status(panel.as_ref(), request, wait),
            Request::Create { .. } | Request::Subscribe | Request::Quit | Request::Reload => {
                return None;
            }
            Request::Toggle { panel }
            | Request::Show { panel }
            | Request::Hide { panel }
            | Request::Focus { panel }
            | Request::SetEdge { panel, .. }
            | Request::SetSize { panel, .. }
            | Request::Resize { panel, .. }
            | Request::SetGutters { panel, .. }
            | Request::SetOutput { panel, .. }
            | Request::SetLayer { panel, .. }
            | Request::SetReserve { panel, .. }
            | Request::SetKeyboard { panel, .. } => self.command(panel, request, wait),
        })
    }

    /// Ask one panel to carry a command out and return its answer as the
    /// reply the client gets (spec "Panel commands", "Status").
    fn command(&self, name: &PanelName, request: &Request, wait: Duration) -> Reply {
        let handle = match self.handle(name) {
            Ok(handle) => handle,
            Err(reply) => return reply,
        };
        match handle.call(request, wait) {
            Ok(PanelReply::Done) => Reply::ok(),
            Ok(PanelReply::Refused(error)) => Reply::Error(error),
            // A panel that answers a change with its status is out of step
            // with the command it was sent, so the request cannot succeed.
            Ok(PanelReply::Status(_)) => Reply::error(
                ErrorKind::Internal,
                format!("{name}: the panel reported a status instead of carrying the command out"),
            ),
            Err(error) => Reply::error(ErrorKind::Internal, format!("{name}: {error}")),
        }
    }

    /// The status reply of every panel, in start order, or of `panel` alone
    /// (spec "Status of all panels", "Status of one panel").
    fn status(&self, panel: Option<&PanelName>, request: &Request, wait: Duration) -> Reply {
        let panels = match self.panels(panel) {
            Ok(panels) => panels,
            Err(reply) => return reply,
        };
        // Ask every panel first, then collect, so the wait is the slowest
        // panel's and not the sum.
        let pending: Vec<_> = panels
            .iter()
            .map(|(_, handle)| handle.send(request, wait))
            .collect();
        let mut entries = Vec::with_capacity(panels.len());
        for ((name, handle), pending) in panels.iter().zip(pending) {
            match entry(name, handle, pending.and_then(PendingCall::finish)) {
                Ok(entry) => entries.push(entry),
                Err(reply) => return reply,
            }
        }
        Reply::status(std::process::id(), entries)
    }

    /// The registry entries to report: one panel's, or every panel in start
    /// order. A name no panel has is `no_panel` (spec "Unknown panel").
    fn panels(
        &self,
        panel: Option<&PanelName>,
    ) -> Result<Vec<(PanelName, Arc<PanelHandle>)>, Reply> {
        let panels = locked(&self.panels);
        let Some(panel) = panel else {
            return Ok(panels
                .iter()
                .map(|(name, handle)| (name.clone(), Arc::clone(handle)))
                .collect());
        };
        match panels.iter().find(|(name, _)| name == panel) {
            Some((name, handle)) => Ok(vec![(name.clone(), Arc::clone(handle))]),
            None => Err(no_panel(panel)),
        }
    }

    /// The handle of the panel named `name`, or the reply that says no panel
    /// has that name (spec "Unknown panel").
    fn handle(&self, name: &PanelName) -> Result<Arc<PanelHandle>, Reply> {
        locked(&self.panels)
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, handle)| Arc::clone(handle))
            .ok_or_else(|| no_panel(name))
    }
}

/// The status entry of one panel: its name and its app's process ID from the
/// registry, the rest from the panel itself.
fn entry(
    name: &PanelName,
    handle: &PanelHandle,
    answer: Result<PanelReply, CommandError>,
) -> Result<PanelEntry, Reply> {
    match answer {
        Ok(PanelReply::Status(status)) => Ok(PanelEntry::new(name.clone(), handle.pid(), status)),
        Ok(PanelReply::Refused(error)) => Err(Reply::Error(error)),
        Ok(PanelReply::Done) => Err(Reply::error(
            ErrorKind::Internal,
            format!("{name}: the panel reported no status"),
        )),
        Err(error) => Err(Reply::error(
            ErrorKind::Internal,
            format!("{name}: {error}"),
        )),
    }
}

/// The reply for a request that names a panel no panel has (spec "Unknown
/// panel").
fn no_panel(name: &PanelName) -> Reply {
    Reply::error(ErrorKind::NoPanel, format!("no panel named '{name}'"))
}
