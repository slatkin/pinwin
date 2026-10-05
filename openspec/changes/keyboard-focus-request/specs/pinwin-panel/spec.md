# Spec Delta

## MODIFIED Requirements

### Requirement: Keyboard focus by clicking
If the startup layout requests `on-demand` mode, the panel SHALL use layer-shell `on-demand`
keyboard interactivity. In this mode, the panel receives keyboard input only after one of two
events. The user clicks inside the panel, or the host requests focus. The panel gives up
keyboard input after the user clicks a compositor window. At open, the panel SHALL NOT take
keyboard focus. The keyboard mode is fixed at start time, and no runtime override exists.

While the panel holds keyboard focus, it SHALL mark itself with a focus accent. The accent is
a stroke around the whole window, in the colour and pixel width that the startup supplies. The
compositor draws no focus ring on layer surfaces, so the panel draws its own. If the accent
is disabled, the panel SHALL draw nothing extra, focused or not.

The keyboard mode is one of exactly `none`, `on-demand` or `exclusive`. An accent is absent,
or it is a colour with a width of 1..=65535 pixels. The library's argument types SHALL make
any other keyboard mode or accent unrepresentable. So no runtime rejection exists for them.

#### Scenario: Click to type
- **WHEN** the user clicks inside the panel and types `j`
- **THEN** the pty master receives the `j` key

#### Scenario: Click away
- **WHEN** the panel has keyboard focus and the user clicks a tiled window
- **THEN** keys go to the tiled window, not the panel

#### Scenario: No focus steal on launch
- **WHEN** the host starts the panel with `on-demand` mode
- **THEN** keyboard input stays with the window that had it

#### Scenario: Opt-in keyboard focus
- **WHEN** the host starts the panel with `exclusive` keyboard mode
- **THEN** at open, the panel has the keyboard without a click

#### Scenario: Invalid keyboard mode
- **WHEN** a host tries to start the panel with a keyboard mode other than the three defined
  ones
- **THEN** the program does not compile, so no such start reaches the panel

#### Scenario: Invalid accent
- **WHEN** a host tries to start the panel with an accent width of 0 or above 65535
- **THEN** the program does not compile, so no such start reaches the panel

#### Scenario: Accent appears while focused
- **WHEN** the panel gains keyboard focus with the accent enabled
- **THEN** the whole window gains an outline of the configured colour and width. After focus
  moves back to a compositor window, the outline disappears.

#### Scenario: Accent disabled
- **WHEN** the host starts the panel with no accent
- **THEN** the panel draws no accent, focused or not

## ADDED Requirements

### Requirement: Focus on request
The `Panel` handle SHALL offer a focus request. The host can call it from any thread, and it
returns `Result<(), PinwinError>`. In `on-demand` mode, a successful request SHALL give the
panel keyboard focus without a click. After that, the panel SHALL keep and give up focus by
the normal `on-demand` rules. The request SHALL NOT change the reserved gap, so tiled windows
do not move or resize. In `none` or `exclusive` mode, the request SHALL return `Ok(())` and
change nothing.

If the panel is no longer live, the request SHALL return `Err(PinwinError::NotRunning)`
without blocking. If the GTK side does not answer within a bounded time, the request SHALL
return `Err(PinwinError::Internal)`, so the host thread never blocks indefinitely. The
library SHALL own no transport for the request. The host decides how a request reaches it.

#### Scenario: Focus from a hotkey
- **WHEN** a tiled window has the keyboard, the panel runs in `on-demand` mode, and the host
  requests focus
- **THEN** the call returns `Ok` and the next typed key goes to the pty master. If the accent
  is enabled, it appears.

#### Scenario: Release by click
- **WHEN** the panel gained focus from a request and the user clicks a tiled window
- **THEN** keys go to the tiled window, not the panel

#### Scenario: Gap unchanged
- **WHEN** the panel pushes tiled windows and the host requests focus
- **THEN** the tiled windows keep their position and size

#### Scenario: Mode without on-demand focus
- **WHEN** the panel runs in `none` mode and the host requests focus
- **THEN** the call returns `Ok` and keyboard input stays with the window that had it

#### Scenario: Request on a dead panel
- **WHEN** the panel's GTK side ended on its own and the host requests focus
- **THEN** the call returns `Err(PinwinError::NotRunning)` without blocking

### Requirement: Focus request from the command line
The `pinwin` program SHALL accept focus requests for its panel from other processes on the
same Wayland display. The program SHALL read `PINWIN_NAME`, with the default `default`. A
name has 1..=64 characters from `[A-Za-z0-9_-]`. Another process SHALL request focus with
`pinwin --focus [name]`, and the name defaults to `default`.

If the named instance accepts the request, the client SHALL exit 0. If no instance with that
name answers on the display, the client SHALL print a message on stderr and exit 1. A name in
`PINWIN_NAME` or after `--focus` can be invalid. Then `pinwin` SHALL print a message on
stderr and exit 2, and the host SHALL open nothing.

A live instance on the same display can already use the name. Then a second host SHALL print
a message and exit 2 before any surface opens. A name from an instance that no longer runs
SHALL NOT block a new host. Different names and different displays SHALL NOT interfere.

#### Scenario: Focus the default instance
- **WHEN** `pinwin htop` runs and the user runs `pinwin --focus`
- **THEN** the panel gets keyboard focus and the client exits 0

#### Scenario: Focus a named instance
- **WHEN** `PINWIN_NAME=notes pinwin nvim` and `pinwin htop` both run and the user runs
  `pinwin --focus notes`
- **THEN** the `notes` panel gets keyboard focus, the other panel does not, and the client
  exits 0

#### Scenario: No instance
- **WHEN** no `pinwin` with the name `notes` runs and the user runs `pinwin --focus notes`
- **THEN** the client prints a message and exits 1

#### Scenario: Invalid name
- **WHEN** `PINWIN_NAME=a/b` is set, or the user runs `pinwin --focus a/b`
- **THEN** `pinwin` prints a message, exits 2 and opens nothing

#### Scenario: Duplicate name
- **WHEN** `pinwin htop` runs and the user starts a second `pinwin` with no `PINWIN_NAME`
- **THEN** the second `pinwin` prints a message, exits 2 and opens nothing, and the first
  panel is unchanged

#### Scenario: Instance that crashed
- **WHEN** a `pinwin` named `notes` was killed with SIGKILL and the user starts a new one
  with the same name
- **THEN** the new `pinwin` starts normally and answers `pinwin --focus notes`
