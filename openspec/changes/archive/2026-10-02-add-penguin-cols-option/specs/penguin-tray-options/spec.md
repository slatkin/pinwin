# Spec Delta

## MODIFIED Requirements

### Requirement: Staged options editing
The options window SHALL present a labeled numeric `Columns` field (the panel width in terminal columns), followed by labeled numeric fields for Top, Bottom, Left and Right gutters in pixels, a Left/Right docking selector, Apply and Close controls. The Columns field SHALL be presented first. It SHALL initialize from the running instance's applied values. Edits SHALL NOT affect the panel or persistent config until Apply succeeds. Apply SHALL keep the options window open. Closing SHALL discard unapplied edits. Repeated `Options...` activation SHALL present the existing window without duplicating it or resetting pending edits. All controls SHALL be keyboard accessible.

#### Scenario: Edit without applying
- **WHEN** the user edits the Columns field, a gutter or the docking side without pressing Apply
- **THEN** the panel, reservation and config remain unchanged

#### Scenario: Close discards edits
- **WHEN** the user closes the window after editing without applying and opens it again
- **THEN** the fields show the previously applied values, including the applied column count

#### Scenario: Reopen existing window
- **WHEN** the user selects `Options...` while its window is already open with pending edits
- **THEN** the same window is presented and its pending edits are preserved

### Requirement: Apply without restarting the command
A successful Apply SHALL update the applied column count and all four gutters and the docking side as one logical operation, update the reservation and the panel's pixel width to the applied column count times the cell width, and resize the existing terminal grid and PTY when necessary. It SHALL NOT respawn the command, recreate the terminal emulator, change the font or change the command. The command SHALL receive updated column counts, rows and pixel sizes matching the drawn grid after the resize. Existing command arguments, terminal rendering and keyboard policy SHALL remain unchanged.

#### Scenario: Apply new layout
- **WHEN** the user changes side, gutters or columns and presses Apply
- **THEN** the running panel moves and resizes, its config is saved, and its original child PID and terminal state are preserved

#### Scenario: Terminal resize
- **WHEN** applied columns or top/bottom gutters change the terminal grid's column or row count
- **THEN** the existing PTY and terminal size reports match the new drawn grid without restarting the command

#### Scenario: Widen the panel
- **WHEN** the user changes Columns from 40 to 60 and presses Apply
- **THEN** the panel's pixel width becomes exactly 60 times the cell width, the reserved strip grows by the same amount, and the terminal grid reports 60 columns

### Requirement: Validate before saving or applying
Apply SHALL reject a Columns value that is non-integer, malformed, zero, negative or beyond the PTY column field's representable range, non-integer or unrepresentable gutters, unknown sides, checked-arithmetic overflow, a reservation sum below zero, vertical space insufficient for one complete terminal row, or a horizontal reservation that leaves no output width for other windows. It SHALL display an actionable error naming the offending field and preserve the previous applied layout, column count and saved config. A failed config save SHALL likewise leave both unchanged and keep pending edits available for correction or retry.

#### Scenario: Invalid geometry
- **WHEN** the user presses Apply with top/bottom gutters leaving less than one row
- **THEN** an error is displayed and neither the live layout nor the config changes

#### Scenario: Invalid columns
- **WHEN** the Columns field is zero, fractional, malformed or exceeds the PTY column field's range
- **THEN** Apply reports the Columns field as invalid and leaves the previous applied state intact

#### Scenario: Too wide for the output
- **WHEN** the user presses Apply with a Columns value whose reservation leaves no output width for other windows
- **THEN** an error is displayed and neither the live layout, the column count nor the config changes

#### Scenario: Invalid value
- **WHEN** a gutter is fractional, malformed or exceeds supported integer limits
- **THEN** Apply reports the invalid field and leaves the previous applied state intact

#### Scenario: Save failure
- **WHEN** the config directory is not writable and the user presses Apply
- **THEN** a save error is shown, the existing config and running layout remain unchanged, and the pending edits remain in the window

### Requirement: Persist and restore layout settings
Penguin SHALL save the docking side, the applied column count and all four gutters to `$XDG_CONFIG_HOME/penguin/config`, using `~/.config/penguin/config` when XDG_CONFIG_HOME is unset or empty. Successful saves SHALL replace the file atomically. A valid saved layout SHALL override the legacy GUTTER- and COLS-derived launch values on subsequent launches. With no saved layout, defaults SHALL be Left docking, the `COLS` column count (default 40), Top/Bottom/Left zero and Right equal to the existing `GUTTER` value (default zero). Existing validation of launch environment variables SHALL remain in effect. Missing config SHALL be normal; unreadable, malformed or geometrically unusable saved settings SHALL produce a diagnostic and fall back to those launch defaults without overwriting the file. Launch and merely opening or closing options SHALL NOT write the config. Layout settings SHALL NOT override fonts, keyboard mode or the command.

#### Scenario: Restore applied settings
- **WHEN** the user applies Right docking, a column count and four gutters, quits and launches penguin again
- **THEN** the saved layout, including the column count, is restored without needing environment variables

#### Scenario: Legacy launch
- **WHEN** no saved config exists and penguin starts with `GUTTER=8`
- **THEN** it opens on the left with Top/Bottom/Left zero, Right eight and the `COLS` column count

#### Scenario: Saved layout precedence
- **WHEN** a valid saved layout exists and the user starts with `GUTTER=8 COLS=60`
- **THEN** the saved four gutters, docking side and column count are used rather than the environment-derived values

#### Scenario: Corrupt config
- **WHEN** the saved config contains an invalid side, gutter or column count
- **THEN** penguin reports the problem, uses launch defaults and leaves that file untouched

#### Scenario: Multiple running instances
- **WHEN** two instances apply different settings
- **THEN** each changes only its own live panel and the last successful save supplies the layout for subsequent launches
