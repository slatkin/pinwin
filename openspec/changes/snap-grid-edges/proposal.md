# Proposal

## Why

Since `gsk-render-nodes`, the terminal grid is drawn as GSK colour nodes in logical pixels.
A logical pixel is the unit that the compositor scales to the screen. A device pixel is one
physical pixel. At a fractional output scale such as 1.25 or 1.5, a cell edge often falls
between two device pixels. GSK blends the pixels at that edge, and a visible line appears
between cells. Block, quadrant and braille sprites show the same lines. The old cairo painter
hid them with `Antialias::None`. The `gsk-render-nodes` design named this risk and left the
check to a manual demo run.

To see the lines, run the demo with `DEMO_DENSE=1` on an output at scale 1.5. Look at a
region of cells that have one background colour.

The fix is to snap each rectangle edge to the device pixel grid. Two cells that share an edge
compute the same snapped value, so no gap and no blend appears. The cell size, the panel
width and the reported sizes stay in logical pixels.

## What Changes

- Add one snapping helper. It rounds each edge of a rectangle to a whole device pixel with
  `round(edge * scale) / scale`. Use it for every rectangle that the grid pass draws: the
  cell backgrounds including the last-row fill, the block, quadrant and braille sprites, the
  cursor shapes, and the underline and strikethrough bands.
- Snap each edge on its own, so two cells that share an edge always get the same value. Keep
  the logical thickness of bars and bands. Give every snapped rectangle at least one device
  pixel in each direction.
- Read the surface scale with `gdk::Surface::scale()`. When the scale changes
  (`notify::scale`), redraw the grid. Raise the `gtk4` and `gdk4` feature level from `v4_8`
  to `v4_12`.
- Snap the grid's docked-edge translate to a whole device pixel. This covers the width tween
  and the frames around its stop. Moving frames then do not bring the lines back.
- Give the cairo fallback painter the same snapped geometry, so the two painters stay equal
  at every scale.
- Replace the scale-1.5 blend-tolerance parity tests with checks that a region of one colour
  has one colour at fractional scales.

Nothing is **BREAKING**. The cell size, the panel width, the gutters, `CSI 16 t` and
`TIOCGWINSZ` stay as they are. At integer scales only half-pixel sprite edges change. They
now snap instead of blending.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: One new requirement says that the cell grid shows no seams at any output
  scale, fractional or integer. No existing requirement changes.

## Impact

- Code: `src/render/` (`nodes.rs`, `node_sprites.rs`, `node_cursor.rs`, `snapshot.rs`, and
  the geometry that the cairo painter shares with them).
- Code: `src/surfaces/` and `src/panel/gtk_side.rs` read the scale and pass it to the draw
  hooks. `Cargo.toml` raises the `gtk4` and `gdk4` features.
- API: the `Panel` handle and `Layout` do not change.
- Dependencies: no new crates. `Surface::scale()` needs GTK 4.12 for the API. GTK uses a
  fractional scale with the default GL renderer on Wayland only from 4.13.6 (4.14). Before
  that, it reports an integer scale unless `GDK_DEBUG=gl-fractional` is set. GTK 4.22.5 is
  installed. The README states both versions.
- Related: `device-pixel-grid` planned a larger fix that measures the cell in device
  pixels. It is parked. This change replaces it.
- Out of scope: font resolution and `gtk-xft-dpi`, device-pixel cell metrics and pixel
  reports, panel width rounding, the rectangles of kitty images, layer-shell geometry, and
  retiring the cairo painter.
