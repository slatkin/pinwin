# Spec Delta

## ADDED Requirements

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

## MODIFIED Requirements

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
