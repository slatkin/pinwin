# pinwin-panel Specification

## Purpose

`pinwin` docks a terminal running the user's chosen command (their `$SHELL` by default) at the left edge of one monitor, reserving that space so the compositor tiles windows to its right, and releases the space when the command exits.

## Requirements

### Requirement: Keyboard focus by clicking
The panel SHALL use layer-shell `on-demand` keyboard interactivity when the startup layout
requests it: it receives keyboard input only after the user clicks inside it, and gives up
keyboard input when the user clicks a compositor window. It SHALL NOT take keyboard focus when
it opens. The keyboard mode is fixed at `pinwin_start` time; there is no runtime override.

While the panel holds keyboard focus, it SHALL mark itself with a focus accent: a stroke
around its whole window, in the colour and pixel width the startup supplies
(`PinwinAccent`).
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
- **THEN** the whole window gains an outline of the configured colour and width,
  and it disappears when focus moves back to a compositor window

#### Scenario: Accent disabled
- **WHEN** the host starts the panel with an accent whose `enabled` is 0
- **THEN** the panel draws no accent, focused or not

#### Scenario: Invalid accent
- **WHEN** the host calls `pinwin_start` with an accent whose `enabled` is not 0 or 1, or
  whose `width` is outside 1..=65535 while enabled
- **THEN** the call returns `PINWIN_ERR_INVALID` and nothing opens

### Requirement: Font follows the Ghostty config
The panel SHALL render with the font configured for the user's Ghostty terminal: the first
`font-family` and the `font-size` from `$XDG_CONFIG_HOME/ghostty/config` (default
`~/.config/ghostty/config`). When the config has no font settings, the panel SHALL fall back
to `monospace 11`. Opening the Ghostty config SHALL NOT be required: a missing or unreadable
config SHALL NOT prevent the panel from opening. There are no font environment overrides.

#### Scenario: Configured font
- **WHEN** the Ghostty config sets a family and size and the host starts the panel
- **THEN** the panel draws with that family and size

#### Scenario: No Ghostty config
- **WHEN** no Ghostty config exists
- **THEN** the panel still opens and draws with `monospace 11`

#### Scenario: Override
- **WHEN** `PINWIN_FONT` or `PINWIN_FONT_SIZE` is set in the host's environment
- **THEN** the panel ignores both (there is no font override) and draws with the Ghostty config values, or the fallback when there is no config

### Requirement: Correct size reports
The panel's terminal SHALL answer terminal queries on the PTY: primary device attributes
(`CSI c`) and `CSI 16 t` (cell size in pixels). The PTY window size SHALL carry both the
column/row count and the pixel width/height, and SHALL be updated whenever the panel's size
changes. The pixel sizes reported SHALL match the cell size actually drawn.

#### Scenario: Cell size query
- **WHEN** the host's child writes `CSI 16 t`
- **THEN** it receives `CSI 6 ; <cell height px> ; <cell width px> t` matching the drawn
  cell size

#### Scenario: Images are not clipped
- **WHEN** the host's child shows a poster image in the panel
- **THEN** the whole image is visible, with no part cut off at the right or bottom edge

#### Scenario: Pixel size in window size
- **WHEN** the host's child reads the terminal size with `TIOCGWINSZ`
- **THEN** `ws_col`/`ws_row` are the cell grid and `ws_xpixel`/`ws_ypixel` are the grid size
  in pixels

### Requirement: Wayland layer-shell is required
The library SHALL run only on a Wayland compositor that implements wlr-layer-shell. When
layer-shell is unavailable (an X11 session, or a Wayland compositor without it such as GNOME),
`pinwin_start` SHALL return `PINWIN_ERR_NO_DISPLAY` and open nothing. It SHALL NOT fall back
to an ordinary window.

#### Scenario: No layer-shell
- **WHEN** the host calls `pinwin_start` in a session whose compositor lacks wlr-layer-shell
- **THEN** the call returns `PINWIN_ERR_NO_DISPLAY` and nothing opens

