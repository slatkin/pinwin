//! The daemon's `create` request (replace-terminal-with-app-host D2, spec
//! "Run an app in a panel"): resolve a new panel's settings, start its thread
//! and report the display its app connects with.
//!
//! `pinwin run` sends its own process ID, the panel's name and the run
//! options. The daemon merges the options with the config file and the
//! built-in defaults (spec "Panel settings and precedence"), starts the
//! panel's thread for that process (D1, D3) and answers with the panel's
//! `WAYLAND_DISPLAY` (D2). The panel's handle reaches the registry through
//! [`Lifetime::add_panel`], which marks the daemon eligible to exit with its
//! last panel; a refused name starts nothing and leaves the existing panel
//! alone (spec "Duplicate name").

use std::sync::Arc;

use super::lifetime::Lifetime;
use crate::config::PanelOptions;
use crate::ipc::{ErrorKind, Reply};
use crate::name::PanelName;
use crate::proxy::{self, StartError};

/// Start a panel named `name` for the process `pid` and answer with the
/// display the app connects with (D2). The options are merged with the
/// config file's tables; a name another panel uses is refused with
/// `duplicate_name` and that panel stays (spec "Duplicate name"). A panel
/// whose thread or socket cannot start is error kind `internal`, so the
/// caller reports it as the daemon's failure rather than its own.
pub fn panel(lifetime: &Arc<Lifetime>, name: PanelName, options: &PanelOptions, pid: u32) -> Reply {
    let settings = lifetime.panel_settings(&name, options);
    match proxy::spawn(name, settings, pid, lifetime) {
        Ok(started) => Reply::created(started.display().to_owned()),
        Err(StartError::Refused(error)) => Reply::Error(error),
        Err(StartError::Io(error)) => Reply::error(
            ErrorKind::Internal,
            format!("the panel did not start: {error}"),
        ),
    }
}
