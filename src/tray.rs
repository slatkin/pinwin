//! The tray icon (replace-terminal-with-app-host D14, spec "One tray icon
//! while panels exist", "No tray is not an error", "Menu for each panel",
//! "Quit from the menu" and "Icon click opens nothing new").
//!
//! The tray is one `StatusNotifierItem` on the session bus. [`TrayService`]
//! starts it with `ksni` (the `blocking` API on `async-io`, not tokio) when
//! the first panel appears and shuts it down when the last panel ends, so no
//! icon shows while no panel exists. `ksni` re-registers the item when a tray
//! host starts later; a session bus that cannot be reached is reported once
//! and otherwise ignored, so the daemon, its panels and the IPC work the
//! same without a tray.
//!
//! [`MenuModel`] is the pure menu content: one submenu for each panel in the
//! order the panels started, carrying the panel's current shown state, edge
//! and reserve mode. The model comes from the same `status` reply the IPC
//! answers, and every menu action sends the same [`Request`] through the
//! same registry the IPC routes, so the tray is one more client and not a
//! second implementation (D14). "Quit pinwin" calls [`Lifetime::quit`], as
//! the `quit` request does.
//!
//! A primary click does nothing (spec "Icon click opens nothing new"): the
//! menu is the only way the tray changes a panel. The item's icon names the
//! oldest running panel's app: its `app_id`, or the theme's [`ICON_NAME`] as
//! the fallback; `ksni` re-registers and re-reads the icon when a tray host
//! appears or an app names itself.

use std::fmt;
use std::sync::Weak;
use std::time::Duration;

use ksni::blocking::{Handle, TrayMethods as _};
use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};

use crate::daemon::lifetime::Lifetime;
use crate::geometry::{Edge, ReserveMode};
use crate::ipc::{PanelEntry, Reply, Request};
use crate::name::PanelName;

/// The theme icon the tray shows when the oldest panel's app has set no valid
/// `app_id` (D14: a themed icon name is enough for the first release). The
/// tray host renders it from the user's icon theme; pinwin installs no icon
/// file of its own.
pub const ICON_NAME: &str = "pinwin";

/// The theme icon name the item shows (row 11.1, D14, spec "Icon of the
/// hosted app", "Icon follows the oldest panel", "Icon before the app names
/// itself" and "Unusable app id"): the oldest panel's `app_id` when it is a
/// valid theme icon name, and [`ICON_NAME`] otherwise. `panels` holds each
/// panel's app id in start order, `None` before its app names itself; the
/// oldest panel decides, so a later panel's app id does not stand in while
/// the oldest one has none.
#[must_use]
pub fn icon_for(panels: &[Option<String>]) -> String {
    panels
        .first()
        .and_then(|app_id| app_id.as_deref())
        .and_then(theme_name)
        .map_or_else(|| ICON_NAME.to_owned(), str::to_owned)
}

/// The theme icon name `app_id` may be used as, or `None` when it is not valid
/// (row 11.1, D14, spec "One tray icon while panels exist"): 1 to 128
/// characters from letters, digits, `.`, `_` and `-`, not starting with `.`.
/// A host may take a theme name that looks like a path as a file, so a
/// separator fails the character rule too.
fn theme_name(app_id: &str) -> Option<&str> {
    let allowed = app_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    let valid = (1..=128).contains(&app_id.chars().count()) && !app_id.starts_with('.') && allowed;
    valid.then_some(app_id)
}

/// The check item that shows or hides a panel (spec "Menu for each panel").
const SHOWN_LABEL: &str = "Shown";

/// The item that ends every panel and the daemon (spec "Quit from the
/// menu").
const QUIT_LABEL: &str = "Quit pinwin";

/// The edge choices of a panel submenu, in the order the spec lists them.
const EDGES: [(Edge, &str); 4] = [
    (Edge::Left, "left"),
    (Edge::Right, "right"),
    (Edge::Top, "top"),
    (Edge::Bottom, "bottom"),
];

/// The reserve choices of a panel submenu.
const RESERVES: [(ReserveMode, &str); 2] =
    [(ReserveMode::Push, "push"), (ReserveMode::Cover, "cover")];

/// One panel as its tray submenu shows it (spec "Menu for each panel"): its
/// name, whether it is shown, its edge and its reserve mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelMenu {
    name: PanelName,
    shown: bool,
    edge: Edge,
    reserve: ReserveMode,
}

impl PanelMenu {
    /// The submenu state of `name`, which is shown or hidden, docks at `edge`
    /// and pushes or covers with `reserve`.
    #[must_use]
    pub const fn new(name: PanelName, shown: bool, edge: Edge, reserve: ReserveMode) -> Self {
        Self {
            name,
            shown,
            edge,
            reserve,
        }
    }

