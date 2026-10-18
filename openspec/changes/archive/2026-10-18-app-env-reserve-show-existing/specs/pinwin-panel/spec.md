# Spec Delta

## MODIFIED Requirements

### Requirement: Animated width transition
An animated change, with a duration above 0 ms, that changes only the panel's width and
reserve mode (same edge and gutters) SHALL change the visible width continuously over the
duration, driven by the compositor's frame callbacks. The strip SHALL move with the panel only
when the panel pushes before the change, the target pushes, and the target's strip differs
from the held one. A change from covering to pushing SHALL hold the strip still during the
animation and SHALL set the target's strip when the animation ends. The duration SHALL be clamped
to 1000 ms. The app's content SHALL stay against the panel's edge in every frame. pinwin SHALL
NOT read any desktop animation setting.

#### Scenario: Animate on request
- **WHEN** a pushing panel gets `set-size` from 400 to 600 pixels with `ms` 200
- **THEN** the panel and the strip grow through intermediate widths over about 200 ms and tiled
  windows reflow alongside

#### Scenario: Covering expand holds tiles still
- **WHEN** a panel changes to covering at a larger width with `ms` 200
- **THEN** the panel grows over about 200 ms while the strip holds still and tiled windows do
  not move

#### Scenario: One new size for the app
- **WHEN** an animated change runs from 400 to 600 pixels
- **THEN** the app gets exactly one new window size, 600 pixels wide, for the whole animation

#### Scenario: Grow waits for the app
- **WHEN** an animated grow starts
- **THEN** the panel stays at its current width until the app has drawn at the target width,
  and snaps to the target if the app has not drawn within 500 ms

#### Scenario: Content does not slide
- **WHEN** a right panel shrinks with an animation
- **THEN** the app's content stays against the right edge in every frame

#### Scenario: Snap on zero duration
- **WHEN** a change has `ms` 0
- **THEN** it applies in one step

#### Scenario: Snap when animations are disabled
- **WHEN** the panel's `tween_ms` is 0 and a client sends `set-size` without `ms`
- **THEN** the change applies in one step

#### Scenario: Desktop animation setting has no effect
- **WHEN** the GTK `gtk-enable-animations` setting is false and a change has `ms` 200
- **THEN** the panel width animates over about 200 ms

#### Scenario: No viewport support
- **WHEN** the compositor does not offer the viewporter protocol and a change has `ms` 200
- **THEN** it applies in one step

#### Scenario: Other fields snap
- **WHEN** a change moves the edge or changes a gutter
- **THEN** it applies in one step with no animation

#### Scenario: Duration clamp
- **WHEN** a change has `ms` 60000
- **THEN** the animation lasts at most 1000 ms


#### Scenario: Collapse back to the held strip
- **WHEN** the held strip is 400 pixels, the panel covers at 1100 pixels, and it gets
  `set-size` to 400 pixels with `reserve` `push` and `ms` 200
- **THEN** the panel shrinks to 400 pixels over about 200 ms, the strip stays at 400 pixels,
  and tiled windows do not move

#### Scenario: Collapse to a new pushing width
- **WHEN** the held strip is 400 pixels, the panel covers at 1100 pixels, and it gets
  `set-size` to 500 pixels with `reserve` `push` and `ms` 200
- **THEN** the panel shrinks to 500 pixels over about 200 ms while the strip holds still at
  400 pixels, and when the animation ends the strip becomes 500 pixels and tiled windows move
  once

### Requirement: Apply layout without restarting the app
A successful change of size, edge, gutters, reserve mode, layer, output or keyboard mode SHALL
keep the same app process and the same window. The app SHALL get a new window size only when
the panel's width or length changes.

#### Scenario: Change the edge
- **WHEN** a left panel moves to the right edge with the same size
- **THEN** the same window shows at the right edge, and the app gets no new size

#### Scenario: Change the size
- **WHEN** a panel's size changes from 640 to 800 pixels
- **THEN** the app gets one new window size of 800 pixels wide and keeps running

#### Scenario: Change the gutters
- **WHEN** a left panel with Top 0 gets `set-gutters` with Top 8 and Bottom 8
- **THEN** the same window shows with its top edge 8 pixels below the output's top edge and
  its bottom edge 8 pixels above the output's bottom edge, and the app gets one new window
  height
