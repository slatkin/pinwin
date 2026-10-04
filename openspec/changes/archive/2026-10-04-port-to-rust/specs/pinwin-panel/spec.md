## MODIFIED Requirements

### Requirement: Keyboard focus by clicking
The panel SHALL use layer-shell `on-demand` keyboard interactivity when the startup layout
requests it: it receives keyboard input only after the user clicks inside it, and gives up
keyboard input when the user clicks a compositor window. It SHALL NOT take keyboard focus when
it opens. The keyboard mode is fixed at start time; there is no runtime override.

While the panel holds keyboard focus, it SHALL mark itself with a focus accent: a stroke
around its whole window, in the colour and pixel width the startup supplies. The compositor
draws no focus ring on layer surfaces, so the panel draws its own. When the accent is
disabled, the panel SHALL draw nothing extra, focused or not.

The keyboard mode is one of exactly `none`, `on-demand` or `exclusive`, and an accent is
either absent or a colour with a width of 1..=65535 pixels; the library's argument types
SHALL make any other keyboard mode or accent unrepresentable, so no runtime rejection exists
for them.

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
- **WHEN** a host tries to start the panel with a keyboard mode other than the three defined
  ones
- **THEN** the program does not compile, so no such start reaches the panel

#### Scenario: Invalid accent
- **WHEN** a host tries to start the panel with an accent width of 0 or above 65535
- **THEN** the program does not compile, so no such start reaches the panel

#### Scenario: Accent appears while focused
- **WHEN** the panel gains keyboard focus with the accent enabled
- **THEN** the whole window gains an outline of the configured colour and width,
  and it disappears when focus moves back to a compositor window

#### Scenario: Accent disabled
- **WHEN** the host starts the panel with no accent
- **THEN** the panel draws no accent, focused or not

### Requirement: Wayland layer-shell is required
The library SHALL run only on a Wayland compositor that implements wlr-layer-shell. When
layer-shell is unavailable (an X11 session, or a Wayland compositor without it such as GNOME),
starting the panel SHALL fail with `PinwinError::NoDisplay` and open nothing. It SHALL NOT fall
back to an ordinary window.

#### Scenario: No layer-shell
- **WHEN** the host starts the panel in a session whose compositor lacks wlr-layer-shell
- **THEN** the start returns `Err(PinwinError::NoDisplay)` and nothing opens

### Requirement: Apply layout without restarting the terminal
A successful layout apply, plain or animated, SHALL update the applied column count, all four
gutters and the docking side as one logical operation, update the reservation and the panel's
pixel width to the applied column count times the cell width (at the end of any animation),
and resize the existing terminal grid and PTY winsize when necessary. It SHALL NOT recreate the
terminal emulator, change the font, or touch the host's child. The host's child SHALL observe
updated column/row counts and pixel sizes matching the drawn grid after the resize.

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
A layout apply SHALL reject checked-arithmetic overflow, a reservation sum below zero,
vertical space insufficient for one complete terminal row, or a horizontal reservation that
leaves no output width for other windows. It SHALL return `Err(PinwinError::InvalidLayout)`
and leave the previous applied layout untouched. An unknown side, a zero column count, a
column count above 65535 and a gutter outside the 32-bit signed range SHALL be unrepresentable
in the layout argument type and therefore need no runtime check. Purely structural rejection
(arithmetic overflow of the gutters' own sum) SHALL NOT require any GTK surface to exist.

#### Scenario: Invalid geometry
- **WHEN** the host applies top/bottom gutters leaving less than one row
- **THEN** the call returns `Err(PinwinError::InvalidLayout)` and neither the live layout nor
  anything else changes

#### Scenario: Too wide for the output
- **WHEN** the host applies columns whose reservation leaves no output width for other windows
- **THEN** the call returns `Err(PinwinError::InvalidLayout)` and the live layout is unchanged

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

### Requirement: Animated width transition
The animated layout apply, given a layout and a duration in milliseconds, SHALL, when the
requested layout differs from the applied layout only in its column count (same side, same
left and right gutters) and the duration is greater than zero and GTK animations are enabled,
change the panel's pixel width and the reservation's exclusive zone from the current width to
the target width continuously over the duration, driven by the panel's frame clock. Both
surfaces SHALL change together in the same frame. The duration SHALL be clamped to 1000 ms. It
SHALL return the same results as the plain apply, where `Ok(())` means the layout was
validated and accepted rather than that the animation finished.

#### Scenario: Animate on request
- **WHEN** the host applies a layout, animated, with columns changed from 40 to 120 and a
  duration of 200 ms
- **THEN** the panel width and the reservation grow through intermediate widths over about
  200 ms and tiled windows reflow alongside

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

## REMOVED Requirements

### Requirement: C ABI lifecycle
**Reason**: The C ABI (`pinwin_api.h`, `pinwin_start`, `pinwin_apply_layout`,
`pinwin_apply_layout_animated`, `pinwin_stop`, `PinwinStartup`, `PINWIN_ERR_*`) is removed
with no replacement C surface; the library is a Rust crate.
**Migration**: Rust hosts depend on the `pinwin` crate and use the `Panel` handle (requirement
"Library Panel API"). A host that still needs C must wrap the crate itself; pinwin ships no C
header.

## ADDED Requirements

### Requirement: Library Panel API
The crate's library SHALL expose a `Panel` handle. Starting takes a host-owned pty master fd,
a full layout (side, columns 1..=65535, four gutters) and a keyboard mode, and an optional
accent, and returns `Result<Panel, PinwinError>` once the panel is on screen or has failed.
Applying a layout, plain or animated with a duration in milliseconds, is a method on the
handle returning `Result<(), PinwinError>`. Dropping the handle closes the panel, cancels any
running animation; the library's GTK thread is process-lifetime and is not joined, so it stays
parked for a later start. It SHALL NOT panic. At most one panel
exists per process: a start while another handle is alive SHALL fail with
`PinwinError::AlreadyRunning` and leave the running panel unchanged. Applying through a handle
whose panel is no longer live (its GTK side ended on its own) SHALL return
`Err(PinwinError::NotRunning)` without blocking. A layout apply SHALL NOT block the host
thread indefinitely: a GTK side that does not answer within a bounded time is
`Err(PinwinError::Internal)`. After a drop, a new start MAY succeed.

#### Scenario: Start, relayout, drop
- **WHEN** the host starts the panel, applies a valid layout, then drops the handle
- **THEN** every call returns `Ok`, the panel follows the layouts, and after the drop no
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