### Requirement: Terminal features for the host's child
The panel's terminal SHALL support, as seen by the host's child: alternate screen; 24-bit and
256-color text with bold, italic and inverse; the kitty keyboard protocol including
"disambiguate escape codes", with key press and release and Shift/Ctrl/Alt/Super modifiers;
mouse reporting in SGR format with coordinates in cells; focus in/out reports (`CSI I` /
`CSI O`) when the child enables them; `CSI > 1 s` (XTSHIFTESCAPE); and the kitty graphics
protocol, including answering the kitty graphics query and drawing transmitted images at their
placements.

#### Scenario: Host's child selects kitty graphics
- **WHEN** the host's child supports the kitty graphics protocol and shows a poster
- **THEN** the child selects the kitty image protocol (not half-blocks) and the whole poster
  renders as an image

#### Scenario: Key disambiguation
- **WHEN** the host's child enables kitty keyboard disambiguation and the user presses Escape
- **THEN** the child receives the kitty-protocol encoding for Escape rather than a bare `ESC` byte

#### Scenario: Mouse click reported in cells
- **WHEN** the host's child enables SGR mouse reporting and the user clicks the cell at column 3, row 5
- **THEN** the child receives an SGR press report for column 3, row 5

#### Scenario: Focus reports
- **WHEN** the host's child enables focus reporting and the user clicks into the panel, then
  clicks a tiled window
- **THEN** the child receives `CSI I` and then `CSI O`

### Requirement: Directional gutters and docking geometry
Folded in from the retired options capability (design D1). Gutters SHALL use
literal screen directions in the same pixel coordinate system as before, independent of
docking side. Let panel width be `cols` times the font cell width. On the left, the panel
SHALL be inset from the left output edge by Left pixels and reserve `Left + panel width +
Right` pixels at the left edge. On the right, the panel SHALL be inset from the right output
edge by Right pixels and reserve the same sum at the right edge. Top and Bottom SHALL inset
the visible panel from the corresponding output edges. Reservation SHALL cover a full-height
strip even when the panel has vertical insets. Gutters MAY be negative: a negative gutter
moves the panel's edge beyond its output edge or ends the reservation before the panel's far
edge, so tiles may overlap the panel; the reservation sum SHALL NOT be negative. Existing
compositor struts remain additive and SHALL NOT be edited.

#### Scenario: Left docking
- **WHEN** panel width is 320 pixels, Left is 8, Right is 12 and docking is Left
- **THEN** the panel starts 8 pixels from the left output edge and reserves 340 pixels at
  that edge, before any compositor struts

#### Scenario: Right docking
- **WHEN** the same values are applied with docking Right
- **THEN** the panel ends 12 pixels before the right output edge and reserves 340 pixels at
  that edge, releasing its previous left reservation

#### Scenario: Vertical insets
- **WHEN** Top is 24 and Bottom is 10
- **THEN** the visible panel begins 24 pixels below the output top and ends 10 pixels above
  its bottom, while the horizontal reservation remains a full-height strip

#### Scenario: Negative gutter
- **WHEN** panel width is 320 pixels, Left is -40, Right is 12 and docking is Left
- **THEN** the panel starts 40 pixels beyond the left output edge and the reservation at that
  edge is 292 pixels

### Requirement: Apply layout without restarting the terminal
A successful `pinwin_apply_layout` or `pinwin_apply_layout_animated` SHALL update the applied
column count, all four gutters and the docking side as one logical operation, update the
reservation and the panel's pixel width to the applied column count times the cell width (at
the end of any animation), and resize the existing terminal grid and PTY winsize when
necessary. It SHALL NOT recreate the terminal emulator, change the font, or touch the host's
child. The host's child SHALL observe updated column/row counts and pixel sizes matching the
drawn grid after the resize.

#### Scenario: Apply new layout
- **WHEN** the host applies a changed side, gutters or columns
- **THEN** the running panel moves and resizes with its terminal state preserved

#### Scenario: Widen the panel
- **WHEN** the host changes columns from 40 to 60
- **THEN** the panel's pixel width becomes exactly 60 times the cell width, the reserved strip
  grows by the same amount, and the terminal grid reports 60 columns

#### Scenario: Terminal resize
- **WHEN** applied columns or top/bottom gutters change the terminal grid's column or row count
- **THEN** the existing PTY and terminal size reports match the new drawn grid with the host's
  child untouched

