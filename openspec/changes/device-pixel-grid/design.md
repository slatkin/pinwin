# Design

## Context

See `proposal.md` for the motivation. This section holds the current state only.

Today every pixel quantity in the grid path is a whole logical pixel. `metrics::measure`
rounds the Pango metrics to integers on the widget's context. `Surfaces` stores
`cell_w` and `cell_h`, derives the panel width as `cols * cell_w`, and passes it to the
layer-shell window. `apply_size_to` derives the rows from the area's logical height. The
node emitters (`nodes.rs`, `node_sprites.rs`, `node_cursor.rs`, `node_images.rs`) draw in
those logical units, and GSK applies the device scale afterward. The tween animates an
integer logical width, and the draw offset that glues the grid to the docked edge is an
integer logical shift. The input controllers subtract that offset from the pointer
position and hand the result to the terminal. Nothing in the crate reads the output scale.

Ghostty, the reference terminal, measures the font at `scale * 96` dpi. It rounds the cell
and the baseline to whole device pixels. It divides the device-pixel surface into cells and
fills the leftover with background padding. Its GTK front end reads the fractional scale
from `GdkSurface`. This change copies that shape. It does not copy Ghostty's GL renderer.

## Goals / Non-Goals

**Goals:**

- One device-pixel lattice for the cell metrics, the draw, the terminal grid size and the
  pointer mapping, so the seams cannot return at any scale.
- One place that converts between logical and device pixels, so the two units cannot be
  mixed by accident.
- Keep the cairo fallback and the parity tests working, at more than one scale.

**Non-Goals:**

- No change to the `Panel` API, the `Layout` type, or the logical-pixel gutters.
- No `gtk-xft-dpi` text scaling. The font size follows the surface scale only.
- No change to the layer-shell protocol use. The surface still has a whole logical size.
- No retirement of the cairo painter.

## Decisions

**D1. The scale comes from `gdk::Surface::scale()`, with a notify.**
The code reads the fractional scale from the panel window's surface. It remeasures when
the surface maps. It remeasures again on every `notify::scale`. The `gtk4` and `gdk4` features move
from `v4_8` to `v4_12`.
Alternatives: `Widget::scale_factor()` is an integer, so it rounds 1.5 to 2.
`Monitor::scale()` needs 4.14 and describes the monitor, not the surface that is drawn.
Before the surface exists, `Surfaces::build` measures once at scale 1 to get a provisional
default size. The map handler replaces it.

**D2. Measure with the Pango resolution set to `96 * scale`.**
`measure_hook` creates a fresh Pango context. The change sets its resolution to
`96 * scale` with `pangocairo::functions::context_set_resolution`. Then `metrics::measure`
returns device-pixel numbers with its existing rounding. The node emitter reuses the same
context, so the glyph sizes and the cell size cannot disagree.
Alternative: scale the font description size by hand. That loses the per-size hinting
metrics and splits the rule into two places.
The first task checks that `append_layout` under a `1/scale` transform draws glyphs at the
context's resolution and not twice scaled.

**D3. Draw inside one `1/scale` transform.**
The snapshot keeps the theme background and the focus accent in logical coordinates. The
grid pass runs between `snapshot.save()`, `snapshot.scale(1/s, 1/s)` and `restore()`. Inside
it, every width, height, rectangle and offset is a device pixel. The net transform to the
output is the identity, so each rectangle covers whole pixels and no edge blends.
Alternatives: snapping each rectangle edge with `round(x * s) / s` fixes only rectangles and
leaves text, images and cell pitch on the logical grid. Switching to the cairo painter at
fractional scales gives up the GPU path where it matters.

**D4. The pixel units live behind one `OutputScale` newtype.**
`OutputScale` in `layout.rs` is a positive, finite scale. It has two conversions:
`logical_ceil(device) -> i32` and `device_round(logical) -> i32`. Every conversion in the
crate goes through them. `CellSize` is documented as device pixels. `Layout::validate`
takes the scale, because the output size and the gutters are logical and the cell is not.
The rule that cell sizes are device pixels is then a type-level fact at the boundary, not a
comment.

**D5. The panel width is `ceil(cols * cell_w / scale)` logical pixels.**
Layer-shell needs a whole logical width. The code rounds up so the grid always fits. The
sliver, under one logical pixel, is covered by the theme background that already fills the
widget. The draw offset puts the grid against the docked edge, so the sliver ends up on the
far side. At scale 1 the width is `cols * cell_w`, as before. This is why the specs relax
"Exact final width".
Alternative: round the width down and clip the last column. That loses terminal content.

**D6. Rows come from the device height.**
`apply_size_to` computes the device height as `device_round(area.height())`. It divides by
`cell_h`. `push_size` and the pty `winsize` receive device-pixel cells. The existing
last-row fill, which already extends the final row to the grid height, now runs in device
pixels.

**D7. The tween offset is a whole device pixel.**
The tween still animates an integer logical width, because the surface size is logical. The
docked-edge shift is `device_round(offset_logical)`. One function returns it, and the
translate in `snapshot_grid` and the input mapping both call it. Ghostty shifts its origin
to the nearest device pixel for the same reason.

**D8. Pointer coordinates convert to device pixels.**
The input controllers multiply the logical position by the scale and subtract the device
offset from D7. The terminal then sees positions in the same units as its cell size. The
spec scenario "pointer coordinates SHALL map to the drawn cells" keeps holding.

**D9. The cairo painter wraps its grid pass in `cr.scale(1/s, 1/s)`.**
It keeps `Antialias::None`. It shares the geometry helpers with the node painter. The two
painters then stay identical by construction. The parity tests run at scales 1, 1.5 and 2.

## Risks / Trade-offs

- [Pango context resolution does not carry through `append_layout`] → Task 1.1 is a spike
  that renders one glyph both ways before any other work. If it fails, fall back to scaling
  the font description (D2 alternative) and update this design.
- [The first draw happens at the wrong scale, because `notify::scale` can arrive after the
  map] → The remeasure on notify redraws and resizes. The cost is one extra SIGWINCH at
  start. A task checks the startup order on a fractional output.
- [A scale change resizes the grid, so the child sees SIGWINCH] → This matches other
  terminals. It is a behavior change for panels that move between outputs, and the spec
  records it.
- [The panel size differs from before at non-1x scales] → This is intended. The proposal
  marks it as a visual break. The README caveats need one line.
- [GTK 4.12 minimum] → The installed GTK is 4.22.5. The README states the new minimum.
- [Large device sizes raise the node count and memory] → The node count is the same as
  before, because it depends on cells and not pixels. Measure once at 120x40 and scale 2.

## Migration Plan

No data migration. The change ships in one release. To roll back, revert the commit. The
`Panel` API is untouched, so hosts need no change.

## Open Questions

- Does the edge column's background need to extend through the sliver, so a full-width
  colour bar has no visible end? The default here is no. Revisit it if the strip is
  noticeable.
