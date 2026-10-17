//! The pure tray-menu tests of replace-terminal-with-app-host row 11.1
//! (spec "Menu for each panel", "Submenus follow panels", "Quit from the
//! menu" and "Icon click opens nothing new"). The model and the menu layout
//! are display-free, so these tests need no session bus and no tray host.

use std::sync::Weak;
use std::time::Duration;

use ksni::Tray as _;
use ksni::menu::MenuItem;

use super::{
    EDGES, ICON_NAME, MenuModel, PanelMenu, PinwinTray, QUIT_LABEL, RESERVES, SHOWN_LABEL,
    edge_index, icon_for, reserve_index, theme_name,
};
use crate::config::PanelSettings;
use crate::daemon::lifetime::Lifetime;
use crate::geometry::{Edge, ReserveMode};
use crate::ipc::{PanelEntry, PanelStatus, Reply};
use crate::name::PanelName;

/// The panel name `name`.
fn panel_name(name: &str) -> PanelName {
    PanelName::parse(name).expect("a panel name")
}

/// One status entry for `name`, shown or hidden, docked at `edge` with
/// `reserve`.
fn entry(name: &str, shown: bool, edge: Edge, reserve: ReserveMode) -> PanelEntry {
    let settings = PanelSettings::built_in()
        .with_edge(edge)
        .with_reserve(reserve);
    PanelEntry::new(
        panel_name(name),
        11,
        PanelStatus::new(settings, shown, true, 640),
    )
}

/// A tray item with `model` and no daemon behind it, for the pure menu
/// checks.
fn tray(model: MenuModel) -> PinwinTray {
    PinwinTray {
        lifetime: Weak::<Lifetime>::new(),
        wait: Duration::ZERO,
        model,
    }
}

/// The menu model lists the panels in start order and carries each panel's
/// current shown state, edge and reserve mode (spec "Menu for each panel",
/// "Submenus follow panels").
#[test]
fn the_model_lists_panels_in_start_order_with_their_state() {
    let reply = Reply::status(
        1,
        vec![
            entry("notes", true, Edge::Left, ReserveMode::Push),
            entry("chat", false, Edge::Right, ReserveMode::Cover),
        ],
    );
    let model = MenuModel::from_reply(&reply);
    let panels = model.panels();

    assert_eq!(panels.len(), 2, "one submenu per panel: {panels:?}");
    assert_eq!(
        panels[0].name().as_str(),
        "notes",
        "start order: {panels:?}"
    );
    assert_eq!(panels[1].name().as_str(), "chat", "start order: {panels:?}");
    assert!(panels[0].shown(), "notes is shown");
    assert!(!panels[1].shown(), "chat is hidden");
    assert_eq!(panels[0].edge(), Edge::Left);
    assert_eq!(panels[1].edge(), Edge::Right);
    assert_eq!(panels[0].reserve(), ReserveMode::Push);
    assert_eq!(panels[1].reserve(), ReserveMode::Cover);
}

/// A reply that is not a status (a panel did not answer) gives an empty
/// model, so the menu keeps only "Quit pinwin" until the next refresh.
#[test]
fn an_error_reply_gives_an_empty_model() {
    let reply = Reply::error(crate::ipc::ErrorKind::Internal, "no answer");
    assert_eq!(MenuModel::from_reply(&reply), MenuModel::default());
}

/// The menu has one submenu for each panel in start order, then "Quit
/// pinwin"; each submenu has the "Shown" check item, the four edge choices
/// and the two reserve choices, with the panel's state selected (spec "Menu
/// for each panel", "Quit from the menu").
#[test]
fn the_menu_has_a_submenu_for_each_panel_and_then_quit() {
    let model = MenuModel::new(vec![
        PanelMenu::new(panel_name("notes"), true, Edge::Left, ReserveMode::Push),
        PanelMenu::new(panel_name("chat"), false, Edge::Right, ReserveMode::Cover),
    ]);
    let items = tray(model).menu();

    assert_eq!(items.len(), 3, "two panels and Quit pinwin");
    let MenuItem::SubMenu(notes) = &items[0] else {
        panic!("the first panel is a submenu: {:?}", items.len());
    };
    assert_eq!(notes.label, "notes");
    assert_eq!(notes.submenu.len(), 3, "Shown, edge and reserve");
    let MenuItem::Checkmark(shown) = &notes.submenu[0] else {
        panic!("the first item of a panel submenu is Shown");
    };
    assert_eq!(shown.label, SHOWN_LABEL);
    assert!(shown.checked, "notes is shown");
    let MenuItem::RadioGroup(edges) = &notes.submenu[1] else {
        panic!("the second item of a panel submenu is the edge choice");
    };
    assert_eq!(edges.selected, edge_index(Edge::Left));
    assert_eq!(edges.options.len(), EDGES.len());
    assert_eq!(
        edges
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>(),
        EDGES.iter().map(|(_, label)| *label).collect::<Vec<_>>()
    );
    let MenuItem::RadioGroup(reserves) = &notes.submenu[2] else {
        panic!("the third item of a panel submenu is the reserve choice");
    };
    assert_eq!(reserves.selected, reserve_index(ReserveMode::Push));
    assert_eq!(
        reserves
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>(),
        RESERVES.iter().map(|(_, label)| *label).collect::<Vec<_>>()
    );

    let MenuItem::SubMenu(chat) = &items[1] else {
        panic!("the second panel is a submenu");
    };
    assert_eq!(chat.label, "chat");
    let MenuItem::Checkmark(chat_shown) = &chat.submenu[0] else {
        panic!("chat's first item is Shown");
    };
    assert!(!chat_shown.checked, "chat is hidden");
    let MenuItem::RadioGroup(chat_edges) = &chat.submenu[1] else {
        panic!("chat's second item is the edge choice");
    };
    assert_eq!(chat_edges.selected, edge_index(Edge::Right));

    let MenuItem::Standard(quit) = &items[2] else {
        panic!("Quit pinwin follows the panel submenus");
    };
    assert_eq!(quit.label, QUIT_LABEL);
    assert_eq!(QUIT_LABEL, "Quit pinwin");
}

