# Spec Delta — pinwin-panel

## MODIFIED Requirements

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
