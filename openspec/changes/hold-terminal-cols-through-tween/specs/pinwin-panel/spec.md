# Spec Delta

## MODIFIED Requirements

### Requirement: Terminal grid during a width animation
During an animated width transition the terminal grid and PTY winsize SHALL stay at the
previously applied column count for the whole tween, and no winsize change or SIGWINCH SHALL
reach the host's child until the tween ends. When the tween ends, the grid and PTY winsize
SHALL be resized to the final column count exactly once, through the same code path as a
non-animated apply. While animating, the grid SHALL be drawn anchored to the docked edge —
during an expand the surface is wider than the grid and the inner edge shows the theme
background, during a collapse the grid's inner columns are clipped — and pointer coordinates
SHALL map to the drawn cells.

#### Scenario: One resize
- **WHEN** an animated transition from 40 to 120 columns runs
- **THEN** the host's child observes no winsize change and no SIGWINCH during the tween, and
  exactly one winsize change to 120 columns with one SIGWINCH when the tween ends

#### Scenario: Content does not slide
- **WHEN** the panel is docked right and shrinks
- **THEN** the drawn cells stay against the right edge

### Requirement: Interrupting an animation
A layout applied while a width animation is running SHALL take effect with last-write-wins
semantics. An animated apply SHALL retarget from the current animated width, and the single
trailing resize at the end SHALL use the newest target. Every path that ends or abandons a
tween SHALL deliver the final column count to the host's child exactly once: natural
completion, the stalled-frame watchdog, teardown via `pinwin_stop`, a non-animated apply or
`duration_ms` 0 while a tween is running, and a failed apply's snap-back. A non-animated
apply SHALL cancel the animation and snap to the requested layout. `pinwin_stop` SHALL end
any running animation before closing the surfaces.

#### Scenario: Retarget
- **WHEN** a collapse animation is half done and the host requests an expand
- **THEN** the width continues from its current value toward the expanded width, and the
  child observes a single winsize change to the expanded column count when the tween ends

#### Scenario: Snap interrupts
- **WHEN** the host calls `pinwin_apply_layout` during an animation
- **THEN** the panel snaps to that layout and the child observes exactly one winsize change
  to that layout's column count

#### Scenario: Stop mid-animation
- **WHEN** the host calls `pinwin_stop` during an animation
- **THEN** the child observes one winsize change to the tween's target column count before
  the surfaces close, and no animation callback runs afterwards

### Requirement: No mid-state on failure
A rejected or failed animated apply SHALL NOT leave the panel at an intermediate width. A
layout the panel refuses SHALL leave the applied layout unchanged. If the terminal grid
cannot be allocated when an animation starts, the panel SHALL be snapped to the requested
layout and the call SHALL return `PINWIN_ERR_INTERNAL`.

#### Scenario: Invalid layout
- **WHEN** the host calls `pinwin_apply_layout_animated` with a layout the panel refuses
- **THEN** it returns `PINWIN_ERR_INVALID` and the width and layout are unchanged

#### Scenario: Terminal allocation failure
- **WHEN** the terminal grid cannot be allocated at animation start
- **THEN** the call returns `PINWIN_ERR_INTERNAL`, the panel is at the requested final width,
  not an intermediate one, and the child's terminal is left at one resize attempt toward that
  width