    /// The panel's name, which labels its submenu.
    #[must_use]
    pub const fn name(&self) -> &PanelName {
        &self.name
    }

    /// Whether the panel is shown, which the "Shown" item checks.
    #[must_use]
    pub const fn shown(&self) -> bool {
        self.shown
    }

    /// The edge the panel docks at, which its edge group selects.
    #[must_use]
    pub const fn edge(&self) -> Edge {
        self.edge
    }

    /// Whether the panel pushes or covers, which its reserve group selects.
    #[must_use]
    pub const fn reserve(&self) -> ReserveMode {
        self.reserve
    }
}

/// The pure tray menu (spec "Menu for each panel", "Submenus follow
/// panels"): one [`PanelMenu`] for each panel, in the order the panels
/// started. The daemon's registry builds the status reply in that order, so
/// the model keeps it as it arrives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuModel {
    panels: Vec<PanelMenu>,
}

impl MenuModel {
    /// A model of `panels`, already in start order.
    #[cfg(test)]
    #[must_use]
    pub const fn new(panels: Vec<PanelMenu>) -> Self {
        Self { panels }
    }

    /// The model of a `status` reply: its panels in start order with their
    /// current state. A reply that is not a status (a panel failed to answer
    /// within the wait) gives an empty model, so the menu keeps only "Quit
    /// pinwin" until the next refresh.
    #[must_use]
    pub fn from_reply(reply: &Reply) -> Self {
        reply
            .as_status()
            .map_or_else(Self::default, |status| Self::from_entries(status.panels()))
    }

    /// The model of the status entries of each panel, in the order the
    /// registry lists them.
    #[must_use]
    pub fn from_entries(entries: &[PanelEntry]) -> Self {
        let panels = entries
            .iter()
            .map(|entry| {
                PanelMenu::new(
                    entry.name().clone(),
                    entry.shown(),
                    entry.edge(),
                    entry.reserve(),
                )
            })
            .collect();
        Self { panels }
    }

    /// The panels the menu lists, in start order.
    #[must_use]
    pub fn panels(&self) -> &[PanelMenu] {
        &self.panels
    }
}

/// The running tray service (D14). It owns the `ksni` handle; dropping it
/// without [`TrayService::stop`] would leave the service running, so the
/// daemon always stops it when the last panel ends.
pub struct TrayService {
    handle: Handle<PinwinTray>,
}

impl fmt::Debug for TrayService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrayService").finish_non_exhaustive()
    }
}

impl TrayService {
    /// Start the tray item for a daemon whose lifetime is `lifetime`; its
    /// menu changes a panel within `wait`. A session bus that cannot be
    /// reached is reported once and the daemon carries on without an icon
    /// (spec "No tray is not an error"); `None` means no tray runs.
    ///
    /// The service assumes the `StatusNotifierWatcher` may appear later, so a
    /// session that has no tray host yet still gets the icon when the host
    /// starts (spec "Tray host starts later").
    #[must_use]
    pub fn start(lifetime: Weak<Lifetime>, wait: Duration) -> Option<Self> {
        let tray = PinwinTray {
            lifetime,
            wait,
            model: MenuModel::default(),
        };
        match tray.assume_sni_available(true).spawn() {
            Ok(handle) => Some(Self { handle }),
            Err(error) => {
                eprintln!("pinwin: no tray icon: {error}");
                None
            }
        }
    }

    /// Ask the tray service to stop, so the icon goes away with the last
    /// panel (spec "No icon without panels"). The request is sent without
    /// waiting, so the panel thread that reports the last panel's end cannot
    /// block on the tray's own event loop.
    pub fn stop(self) {
        drop(self.handle.shutdown());
    }

    /// A clone of the `ksni` handle, so a caller can ask the tray to re-read
    /// its item (the icon names the oldest panel's app id, row 11.1, D14)
    /// without holding the daemon's tray lock across the tray thread's work.
    pub(crate) fn handle(&self) -> Handle<PinwinTray> {
        self.handle.clone()
    }
}

/// The `StatusNotifierItem` (spec "One tray icon while panels exist"): its one
/// menu lists the panels and ends the daemon.
#[derive(Debug)]
pub(crate) struct PinwinTray {
    /// The daemon whose registry the menu reads and acts on. Weak, so the
    /// service cannot keep the daemon alive and no cycle outlives it.
    lifetime: Weak<Lifetime>,
    /// The bound of each panel command and status the menu asks for.
    wait: Duration,
    /// The panels the menu shows, refreshed when the menu opens and after
    /// each action so it follows the IPC (spec "Menu follows the IPC").
    model: MenuModel,
}