### Requirement: Validate before applying
`pinwin_apply_layout` SHALL reject an unknown side, a column count outside 1..=65535,
checked-arithmetic overflow, a reservation sum below zero, vertical space insufficient for one
complete terminal row, or a horizontal reservation that leaves no output width for other
windows. It SHALL return `PINWIN_ERR_INVALID` and leave the previous applied layout
untouched. Purely structural rejection (bad side, out-of-range columns, arithmetic overflow)
SHALL NOT require any GTK surface to exist.

#### Scenario: Invalid geometry
- **WHEN** the host applies top/bottom gutters leaving less than one row
- **THEN** the call returns `PINWIN_ERR_INVALID` and neither the live layout nor anything
  else changes

#### Scenario: Invalid columns
- **WHEN** the host applies zero columns, or more than 65535
- **THEN** the call returns `PINWIN_ERR_INVALID` and the previous applied state is intact

#### Scenario: Too wide for the output
- **WHEN** the host applies columns whose reservation leaves no output width for other windows
- **THEN** the call returns `PINWIN_ERR_INVALID` and the live layout is unchanged

#### Scenario: Invalid value
- **WHEN** a gutter value is outside the int32 range the ABI carries
- **THEN** the call returns `PINWIN_ERR_INVALID` and the live layout is unchanged

### Requirement: Dock at one edge of the panel's monitor
The library SHALL present its panel as a layer-shell surface on the `overlay` layer, flush
against the top and bottom edges of its monitor, spanning that monitor's full height even when
another bar reserves the top edge. The docking side comes from the ABI-supplied layout
(`PINWIN_SIDE_LEFT` / `PINWIN_SIDE_RIGHT`): flush against that edge, inset from it only by
that edge's gutter. The last terminal row's background SHALL reach the bottom edge without a
dark gap. The panel SHALL stay on its original monitor and SHALL be visible on every workspace
of that monitor, across workspace switches, focus moves and side changes. It SHALL NOT appear
on other monitors.

#### Scenario: Opens on the focused monitor
- **WHEN** DP-2 has focus and the host starts the panel
- **THEN** the panel appears at DP-2's ABI-supplied edge and nothing appears on DP-1

#### Scenario: Covers an existing top bar without a bottom gap
- **WHEN** a bar reserves the top edge on the panel's monitor
- **THEN** the panel covers that bar across its own width and its last row's background reaches
  the monitor's bottom edge

#### Scenario: Visible across workspace switches
- **WHEN** the panel is open and the user switches to another workspace on the same monitor
- **THEN** the panel stays at its docking edge without moving or flickering

#### Scenario: Cannot be moved
- **WHEN** the user tries to drag or move the panel with the compositor's window actions
- **THEN** the panel stays at its docking edge

### Requirement: Reserve space so tiles start beside the panel
The library SHALL reserve a strip at its docking edge equal to the panel width plus the two
horizontal gutters, so the compositor places tiled windows beside that strip. The reservation
SHALL apply only to the panel's monitor. The gap between the panel and the first tile is the
far gutter plus whatever strut the compositor itself adds.

#### Scenario: Tiles move right
- **WHEN** the panel opens docked left with panel width 320, Left 0 and Right 12
- **THEN** tiled windows' left edge moves to at least 332 px from the monitor's left edge

#### Scenario: Other monitors unaffected
- **WHEN** the panel is open on DP-2
- **THEN** tiled windows on DP-1 keep their original position

### Requirement: C ABI lifecycle
The library SHALL expose `pinwin_start(const PinwinStartup*)`,
`pinwin_apply_layout(const PinwinLayout*)`,
`pinwin_apply_layout_animated(const PinwinLayout*, uint32_t)` and `pinwin_stop(void)` from
`src/pinwin_api.h`. `pinwin_start` takes a host-owned pty master fd, a full layout
(side, cols 1..=65535, four gutters) and a keyboard mode, spawns the GTK thread, and returns
a synchronous result code. `pinwin_stop` closes the panel and joins the thread, and is a
no-op when not running. A second `pinwin_start` while running returns
`PINWIN_ERR_ALREADY_RUNNING`; `pinwin_apply_layout` and `pinwin_apply_layout_animated` when
not running return `PINWIN_ERR_NOT_RUNNING`. The layouts and structs passed across the ABI
SHALL keep their existing memory layout; `pinwin_apply_layout_animated` is additive.

