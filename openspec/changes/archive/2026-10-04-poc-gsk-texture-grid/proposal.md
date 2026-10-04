# Proposal

## Why

Issue #4: the width animation stutters on the laptop (not the desktop). Every tween frame
paints the cached grid through a `DrawingArea` Cairo draw callback, so GTK re-rasterises
and re-uploads that surface on the CPU each frame. The Rust port (#3) has landed, so the
change now targets the Rust widget layer.

## What Changes

- Stage 1 proof of concept only: during a width tween, present the once-per-tween grid
  cache as a `GdkTexture` in a GSK texture node, translated by the dock offset, instead of
  a Cairo blit. The texture uploads once per tween; per-frame work is a transform.
- The panel's `DrawingArea` becomes a thin subclass that overrides `snapshot`. Non-tween
  frames keep the existing Cairo draw path unchanged.
- Out of scope: live text/colour nodes, kitty-image nodes, removing the grid cache,
  and the per-frame layer-shell geometry calls.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. No user-visible requirement changes; the change is `skip_specs: true`.

## Impact

- `src/surfaces/mod.rs` (widget construction), `src/render/mod.rs` (cache + tween branch).
- New small module for the `DrawingArea` subclass; possibly `gdk4` `MemoryTexture` use
  (already a dependency).
- Success criterion: laptop frame-time comparison of a width tween before and after.
