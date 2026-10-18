# Spec Delta

## Purpose

Gives users with a system tray one pinwin icon whose menu shows and controls every panel,
without one icon for each panel and without an icon when no panel exists.

## ADDED Requirements

### Requirement: One tray icon while panels exist
The daemon SHALL show one StatusNotifierItem tray icon while at least one panel exists. Its icon
SHALL be the icon-theme name that is the `app_id` of the oldest running panel's app, and
`pinwin` when that app has set no valid `app_id`. When that panel ends the icon SHALL follow the
next oldest panel. Without a panel, no icon SHALL show. The panel count SHALL NOT change the
icon count.

A valid `app_id` SHALL be 1 to 128 characters from letters, digits, `.`, `_` and `-`, not
starting with `.`.

#### Scenario: Icon with panels
- **WHEN** a tray host runs and the user starts two panels
- **THEN** the tray shows one item, with the icon of the oldest panel's app

#### Scenario: Icon of the hosted app
- **WHEN** the `notes` panel hosts an app with app id `org.example.Notes`
- **THEN** the item shows the icon named `org.example.Notes`

#### Scenario: Icon follows the oldest panel
- **WHEN** panels `notes` (oldest) and `chat` exist and `notes` ends
- **THEN** the icon becomes the one of `chat`'s app

#### Scenario: Icon before the app names itself
- **WHEN** the oldest panel's app has not set an app id
- **THEN** the icon is `pinwin`

#### Scenario: Unusable app id
- **WHEN** the app id contains a path separator
- **THEN** the icon is `pinwin`

#### Scenario: No icon without panels
- **WHEN** `exit_when_idle` is false and the last panel ends
- **THEN** the pinwin icon goes away while the daemon keeps running

#### Scenario: Icon comes back
- **WHEN** no panel exists and the user starts a panel
- **THEN** the tray item shows again

### Requirement: No tray is not an error
When the session has no D-Bus session bus or no tray host, the daemon SHALL run without an
icon. Panels and the IPC SHALL work the same. When a tray host starts later, the icon SHALL
show on it while panels exist.

#### Scenario: No tray host
- **WHEN** no tray host runs and the user starts a panel
- **THEN** the panel works, the IPC answers, and the daemon reports no error

#### Scenario: Tray host starts later
- **WHEN** a panel exists and a tray host starts
- **THEN** the tray item shows on it

### Requirement: Menu for each panel
The icon's menu SHALL have one submenu for each panel, labelled with the panel name, in the
order the panels started. Each submenu SHALL have: a "Shown" check item that toggles the panel;
an edge choice of left, right, top and bottom; and a reserve choice of push and cover. Each item
SHALL show the panel's current state and SHALL act through the same commands as the IPC.

#### Scenario: Hide from the menu
- **WHEN** the `notes` panel is shown and the user unchecks "Shown" in the `notes` submenu
- **THEN** the `notes` panel hides, and a subscribed IPC client receives a `hidden` event

#### Scenario: Menu follows the IPC
- **WHEN** a client sends `set-edge notes right`
- **THEN** the `notes` submenu shows right as the chosen edge

#### Scenario: Submenus follow panels
- **WHEN** panels `notes` and `chat` exist and `chat` ends
- **THEN** the menu has only the `notes` submenu

### Requirement: Quit from the menu
The icon's menu SHALL have a "Quit pinwin" item after the panel submenus. It SHALL do the same
as the `quit` request.

#### Scenario: Quit
- **WHEN** the user chooses "Quit pinwin"
- **THEN** every panel ends and the daemon exits

### Requirement: Icon click opens nothing new
Activating the icon (a primary click) SHALL NOT start, end, show or hide any panel. The menu is
the only way the tray changes a panel.

#### Scenario: Primary click
- **WHEN** the user clicks the tray icon with the primary button
- **THEN** no panel changes
