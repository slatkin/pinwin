# pinwin-panel Specification

## Purpose

`pinwin` docks a terminal running the user's chosen command (their `$SHELL` by default) at the left edge of one monitor, reserving that space so the compositor tiles windows to its right, and releases the space when the command exits.

## Requirements

### Requirement: Launch a command in the panel
`pinwin` SHALL run the command given after its own options inside its terminal, or the user's `$SHELL` when no command is given (`/bin/sh` when `SHELL` is unset or empty). Arguments SHALL be passed to the command unchanged. pinwin's own options SHALL precede the command; `--` SHALL end pinwin's options so that the next argument is always taken as the command. A leading argument starting with `--` that is not a pinwin option SHALL make `pinwin` exit with a non-zero status and an error message on stderr naming the argument, before opening any surface.

#### Scenario: Default command
- **WHEN** the user runs `pinwin` with no arguments and `SHELL=/bin/zsh`
- **THEN** the panel opens and runs `/bin/zsh`

#### Scenario: Custom command
- **WHEN** the user runs `pinwin htop -d 10`
- **THEN** the panel opens and runs `htop` with arguments `-d` and `10`

#### Scenario: Options before the command
- **WHEN** the user runs `pinwin --no-tray mbv`
- **THEN** the panel opens without a tray entry and runs `mbv` with no arguments

#### Scenario: Unknown option
- **WHEN** the user runs `pinwin --sideways mbv`
- **THEN** `pinwin` prints an error naming `--sideways` to stderr, opens nothing, and exits non-zero

### Requirement: Dock at the left edge of the focused monitor
`pinwin` SHALL appear as a layer-shell surface on the `overlay` layer, flush against the left, top and bottom edges of the monitor that has focus when it starts, spanning that monitor's full height even when another bar reserves the top edge. The last terminal row's background SHALL reach the bottom edge without a dark gap. It SHALL stay on that monitor and SHALL be visible on every workspace of that monitor. It SHALL NOT appear on other monitors.

#### Scenario: Opens on the focused monitor
- **WHEN** DP-2 has focus and the user runs `pinwin`
- **THEN** the panel appears at the left edge of DP-2 and nothing appears on DP-1

#### Scenario: Covers an existing top bar without a bottom gap
- **WHEN** a bar reserves the top edge on the focused monitor and the user runs `pinwin`
- **THEN** the panel covers that bar across its own width and its last row's background reaches the monitor's bottom edge

#### Scenario: Visible across workspace switches
- **WHEN** the panel is open and the user switches to another workspace on the same monitor
- **THEN** the panel stays at the left edge without moving or flickering

#### Scenario: Cannot be moved
- **WHEN** the user tries to drag or move the panel with the compositor's window actions
- **THEN** the panel stays at the left edge

### Requirement: Reserve space so tiles start to its right
`pinwin` SHALL reserve a strip at the left edge of its monitor equal to the panel width plus `GUTTER` pixels, so the compositor places tiled windows to the right of that strip. The reservation SHALL apply only to the panel's monitor. The gap between the panel and the first tile is `GUTTER` plus whatever left strut the compositor itself adds.

#### Scenario: Tiles move right
- **WHEN** the panel opens with `GUTTER=12` on a monitor with tiled windows
- **THEN** the tiled windows' left edge moves to at least panel width + 12 px from the monitor's left edge

#### Scenario: Other monitors unaffected
- **WHEN** the panel is open on DP-2
- **THEN** tiled windows on DP-1 keep their original position

### Requirement: Width and gutter settings
`pinwin` SHALL read the environment variable `COLS` as the launch-time panel width in terminal columns (default 40) and `GUTTER` as the extra reserved pixels (default 0). The panel's pixel width SHALL be exactly the applied column count times the cell width of its font. A valid saved layout's `cols` SHALL override `COLS` as described by the pinwin-tray-options capability; with no saved layout, the applied column count is `COLS`. A `COLS` that is not a non-negative integer, or a `COLS` of 0, SHALL make `pinwin` exit with a non-zero status and an error message on stderr naming the variable, before opening any surface.

