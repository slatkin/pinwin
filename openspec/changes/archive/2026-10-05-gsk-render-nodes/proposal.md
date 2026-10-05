# Proposal

## Why

Issue #4 asks for GPU-accelerated rendering so the width animation is smooth. The
`poc-gsk-texture-grid` change (stage 1) only moved one thing onto GSK: the once-per-tween grid
cache as a texture node. Everything else the panel draws is still rasterised on the CPU through
Cairo/Pango: the grid (backgrounds, text, sprites, cursor), the kitty images and the focus
accent, on every non-tween redraw and once more per tween to build the cache. The issue's
target is a `snapshot` that emits GSK render nodes for all of it, with Cairo kept as the
fallback, and that is still undone.

## What Changes

- Emit the terminal grid as GSK nodes: cell backgrounds as colour nodes, text as Pango layout
  nodes, block/braille sprites as colour nodes, underline/strikethrough and the cursor as
  colour nodes. Powerline triangle sprites stay Cairo-drawn, as cairo nodes.
- Cache the grid as a retained `gsk::RenderNode`, not a rasterised texture: the node is built on
  the tween's first frame and drawn translated to the docked edge for the rest of the tween (no
  raster, no upload). Every non-tween draw rebuilds the node, as the Cairo path redraws today;
  the terminal has no content-changed signal, so no change-tracking is added.
- Emit kitty images as texture nodes, keyed by the existing image cache, with the placement's
  transform and clip.
- Retire the Cairo-built grid cache and the `MemoryTexture` bridge from stage 1.
- The Cairo draw path remains as the fallback (`GSK_RENDERER=cairo` still renders correctly,
  and the display-free tests keep exercising the Cairo painter).
- Verify on the Vulkan, GL (`ngl`) and Cairo renderers; close issue #4 when done.
- Out of scope: the per-frame layer-shell geometry calls, per-row node caching, GPU-side
  colour or font changes, any change to terminal behaviour.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. No user-visible requirement changes: the panel draws the same cells, images and accent;
the change is `skip_specs: true`.

## Impact

- `src/render/` (`snapshot.rs`, `text.rs`, `sprites.rs`, `images.rs`, plus new node-emitter
  modules; `mod.rs` is at its 800-line cap and only gains the hand-off), `src/surfaces/area.rs` (snapshot now emits for every
  frame), `src/panel/gtk_side.rs` (hook).
- Requires GTK feature level and API checks: `gtk4` is on `v4_8` today, which has no
  `gsk::Path`, so every sprite must be expressible as rect/colour/cairo nodes or the crate
  feature level is raised (decision in design.md).
- Builds on PR #6 (`poc-gsk-texture-grid`), already merged.
- Success criteria: Cairo and node output match in display-free tests (exact at scale 1 for
  rects and sprites; a stated tolerance at scale 1.5 and for text); no visible cell seams on the
  Vulkan, `ngl` and Cairo renderers; tween frame times are no worse than the stage-1 numbers
  (task 5.2); every existing render test still passes.
