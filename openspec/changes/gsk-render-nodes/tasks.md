# Tasks

## 1. Node emitter foundations

- [x] 1.1 Spike: verify `gsk::RenderNode::draw` renders a snapshot node to an `ImageSurface` with no display, and add a parity helper that diffs it against the Cairo painter; verify with a unit test on a flat colour node
- [x] 1.2 Emit cell backgrounds (incl. the last-row fill) and the theme background as colour nodes; verify parity against `render_grid` backgrounds: exact at scale 1, stated tolerance at 1.5

## 2. Text, sprites, decorations, cursor

- [x] 2.1 Emit cell text as Pango layout nodes with the baseline pin and the nerd-font constraint transform from `text::draw_text`; verify parity (with tolerance) on ASCII, wide, bold/italic and constrained glyphs
- [x] 2.2 Emit block, quadrant, shade-free and braille sprites as colour nodes and powerline triangles as cairo nodes; verify the existing sprite tests pass for both painters
- [x] 2.3 Emit underline, strikethrough and all four cursor styles (incl. block cursor text); verify parity per style

## 3. Kitty images

- [x] 3.1 Hold a `gdk::MemoryTexture` per kitty image in `ImageCache` (same keys and eviction) and emit placements as texture nodes with clip; verify the image cache tests

## 4. Retained node cache

- [ ] 4.1 Build the grid (including images) into one `RenderNode` on the tween's first frame and draw it, translated, for the rest of the tween; rebuild it on every non-tween draw, in `GridArea::snapshot`; verify the display-free suite passes
- [ ] 4.2 Remove the `grid_cache` surface and the `MemoryTexture` bridge once 4.1 is accepted; verify `cargo build` and no dead code

## 5. Verification

- [ ] 5.1 Check the display-free suite, clippy and fmt, and run the demo (including `DEMO_DENSE=1`) under `GSK_RENDERER=ngl`, `vulkan` and `cairo`; verify no visual regression or cell seams (at 1.0 and 1.5 scale), input/resize unaffected, and Cairo fallback intact
- [ ] 5.2 Compare tween frame times with `PINWIN_FRAMELOG=1` before and after (no worse than stage 1); record the result in the change and close issue #4