/// The icon names the oldest panel's app id, and `pinwin` when the oldest
/// panel's app has not named itself yet (spec "Icon with panels", "Icon of
/// the hosted app", "Icon follows the oldest panel" and "Icon before the app
/// names itself"): a later panel's app id does not stand in for the oldest
/// one, which decides the icon.
#[test]
fn the_icon_follows_the_oldest_panels_app_id() {
    assert_eq!(icon_for(&[]), ICON_NAME, "no panel, so no app id");
    assert_eq!(
        icon_for(&[None, Some("chat".to_owned())]),
        ICON_NAME,
        "the oldest panel has not named itself yet"
    );
    assert_eq!(
        icon_for(&[Some("org.example.Notes".to_owned())]),
        "org.example.Notes"
    );
    assert_eq!(
        icon_for(&[Some("notes".to_owned()), Some("chat".to_owned())]),
        "notes",
        "the oldest panel's app id wins"
    );
}

/// An app id that is not a plain theme icon name is unusable, so the icon
/// falls back to `pinwin` (spec "Unusable app id"): 1 to 128 characters from
/// letters, digits, `.`, `_` and `-`, not starting with `.`, and no path
/// separator.
#[test]
fn an_unusable_app_id_gives_the_fallback_icon() {
    let longest = "x".repeat(128);
    for app_id in [
        String::new(),
        ".hidden".to_owned(),
        "a/b".to_owned(),
        "/absolute".to_owned(),
        "with space".to_owned(),
        "ümlaut".to_owned(),
        "x".repeat(129),
    ] {
        assert_eq!(
            icon_for(&[Some(app_id.clone())]),
            ICON_NAME,
            "'{app_id}' is not a theme icon name"
        );
    }
    assert_eq!(icon_for(&[Some(longest)]), "x".repeat(128));
}

/// The sanitizer accepts only a plain theme icon name and keeps the value it
/// was given.
#[test]
fn the_theme_name_accepts_only_a_plain_icon_name() {
    assert_eq!(
        theme_name("org.example.Notes-1_2"),
        Some("org.example.Notes-1_2")
    );
    assert_eq!(theme_name(""), None, "an empty name names no icon");
    assert_eq!(
        theme_name(".hidden"),
        None,
        "a leading dot looks like a file"
    );
    assert_eq!(theme_name("a/b"), None, "a separator looks like a path");
    assert_eq!(theme_name("a b"), None, "a space is not in the grammar");
    assert_eq!(
        theme_name("\u{fc}mlaut"),
        None,
        "only ASCII letters are allowed"
    );
    let longest = "x".repeat(128);
    assert_eq!(theme_name(&longest), Some(longest.as_str()));
    let too_long = "x".repeat(129);
    assert_eq!(theme_name(&too_long), None, "129 characters is too long");
}

/// Without a daemon behind it the item keeps the fallback `pinwin` icon and
/// its one identity (D14: a themed icon name).
#[test]
fn the_item_has_the_pinwin_icon_and_identity() {
    let item = tray(MenuModel::default());
    assert_eq!(item.icon_name(), ICON_NAME);
    assert_eq!(item.icon_name(), "pinwin");
    assert_eq!(item.id(), "pinwin");
    assert_eq!(item.title(), "pinwin");
}

/// A primary click starts, ends, shows and hides nothing (spec "Icon click
/// opens nothing new"): it leaves the menu model and every panel alone, and
/// it does not open the menu either.
#[test]
fn a_primary_click_changes_nothing() {
    let model = MenuModel::new(vec![PanelMenu::new(
        panel_name("notes"),
        true,
        Edge::Left,
        ReserveMode::Push,
    )]);
    let mut item = tray(model.clone());

    item.activate(10, 20);

    assert_eq!(item.model, model, "the panels did not change");
    const { assert!(!<PinwinTray as ksni::Tray>::MENU_ON_ACTIVATE) };
}
