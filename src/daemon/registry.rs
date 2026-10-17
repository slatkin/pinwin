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

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::locked;
use super::panel::{PanelHandle, PanelReply, PanelRequest};
use crate::ipc::{ErrorKind, ErrorReply, PanelEntry, Reply, Request};
use crate::name::PanelName;

/// The daemon's panels, keyed by name (D12). Every thread that serves a
/// client connection shares it, so one registry lives in the daemon and each
/// request is routed under its lock only long enough to find the handle.
#[derive(Debug, Default)]
pub struct Registry {
    inner: Mutex<Panels>,
}

/// The registry's contents under one lock: the panels and the next start
/// order to hand out, so the start order survives a panel's removal.
#[derive(Debug, Default)]
struct Panels {
    panels: BTreeMap<PanelName, Panel>,
    next_start: u64,
}

/// One registered panel: its handle and the order it started in.
#[derive(Debug)]
struct Panel {
    handle: Arc<PanelHandle>,
    start: u64,
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
        let mut panels = locked(&self.inner);
        if panels.panels.contains_key(&name) {
            return Err(ErrorReply::new(
                ErrorKind::DuplicateName,
                format!("a panel named '{name}' already exists"),
            ));
        }
        let start = panels.next_start;
        panels.next_start += 1;
        panels.panels.insert(
            name,
            Panel {
                handle: Arc::new(handle),
                start,
            },
        );
        Ok(())
    }

    /// Drop the panel named `name` when it ends, so its name is free for the
    /// next panel (spec "Name is free again"); false when no panel had it.
    pub fn remove(&self, name: &PanelName) -> bool {
        locked(&self.inner).panels.remove(name).is_some()
    }

    /// Route one decoded request (D12). `None` means the daemon answers it
    /// itself: `subscribe` (the event bus), `quit` (the lifetime) and
    /// `reload` (the config file) name no panel.
    #[must_use]
    pub fn route(&self, request: &Request, wait: Duration) -> Option<Reply> {
        Some(match request {
            Request::Status { panel } => self.status(panel.as_ref(), wait),
            Request::Subscribe | Request::Quit | Request::Reload => return None,
            Request::Toggle { panel } => self.command(panel, PanelRequest::Toggle, wait),
            Request::Show { panel } => self.command(panel, PanelRequest::Show, wait),
            Request::Hide { panel } => self.command(panel, PanelRequest::Hide, wait),
            Request::Focus { panel } => self.command(panel, PanelRequest::Focus, wait),
            Request::SetEdge { panel, edge } => {
                self.command(panel, PanelRequest::SetEdge(*edge), wait)
            }
            Request::SetSize { panel, size, ms } => self.command(
                panel,
                PanelRequest::SetSize {
                    size: *size,
                    ms: *ms,
                },
                wait,
            ),
            Request::Resize { panel, by, ms } => {
                self.command(panel, PanelRequest::Resize { by: *by, ms: *ms }, wait)
            }
            Request::SetOutput { panel, output } => {
                self.command(panel, PanelRequest::SetOutput(output.clone()), wait)
            }
            Request::SetLayer { panel, layer } => {
                self.command(panel, PanelRequest::SetLayer(*layer), wait)
            }
            Request::SetReserve { panel, reserve } => {
                self.command(panel, PanelRequest::SetReserve(*reserve), wait)
            }
            Request::SetKeyboard { panel, keyboard } => {
                self.command(panel, PanelRequest::SetKeyboard(*keyboard), wait)
            }
        })
    }

    /// Ask one panel to carry a command out and return its answer as the
    /// reply the client gets (spec "Panel commands", "Status").
    fn command(&self, name: &PanelName, command: PanelRequest, wait: Duration) -> Reply {
        let handle = match self.handle(name) {
            Ok(handle) => handle,
            Err(reply) => return reply,
        };
        match handle.call(command, wait) {
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
    fn status(&self, panel: Option<&PanelName>, wait: Duration) -> Reply {
        let panels = match self.panels(panel) {
            Ok(panels) => panels,
            Err(reply) => return reply,
        };
        let mut entries = Vec::with_capacity(panels.len());
        for (name, handle) in panels {
            match entry(&name, &handle, wait) {
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
        let panels = locked(&self.inner);
        let Some(panel) = panel else {
            let mut started: Vec<(u64, &PanelName, &Panel)> = panels
                .panels
                .iter()
                .map(|(name, panel)| (panel.start, name, panel))
                .collect();
            started.sort_by_key(|(start, ..)| *start);
            return Ok(started
                .into_iter()
                .map(|(_, name, panel)| (name.clone(), Arc::clone(&panel.handle)))
                .collect());
        };
        match panels.panels.get(panel) {
            Some(found) => Ok(vec![(panel.clone(), Arc::clone(&found.handle))]),
            None => Err(no_panel(panel)),
        }
    }

    /// The handle of the panel named `name`, or the reply that says no panel
    /// has that name (spec "Unknown panel").
    fn handle(&self, name: &PanelName) -> Result<Arc<PanelHandle>, Reply> {
        locked(&self.inner)
            .panels
            .get(name)
            .map(|panel| Arc::clone(&panel.handle))
            .ok_or_else(|| no_panel(name))
    }
}

/// The status entry of one panel: its name and its app's process ID from the
/// registry, the rest from the panel itself.
fn entry(name: &PanelName, handle: &PanelHandle, wait: Duration) -> Result<PanelEntry, Reply> {
    match handle.call(PanelRequest::Status, wait) {
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
