# Design

## Context

`Surfaces::new` builds a `gtk4::DrawingArea` with `set_draw_func`; during a tween
`Renderer::draw_inner` blits `grid_cache` (a Cairo `ImageSurface`, device-scaled) at the
dock offset. Input controllers attach to the `DrawingArea` (`src/input/mod.rs`) and read
`draw_offset`. See proposal.md for motivation.

## Goals / Non-Goals

**Goals:**
- Per-frame tween cost on the CPU is a snapshot transform, no raster or upload.
- Everything else (input, resize, non-tween draw) behaves identically.

**Non-Goals:**
- Live grid during the tween, GSK text nodes, image nodes, geometry-call reduction.

## Decisions

- **Subclass `DrawingArea`, override `snapshot`.** Keeps `set_draw_func`, controllers and
  the `connect_resize` hook working untouched. Alternative, a fresh `Widget` subclass,
  forces rewriting input attachment and the draw path for no PoC benefit.
- **Texture from the existing `ImageSurface`.** Wrap its pixels as a
  `gdk::MemoryTexture` (`B8g8r8a8Premultiplied`) once when `grid_cache_ensure` builds the
  cache; store it beside the surface and drop it in `drop_grid_cache`. Bounds are the
  logical size (device pixels / device scale) so fractional scales stay crisp.
- **Snapshot branch:** when animating with a cache, append the background colour, push a
  translate by the offset, `append_texture`, pop, then draw the focus accent (via
  `append_cairo`). Otherwise chain to the parent `snapshot`, which runs the draw func.
- **Fallback:** nothing renderer-specific; `GSK_RENDERER=cairo` renders the same nodes.

## Risks / Trade-offs

- Subclass state needs the animating flag, offset and texture reachable from `snapshot`
  -> share them through the existing `Rc` surfaces/renderer handles, no new globals.
- The poisoned-guard rule (panics never escape) must also wrap the snapshot path.
- Per-frame `glue_apply_geometry`-style layer-shell calls remain; if the laptop still
  stutters after this, they are the next suspect.
