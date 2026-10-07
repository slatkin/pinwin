# pinwin-panel Specification

## Purpose

`pinwin` docks a terminal running the user's chosen command (their `$SHELL` by default) at the left edge of one monitor, reserving that space so the compositor tiles windows to its right, and releases the space when the command exits.

## Requirements

### Requirement: Keyboard focus by clicking
If the startup layout requests `on-demand` mode, the panel SHALL use layer-shell `on-demand`
keyboard interactivity. In this mode, the panel receives keyboard input only after one of two
events. The user clicks inside the panel, or a toggle shows the panel. The panel gives up
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

The pixel sizes SHALL be in device pixels of the panel's output: the cell size reported by
`CSI 16 t` is the panel's logical cell size scaled by the panel's resolved output scale and
rounded to a whole device pixel, and `CSI 14 t` and the window size's pixel fields are the
column and row counts times that reported device cell size. The column and row counts
SHALL stay the
layout's logical ones. When the output scale changes, the panel SHALL update the window
size's pixel fields (raising `SIGWINCH` as any winsize update does) so later queries and
reads answer the new scale; the grid's column and row counts SHALL NOT change with the
scale.

#### Scenario: Cell size query
- **WHEN** the host's child writes `CSI 16 t`
- **THEN** it receives `CSI 6 ; <cell height px> ; <cell width px> t` matching the drawn
  cell size in device pixels

#### Scenario: Images are not clipped
- **WHEN** the host's child shows a poster image in the panel
- **THEN** the whole image is visible, with no part cut off at the right or bottom edge

#### Scenario: Pixel size in window size
- **WHEN** the host's child reads the terminal size with `TIOCGWINSZ`
- **THEN** `ws_col`/`ws_row` are the cell grid and `ws_xpixel`/`ws_ypixel` are the grid size
  in device pixels

#### Scenario: Fractional scale reports device pixels
- **WHEN** the panel runs at an output scale of 1.8 with a 9 logical pixel wide cell, 40
  columns, and the host's child queries the cell size, the text-area size, or reads the
  window size
- **THEN** the cell width reported is 16 device pixels (9 × 1.8, rounded) and the `CSI 14 t`
  text-area width and the window size's pixel width are 640 device pixels (40 columns ×
  the reported 16-pixel cell)

#### Scenario: Scale change updates the reports
- **WHEN** the panel's output scale changes while the panel runs
- **THEN** the window size's pixel fields are updated to the new device size and the
  host's child receives `SIGWINCH`, while the column and row counts stay unchanged

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
a compositor connection or any surface to exist.

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
applies. While the panel is hidden, the reservation SHALL be released, and showing the panel
SHALL restore the held strip. The reservation SHALL apply only to the panel's monitor. The gap
between the panel and the first tile is the far gutter plus whatever strut the compositor
itself adds.

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

#### Scenario: Hidden panel reserves nothing
- **WHEN** the held strip is 332 px on the left and the panel is hidden
- **THEN** tiled windows take the strip, and they move back beside it when the panel is shown

### Requirement: Host-owned pty
The library SHALL read and write the pty master fd supplied at start and apply the window
size to it (`TIOCSWINSZ`); it SHALL NOT fork, wait on a child, close the fd, or set any child
environment (the host owns the fd's lifetime and its child's `TERM` and `COLORTERM`). A start
whose fd is not an open descriptor SHALL fail with `PinwinError::InvalidFd` before the library
connects to the compositor. Hangup on the master drops the read source without touching
process lifetime.

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
missing Ghostty config, terminal allocation failure, Wayland protocol errors, a lost
compositor connection and panics inside the library (on the host's thread, on the panel
thread, or inside a terminal or Wayland callback) are error values or degraded states, never
`exit()`, abort or an unwinding panic that crosses the library's API. After a panic inside the
library, the panel SHALL stop drawing and applying layouts and every later call on its handle
SHALL return `Err(PinwinError::Internal)`. Terminal allocation failure keeps the previous grid
and surfaces as `PinwinError::Internal`.

#### Scenario: Bad layout does not kill the host
- **WHEN** the host applies a layout the monitor cannot hold
- **THEN** the call returns `Err(PinwinError::InvalidLayout)` and the host process keeps
  running with its previous panel state

