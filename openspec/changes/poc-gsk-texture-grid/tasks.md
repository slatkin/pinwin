# Tasks

## 1. Texture cache

- [x] 1.1 Wrap the tween cache as a `gdk::MemoryTexture` built in `grid_cache_ensure` and dropped in `drop_grid_cache`; verify with a unit test that cache dimensions and texture size match the device-scaled surface at scale 1 and 1.5
- [x] 1.2 Expose the cached texture and its logical size from the renderer; verify `cargo build` succeeds

## 2. Snapshot widget

- [x] 2.1 Add a `DrawingArea` subclass overriding `snapshot`: tween with cache emits bg + translated texture node + focus accent, otherwise chains to the parent; guard it with the poisoned latch; verify the panel still opens and draws normally via `cargo run --example demo`
- [x] 2.2 Construct the subclass in `Surfaces::new` in place of `DrawingArea::new()`; verify input (click, scroll, typing) and resize still work in the demo

## 3. Verification on hardware

- [ ] 3.1 Run a width tween under `GSK_RENDERER=ngl` and `GSK_RENDERER=cairo`; verify it completes with the grid positioned correctly and no visual regression in both
- [ ] 3.2 On the laptop, compare tween frame times before and after (frame-clock log from the tick callback); record the result in the change and verify the stutter is gone or note the remaining suspect
