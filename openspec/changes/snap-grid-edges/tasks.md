# Tasks

## 1. Snapping helper

- [x] 1.1 Add `src/render/snap.rs` with `OutputScale` and the edge and rectangle snap (design D2, D4). Register the module in `src/render/mod.rs`. Add one unit test and make sure that it passes. The test checks three facts. At scale 1, an edge on a whole pixel does not change and an edge on a half pixel rounds up (4.5 becomes 5). Two cells that share an edge get the same snapped edge at scales 1.5 and 1.25. A rectangle with a positive size never collapses below one device pixel.

## 2. Scale plumbing

- [x] 2.1 Raise the `gtk4` and `gdk4` features to `v4_12` in `Cargo.toml`. Keep the comment about `ScrollUnit` and add the `Surface::scale()` reason. Make sure that `cargo build` succeeds.
- [x] 2.2 Add the `scale` field to `CellMetrics` and `DrawState::set_scale` (design D2, D7). The hooks call `set_scale` on every draw. If the scale differs from the stored one, `set_scale` drops the retained grid node. Otherwise it keeps the node, so the tween does not rebuild it each frame. Make `cell_metrics_update` keep the scale. Add an assertion to the existing `dropping_the_cache_invalidates_the_tween_node` test in `src/render/snapshot.rs`: a changed scale drops the node and an unchanged scale keeps it. Make sure that the test passes.
- [x] 2.3 Add `Surfaces::scale()` and connect `notify::scale` to a redraw in `on_map` (design D3). Call `DrawState::set_scale` from `draw_hook` and `grid_snapshot_hook` in `src/panel/gtk_side.rs` before they draw. Make sure that `cargo build` succeeds. Task 5.2 checks the live behavior.

## 3. Snap the shared geometry

- [x] 3.1 Snap the output of `cell_background_rect` and the rectangles of `sprite_shape` (design D1, D4). Snap the triangle vertices in `sprite_shape` (design D5). Call `set_scale` with the drawing scale in each test helper that draws. Do this in the helpers in `src/render/parity.rs`, in `cairo_backgrounds` in `nodes.rs`, and in `cairo_cursor` and `node_cursor_surface` in `node_cursor.rs`. Tighten these tests from tolerance to exact and make sure that they pass: `backgrounds_match_the_cairo_painter_within_tolerance_at_scale_1_5`, `sprites_match_the_cairo_painter_within_tolerance_at_scale_1_5` and `fractional_pitch_matches_within_tolerance`. (Accepted deviation: triangle sprites keep a tolerance of 16 at scale 1.5, because GSK replays diagonal edges with AA coverage; D5 allows diagonal blend. Rectangles are exact.)
- [x] 3.2 Snap `cursor_shape`, `underline_rect` and `strikethrough_rect`. Make `cursor_shape` return four band rectangles for the hollow cursor. Make the cairo painter fill the bands, the underline and the strikethrough instead of stroking them (design D6). Tighten `every_cursor_style_matches_the_cairo_painter_within_tolerance_at_scale_1_5` to exact. Make sure that the scale-1 cursor and frame tests still pass. Keep the text tolerance tests as they are. The Block case redraws a glyph. If it is not exact at scale 1.5, keep the text tolerance for that case only. Make the other styles exact.

## 4. Docked-edge offset and seam test

- [x] 4.1 Make `Surfaces::draw_offset()` return the snapped offset (design D7). Change `draw_offset` from `i32` to `f64` in `DrawState::draw`, `DrawState::snapshot_grid` and the two hooks in `src/panel/gtk_side.rs`. Update the test call sites that pass an integer offset. Make sure that the existing `anim` tests pass unchanged and that `cargo test` passes.
- [x] 4.2 Make sure that the input controllers read the snapped offset through `Surfaces::draw_offset()` and that nothing casts it to an integer. Search `src/` for `draw_offset() as` and make sure that no match remains.
- [x] 4.3 Add one seam test in `src/render/snapshot.rs` at scales 1.25 and 1.5. It renders two regions with a docked-edge offset: a row of full blocks, and a right-half block beside a left-half block of the same colour. Use an odd logical offset such as 135 and snap it with `OutputScale` first. The real snap runs in `Surfaces::draw_offset()`, which this test does not call. Make sure that every pixel in each region has the same colour. This test covers the spec scenarios for seams and half blocks. The exact parity test in 3.1 covers the uniform background field. Task 5.2 covers the snapped offset during a width animation.

## 5. Documentation and acceptance

- [ ] 5.1 Update `README.md` with the GTK version note: the 4.12 API floor, and 4.14 for fractional scale with the default renderer. Update the render file list in `AGENTS.md` for `snap.rs`. Make sure that `git diff` shows only these changes.
- [ ] 5.2 Final acceptance. Run `cargo run --example demo` with `DEMO_DENSE=1` on an output at scale 1.25, 1.5 and 2. Repeat with `GSK_RENDERER` set to `ngl`, `vulkan` and `cairo`. Make sure with a screenshot that no seam is visible in a field of one colour and in a field of block glyphs. Run a width animation on a right-docked panel and make sure that no seam appears. If a second output with a different scale exists, move the panel to it and make sure that the grid redraws without a seam. If a seam shows only in frames with a docked-edge offset, the grid origin is off the device pixel lattice (design D7). Stop and report it to the user instead of adding a workaround.
- [ ] 5.3 Run `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` and `openspec validate snap-grid-edges --strict`. Make sure that all four pass.
- [ ] 5.4 Sync the delta into `openspec/specs/pinwin-panel/spec.md` and archive the change.