impl PinwinTray {
    /// Re-read the panels' current state from the daemon's registry. The
    /// registry answers in start order, so the submenus stay in that order
    /// after a panel ends (spec "Submenus follow panels").
    fn refresh(&mut self) {
        let Some(lifetime) = self.lifetime.upgrade() else {
            return;
        };
        let reply = lifetime
            .registry()
            .route(&Request::Status { panel: None }, self.wait);
        if let Some(reply) = reply {
            self.model = MenuModel::from_reply(&reply);
        }
    }

    /// Send one menu action through the same registry the IPC routes to
    /// (D14), then refresh the menu so the item shows the new state.
    fn act(&mut self, request: &Request) {
        let Some(lifetime) = self.lifetime.upgrade() else {
            return;
        };
        let _ = lifetime.registry().route(request, self.wait);
        self.refresh();
    }

    /// End every panel and then the daemon, as the `quit` request does
    /// (spec "Quit from the menu", D14).
    fn quit(&mut self) {
        if let Some(lifetime) = self.lifetime.upgrade() {
            lifetime.quit();
        }
    }

    /// The submenu of one panel: its "Shown" check item, its edge group and
    /// its reserve group (spec "Menu for each panel").
    fn panel_item(panel: &PanelMenu) -> MenuItem<Self> {
        let toggle = panel.name().clone();
        let edge_name = panel.name().clone();
        let reserve_name = panel.name().clone();
        SubMenu {
            label: panel.name().as_str().to_owned(),
            submenu: vec![
                CheckmarkItem {
                    label: SHOWN_LABEL.to_owned(),
                    checked: panel.shown(),
                    activate: Box::new(move |tray: &mut Self| {
                        tray.act(&Request::Toggle {
                            panel: toggle.clone(),
                        });
                    }),
                    ..Default::default()
                }
                .into(),
                RadioGroup {
                    selected: edge_index(panel.edge()),
                    select: Box::new(move |tray: &mut Self, index: usize| {
                        if let Some((edge, _)) = EDGES.get(index) {
                            tray.act(&Request::SetEdge {
                                panel: edge_name.clone(),
                                edge: *edge,
                            });
                        }
                    }),
                    options: EDGES
                        .iter()
                        .map(|(_, label)| RadioItem {
                            label: (*label).to_owned(),
                            ..Default::default()
                        })
                        .collect(),
                }
                .into(),
                RadioGroup {
                    selected: reserve_index(panel.reserve()),
                    select: Box::new(move |tray: &mut Self, index: usize| {
                        if let Some((reserve, _)) = RESERVES.get(index) {
                            tray.act(&Request::SetReserve {
                                panel: reserve_name.clone(),
                                reserve: *reserve,
                            });
                        }
                    }),
                    options: RESERVES
                        .iter()
                        .map(|(_, label)| RadioItem {
                            label: (*label).to_owned(),
                            ..Default::default()
                        })
                        .collect(),
                }
                .into(),
            ],
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for PinwinTray {
    fn id(&self) -> String {
        "pinwin".to_owned()
    }

    fn title(&self) -> String {
        "pinwin".to_owned()
    }

    /// The icon names the oldest running panel's app id, with [`ICON_NAME`]
    /// as the fallback (row 11.1, D14, spec "Icon of the hosted app", "Icon
    /// before the app names itself", "Unusable app id"). The daemon asks
    /// `ksni` to re-read it when an app names itself or the oldest panel
    /// ends.
    fn icon_name(&self) -> String {
        let Some(lifetime) = self.lifetime.upgrade() else {
            return ICON_NAME.to_owned();
        };
        icon_for(&lifetime.registry().app_ids())
    }

    /// A primary click changes nothing (spec "Icon click opens nothing
    /// new"), so it keeps the default no-op and does not open the menu
    /// either.
    fn activate(&mut self, _x: i32, _y: i32) {}

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.model
            .panels()
            .iter()
            .map(Self::panel_item)
            .chain(std::iter::once(
                StandardItem {
                    label: QUIT_LABEL.to_owned(),
                    activate: Box::new(|tray: &mut Self| tray.quit()),
                    ..Default::default()
                }
                .into(),
            ))
            .collect()
    }

    /// The menu is about to open: read the panels' current state first, so a
    /// change made through the IPC shows up when the user looks (spec "Menu
    /// follows the IPC").
    fn menu_about_to_show(&mut self) {
        self.refresh();
    }
}

/// The index of `edge` in the edge group.
fn edge_index(edge: Edge) -> usize {
    EDGES
        .iter()
        .position(|(candidate, _)| *candidate == edge)
        .unwrap_or(0)
}

/// The index of `reserve` in the reserve group.
fn reserve_index(reserve: ReserveMode) -> usize {
    RESERVES
        .iter()
        .position(|(candidate, _)| *candidate == reserve)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