#### Scenario: Start, relayout, stop
- **WHEN** the host starts the panel, applies a valid layout, then stops it
- **THEN** each call returns `PINWIN_OK`, the panel follows the layouts, and after stop no
  surface remains

#### Scenario: Double start
- **WHEN** the host calls `pinwin_start` twice without stopping
- **THEN** the second call returns `PINWIN_ERR_ALREADY_RUNNING` and the running panel is
  unchanged

#### Scenario: Animated apply when not running
- **WHEN** the host calls `pinwin_apply_layout_animated` with a valid layout and no panel is
  running
- **THEN** it returns `PINWIN_ERR_NOT_RUNNING` without blocking

### Requirement: Host-owned pty
The library SHALL read and write the pty master fd supplied at start and apply the window
size to it (`TIOCSWINSZ`); it SHALL NOT fork, wait on a child, or set any child environment
(the host owns its child's `TERM` and `COLORTERM`). Hangup on the master drops the read
source without touching process lifetime.

#### Scenario: Sizes reach the child
- **WHEN** the panel resizes after a layout apply
- **THEN** the host's child observes the new grid and pixel sizes via `TIOCGWINSZ`

### Requirement: The library never exits
No library call SHALL terminate the host process for any reason — bad arguments, bad layout,
missing display, missing Ghostty config, terminal allocation failure, or GTK errors are
result codes or degraded states, never `exit()`. Terminal allocation failure keeps the
previous grid and surfaces as `PINWIN_ERR_INTERNAL`.

#### Scenario: Bad layout does not kill the host
- **WHEN** the host applies an invalid layout
- **THEN** the call returns `PINWIN_ERR_INVALID` and the host process keeps running with its
  previous panel state

### Requirement: No pinwin-owned configuration
The library SHALL NOT read any pinwin-specific environment variable (`COLS`, `GUTTER`,
`PINWIN_KEYBOARD`, `PINWIN_FONT`, `PINWIN_FONT_SIZE`, `PINWIN_DEBUG` have no effect) and
SHALL NOT read or write any configuration file of its own: the full layout arrives with
every start and every apply, and the host owns persistence. Ghostty font/theme following
(the requirement above) is appearance, never layout, and is unaffected by this requirement.
A host that previously relied on those variables or the saved `pinwin/config` layout SHALL
pass their equivalents explicitly instead.

#### Scenario: Full layout at start
- **WHEN** the host starts the panel with side Right, 52 columns and four gutters
- **THEN** the panel opens with exactly that layout, regardless of any pinwin-owned
  configuration or environment present

#### Scenario: Ghostty config affects appearance only
- **WHEN** the Ghostty config sets a font and theme, and the host starts the panel with a
  given layout
- **THEN** the panel draws with that font and theme, and opens with exactly the supplied
  layout — the Ghostty config never influences layout

### Requirement: Winsize updates signal the host
The library SHALL raise `SIGWINCH` in the host process after every successful
`TIOCSWINSZ` on the pty master — the initial attach and every layout apply that updates
the winsize — so a host that handles SIGWINCH (for example a crossterm event loop) sees
the resize without polling. A failed winsize ioctl raises nothing; a host that installs
no SIGWINCH handler is unaffected (the default disposition is ignore).

#### Scenario: Resize delivers SIGWINCH
- **WHEN** a layout apply updates the pty winsize successfully
- **THEN** the host process receives SIGWINCH after the update

#### Scenario: No update, no signal
- **WHEN** the panel has no attached pty
- **THEN** a resize request raises no SIGWINCH

### Requirement: Animated width transition
`pinwin_apply_layout_animated(const PinwinLayout*, uint32_t duration_ms)` SHALL, when the
requested layout differs from the applied layout only in its column count (same side, same
left and right gutters) and `duration_ms` is greater than zero and GTK animations are
enabled, change the panel's pixel width and the reservation's exclusive zone from the current
width to the target width continuously over `duration_ms`, driven by the panel's frame clock.
Both surfaces SHALL change together in the same frame. The duration SHALL be clamped to
1000 ms. It SHALL return the same result codes as `pinwin_apply_layout`, where `PINWIN_OK`
means the layout was validated and accepted rather than that the animation finished.

#### Scenario: Animate on request
- **WHEN** the host calls `pinwin_apply_layout_animated` with columns changed from 40 to 120
  and `duration_ms` 200
- **THEN** the panel width and the reservation grow through intermediate widths over about
  200 ms and tiled windows reflow alongside

#### Scenario: Snap on zero duration
- **WHEN** the host calls `pinwin_apply_layout_animated` with `duration_ms` 0
- **THEN** the layout is applied in one step exactly as `pinwin_apply_layout` would

#### Scenario: Snap when animations are disabled
- **WHEN** the GTK `gtk-enable-animations` setting is false
- **THEN** `pinwin_apply_layout_animated` applies the layout in one step

#### Scenario: Other fields snap
- **WHEN** the requested layout changes the side or the left or right gutter
- **THEN** it is applied in one step with no width animation

#### Scenario: Duration clamp
- **WHEN** the host passes `duration_ms` 60000
- **THEN** the animation lasts at most 1000 ms

### Requirement: Exact final width
When an animated width transition ends, the panel's pixel width SHALL equal the target
columns times the cell width and the reservation SHALL equal left plus that width plus right,
with no residual offset from intermediate frames. The final frame SHALL be produced by the
same code path as a non-animated apply.

#### Scenario: Animation completes
- **WHEN** an animated transition to 120 columns finishes
- **THEN** the panel width is exactly 120 times the cell width and the reservation matches

#### Scenario: Stalled frame clock
- **WHEN** the frame clock does not deliver frames for the duration plus 100 ms
- **THEN** the panel is snapped to the exact target width

### Requirement: Terminal grid during a width animation
At the start of an animated width transition the terminal grid and PTY winsize SHALL be
resized once to the target column count, and no further grid or winsize change SHALL occur
until the next apply. While animating, the grid SHALL be drawn anchored to the docked edge
with the remainder of the surface filled with the theme background, and pointer coordinates
SHALL map to the drawn cells.

#### Scenario: One resize
- **WHEN** an animated transition from 40 to 120 columns runs
- **THEN** the host's child observes a single winsize change to 120 columns and one SIGWINCH

#### Scenario: Content does not slide
- **WHEN** the panel is docked right and shrinks
- **THEN** the drawn cells stay against the right edge

### Requirement: Interrupting an animation
A layout applied while a width animation is running SHALL take effect with last-write-wins
semantics. An animated apply SHALL retarget from the current animated width. A
non-animated apply or `duration_ms` 0 SHALL cancel the animation and snap to the requested
layout. `pinwin_stop` SHALL cancel any running animation before closing the surfaces.

#### Scenario: Retarget
- **WHEN** a collapse animation is half done and the host requests an expand
- **THEN** the width continues from its current value toward the expanded width

#### Scenario: Snap interrupts
- **WHEN** the host calls `pinwin_apply_layout` during an animation
- **THEN** the panel snaps to that layout and no further animation frames change it

#### Scenario: Stop mid-animation
- **WHEN** the host calls `pinwin_stop` during an animation
- **THEN** no surface remains and no animation callback runs afterwards

### Requirement: No mid-state on failure
A rejected or failed animated apply SHALL NOT leave the panel at an intermediate width. A
layout the panel refuses SHALL leave the applied layout unchanged. If the terminal grid cannot
be allocated when an animation starts, the panel SHALL be snapped to the requested layout and
the call SHALL return `PINWIN_ERR_INTERNAL`.

#### Scenario: Invalid layout
- **WHEN** the host calls `pinwin_apply_layout_animated` with a layout the panel refuses
- **THEN** it returns `PINWIN_ERR_INVALID` and the width and layout are unchanged

#### Scenario: Terminal allocation failure
- **WHEN** the terminal grid cannot be allocated at animation start
- **THEN** the call returns `PINWIN_ERR_INTERNAL` and the panel is at the requested final
  width, not an intermediate one