#### Scenario: Panic does not reach the host
- **WHEN** code inside the library panics while handling a draw, input or layout request
- **THEN** the host process keeps running, no panic unwinds into the host's calls, later calls
  on the handle return `Err(PinwinError::Internal)`, and dropping the handle still returns

#### Scenario: Compositor goes away
- **WHEN** the compositor connection closes while the panel runs
- **THEN** the host process keeps running and later calls on the handle return
  `Err(PinwinError::NotRunning)`

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
choice (same side, same left and right gutters) and the duration is greater than zero, change
the panel's pixel width from the current width to the target width continuously over the
duration, driven by the compositor's frame callbacks. The reservation's exclusive zone SHALL
move with the panel only when the requested layout pushes and its strip differs from the held
strip; otherwise it SHALL hold still, in the same frames. The duration SHALL be clamped to
1000 ms. It SHALL return the same results as the plain apply, where `Ok(())` means the layout
was validated and accepted rather than that the animation finished. The library SHALL NOT
read any desktop animation setting. The host's duration is the only control.

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
- **WHEN** the host turns animations off by passing a duration of 0 to every animated apply
- **THEN** each apply takes effect in one step

#### Scenario: Desktop animation setting has no effect
- **WHEN** the GTK `gtk-enable-animations` setting is false and the host makes an animated
  apply with a duration of 200 ms
- **THEN** the panel width animates over about 200 ms

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
`Result<(), PinwinError>`. Dropping the handle closes the panel, cancels any running animation
and ends the panel's thread. A drop SHALL NOT block the host thread beyond a bounded wait, and
it SHALL NOT panic. At most one panel exists per process: a start while another handle is
alive SHALL fail with `PinwinError::AlreadyRunning` and leave the running panel unchanged.
Applying through a handle whose panel is no longer live (its panel thread ended on its own)
SHALL return `Err(PinwinError::NotRunning)` without blocking. A layout apply SHALL NOT block
the host thread indefinitely: a panel thread that does not answer within a bounded time is
`Err(PinwinError::Internal)`. After a drop, a new start can succeed.

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

#### Scenario: Start after drop
- **WHEN** the host drops the handle and starts a new panel
- **THEN** the new start returns `Ok` and the new panel opens

#### Scenario: Apply on a dead panel
- **WHEN** the panel's thread ended on its own and the host applies an animated layout
- **THEN** the call returns `Err(PinwinError::NotRunning)` without blocking

#### Scenario: Wedged panel thread
- **WHEN** the panel thread does not answer an apply within the bounded wait
- **THEN** the call returns `Err(PinwinError::Internal)` and the host thread is not blocked
  further

### Requirement: The pinwin binary
The crate SHALL build a standalone `pinwin` program that runs a command (default `$SHELL`,
else `/bin/sh`) in the panel docked left, until the command exits. The program owns the pty,
sets `TERM=xterm-256color` and `COLORTERM=truecolor` for the child, forwards SIGINT and SIGTERM
to the child as SIGHUP, and exits with the child's exit status (1 when the child did not exit
normally). It reads `COLS` (1..=65535, default 40), `GUTTER` (0..=65535, the right gutter,
default 0), `PINWIN_KEYBOARD` (`on-demand` default, `exclusive`, `none`), `PINWIN_ZONE`
(`reserve` default, `overlay`), `PINWIN_ACCENT` (`on` default, `off`), `PINWIN_ACCENT_COLOR`
(`#RRGGBB` or `RRGGBB`, default `#dabc7f`) and `PINWIN_ACCENT_WIDTH` (1..=65535, default 1,
read only when the accent is on). With `PINWIN_ZONE=reserve`, the program starts with a
pushing layout, so tiled windows sit beside the panel and a toggle moves them. With
`PINWIN_ZONE=overlay`, it starts with a covering layout, so it reserves nothing, the panel
draws over tiled windows and a toggle moves no window. An invalid value, or an unknown `--`
option, SHALL exit 2 with a message on stderr before any surface opens; `--` ends option
parsing. If the panel cannot start, the program SHALL hang up the child, wait for it, print a
message and exit 1.

#### Scenario: Run a command
- **WHEN** the user runs `pinwin htop` in a niri session
- **THEN** a panel docks at the left edge running htop, and when htop exits the panel closes
  and `pinwin` exits with htop's status

