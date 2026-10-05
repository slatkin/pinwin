# Spec Delta

## MODIFIED Requirements

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
