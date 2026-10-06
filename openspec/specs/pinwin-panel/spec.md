# pinwin-panel Specification

## Purpose

`pinwin` docks a terminal running the user's chosen command (their `$SHELL` by default) at the left edge of one monitor, reserving that space so the compositor tiles windows to its right, and releases the space when the command exits.

## Requirements

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
starting the panel SHALL fail with `PinwinError::NoDisplay` and open nothing. It SHALL NOT fall
back to an ordinary window.

#### Scenario: No layer-shell
- **WHEN** the host starts the panel in a session whose compositor lacks wlr-layer-shell
- **THEN** the start returns `Err(PinwinError::NoDisplay)` and nothing opens

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
docking side. Each layout is either pushing or covering: a pushing layout moves the
compositor gap to its own geometry, while a covering layout leaves the gap exactly where
the last pushing layout put it and draws its panel over tiled windows. Let panel width be
`cols` times the font cell width. On the left, a pushing layout SHALL be inset from the
left output edge by Left pixels and reserve `Left + panel width + Right` pixels at the
left edge. On the right, a pushing layout SHALL be inset from the right output edge by
Right pixels and reserve the same sum at the right edge. A covering layout SHALL inset
its visible panel by its own edge gutter on its docking side exactly like a pushing
layout with the same side and gutters, and SHALL NOT change the reserved strip: side,
width and edge of the reservation stay at the last pushing layout's values. Top and
Bottom SHALL inset the visible panel from the corresponding output edges. The pushing
reservation SHALL cover a full-height strip even when the panel has vertical insets.
Gutters MAY be negative: a negative gutter moves the panel's edge beyond its output
edge or ends the pushing reservation before the panel's far edge, so tiles may overlap
the panel; a pushing reservation sum SHALL NOT be negative. A side switch always moves
the gap: applying any layout (pushing or covering) on the opposite side moves the
reservation to the new side at the last pushing width. Existing compositor struts remain
additive and SHALL NOT be edited.

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

#### Scenario: Covering layout holds the gap
- **WHEN** the gap is 340 pixels on the left from a pushing layout and the host applies a
  covering layout on the same side with a 120-column panel
- **THEN** the visible panel widens to 120 columns times the cell width and the reserved
  strip stays 340 pixels on the left

#### Scenario: Side switch moves the gap
- **WHEN** the gap is held on the right and the host applies a layout docked left
- **THEN** the reservation moves to the left edge at the last pushing width

### Requirement: Apply layout without restarting the terminal
A successful layout apply, plain or animated, SHALL update the applied column count, all four
gutters, the docking side and the push/cover choice as one logical operation, update the
panel's pixel width to the applied column count times the cell width (at the end of any
animation), update the reservation when the requested layout pushes (at the end of any
animation) and leave the reservation untouched when the requested layout covers, and resize
the existing terminal grid and PTY winsize when necessary. It SHALL NOT recreate the
terminal emulator, change the font, or touch the host's child. The host's child SHALL observe
updated column/row counts and pixel sizes matching the drawn grid after the resize.

#### Scenario: Apply new layout
- **WHEN** the host applies a changed side, gutters or columns
- **THEN** the running panel moves and resizes with its terminal state preserved

#### Scenario: Widen the panel
- **WHEN** the host changes columns from 40 to 60 with a pushing layout
- **THEN** the panel's pixel width becomes exactly 60 times the cell width, the reserved strip
  grows by the same amount, and the terminal grid reports 60 columns

#### Scenario: Terminal resize
- **WHEN** applied columns or top/bottom gutters change the terminal grid's column or row count
- **THEN** the existing PTY and terminal size reports match the new drawn grid with the host's
  child untouched

#### Scenario: Covering expand moves no tiles
- **WHEN** the host applies a covering layout with columns changed from 40 to 120 on the same
  side with the same gutters
- **THEN** the panel's pixel width becomes exactly 120 times the cell width, the reserved strip
  does not change, tiled windows do not move, and the terminal grid reports 120 columns

#### Scenario: Covering shrink moves no tiles
- **WHEN** a covering 120-column layout is active over a 40-column gap and the host applies the
  pushing 40-column layout
- **THEN** the panel narrows to 40 columns, the reserved strip does not change, and tiled
  windows do not move