#### Scenario: Default command
- **WHEN** the user runs `pinwin` with no arguments and `$SHELL` is set
- **THEN** the panel runs `$SHELL`

#### Scenario: Invalid environment
- **WHEN** `COLS=0`, `PINWIN_KEYBOARD=sometimes` or `PINWIN_ZONE=both` is set
- **THEN** `pinwin` prints a message, exits 2 and opens nothing

#### Scenario: Overlay zone
- **WHEN** the user runs `PINWIN_ZONE=overlay pinwin htop` and toggles it twice
- **THEN** tiled windows never move, and the panel draws over them while shown

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
SHALL redraw the grid at the new scale.

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
- **WHEN** the panel draws the grid at an output scale of 1.5
- **THEN** one painter draws every frame, with no fallback painter, and no seam appears

#### Scenario: Bar cursor keeps its physical size
- **WHEN** the output scale is 1.5 and the cursor is a bar 2 logical pixels wide
- **THEN** the bar is 3 device pixels wide

#### Scenario: Sizes stay logical
- **WHEN** the output scale is 1.5
- **THEN** the logical cell size, the panel's logical width and the terminal's column and
  row counts equal their values at an output scale of 1, while the sizes the panel reports
  to the host carry device pixels (see Correct size reports)

### Requirement: Cell text on the device pixel lattice
The panel SHALL place the origin of the text in each cell on a whole device pixel. A device
pixel is one physical pixel. The origin SHALL come from the cell's own snapped corner plus an
offset inside the cell that is also a whole number of device pixels. The same glyph, in the
same style and colour, SHALL then render to the same device pixels inside every cell, at any
output scale. This holds for plain text, wide cells and glyphs that the nerd-font constraints
scale and centre.

The panel SHALL NOT change the glyph size, the cell size, the panel width or any size that
the panel reports. At a fractional output scale, the distance between the origins of two
adjacent cells can differ by one device pixel.

#### Scenario: Identical letters render alike at a fractional scale
- **WHEN** the panel runs at an output scale of 1.8 and a row holds the same letter in every
  cell
- **THEN** the device pixels of the glyph, taken from each cell's snapped top-left corner,
  are the same in every cell

#### Scenario: Cell height that is not a whole device pixel count
- **WHEN** the output scale is 1.8, the cell is 19 logical pixels high and several rows hold
  the same letter
- **THEN** the glyph sits at the same device pixel offset from the top of its cell in every
  row

#### Scenario: Constrained glyph
- **WHEN** the output scale is 1.5 and a nerd-font glyph that the constraints scale and
  centre appears in cells at different columns
- **THEN** the glyph sits at the same device pixel offset inside each cell

#### Scenario: Text sizes stay logical
- **WHEN** the output scale is 1.8
- **THEN** the logical cell size, the panel's logical width and the terminal's column and
  row counts equal their values at an output scale of 1, while the sizes the panel reports
  to the host carry device pixels (see Correct size reports)

### Requirement: Key repeat follows the compositor
While the panel holds keyboard focus and the user holds down a key, the panel SHALL repeat
that key at the delay and rate that the compositor sends. Each repeat SHALL reach the
terminal as a repeat event. With the kitty keyboard protocol and event types enabled, the
child receives a repeat report. Under the other encodings, the child receives the key again.
Modifier keys SHALL NOT repeat. When the user releases the key, the repeat SHALL stop. When
the panel loses keyboard focus, the repeat SHALL also stop. If the compositor sends a repeat rate of 0, no key SHALL
repeat.

#### Scenario: Held key repeats
- **WHEN** the panel has keyboard focus and the user holds `j` past the compositor's repeat
  delay
- **THEN** the pty master receives `j` once for the press and again at the compositor's
  repeat rate until the release

#### Scenario: Focus loss stops the repeat
- **WHEN** the user holds `j` in the panel and clicks a tiled window with the mouse
- **THEN** the pty master receives no further `j` after the panel loses keyboard focus

#### Scenario: Repeat disabled
- **WHEN** the compositor sends a repeat rate of 0 and the user holds `j`
- **THEN** the pty master receives `j` once