#### Scenario: Defaults
- **WHEN** the user runs `pinwin` with neither variable set
- **THEN** the panel is 40 columns wide and reserves exactly its width

#### Scenario: Custom width
- **WHEN** the user runs `COLS=60 GUTTER=0 pinwin`
- **THEN** the panel is 60 columns wide and reserves exactly its width

#### Scenario: Saved columns take precedence
- **WHEN** a valid saved layout exists with `cols=52` and the user runs `COLS=60 pinwin`
- **THEN** the panel is 52 columns wide and reserves exactly its width

#### Scenario: Invalid value
- **WHEN** the user runs `COLS=abc pinwin`
- **THEN** `pinwin` prints an error naming `COLS` to stderr, opens nothing, and exits non-zero

### Requirement: Keyboard focus by clicking
`pinwin` SHALL use layer-shell `on-demand` keyboard interactivity by default: it receives keyboard input only after the user clicks inside it, and gives up keyboard input when the user clicks a compositor window. It SHALL NOT take keyboard focus when it opens. The environment variable `PINWIN_KEYBOARD` MAY override the interactivity with `exclusive` (take the keyboard as soon as the panel opens) or `none` (never take it); any other value SHALL make `pinwin` exit with a non-zero status and an error message on stderr naming the variable.

#### Scenario: Click to type
- **WHEN** the user clicks inside the panel and types `j`
- **THEN** the command in the panel receives the `j` key

#### Scenario: Click away
- **WHEN** the panel has keyboard focus and the user clicks a tiled window
- **THEN** keys go to the tiled window, not the panel

#### Scenario: No focus steal on launch
- **WHEN** the user runs `pinwin` from a terminal
- **THEN** keyboard input stays with the window that had it

#### Scenario: Opt-in keyboard focus
- **WHEN** the user runs `PINWIN_KEYBOARD=exclusive pinwin`
- **THEN** the panel has the keyboard when it opens, without a click

#### Scenario: Invalid keyboard mode
- **WHEN** the user runs `PINWIN_KEYBOARD=sideways pinwin`
- **THEN** `pinwin` prints an error naming `PINWIN_KEYBOARD` to stderr, opens nothing, and exits non-zero

### Requirement: Terminal features mbv depends on
The panel's terminal SHALL support, as seen by the program running in it: alternate screen; 24-bit and 256-color text with bold, italic and inverse; the kitty keyboard protocol including "disambiguate escape codes", with key press and release and Shift/Ctrl/Alt/Super modifiers; mouse reporting in SGR format with coordinates in cells; focus in/out reports (`CSI I` / `CSI O`) when the program enables them; `CSI > 1 s` (XTSHIFTESCAPE); and the kitty graphics protocol, including answering the kitty graphics query and drawing transmitted images at their placements.

#### Scenario: mbv detects kitty graphics
- **WHEN** `pinwin` runs `mbv` with no image protocol override in mbv's config
- **THEN** mbv selects the kitty image protocol (not half-blocks) and posters render as images

#### Scenario: Key disambiguation
- **WHEN** a program in the panel enables kitty keyboard disambiguation and the user presses Escape
- **THEN** the program receives the kitty-protocol encoding for Escape rather than a bare `ESC` byte

#### Scenario: Mouse click reported in cells
- **WHEN** a program in the panel enables SGR mouse reporting and the user clicks the cell at column 3, row 5
- **THEN** the program receives an SGR press report for column 3, row 5

#### Scenario: Focus reports
- **WHEN** a program in the panel enables focus reporting and the user clicks into the panel, then clicks a tiled window
- **THEN** the program receives `CSI I` and then `CSI O`

