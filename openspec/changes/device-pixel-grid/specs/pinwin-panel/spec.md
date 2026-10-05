# Spec Delta

## ADDED Requirements

### Requirement: Device-pixel cell grid
The panel SHALL measure the terminal cell and draw the cell grid in device pixels. The
font SHALL be sized at the configured size times the surface's output scale. The cell
width, the cell height and the baseline SHALL be whole device pixels. Every cell
background and every block glyph SHALL be drawn on whole device pixels. Block glyphs are
the full, half, eighth, quadrant and braille blocks. Adjacent cells SHALL share exact
edges. No line or seam SHALL be visible between cells at any output scale, fractional or
integer. When the output scale changes, the panel SHALL remeasure the cell, resize the
grid and the PTY, and redraw. The fallback painter SHALL meet the same rules.

#### Scenario: Fractional scale has no seams
- **WHEN** the panel runs at an output scale of 1.5 and a region of cells has one
  background colour or holds full-block glyphs
- **THEN** the region shows one uniform colour with no lines between cells

#### Scenario: Integer scale has no seams
- **WHEN** the panel runs at an output scale of 2 and shows the same region
- **THEN** the region shows one uniform colour with no lines between cells

#### Scenario: Whole device-pixel cell
- **WHEN** the cell is measured at any output scale
- **THEN** its width and its height are each a whole number of device pixels

#### Scenario: Font size follows the scale
- **WHEN** the configured font size is the same at output scales 1 and 1.5
- **THEN** glyphs at scale 1.5 span 1.5 times as many device pixels as at scale 1

#### Scenario: Scale change
- **WHEN** the panel moves to an output with a different scale
- **THEN** the cell is remeasured, the grid and the PTY are resized once, and the host's
  child observes one SIGWINCH

#### Scenario: Seams during a width animation
- **WHEN** a width animation runs at an output scale of 1.5
- **THEN** no seam appears between cells in any frame

#### Scenario: Fallback painter
- **WHEN** the fallback painter draws the grid at an output scale of 1.5
- **THEN** the cell size matches the primary painter and no seam appears

## MODIFIED Requirements

### Requirement: Correct size reports
The panel's terminal SHALL answer terminal queries on the PTY: primary device attributes
(`CSI c`) and `CSI 16 t` (cell size in pixels). The PTY window size SHALL carry both the
column/row count and the pixel width/height, and SHALL be updated whenever the panel's size
changes. All pixel sizes reported SHALL be device pixels. They SHALL match the cell size
actually drawn.

#### Scenario: Cell size query
- **WHEN** the host's child writes `CSI 16 t`
- **THEN** it receives `CSI 6 ; <cell height px> ; <cell width px> t` matching the drawn
  cell size in device pixels

#### Scenario: Cell size at a fractional scale
- **WHEN** the panel runs at an output scale of 1.5 and the child writes `CSI 16 t`
- **THEN** the reported cell size is the whole device-pixel cell, not a logical size

#### Scenario: Images are not clipped
- **WHEN** the host's child shows a poster image in the panel
- **THEN** the whole image is visible, with no part cut off at the right or bottom edge

#### Scenario: Pixel size in window size
- **WHEN** the host's child reads the terminal size with `TIOCGWINSZ`
- **THEN** `ws_col`/`ws_row` are the cell grid and `ws_xpixel`/`ws_ypixel` are the grid size
  in device pixels

### Requirement: Directional gutters and docking geometry
Folded in from the retired options capability (design D1). Gutters SHALL use literal screen
directions in logical pixels, independent of docking side. Let the grid width be `cols`
times the cell width in device pixels. Let the panel width be the grid width converted to
logical pixels and rounded up to a whole logical pixel. At output scale 1 the panel width
equals `cols` times the cell width. Any part of the panel beyond the grid SHALL be filled
with the theme background, on the side away from the docked edge.

On the left, the panel SHALL be inset from the left output edge by Left pixels. It SHALL
reserve `Left + panel width + Right` pixels at the left edge. On the right, the panel SHALL
be inset from the right output edge by Right pixels. It SHALL reserve the same sum at the
right edge. Top and Bottom SHALL inset the visible panel from the corresponding output
edges. The reservation SHALL cover a full-height strip, also when the panel has vertical
insets.

Gutters can be negative. A negative gutter moves the panel's edge beyond its output edge.
It can also end the reservation before the panel's far edge. In both cases tiles can overlap
the panel. The reservation sum SHALL NOT be negative. Existing compositor struts remain
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
  its bottom
- **AND** the horizontal reservation remains a full-height strip

#### Scenario: Negative gutter
- **WHEN** panel width is 320 pixels, Left is -40, Right is 12 and docking is Left
- **THEN** the panel starts 40 pixels beyond the left output edge and the reservation at that
  edge is 292 pixels

#### Scenario: Fractional scale rounds the width up
- **WHEN** the output scale is 1.5, the grid is 25 columns of 13 device pixels, Left is 8,
  Right is 12 and docking is Left
- **THEN** the grid is 325 device pixels, the panel is 217 logical pixels wide, and the
  reservation is 237 pixels

#### Scenario: The leftover is background
- **WHEN** the panel is wider than the grid by a sub-pixel amount
- **THEN** the extra strip shows the theme background on the side away from the docked edge

### Requirement: Exact final width
When an animated width transition ends, the panel's width SHALL equal the target grid
width. The grid width is the target columns times the cell width in device pixels. It is
converted to logical pixels and rounded up to a whole logical pixel. The reservation SHALL
equal left plus that width plus right. No residual offset from intermediate frames SHALL
remain. The final frame SHALL be produced by the same code path as a non-animated apply.

#### Scenario: Animation completes
- **WHEN** an animated transition to 120 columns finishes
- **THEN** the panel width is exactly the 120-column grid width, rounded up to a logical
  pixel, and the reservation matches

#### Scenario: Stalled frame clock
- **WHEN** the frame clock does not deliver frames for the duration plus 100 ms
- **THEN** the panel is snapped to the exact target width