### Requirement: Pointer cursor over the panel
If the compositor offers the cursor-shape protocol, the panel SHALL set the default pointer
cursor shape each time the pointer enters the panel. Without that protocol, the panel SHALL
leave the pointer cursor unset.

#### Scenario: Cursor on enter
- **WHEN** the pointer moves from a tiled window that shows a text cursor into the panel on
  niri
- **THEN** the pointer shows the default cursor shape over the panel

### Requirement: No toolkit libraries
The library and the `pinwin` binary SHALL NOT link these libraries: GTK, GDK, GLib, GObject,
GIO, Pango, Cairo, gdk-pixbuf and HarfBuzz. The rule also covers a link through another
library.

#### Scenario: Dynamic dependencies
- **WHEN** the dynamic library dependencies of the built `pinwin` binary are listed
- **THEN** no GTK, GDK, GLib, GObject, GIO, Pango, Cairo, gdk-pixbuf or HarfBuzz library
  appears in the list

### Requirement: Show and hide on request
The `Panel` handle SHALL offer a toggle and a show. The host can call either from any thread,
and both return `Result<(), PinwinError>`. A toggle hides a shown panel and shows a hidden
one. A show shows a hidden panel and leaves a shown panel unchanged; on a shown panel it SHALL
return `Ok` and change nothing on screen. The panel is shown at start.

Hiding SHALL unmap the panel surface and release the held reservation. A width animation in
progress SHALL end at its target layout first. The terminal and the pty keep running while
the panel is hidden, and the host's child keeps receiving input from the pty.

Showing SHALL map the panel surface again, draw the current grid and restore the held
reservation. In `on-demand` mode, the panel SHALL map with `on-demand` keyboard
interactivity, so a compositor that focuses a newly mapped `on-demand` surface gives it the
keyboard without a click. In `exclusive` mode, the panel maps as `exclusive`. In `none` mode,
the panel only appears. After a show, the panel keeps and gives up focus by the rules of its
keyboard mode.

Neither hiding nor showing SHALL change the terminal grid or the pty window size, and neither
SHALL raise a `SIGWINCH`. No frame SHALL show the panel at another size. Two cases are
exempt. A width animation that a hide ends runs its deferred grid resize as it would at its
own end. If the output's height changed while the panel was hidden, the configure that a show
receives resizes the grid and the pty like any other configure with a new height. While the panel is
hidden, a layout apply SHALL validate and store the layout as it does when shown, with no
animation and nothing on screen. The next show uses that layout.

If the panel is no longer live, the toggle and the show SHALL return
`Err(PinwinError::NotRunning)` without blocking. If the panel thread does not answer within a
bounded time, they SHALL return `Err(PinwinError::Internal)`, so the host thread never blocks
indefinitely. Requests from other processes reach the panel only through the instance socket
the host opted into.

#### Scenario: Show takes focus
- **WHEN** the panel runs in `on-demand` mode on niri, is hidden, a tiled window has the
  keyboard, and the host toggles
- **THEN** the call returns `Ok`, the panel appears, and the next typed key goes to the pty
  master. If the accent is enabled, it appears.

#### Scenario: Hide
- **WHEN** the panel is shown and the host toggles
- **THEN** the call returns `Ok`, the panel leaves the screen, and keys go to a compositor
  window

#### Scenario: Show a hidden panel
- **WHEN** the panel is hidden and the host calls show
- **THEN** the call returns `Ok` and the panel appears as it does after a toggle from hidden

#### Scenario: Show a shown panel
- **WHEN** the panel is shown and the host calls show
- **THEN** the call returns `Ok`, the panel stays on screen, no surface is unmapped or
  remapped, and the reservation does not change

#### Scenario: Grid and pty untouched
- **WHEN** the host's child draws a layout that depends on the terminal size and the host
  toggles twice
- **THEN** the child observes no window size change and receives no `SIGWINCH`, and no frame
  shows the panel drawn at another size

#### Scenario: Show in none mode
- **WHEN** the panel runs in `none` mode, is hidden, and the host toggles
- **THEN** the panel appears and keyboard input stays with the window that had it

#### Scenario: Hide releases the reservation
- **WHEN** the panel pushes tiled windows and the host toggles twice
- **THEN** the tiled windows take the panel's strip after the first toggle and move back
  beside the panel after the second

