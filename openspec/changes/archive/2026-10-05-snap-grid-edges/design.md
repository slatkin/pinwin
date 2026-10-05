# Design

## Context

See `proposal.md` for the motivation. This section holds the current state only.

The grid geometry already lives in shared functions that both painters call:
`cell_background_rect` (`src/render/nodes.rs`), `sprite_shape` (`src/render/sprites.rs`), and
`cursor_shape`, `underline_rect` and `strikethrough_rect` (`src/render/node_cursor.rs`). Each
takes a `CellMetrics` and returns rectangles or triangle points in logical pixels.

The node emitter turns those rectangles into colour nodes. GSK applies the output scale and
blends any edge that falls between device pixels. The cairo painter fills the same rectangles
with `Antialias::None`, which snaps them (`paint_backgrounds`, `fill_rect`). It strokes the
underline, the strikethrough band and the hollow cursor as centre lines of width 1.

Nothing in the crate reads the output scale. The tween shifts the grid to the docked edge by an
integer logical offset (`Anim::draw_offset`). The hooks cast the result of
`Surfaces::draw_offset()` to `i32` (`src/panel/gtk_side.rs`, in `draw_hook` and
`grid_snapshot_hook`). The input controllers subtract the same value from the pointer
position (`src/input/mod.rs`).

## Goals / Non-Goals

**Goals:**

- One snapping rule that every grid rectangle goes through, so the two painters cannot drift.
- Exact pixel parity between the two painters for rectangles at every scale.

**Non-Goals:**

- No change to the cell metrics, the Pango resolution, the panel width, the gutters or the
  reported sizes. The device-pixel version of this fix is parked in `device-pixel-grid`.
- No snapping of the theme background, the focus accent, text glyph origins or kitty images.
  None of them forms a line between two cells.

## Decisions

**D1. Snap in the shared geometry functions, in logical coordinates.**

Each function snaps its own output. Both painters then receive snapped rectangles and need no
snapping code of their own.

Alternatives: A device-pixel lattice (the parked plan) fixes text and images as well. It also
changes the metrics, the winsize reports, the panel width and the GTK scale handling. Snapping
inside each painter puts the rule in two places that can drift. Switching to the cairo painter
at fractional scales gives up the GPU path where it matters.

**D2. `OutputScale` is a newtype in a new `src/render/snap.rs`, carried in `CellMetrics`.**

`OutputScale` is positive and finite, and its default is 1. It has one method that snaps an
edge and one that snaps a rectangle. The module has no GTK dependency, so tests run without a
display.

The geometry functions already take `&CellMetrics`. The scale rides in a new `scale` field
there, not in a new argument on every function. `DrawState::set_scale` sets the field.
`cell_metrics_update` keeps it, because that call replaces the rest of the metrics.

Alternative: A separate argument on every geometry function. That touches about ten more
signatures and every test that calls them.

**D3. The scale comes from `gdk::Surface::scale()`, read at each draw.**

`Surfaces::scale()` returns `win.surface().scale()` as an `OutputScale`. A missing surface gives
1. A value that is not positive and finite also gives 1. Both draw hooks call it before they
draw. The features on `gtk4` and `gdk4` move from `v4_8` to `v4_12`.

The `notify::scale` handler, connected in `on_map`, only queues a redraw. It does not store the
scale. A stored copy can go stale between the notify and the draw.

Alternatives: `Widget::scale_factor()` is an integer, so it rounds 1.5 to 2. `Monitor::scale()`
needs 4.14 and describes the monitor, not the surface that is drawn.

**D4. Each edge snaps on its own with `round(edge * scale) / scale`.**

`f64::round` rounds ties away from zero. For the non-negative edges here, that means up. The
right edge of a rectangle is `snap(x + w)`, never `snap(x) + snap(w)`.

Two cells that share an edge pass the same number to the snap. That number is an exact integer
or an exact half, for example `ch / 2.0`. The two cells get the same result, so no gap and no
overlap appears.

