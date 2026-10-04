# Tasks

## 1. Texture cache

- [x] 1.1 Wrap the tween cache as a `gdk::MemoryTexture` built in `grid_cache_ensure` and dropped in `drop_grid_cache`; verify with a unit test that cache dimensions and texture size match the device-scaled surface at scale 1 and 1.5
- [x] 1.2 Expose the cached texture and its logical size from the renderer; verify `cargo build` succeeds

## 2. Snapshot widget

- [x] 2.1 Add a `DrawingArea` subclass overriding `snapshot`: tween with cache emits bg + translated texture node + focus accent, otherwise chains to the parent; guard it with the poisoned latch; verify the panel still opens and draws normally via `cargo run --example demo`
- [x] 2.2 Construct the subclass in `Surfaces::new` in place of `DrawingArea::new()`; verify input (click, scroll, typing) and resize still work in the demo

## 3. Verification on hardware

- [x] 3.1 Run a width tween under `GSK_RENDERER=ngl` and `GSK_RENDERER=cairo`; verify it completes with the grid positioned correctly and no visual regression in both
- [x] 3.2 On the laptop, compare tween frame times before and after (frame-clock log from the tick callback); record the result in the change and verify the stutter is gone or note the remaining suspect
  - Result (desktop, DP-2, `PINWIN_FRAMELOG=1 DEMO_DENSE=1`, 4 width tweens each; frames/mean/p95/max):
    baseline 7a65a0c+logger: 32/6.5/11.1/16.7, 30/6.9/5.6/44.4, 17/13.1/22.2/22.2, 31/6.9/5.6/44.4 ms;
    texture 8e6662e: 31/6.7/11.1/16.7, 19/11.1/50.0/50.0, 17/12.7/22.2/22.2, 20/10.7/50.0/50.0 ms.
    No measurable improvement and no stutter reproduced on this machine (~144 Hz frame clock); the
    texture build frame (cairo fallback) shows as the 50 ms max. The laptop stutter is unconfirmed:
    the remaining suspect is the per-frame layer-shell geometry calls named in design.md.