#### Scenario: Apply while hidden
- **WHEN** the panel is hidden and the host applies a layout with more columns, animated with
  a duration of 200 ms
- **THEN** the call returns `Ok`, nothing appears, and the next toggle shows the panel at the
  new width with no animation

#### Scenario: Toggle on a dead panel
- **WHEN** the panel's thread ended on its own and the host toggles or shows
- **THEN** the call returns `Err(PinwinError::NotRunning)` without blocking

### Requirement: Toggle from the command line
The `pinwin` program SHALL accept toggle and show requests for its panel from other processes
on the same Wayland display, through the library's instance socket. The program SHALL read
`PINWIN_NAME`, with the default `default`. A name has 1..=64 characters from
`[A-Za-z0-9_-]`. Another process SHALL request a toggle with `pinwin --toggle [name]` and a
show with `pinwin --show [name]`, and the name defaults to `default`. Both reach any panel
bound to that name, whether the `pinwin` program or a library host started it.

If the named instance accepts the request, the client SHALL exit 0. If no instance with that
name answers on the display, or the instance refuses the request, the client SHALL print a
message on stderr and exit 1. A name in `PINWIN_NAME` or after `--toggle` or `--show` can be
invalid. Then `pinwin` SHALL print a message on stderr and exit 2, and the host SHALL open
nothing.

A live instance on the same display can already use the name. Then a second host SHALL print
a message and exit 2 before any surface opens. A name from an instance that no longer runs
SHALL NOT block a new host. Different names and different displays SHALL NOT interfere.

#### Scenario: Toggle the default instance
- **WHEN** `pinwin htop` runs in `on-demand` mode on niri and a compositor key binding runs
  `pinwin --toggle` twice
- **THEN** the first run hides the panel, the second shows it with keyboard focus, and the
  client exits 0 each time

#### Scenario: Toggle a named instance
- **WHEN** `PINWIN_NAME=notes pinwin nvim` and `pinwin htop` both run, and a compositor key
  binding runs `pinwin --toggle notes`
- **THEN** the `notes` panel hides, the other panel does not change, and the client exits 0

#### Scenario: Show from the command line
- **WHEN** `pinwin htop` runs and the user runs `pinwin --show` twice, then `pinwin --toggle`,
  then `pinwin --show`
- **THEN** the panel stays shown after both shows, hides on the toggle, appears again on the
  last show, and each client exits 0

#### Scenario: Toggle a library-hosted panel
- **WHEN** a library host binds the name `notes` and starts its panel, and a key binding runs
  `pinwin --toggle notes`
- **THEN** that panel hides and the client exits 0

#### Scenario: No instance
- **WHEN** no instance with the name `notes` runs and a key binding runs
  `pinwin --toggle notes` or `pinwin --show notes`
- **THEN** the client prints a message and exits 1

#### Scenario: Invalid name
- **WHEN** `PINWIN_NAME=a/b` is set, or the user runs `pinwin --toggle a/b` or
  `pinwin --show a/b`
- **THEN** `pinwin` prints a message, exits 2 and opens nothing

#### Scenario: Duplicate name
- **WHEN** `pinwin htop` runs and the user starts a second `pinwin` with no `PINWIN_NAME`
- **THEN** the second `pinwin` prints a message, exits 2 and opens nothing, and the first
  panel is unchanged

#### Scenario: Instance that crashed
- **WHEN** a `pinwin` named `notes` was killed with SIGKILL and the user starts a new one
  with the same name
- **THEN** the new `pinwin` starts normally and answers `pinwin --toggle notes`

### Requirement: Instance socket opt-in for library hosts
The library SHALL let a host bind an instance socket for a name on its Wayland display, then
pass the bound socket to the start. A name has 1..=64 characters from `[A-Za-z0-9_-]`. The
bind SHALL need no panel, so a host can bind before any surface opens. A start without a bound
socket SHALL listen on nothing.

#### Scenario: No socket without opt-in
- **WHEN** a host starts a panel without a bound socket
- **THEN** no socket file is created, and a client call to any name reports that no instance
  is listening

#### Scenario: Library host answers a toggle
- **WHEN** a host binds the name `notes`, starts a panel with that socket, and another process
  sends `toggle` to `notes`