### Requirement: Font follows the Ghostty config
`pinwin` SHALL render with the font configured for the user's Ghostty terminal: the first `font-family` and the `font-size` from `$XDG_CONFIG_HOME/ghostty/config` (default `~/.config/ghostty/config`). The environment variable `PINWIN_FONT` SHALL override the family and `PINWIN_FONT_SIZE` SHALL override the size. When the config has no font settings, the panel SHALL fall back to `monospace 11`. Opening the Ghostty config SHALL NOT be required: a missing or unreadable config SHALL NOT prevent the panel from opening.

#### Scenario: Configured font
- **WHEN** the Ghostty config sets `font-family = "JetBrainsMono Nerd Font"` and `font-size = 11` and the user runs `pinwin`
- **THEN** the panel draws with that family and size

#### Scenario: Override
- **WHEN** the user runs `PINWIN_FONT=DejaVu\ Sans\ Mono PINWIN_FONT_SIZE=13 pinwin`
- **THEN** the panel draws with that family and size instead of the configured ones

#### Scenario: No Ghostty config
- **WHEN** no Ghostty config exists and `PINWIN_FONT` is unset
- **THEN** the panel still opens and draws with `monospace 11`

### Requirement: Correct size reports
The panel's terminal SHALL answer terminal queries on the PTY: primary device attributes (`CSI c`) and `CSI 16 t` (cell size in pixels). The PTY window size SHALL carry both the column/row count and the pixel width/height, and SHALL be updated whenever the panel's size changes. The pixel sizes reported SHALL match the cell size actually drawn.

#### Scenario: Cell size query
- **WHEN** a program in the panel writes `CSI 16 t`
- **THEN** it receives `CSI 6 ; <cell height px> ; <cell width px> t` matching the drawn cell size

#### Scenario: Images are not clipped
- **WHEN** mbv shows a poster in the panel
- **THEN** the whole image is visible, with no part cut off at the right or bottom edge

#### Scenario: Pixel size in window size
- **WHEN** a program in the panel reads the terminal size with `TIOCGWINSZ`
- **THEN** `ws_col`/`ws_row` are the cell grid and `ws_xpixel`/`ws_ypixel` are the grid size in pixels

### Requirement: Install from the checkout
The repository SHALL provide `make install`, which builds `pinwin` and installs an executable at `~/.local/bin/pinwin` without modifying desktop entries or niri configuration.

#### Scenario: Install the panel
- **WHEN** the user runs `make install` from the repository root
- **THEN** `~/.local/bin/pinwin` exists, is executable, and matches the built binary

### Requirement: Exit with the command
`pinwin` SHALL close its surface and exit when the command exits, with the command's exit status. The reserved space SHALL be released without any cleanup step and without writing any files. `pinwin` SHALL NOT read or write niri configuration.

#### Scenario: Normal exit
- **WHEN** the user quits mbv inside the panel
- **THEN** the panel disappears, tiled windows move back to the left, and `pinwin` exits with mbv's exit status

#### Scenario: pinwin killed
- **WHEN** `pinwin` is killed with `SIGKILL`
- **THEN** the compositor releases the reserved space and tiled windows move back to the left

### Requirement: Coexists with pinwin
Adding `pinwin` SHALL NOT change the behavior, files or configuration of the `pinwin` script.

#### Scenario: pinwin unchanged
- **WHEN** the user runs `pinwin` after `pinwin` is installed
- **THEN** `pinwin` behaves exactly as before, including writing and emptying `~/.config/niri/woims/pin.kdl`

### Requirement: Wayland layer-shell is required
`pinwin` SHALL run only on a Wayland compositor that implements wlr-layer-shell. When layer-shell is unavailable (an X11 session, or a Wayland compositor without it such as GNOME), `pinwin` SHALL exit with a non-zero status and an error message on stderr, before spawning the command. It SHALL NOT fall back to an ordinary window.

#### Scenario: No layer-shell
- **WHEN** the user runs `pinwin` in a session whose compositor lacks wlr-layer-shell
- **THEN** `pinwin` prints an error to stderr, runs no command, and exits non-zero
