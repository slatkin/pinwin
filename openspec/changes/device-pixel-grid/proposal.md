# Proposal

## Why

Since `gsk-render-nodes`, the terminal grid is emitted as GSK colour nodes in logical
pixels. Each cell is a whole number of logical pixels, for example 9x20. At a fractional
output scale such as 1.25 or 1.5, the cell edges fall between device pixels. GSK blends
the edge pixels. The grid lines and the seams between blocks that the old cairo path
removed with `Antialias::None` come back. Block, quadrant and braille sprites show the
same seams.

The `gsk-render-nodes` design named this risk. Its real-renderer check (row 5.1) was
waived, so nothing caught it.

The standard fix is the one Ghostty and the other GPU terminals use. The font and the
cell are measured in device pixels, and the grid is drawn on that whole-pixel lattice.
Adjacent cells then share exact edges at every scale.

## What Changes

- Measure the font at the font size times the surface scale. Round the cell width,
  height, baseline and decoration metrics to whole **device** pixels. The scale comes
  from the surface (`gdk::Surface::scale()`). A scale change remeasures the cells.
- Draw the grid in device-pixel coordinates. The snapshot applies one `1/scale`
  transform, so the net transform to the output is the identity. Every rectangle lands
  on whole pixels. The cairo fallback painter uses the same device-pixel geometry.
- Report the cell size and the pty pixel size in device pixels (`CSI 16 t` and
  `TIOCGWINSZ`). A scale change at runtime is a real grid resize. The rows change and
  the child gets SIGWINCH.
- Derive the panel's logical width from the grid's device width. Convert it to logical
  pixels and round up. Fill the sliver beyond the grid with the theme background. The
  sliver sits on the side away from the docked edge. Rows still come from the device
  height, and the last row still fills to the edge.
- Round the tween's docked-edge translate to a whole device pixel. Moving frames then
  do not bring the seams back.
- Raise the `gtk4` and `gdk4` feature level and the minimum GTK version to what
  `Surface::scale()` needs.
- **BREAKING (visual, not API):** the panel's pixel width and the reported cell size
  differ from before at scales other than 1x. The cell is now rounded once, at device
  resolution. At fractional scales, the panel can be wider than columns times the cell
  width by less than one logical pixel.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: A new requirement says the cell grid is measured and drawn in device
  pixels with no visible seams at any scale. "Correct size reports" reports device-pixel
  sizes. "Directional gutters and docking geometry" derives the panel width from the
  device-pixel grid, with rounding. "Exact final width" allows the sub-pixel rounding.

## Impact

- Code: `src/render/` (`metrics.rs`, `mod.rs`, `nodes.rs`, `node_sprites.rs`,
  `node_cursor.rs`, `node_images.rs`, `text.rs`, `snapshot.rs` and the cairo path).
- Code: `src/surfaces/mod.rs` sets the width and remeasures on a scale change.
  `src/panel/gtk_side.rs` holds the measure and size hooks.
- Code: `src/layout.rs` derives the width with the scale. `src/anim.rs` rounds the
  offset. `Cargo.toml` raises the features.
- API: the `Panel` handle and `Layout` do not change. The host still passes columns and
  logical-pixel gutters.
- Dependencies: GTK 4.12 or newer at run time. GTK 4.22.5 is installed. No new crates.
- Out of scope: `gtk-xft-dpi` text scaling, layer-shell geometry beyond the width
  rounding, and retiring the cairo painter.