- **THEN** the panel hides and the client call returns `Ok`

### Requirement: Instance bind errors
If a live instance on the same display already uses the name, the bind SHALL fail with a
typed duplicate error and leave that instance unchanged. A socket file left by an instance
that no longer runs SHALL NOT block the bind. Different names and different displays SHALL
NOT interfere. A missing runtime directory, or a display name that cannot be used in a path,
SHALL fail the bind with a typed error and create nothing.

#### Scenario: Duplicate bind
- **WHEN** a panel bound to `notes` runs and a second host binds `notes` on the same display
- **THEN** the second bind returns the duplicate error before that host opens any surface,
  and the first panel is unchanged

#### Scenario: Stale socket file
- **WHEN** a host bound to `notes` was killed with SIGKILL and a new host binds `notes`
- **THEN** the bind succeeds and the new panel answers requests

### Requirement: Serving instance requests
A panel started with a bound socket SHALL pass `toggle` requests to its toggle and `show`
requests to its show. It SHALL refuse any other request without changing the panel. A client
that stalls or floods SHALL NOT block later requests or the host.

#### Scenario: Library host answers a show
- **WHEN** a host's panel bound to `notes` is hidden and another process sends `show` to
  `notes` twice
- **THEN** the first request shows the panel, the second leaves it shown, and both calls
  return `Ok`

#### Scenario: Unknown request
- **WHEN** a process writes `hide\n` to the socket of a panel bound to `notes`
- **THEN** the panel answers with an error and does not change

### Requirement: Instance socket lifetime
Dropping the handle SHALL remove the socket file and SHALL NOT wait for the listener to end.
After the drop, no request reaches the old panel. If a start with a bound socket fails, the
library SHALL close the socket and remove its file.

#### Scenario: Drop does not wait on the listener
- **WHEN** a host drops a handle whose panel serves a socket
- **THEN** the drop returns within the bounded teardown wait, the socket file is gone, and
  the host process can exit at once

#### Scenario: Failed start releases the name
- **WHEN** a host binds `notes` and the start fails with `NoDisplay`
- **THEN** the socket file is gone, and a new bind of `notes` succeeds

### Requirement: Instance client call
The library SHALL offer a client call that sends `toggle` or `show` to a named instance on the
current display. The call SHALL return `Result` and SHALL NOT block indefinitely. Its error
SHALL tell four cases apart: the environment gives no usable socket path; no instance with
that name is listening; the instance refused or failed the request; no valid answer came in
time.

#### Scenario: Client finds no instance
- **WHEN** nothing is bound to `notes` and a process sends `show` to `notes`
- **THEN** the client call returns the "no instance listening" error, distinct from a refused
  request or a missing answer

#### Scenario: Request fails on the host
- **WHEN** the panel bound to `notes` has ended and a process sends `toggle` to `notes`
- **THEN** the client call returns the "refused or failed" error

### Requirement: Kitty images drawn at device resolution
A kitty image whose drawn size comes from its own pixel size SHALL be drawn at that size in
device pixels: its logical size is the image's pixel size divided by the panel's resolved
output scale, rounded to a whole logical pixel. This applies to unicode-placeholder
placements and to placements that give neither a column nor a row count. A placement that
gives a column or row count SHALL keep the size its cells give it.

#### Scenario: Placeholder image at a fractional scale
- **WHEN** the panel runs at an output scale of 1.8 and the host's child shows a 576×324
  pixel image through unicode placeholders
- **THEN** the image is drawn 320×180 logical pixels (576×324 device pixels) and stays
  inside the panel

#### Scenario: Image-sized placement at a fractional scale
- **WHEN** the panel runs at an output scale of 1.8 and the host's child places a 576×324
  pixel image with no column or row count
- **THEN** the image is drawn 320×180 logical pixels

#### Scenario: Cell-sized placement is unchanged
- **WHEN** the panel runs at an output scale of 1.8 and the host's child places an image
  with a column count of 10 and a row count of 5
- **THEN** the image is drawn 10 cells wide and 5 cells high, the same as at scale 1

#### Scenario: Scale 1 is unchanged
- **WHEN** the panel runs at an output scale of 1 and shows a 320×180 pixel image through
  unicode placeholders
- **THEN** the image is drawn 320×180 logical pixels