A rectangle with a positive size keeps at least one device pixel. If both snapped edges land on
the same pixel, the code pushes the far edge out by one device pixel. A rectangle with no size
stays empty.

The thickness of a bar, band or outline is then within one device pixel of its logical
thickness times the scale. When that product is a whole number, the thickness is exact.

**D5. Triangle vertices snap with the same rule.**

`sprite_shape` snaps the x and y of each vertex of the corner and powerline triangles. The
straight sides of a corner triangle lie on the cell boundary, and an unsnapped side blends with
the neighbour. Diagonal sides blend at any scale. The change moves them by at most half a
device pixel. The 2 px stroke width of the hollow powerline triangles stays as it is.

**D6. The cairo painter fills the same rectangles instead of stroking centre lines.**

`cursor_shape` returns four band rectangles for the hollow cursor. The node emitter already
derives them from the stroke bounds (`emit_cursor`), so that derivation moves into the shared
function. The cairo painter fills the bands, the underline and the strikethrough with
`Antialias::None` instead of stroking them. A stroke of width 1 does not match a snapped band
that is 1 or 2 device pixels thick.

After this, both painters draw identical rectangles for backgrounds, sprites, cursor and
bands. Text keeps its blend tolerance.

**D7. The docked-edge shift snaps once, in `Surfaces::draw_offset()`.**

`Anim::draw_offset` stays an integer in logical pixels, and its tests stay unchanged.
`Surfaces::draw_offset()` returns the snapped value as `f64`. The draw hooks, `DrawState::draw`,
`DrawState::snapshot_grid` and the input controllers all use that one value. The drawn cell and
the cell under the pointer then agree. `draw` and `snapshot_grid` take `f64` where they took
`i32`.

The retained tween node holds snapped geometry for the scale it was built at. When the scale
changes, `set_scale` drops it.

The grid pass starts at the widget origin. The origin must be on the device pixel lattice for a
snapped offset to be on it. The panel window has no decoration and the drawing area fills it,
so the origin is on the lattice. Task 5.2 checks this with a screenshot.

No automated test observes the snapped offset. The seam test passes an offset that it snaps
itself, and task 5.2 observes the real one during a width animation.

## Risks / Trade-offs

- [Cell pitch is uneven in device pixels] At 1.5, a cell 9 logical pixels wide is 13.5 device
  pixels, so snapped cells alternate between 13 and 14. Text keeps the logical pitch. A glyph
  can sit up to half a device pixel from its background. → Accepted. No seam appears. The
  parked `device-pixel-grid` removes it at a much higher cost.
- [Compositor resampling] GDK sizes the buffer as `ceil(scale * width)` (`gdk/gdksurface.c`).
  If `width * scale` is not a whole number, the compositor stretches the buffer by under one
  percent. → No app-side fix. A uniform region stays uniform under a stretch, so the spec
  scenarios still hold. Edges between different colours can blur slightly.
- [GTK before 4.14] Until 4.13.6, the default GL renderer reports an integer scale on Wayland.
  The compositor then rescales the whole frame. → No app-side fix. The README states the 4.14
  requirement. GTK 4.22.5 is installed.
- [Float error] GSK multiplies the snapped logical edge by the scale in `f32`. An edge can land
  about 1e-6 device pixels from the lattice. → The seam test checks pixels, and the final
  acceptance checks the real renderers.
- [Changed output at integer scales] A sprite edge on a half pixel now snaps. It used to
  blend. → Intended. The change is at most half a device pixel, and the parity tests pin it.
- [Thickness varies with position] At 1.25, a 2 px bar is 2.5 device pixels. It is 2 or 3
  pixels wide, depending on its column. → Accepted. The spec states the one-pixel bound.

## Migration Plan

No data migration. To roll back, revert the commit. The `Panel` API is untouched, so hosts need
no change. The README gains the GTK version note.
