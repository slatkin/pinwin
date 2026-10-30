# Spec Delta

## MODIFIED Requirements

### Requirement: Reserve space so tiles start beside the panel
The panel SHALL hold one reservation: a strip at one edge of its output. A pushing panel sets
the strip to its edge with the size `near + width + far`, so the compositor places tiled
windows beside it. A covering panel leaves the strip as the last pushing setting made it, so
tiled windows do not move while the panel draws over them. A panel in `none` mode draws over
tiled windows like a covering panel and sets the strip to zero, releasing any strip an earlier
push held. A panel that has never pushed
SHALL reserve nothing. While the panel is hidden or holds no window, the strip SHALL be
released; showing a window SHALL restore it. The strip SHALL apply only to the panel's output.
The strip SHALL be a separate layer-shell surface on the `background` layer, whatever layer
the panel window draws on, so a compositor that applies exclusive zones layer by layer applies
the strip's zone after the zones of every other layer. A `set-layer` request SHALL move the
panel window only and SHALL NOT move the strip.

#### Scenario: Tiles move right
- **WHEN** a panel docks left with width 320, Left 0 and Right 12
- **THEN** tiled windows' left edge moves to at least 332 pixels from the output's left edge

#### Scenario: Other monitors unaffected
- **WHEN** the panel shows on DP-2
- **THEN** tiled windows on DP-1 keep their position

#### Scenario: Covering start reserves nothing
- **WHEN** a panel starts with `--reserve cover`
- **THEN** tiled windows stay where they are until the panel first pushes

#### Scenario: Covering expand holds the strip
- **WHEN** the held strip is 332 pixels on the left and the panel changes to covering at a
  larger width on the same edge
- **THEN** tiled windows keep their position and the panel extends past the strip

#### Scenario: None start reserves nothing
- **WHEN** a panel starts with `--reserve none`
- **THEN** tiled windows stay where they are and the panel draws over them

#### Scenario: None releases the held strip
- **WHEN** the held strip is 332 pixels on the left and the shown panel changes to `none`
- **THEN** the strip becomes zero, tiled windows take the space, and the panel keeps drawing
  over them

#### Scenario: None stays released
- **WHEN** a `none` panel changes its size
- **THEN** the strip stays zero and tiled windows do not move

#### Scenario: Pushing again after none
- **WHEN** a `none` panel changes to `push` at 332 pixels
- **THEN** the strip becomes 332 pixels and tiled windows move beside it

#### Scenario: Hidden panel reserves nothing
- **WHEN** the held strip is 332 pixels on the left and the panel hides
- **THEN** tiled windows take the strip, and they move back beside it when the panel shows

#### Scenario: A reserved top bar keeps its place
- **WHEN** a bar on the `top` layer reserves 30 pixels at the top edge and a left panel on the
  `overlay` layer pushes a 400 pixel strip
- **THEN** the bar keeps its position and width, and tiled windows start at least 400 pixels
  from the left edge and 30 pixels from the top edge

#### Scenario: Changing the layer leaves the strip in place
- **WHEN** a shown panel that pushes a 400 pixel strip gets `set-layer` with `bottom`
- **THEN** the panel window shows on the bottom layer, the bar keeps its position, and tiled
  windows stay beside the 400 pixel strip

### Requirement: Animated width transition
An animated change, with a duration above 0 ms, that changes only the panel's width and
reserve mode (same edge and gutters) SHALL change the visible width continuously over the
duration, driven by the compositor's frame callbacks. The strip SHALL move with the panel only
when the panel pushes before the change, the target pushes, and the target's strip differs
from the held one. A change from covering to pushing SHALL hold the strip still during the
animation and SHALL set the target's strip when the animation ends. A change to `none` SHALL
hold the strip still during the animation and SHALL set it to zero when the animation ends.
The duration SHALL be clamped
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

#### Scenario: Push to none
- **WHEN** the held strip is 400 pixels, the panel pushes at 400 pixels, and it gets
  `set-size` to 300 pixels with `reserve` `none` and `ms` 200
- **THEN** the panel shrinks to 300 pixels over about 200 ms while the strip holds still at
  400 pixels, and when the animation ends the strip becomes zero and tiled windows move once

### Requirement: Exact final width
When an animation ends, the panel's width SHALL equal the target width. A pushing target SHALL
leave the reservation at `near + width + far`; a covering target SHALL leave it at the held
strip; a `none` target SHALL leave it at zero, with no offset left from intermediate frames.
The final state SHALL be the same as a change with `ms` 0 to the same target.

#### Scenario: Animation completes
- **WHEN** an animated pushing change to 600 pixels ends
- **THEN** the panel is exactly 600 pixels wide and the reservation matches

#### Scenario: Covering animation completes
- **WHEN** an animated covering change to 1000 pixels ends over a 400 pixel gap
- **THEN** the panel is exactly 1000 pixels wide and the reservation still equals the 400 pixel
  strip

#### Scenario: None animation completes
- **WHEN** an animated change to 1000 pixels with `reserve` `none` ends over a 400 pixel gap
- **THEN** the panel is exactly 1000 pixels wide and the reservation is zero

#### Scenario: Stalled frame clock
- **WHEN** the compositor sends no frame callbacks for the duration plus 100 ms
- **THEN** the panel snaps to the exact target width
