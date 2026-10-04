# Spec Delta

## MODIFIED Requirements

### Requirement: Launch a command in the panel
`pinwin` SHALL run the command given after its own options inside its terminal, or the user's
`$SHELL` when no command is given (`/bin/sh` when `SHELL` is unset or empty). Arguments SHALL be
passed to the command unchanged. pinwin's own options SHALL precede the command; `--` SHALL end
pinwin's options so that the next argument is always taken as the command. A leading argument
starting with `--` that is not a pinwin option SHALL make `pinwin` exit with a non-zero status and
an error message on stderr naming the argument, before opening any surface.

#### Scenario: Default command
- **WHEN** the user runs `pinwin` with no arguments and `SHELL=/bin/zsh`
- **THEN** the panel opens and runs `/bin/zsh`

#### Scenario: Custom command
- **WHEN** the user runs `pinwin htop -d 10`
- **THEN** the panel opens and runs `htop` with arguments `-d` and `10`

#### Scenario: Options before the command
- **WHEN** the user runs `pinwin --no-tray <command>`
- **THEN** the panel opens without a tray entry and runs the client with no arguments

#### Scenario: Unknown option
- **WHEN** the user runs `pinwin --sideways <command>`
- **THEN** `pinwin` prints an error naming `--sideways` to stderr, opens nothing, and exits non-zero

## ADDED Requirements

### Requirement: Wayland layer-shell is required
`pinwin` SHALL run only on a Wayland compositor that implements wlr-layer-shell. When layer-shell
is unavailable (an X11 session, or a Wayland compositor without it such as GNOME), `pinwin` SHALL
exit with a non-zero status and an error message on stderr, before spawning the command. It SHALL
NOT fall back to an ordinary window.

#### Scenario: No layer-shell
- **WHEN** the user runs `pinwin` in a session whose compositor lacks wlr-layer-shell
- **THEN** `pinwin` prints an error to stderr, runs no command, and exits non-zero
