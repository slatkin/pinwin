# Spec Delta

## MODIFIED Requirements

### Requirement: Reserve space so tiles start beside the panel
The panel SHALL hold one reservation: a strip at one edge of its output. A pushing panel sets
the strip to its edge with the size `near + width + far`, so the compositor places tiled
windows beside it. A covering panel leaves the strip as the last pushing setting made it, so
tiled windows do not move while the panel draws over them. A panel that has never pushed
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
