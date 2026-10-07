# Spec Delta

## ADDED Requirements

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
- **THEN** the cell size, the panel width and the reply to `CSI 16 t` equal their values at
  an output scale of 1

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
The `Panel` handle SHALL offer a toggle. The host can call it from any thread, and it returns
`Result<(), PinwinError>`. A toggle hides a shown panel and shows a hidden one. The panel is
shown at start.

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
SHALL raise a `SIGWINCH`. No frame SHALL show the panel at another size. While the panel is
hidden, a layout apply SHALL validate and store the layout as it does when shown, with no
animation and nothing on screen. The next show uses that layout.

If the panel is no longer live, the toggle SHALL return `Err(PinwinError::NotRunning)` without
blocking. If the panel thread does not answer within a bounded time, the toggle SHALL return
`Err(PinwinError::Internal)`, so the host thread never blocks indefinitely. The library SHALL
own no transport for the request.

#### Scenario: Show takes focus
- **WHEN** the panel runs in `on-demand` mode on niri, is hidden, a tiled window has the
  keyboard, and the host toggles
- **THEN** the call returns `Ok`, the panel appears, and the next typed key goes to the pty
  master. If the accent is enabled, it appears.

#### Scenario: Hide
- **WHEN** the panel is shown and the host toggles
- **THEN** the call returns `Ok`, the panel leaves the screen, and keys go to a compositor
  window

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
- **WHEN** the panel's thread ended on its own and the host toggles
- **THEN** the call returns `Err(PinwinError::NotRunning)` without blocking

### Requirement: Toggle from the command line
The `pinwin` program SHALL accept toggle requests for its panel from other processes on the
same Wayland display. The program SHALL read `PINWIN_NAME`, with the default `default`. A
name has 1..=64 characters from `[A-Za-z0-9_-]`. Another process SHALL request a toggle with
`pinwin --toggle [name]`, and the name defaults to `default`. The host SHALL pass the request
to its panel's toggle.

If the named instance accepts the request, the client SHALL exit 0. If no instance with that
name answers on the display, the client SHALL print a message on stderr and exit 1. A name in
`PINWIN_NAME` or after `--toggle` can be invalid. Then `pinwin` SHALL print a message on
stderr and exit 2, and the host SHALL open nothing.

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

#### Scenario: No instance
- **WHEN** no `pinwin` with the name `notes` runs and a key binding runs `pinwin --toggle notes`
- **THEN** the client prints a message and exits 1

#### Scenario: Invalid name
- **WHEN** `PINWIN_NAME=a/b` is set, or the user runs `pinwin --toggle a/b`
- **THEN** `pinwin` prints a message, exits 2 and opens nothing

#### Scenario: Duplicate name
- **WHEN** `pinwin htop` runs and the user starts a second `pinwin` with no `PINWIN_NAME`
- **THEN** the second `pinwin` prints a message, exits 2 and opens nothing, and the first
  panel is unchanged

#### Scenario: Instance that crashed
- **WHEN** a `pinwin` named `notes` was killed with SIGKILL and the user starts a new one
  with the same name
- **THEN** the new `pinwin` starts normally and answers `pinwin --toggle notes`

## MODIFIED Requirements

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

#### Scenario: Wedged GTK side
- **WHEN** the panel thread does not answer an apply within the bounded wait
- **THEN** the call returns `Err(PinwinError::Internal)` and the host thread is not blocked
  further

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
- **THEN** the cell size, the panel width and the reply to `CSI 16 t` equal their values at
  an output scale of 1

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

## REMOVED Requirements

### Requirement: Focus on request
**Reason**: Focus now comes from showing the panel. Stock niri focuses a newly mapped layer
surface and ignores xdg-activation for one, and the remap this requirement replaced flashed
an interim size.
**Migration**: Call `Panel::toggle`; see "Show and hide on request".

### Requirement: Focus request from the command line
**Reason**: `pinwin --toggle` replaces `pinwin --focus`.
**Migration**: Bind the compositor key to `pinwin --toggle [name]` instead of
`pinwin --focus [name]`; see "Toggle from the command line".
