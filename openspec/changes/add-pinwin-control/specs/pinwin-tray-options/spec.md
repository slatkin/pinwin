# Spec Delta

## MODIFIED Requirements

### Requirement: Tray access to options
Unless started with `--no-tray`, each running pinwin instance SHALL expose a system tray entry
while a compatible tray host is available. The icon SHALL be the icon theme's `pinwin` icon,
falling back to the theme's `utilities-terminal` icon when the theme has no `pinwin` icon.
Right-click SHALL expose an `Options...` menu item that opens that instance's options window.
Absence or restart of the tray host SHALL NOT close or restart the panel; the entry SHALL register
when the host becomes available and disappear when pinwin exits.

#### Scenario: Open options
- **WHEN** the user right-clicks the pinwin tray icon and selects `Options...`
- **THEN** that instance opens an options window

#### Scenario: Host returns
- **WHEN** the tray host disappears and later returns while pinwin runs
- **THEN** the child command continues uninterrupted and the tray entry becomes available again

#### Scenario: No pinwin icon in the theme
- **WHEN** the icon theme has no `pinwin` icon
- **THEN** the tray entry uses the `utilities-terminal` icon and the terminal continues

#### Scenario: Exit
- **WHEN** the command exits
- **THEN** pinwin closes its panel, options window and tray entry, releases the reserved space and retains the command's exit status

### Requirement: Staged options editing
The options window SHALL present a labeled numeric `Columns` field (the panel width in terminal
columns), followed by labeled numeric fields for Top, Bottom, Left and Right gutters in pixels, a
Left/Right docking selector, Apply and Close controls. The Columns field SHALL be presented first.
It SHALL initialize from the running instance's applied values. Edits SHALL NOT affect the panel or
persistent config until Apply succeeds. Apply SHALL keep the options window open. Closing SHALL
discard unapplied edits. Repeated opening requests, from the tray's `Options...` or from a control
request, SHALL present the existing window without duplicating it or resetting pending edits. All
controls SHALL be keyboard accessible.

#### Scenario: Edit without applying
- **WHEN** the user edits the Columns field, a gutter or the docking side without pressing Apply
- **THEN** the panel, reservation and config remain unchanged

#### Scenario: Close discards edits
- **WHEN** the user closes the window after editing without applying and opens it again
- **THEN** the fields show the previously applied values, including the applied column count

#### Scenario: Reopen existing window
- **WHEN** an opening request arrives while the options window is already open with pending edits
- **THEN** the same window is presented and its pending edits are preserved

## ADDED Requirements

### Requirement: Running without a tray
`pinwin --no-tray` SHALL NOT register a system tray entry at any point in its life. Its options
window SHALL remain available through the control API (pinwin-control), and every other behavior
SHALL be unchanged.

#### Scenario: Hosted without a tray
- **WHEN** the user runs `pinwin --no-tray mbv` with a tray host running
- **THEN** no pinwin tray entry appears, and the panel runs `mbv` normally

#### Scenario: Options without a tray
- **WHEN** a pinwin started with `--no-tray` receives an `options` control request
- **THEN** it opens its options window exactly as the tray's `Options...` would
