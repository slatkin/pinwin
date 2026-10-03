# Spec Delta

## MODIFIED Requirements

### Requirement: Keyboard focus by clicking
The panel SHALL use layer-shell `on-demand` keyboard interactivity when the startup layout
requests it: it receives keyboard input only after the user clicks inside it, and gives up
keyboard input when the user clicks a compositor window. It SHALL NOT take keyboard focus when
it opens. The keyboard mode is fixed at `pinwin_start` time; there is no runtime override.

While the panel holds keyboard focus, it SHALL mark itself with a focus accent: a strip on its
workspace-facing edge, in the colour and pixel width the startup supplies (`PinwinAccent`).
The compositor draws no focus ring on layer surfaces, so the panel draws its own. When the
accent is disabled, the panel SHALL draw nothing extra, focused or not.

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
- **THEN** the panel has the keyboard when it opens, without a click

#### Scenario: Invalid keyboard mode
- **WHEN** the host calls `pinwin_start` with a keyboard mode that is not a
  `PINWIN_KEYBOARD_*` value
- **THEN** the call returns `PINWIN_ERR_INVALID` and nothing opens

#### Scenario: Accent appears while focused
- **WHEN** the panel gains keyboard focus with the accent enabled
- **THEN** a strip of the configured colour and width appears on the workspace-facing edge,
  and disappears when focus moves back to a compositor window

#### Scenario: Accent disabled
- **WHEN** the host starts the panel with an accent whose `enabled` is 0
- **THEN** the panel draws no accent strip, focused or not

#### Scenario: Invalid accent
- **WHEN** the host calls `pinwin_start` with an accent whose `enabled` is not 0 or 1, or
  whose `width` is outside 1..=65535 while enabled
- **THEN** the call returns `PINWIN_ERR_INVALID` and nothing opens
