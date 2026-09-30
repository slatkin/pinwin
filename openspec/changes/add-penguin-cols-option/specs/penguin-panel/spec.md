# Spec Delta

## MODIFIED Requirements

### Requirement: Width and gutter settings
`penguin` SHALL read the environment variable `COLS` as the launch-time panel width in terminal columns (default 40) and `GUTTER` as the extra reserved pixels (default 0). The panel's pixel width SHALL be exactly the applied column count times the cell width of its font. A valid saved layout's `cols` SHALL override `COLS` as described by the penguin-tray-options capability; with no saved layout, the applied column count is `COLS`. A `COLS` that is not a non-negative integer, or a `COLS` of 0, SHALL make `penguin` exit with a non-zero status and an error message on stderr naming the variable, before opening any surface.

#### Scenario: Defaults
- **WHEN** the user runs `penguin` with neither variable set
- **THEN** the panel is 40 columns wide and reserves exactly its width

#### Scenario: Custom width
- **WHEN** the user runs `COLS=60 GUTTER=0 penguin`
- **THEN** the panel is 60 columns wide and reserves exactly its width

#### Scenario: Saved columns take precedence
- **WHEN** a valid saved layout exists with `cols=52` and the user runs `COLS=60 penguin`
- **THEN** the panel is 52 columns wide and reserves exactly its width

#### Scenario: Invalid value
- **WHEN** the user runs `COLS=abc penguin`
- **THEN** `penguin` prints an error naming `COLS` to stderr, opens nothing, and exits non-zero