### Requirement: Validate before applying
A pushing layout apply SHALL reject checked-arithmetic overflow, a reservation sum below zero,
vertical space insufficient for one complete terminal row, or a horizontal reservation that
leaves no output width for other windows. A covering layout apply SHALL reject
checked-arithmetic overflow, a visible panel wider than the output, and vertical space
insufficient for one complete terminal row; it SHALL NOT reject on the untouched reservation.
Either kind SHALL return `Err(PinwinError::InvalidLayout)` and leave the previous applied
layout and the held reservation untouched. An unknown side, a zero column count, a column
count above 65535 and a gutter outside the 32-bit signed range SHALL be unrepresentable in the
layout argument type and therefore need no runtime check. The push/cover choice SHALL be
representable in the layout argument type with pushing as the backward-compatible default.
Purely structural rejection (arithmetic overflow of the gutters' own sum) SHALL NOT require
any GTK surface to exist.

#### Scenario: Invalid geometry
- **WHEN** the host applies top/bottom gutters leaving less than one row
- **THEN** the call returns `Err(PinwinError::InvalidLayout)` and neither the live layout nor
  anything else changes

#### Scenario: Too wide for the output
- **WHEN** the host applies pushing columns whose reservation leaves no output width for other
  windows
- **THEN** the call returns `Err(PinwinError::InvalidLayout)` and the live layout is unchanged

#### Scenario: Covering panel wider than the output
- **WHEN** the host applies covering columns whose panel pixel width exceeds the output width
- **THEN** the call returns `Err(PinwinError::InvalidLayout)` and the live layout and the held
  reservation are unchanged

#### Scenario: Invalid columns
- **WHEN** a host tries to build a layout with zero columns, more than 65535 columns or an
  unknown side
- **THEN** the program does not compile, so no such layout reaches the panel

#### Scenario: Invalid value
- **WHEN** a host tries to build a layout with a gutter outside the 32-bit signed range
- **THEN** the program does not compile, so no such layout reaches the panel

### Requirement: Dock at one edge of the panel's monitor
The library SHALL present its panel as a layer-shell surface on the `overlay` layer, flush
against the top and bottom edges of its monitor, spanning that monitor's full height even when
another bar reserves the top edge. The docking side comes from the layout the host supplies
(`Side::Left` / `Side::Right`): flush against that edge, inset from it only by that edge's
gutter. The last terminal row's background SHALL reach the bottom edge without a dark gap. The
panel SHALL stay on its original monitor and SHALL be visible on every workspace of that
monitor, across workspace switches, focus moves and side changes. It SHALL NOT appear on other
monitors.

#### Scenario: Opens on the focused monitor
- **WHEN** DP-2 has focus and the host starts the panel
- **THEN** the panel appears at DP-2's edge chosen by the supplied layout and nothing appears
  on DP-1

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
The library SHALL hold one reservation: a strip on one side of the panel's monitor. A pushing
layout sets that strip to its docking edge with width `Left + panel width + Right`, so the
compositor places tiled windows beside that strip. A covering layout leaves that strip
exactly as the last pushing layout set it, so tiled windows do not move while the visible
panel draws over them. When no pushing layout has ever applied (a covering start), the held
reservation SHALL be an empty strip: nothing is reserved until the first pushing layout
applies. The reservation SHALL apply only to the panel's monitor. The gap between the panel
and the first tile is the far gutter plus whatever strut the compositor itself adds.

#### Scenario: Tiles move right
- **WHEN** the panel opens docked left with panel width 320, Left 0 and Right 12
- **THEN** tiled windows' left edge moves to at least 332 px from the monitor's left edge

#### Scenario: Other monitors unaffected
- **WHEN** the panel is open on DP-2
- **THEN** tiled windows on DP-1 keep their original position

#### Scenario: Covering start reserves nothing
- **WHEN** the panel starts with a covering layout
- **THEN** tiled windows stay where they are until the first pushing layout applies

#### Scenario: Covering expand holds the strip
- **WHEN** the held strip is 332 px on the left and the host applies a covering 120-column
  layout on the same side
- **THEN** tiled windows keep their position and the visible panel extends past the strip

### Requirement: Host-owned pty
The library SHALL read and write the pty master fd supplied at start and apply the window
size to it (`TIOCSWINSZ`); it SHALL NOT fork, wait on a child, close the fd, or set any child
environment (the host owns the fd's lifetime and its child's `TERM` and `COLORTERM`). A start
whose fd is not an open descriptor SHALL fail with `PinwinError::InvalidFd` before any GTK
work. Hangup on the master drops the read source without touching process lifetime.

#### Scenario: Sizes reach the child
- **WHEN** the panel resizes after a layout apply
- **THEN** the host's child observes the new grid and pixel sizes via `TIOCGWINSZ`

#### Scenario: Closed fd
- **WHEN** the host starts the panel with a descriptor that is not open
- **THEN** the start returns `Err(PinwinError::InvalidFd)` and nothing opens

#### Scenario: Fd outlives the panel
- **WHEN** the panel is dropped
- **THEN** the pty master fd is still open and usable by the host

### Requirement: The library never exits
No library call SHALL terminate the host process for any reason: bad layout, missing display,
missing Ghostty config, terminal allocation failure, GTK errors and panics inside the library
(on the host's thread, on the GTK thread, or inside a terminal or GTK callback) are error
values or degraded states, never `exit()`, abort or an unwinding panic that crosses the
library's API. After a panic inside the library, the panel SHALL stop drawing and applying
layouts and every later call on its handle SHALL return `Err(PinwinError::Internal)`.
Terminal allocation failure keeps the previous grid and surfaces as
`PinwinError::Internal`.

#### Scenario: Bad layout does not kill the host
- **WHEN** the host applies a layout the monitor cannot hold
- **THEN** the call returns `Err(PinwinError::InvalidLayout)` and the host process keeps
  running with its previous panel state

#### Scenario: Panic does not reach the host
- **WHEN** code inside the library panics while handling a draw, input or layout request
- **THEN** the host process keeps running, no panic unwinds into the host's calls, later calls
  on the handle return `Err(PinwinError::Internal)`, and dropping the handle still returns

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
The animated layout apply, given a layout and a duration in milliseconds, SHALL, when the
requested layout differs from the applied layout only in its column count and push/cover
choice (same side, same left and right gutters) and the duration is greater than zero and GTK
animations are enabled, change the panel's pixel width from the current width to the target
width continuously over the duration, driven by the panel's frame clock. The reservation's
exclusive zone SHALL move with the panel only when the requested layout pushes and its strip
differs from the held strip; otherwise it SHALL hold still, in the same frames. The duration
SHALL be clamped to 1000 ms. It SHALL return the same results as the plain apply, where
`Ok(())` means the layout was validated and accepted rather than that the animation finished.

#### Scenario: Animate on request
- **WHEN** the host applies a pushing layout, animated, with columns changed from 40 to 60 and
  a duration of 200 ms
- **THEN** the panel width and the reservation grow through intermediate widths over about
  200 ms and tiled windows reflow alongside

#### Scenario: Covering expand holds tiles still
- **WHEN** the host applies a covering layout, animated, with columns changed from 40 to 120
  and a duration of 200 ms
- **THEN** the panel width grows through intermediate widths over about 200 ms while the
  reservation holds still and tiled windows do not move

#### Scenario: Snap on zero duration
- **WHEN** the host makes an animated apply with a duration of 0
- **THEN** the layout is applied in one step exactly as the plain apply would

#### Scenario: Snap when animations are disabled
- **WHEN** the GTK `gtk-enable-animations` setting is false
- **THEN** the animated apply applies the layout in one step

#### Scenario: Other fields snap
- **WHEN** the requested layout changes the side or the left or right gutter
- **THEN** it is applied in one step with no width animation

#### Scenario: Duration clamp
- **WHEN** the host passes a duration of 60000 ms
- **THEN** the animation lasts at most 1000 ms

### Requirement: Exact final width
When an animated width transition ends, the panel's pixel width SHALL equal the target
columns times the cell width. A pushing target SHALL leave the reservation equal to left plus
that width plus right; a covering target SHALL leave the reservation equal to the held strip
from the last pushing layout, with no residual offset from intermediate frames. The final
frame SHALL be produced by the same code path as a non-animated apply.

#### Scenario: Animation completes
- **WHEN** an animated pushing transition to 60 columns finishes
- **THEN** the panel width is exactly 60 times the cell width and the reservation matches

#### Scenario: Covering animation completes
- **WHEN** an animated covering transition to 120 columns finishes over a 40-column gap
- **THEN** the panel width is exactly 120 times the cell width and the reservation still
  equals the 40-column strip

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
semantics. An animated apply SHALL retarget from the current animated width. A non-animated
apply or a duration of 0 SHALL cancel the animation and snap to the requested layout.
Dropping the panel handle SHALL cancel any running animation before closing the surfaces.

#### Scenario: Retarget
- **WHEN** a collapse animation is half done and the host requests an expand
- **THEN** the width continues from its current value toward the expanded width

#### Scenario: Snap interrupts
- **WHEN** the host applies a plain layout during an animation
- **THEN** the panel snaps to that layout and no further animation frames change it

#### Scenario: Stop mid-animation
- **WHEN** the host drops the panel handle during an animation
- **THEN** no surface remains and no animation callback runs afterwards

### Requirement: No mid-state on failure
A rejected or failed animated apply SHALL NOT leave the panel at an intermediate width. A
layout the panel refuses SHALL leave the applied layout unchanged. If the terminal grid cannot
be allocated when an animation starts, the panel SHALL be snapped to the requested layout and
the call SHALL return `Err(PinwinError::Internal)`.

#### Scenario: Invalid layout
- **WHEN** the host applies, animated, a layout the panel refuses
- **THEN** it returns `Err(PinwinError::InvalidLayout)` and the width and layout are unchanged

#### Scenario: Terminal allocation failure
- **WHEN** the terminal grid cannot be allocated at animation start
- **THEN** the call returns `Err(PinwinError::Internal)` and the panel is at the requested
  final width, not an intermediate one

### Requirement: Library Panel API
The crate's library SHALL expose a `Panel` handle. Starting takes a host-owned pty master fd,
a full layout (side, columns 1..=65535, four gutters, push/cover choice with pushing as the
backward-compatible default) and a keyboard mode, and an optional accent, and returns
`Result<Panel, PinwinError>` once the panel is on screen or has failed. Applying a layout,
plain or animated with a duration in milliseconds, is a method on the handle returning
`Result<(), PinwinError>`. Dropping the handle closes the panel, cancels any running
animation; the library's GTK thread is process-lifetime and is not joined, so it stays parked
for a later start. It SHALL NOT panic. At most one panel exists per process: a start while
another handle is alive SHALL fail with `PinwinError::AlreadyRunning` and leave the running
panel unchanged. Applying through a handle whose panel is no longer live (its GTK side ended
on its own) SHALL return `Err(PinwinError::NotRunning)` without blocking. A layout apply SHALL
NOT block the host thread indefinitely: a GTK side that does not answer within a bounded time
is `Err(PinwinError::Internal)`. After a drop, a new start MAY succeed.

#### Scenario: Start, relayout, drop
- **WHEN** the host starts the panel, applies a valid layout, then drops the handle
- **THEN** every call returns `Ok`, the panel follows the layouts, and after the drop no
  surface remains

#### Scenario: Covering relayout
- **WHEN** the host starts pushing at 40 columns and applies a covering 120-column layout
- **THEN** both calls return `Ok`, the gap stays at the 40-column strip, and after the drop no
  surface remains

#### Scenario: Double start
- **WHEN** the host starts a panel while another handle is alive
- **THEN** the second start returns `Err(PinwinError::AlreadyRunning)` and the running panel
  is unchanged

#### Scenario: Apply on a dead panel
- **WHEN** the panel's GTK side has ended on its own and the host applies an animated layout
- **THEN** the call returns `Err(PinwinError::NotRunning)` without blocking

#### Scenario: Wedged GTK side
- **WHEN** the GTK side does not answer an apply within the bounded wait
- **THEN** the call returns `Err(PinwinError::Internal)` and the host thread is not blocked
  further

### Requirement: The pinwin binary
The crate SHALL build a standalone `pinwin` program that runs a command (default `$SHELL`,
else `/bin/sh`) in the panel docked left, until the command exits. The program owns the pty,
sets `TERM=xterm-256color` and `COLORTERM=truecolor` for the child, forwards SIGINT and SIGTERM
to the child as SIGHUP, and exits with the child's exit status (1 when the child did not exit
normally). It reads `COLS` (1..=65535, default 40), `GUTTER` (0..=65535, the right gutter,
default 0), `PINWIN_KEYBOARD` (`on-demand` default, `exclusive`, `none`), `PINWIN_ACCENT`
(`on` default, `off`), `PINWIN_ACCENT_COLOR` (`#RRGGBB` or `RRGGBB`, default `#dabc7f`) and
`PINWIN_ACCENT_WIDTH` (1..=65535, default 1, read only when the accent is on). An invalid value, or an unknown `--` option,
SHALL exit 2 with a message on stderr before any surface opens; `--` ends option parsing. If
the panel cannot start, the program SHALL hang up the child, wait for it, print a message and
exit 1.

#### Scenario: Run a command
- **WHEN** the user runs `pinwin htop` in a niri session
- **THEN** a panel docks at the left edge running htop, and when htop exits the panel closes
  and `pinwin` exits with htop's status

#### Scenario: Default command
- **WHEN** the user runs `pinwin` with no arguments and `$SHELL` is set
- **THEN** the panel runs `$SHELL`

#### Scenario: Invalid environment
- **WHEN** `COLS=0` or `PINWIN_KEYBOARD=sometimes` is set
- **THEN** `pinwin` prints a message, exits 2 and opens nothing

#### Scenario: Unknown option
- **WHEN** the user runs `pinwin --frobnicate`
- **THEN** `pinwin` prints a message and exits 2

#### Scenario: Panel cannot start
- **WHEN** `pinwin` runs without a Wayland compositor that has layer-shell
- **THEN** the child is hung up and reaped, a message is printed and `pinwin` exits 1

### Requirement: Optimised terminal library build
The build SHALL compile libghostty-vt with an explicit Zig optimise mode, never the Zig
default (Debug). The mode SHALL be recorded in the archive's source identity so that changing
it rebuilds the archive.

#### Scenario: Explicit optimise mode
- **WHEN** `build.rs` builds ghostty
- **THEN** the zig command line includes `-Doptimize=` with the chosen mode

#### Scenario: Mode change invalidates the cache
- **WHEN** an archive exists built under one optimise mode and the build now requests another
- **THEN** the archive is rebuilt

### Requirement: Seamless cell grid at any output scale
The panel SHALL snap the edges of every rectangle in the cell grid to whole device pixels.
These rectangles are the cell backgrounds, the block glyphs, the cursor, and the underline
and strikethrough bands. Block glyphs are the full, half, eighth, quadrant and braille
blocks. Two cells that share an edge SHALL get the same snapped edge. No line or seam SHALL
be visible between cells at any output scale, fractional or integer.

The grid's offset from the docked edge SHALL be a whole number of device pixels in every
frame, including each frame of a width animation. When the output scale changes, the panel
SHALL redraw the grid at the new scale. The fallback painter SHALL meet the same rules.

Snapping SHALL NOT change the cell size, the panel width or any size that the panel reports.
A bar, band or outline SHALL be at least one device pixel thick. Its thickness SHALL be
within one device pixel of its logical thickness times the output scale.

#### Scenario: Fractional scale has no seams
- **WHEN** the panel runs at an output scale of 1.5 and a region of cells has one background
  colour or holds full-block glyphs
- **THEN** the region shows one uniform colour with no lines between cells

#### Scenario: Integer scale has no seams
- **WHEN** the panel runs at an output scale of 1 or 2 and shows the same region
- **THEN** the region shows one uniform colour with no lines between cells

#### Scenario: Half blocks meet without a seam
- **WHEN** the panel runs at an output scale of 1.25 and a right-half block sits beside a
  left-half block of the same colour
- **THEN** the two blocks show one uniform colour with no line between them

#### Scenario: Width animation
- **WHEN** a width animation runs at an output scale of 1.5 on a right-docked panel
- **THEN** the grid's offset from the docked edge is a whole number of device pixels in
  every frame and no seam appears between cells

#### Scenario: Scale change
- **WHEN** the panel moves to an output with a different scale
- **THEN** the panel redraws the grid at the new scale and no seam appears between cells

#### Scenario: Fallback painter
- **WHEN** the fallback painter draws the grid at an output scale of 1.5
- **THEN** it snaps the same edges as the primary painter and no seam appears

#### Scenario: Bar cursor keeps its physical size
- **WHEN** the output scale is 1.5 and the cursor is a bar 2 logical pixels wide
- **THEN** the bar is 3 device pixels wide

#### Scenario: Sizes stay logical
- **WHEN** the output scale is 1.5
- **THEN** the cell size, the panel width and the reply to `CSI 16 t` equal their values at
  an output scale of 1

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
