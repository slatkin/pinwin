# Spec Delta

## MODIFIED Requirements

### Requirement: Menu for each panel
The icon's menu SHALL have one submenu for each panel, labelled with the panel name, in the
order the panels started. Each submenu SHALL have: a "Shown" check item that toggles the panel;
an edge choice of left, right, top and bottom; and a reserve choice of push, cover and none.
Each item SHALL show the panel's current state and SHALL act through the same commands as the
IPC.

#### Scenario: Hide from the menu
- **WHEN** the `notes` panel is shown and the user unchecks "Shown" in the `notes` submenu
- **THEN** the `notes` panel hides, and a subscribed IPC client receives a `hidden` event

#### Scenario: Menu follows the IPC
- **WHEN** a client sends `set-edge notes right`
- **THEN** the `notes` submenu shows right as the chosen edge

#### Scenario: Submenus follow panels
- **WHEN** panels `notes` and `chat` exist and `chat` ends
- **THEN** the menu has only the `notes` submenu

#### Scenario: Release the strip from the menu
- **WHEN** the `notes` panel pushes and the user chooses none in the reserve choice
- **THEN** the `notes` panel changes to `none` and its strip becomes zero
