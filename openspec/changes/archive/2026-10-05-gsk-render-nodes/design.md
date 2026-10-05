# Design

## Context

Stage 1 (`poc-gsk-texture-grid`, PR #6) added `GridArea`, a `DrawingArea` subclass whose
`snapshot` emits bg + a cached `gdk::MemoryTexture` + the accent during a tween and otherwise
chains to the parent, which runs the Cairo draw func. `DrawState::render_grid` paints through a
`cairo::Context`: cell backgrounds (`fill`), per-cell sprite or Pango text (`sprites::draw_sprite`,
`text::draw_text`), underline/strikethrough strokes, the cursor (`draw_cursor`) and kitty images
(`ImageCache::draw`). Tests draw into `ImageSurface`s with no display.

## Goals / Non-Goals

**Goals:**
- Every grid element reaches the screen as a GSK node; per-frame CPU cost during a tween is a
  transform, and idle frames cost nothing.
- Pixel parity with the Cairo painter, proven display-free.
- Cairo painter retained as the fallback and as the test oracle.

**Non-Goals:** layer-shell geometry calls, per-row/damage node caching, new terminal features,
spec changes.

## Decisions

- **A second emitter beside `render_grid`, not a rewrite.** `render_grid` stays (fallback and
  oracle). A new emitter walks the same cell iteration into a `gtk4::Snapshot`. Alternative,
  one painter trait over Cairo and Snapshot, duplicates every call shape for no benefit while
  the two backends differ this much; revisit only if the two drift.
- **Cache a `gsk::RenderNode`, not a texture.** Build the grid into a `Snapshot`, call
  `to_node()` once on the tween's first frame, and append that node (translated) per tween frame;
  every non-tween draw rebuilds it. Rejected: rebuilding only on terminal content change, which
  needs a change counter and a full invalidation key (cols, height, metrics, fonts, theme, focus,
  images) the terminal does not provide (`Terminal` doc: every draw is a complete frame).
  Glyphs live in GSK's GPU atlas; the CPU never rasterises or uploads. This replaces
  `grid_cache` + `grid_cache_texture` from stage 1. Open cost to measure: Pango layout work per
  cell on each content rebuild, which Cairo also paid.
- **Parity oracle.** `gsk::RenderNode::draw(&cairo::Context)` renders a node without a display.
  Tests render the Cairo painter and the node emitter to two `ImageSurface`s and compare
  pixels (tolerance per element, e.g. text). Early task verifies this API works headless;
  if not, parity is checked under the ignored GTK-session tests.
- **Sprites on `v4_8`.** Blocks, quadrants and braille dots are integer rects, so colour nodes.
  Powerline triangles need a path; without raising the feature level they use
  `snapshot.append_cairo` per cell (still a retained node, drawn on the CPU once). Raising
  `gtk4` to `v4_14` for `gsk::Path` is an alternative; recommend against for stage 2 unless
  the cairo-node triangles show in profiles.
- **Nerd-font constrained glyphs** keep the same scale/translate maths as `text::draw_text`,
  expressed as `snapshot.save/translate/scale/append_layout/restore`.
- **Kitty images** become `gdk::MemoryTexture`s held in `ImageCache` beside (then instead of)
  the cairo surfaces, same keys and eviction, appended with a clip to the placement rect.
- **Fallback.** The Cairo `render_grid` path stays reachable when node emission is not possible
  (a hook panic or `false` return; a poisoned latch draws nothing on either path); `GSK_RENDERER=cairo` simply renders the nodes.

## Risks / Trade-offs

- Pixel parity across Pango text through two backends can differ by antialiasing; compare
  with a tolerance, and treat visible regressions on the three renderers as the real gate.
- Colour nodes are not forced to Cairo's `Antialias::None` device-pixel snapping, so cell edges
  may blend at fractional scales; parity there is a tolerance and seams are checked on the
  renderers (task 5.1). Scale-1 sprite parity is exact only with cell metrics pinned to 8x16 in
  the tests; at the real fractional cell pitch (the suite's 9x20, and scale 1.5) the stated blend
  tolerance applies (measured max delta 128 at scale 1, up to ~191 at 1.5), so real-pitch
  fidelity is guarded by the manual row 5.1 demo run.
- Fractional scales: node geometry must stay in logical coordinates and rely on GSK's
  device scale, unlike the Cairo path's device-scaled surfaces; check 1.0 and 1.5.
- Two painters can drift. The parity test is the guard; keep both behind the same cell iteration
  helpers where possible.
- A retained node tree for a full grid is memory proportional to cells; measure at 120x40.

## Post-task decisions

- **Shared first-draw monitor (C5).** The layout monitor is resolved once through a shared helper
  (`resolve_first_draw_monitor`, the first-draw latch) used by both the cairo draw hook and the
  GSK snapshot hook. A snapshot frame bypasses the draw func, so both painters must consume the
  same one-shot before emitting or the start handshake never completes (dc70165).
- **Tween draw shift glued to the live grid (C52).** `SurfaceHooks::live_grid_px` supplies the
  width `Anim::draw_offset` glues against, independent of a running tween; the deferred grid
  resize runs synchronously at the tween stop; and `Surfaces::stale_grid_px` keeps the old,
  pre-resize content glued to the docked edge until the host's first output, because
  libghostty-vt's resize does not rewrap the active screen. These fix a black-on-expand and an
  end-of-tween gap that predate this change; they are outside the task rows but included.
- **Kitty placeholders (deliberate terminal-crate change).** `Terminal::frame_begin_images`
  keeps the placeholder origins a cell pass recorded, so `U=1` images render in the node image
  pass (row 3.1 requires images in the node tree; a fresh `frame_begin` silently dropped every
  U=1 placement), and `MAX_PLACEHOLDERS` rose 8 -> 64 so grids with many covers are not cut off.
  This is the change's one terminal-crate edit, deliberate despite the proposal's "no terminal
  behaviour change" non-goal.
