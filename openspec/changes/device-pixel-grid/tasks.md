# Tasks

## 1. Scale plumbing

- [ ] 1.1 Spike the font path (design D2). Render one glyph through `append_layout` under a `1/scale` transform. Use a Pango context at resolution `96 * scale`. Verify that the glyph is 1.5 times taller at scale 1.5 than at scale 1, then record the result in `design.md`.
- [ ] 1.2 Raise the `gtk4` and `gdk4` features to `v4_12` in `Cargo.toml`. Verify that `cargo build` succeeds.
- [ ] 1.3 Add the `OutputScale` newtype to `src/layout.rs` with `logical_ceil` and `device_round`. Verify the unit tests: scale 1 is the identity, 325 device pixels at 1.5 gives 217, and zero, negative and NaN scales are rejected.
- [ ] 1.4 Make `Layout::validate` and the panel width take the scale. Verify that the existing layout tests pass at scale 1. Verify that a new test gives 217 logical pixels and a reservation of 237 for the 25-column example in the spec.

## 2. Device-pixel metrics and sizes

- [ ] 2.1 Make `measure_hook` set the Pango resolution to `96 * scale` and return device-pixel cell sizes. Measure again when the window maps. Measure again on every `notify::scale`. Verify with a unit test that the cell at scale 1.5 is within one pixel of 1.5 times the cell at scale 1. Verify that both are whole numbers.
- [ ] 2.2 Store the scale in `Surfaces`. Derive `grid_px` and the panel width from it with `logical_ceil`. Keep the provisional scale-1 measure in `build` (design D1). Verify that the window width and the reservation use the rounded width, with a test at scales 1, 1.5 and 2.
- [ ] 2.3 Compute the rows in `apply_size_to` from the device height. Pass device-pixel cells to `push_size` and the pty `winsize`. Verify with a test that `CSI 16 t` and `TIOCGWINSZ` report the device-pixel cell and grid size at scale 1.5.
- [ ] 2.4 On a scale change, drop the grid node, push the new size once and redraw. Verify with a test that one scale change gives one resize and one SIGWINCH.

## 3. Drawing on the device-pixel lattice

- [ ] 3.1 Wrap the grid pass in `snapshot_grid` with `save`, `scale(1/s, 1/s)` and `restore`. Pass device-pixel width and height to the emitters. Keep the theme background and the focus accent in logical coordinates. Verify with a test at scale 1.5 that every colour-node edge in the grid pass is a whole device pixel.
- [ ] 3.2 Review the node painters for logical-pixel assumptions: the cursor bands, the decorations, the sprite padding, the triangle nodes and the images. Fix each one found. Verify that the existing node tests pass and that a cursor and an underline are one device pixel thick at scale 1.5.
- [ ] 3.3 Wrap the cairo fallback grid pass in `cr.scale(1/s, 1/s)`. Verify that the fallback test draws the same cell size as the node painter.
- [ ] 3.4 Run the parity tests at scales 1, 1.5 and 2. Add a seam test that renders a field of one background colour and a field of full blocks. Verify that every pixel in each field has the same colour at all three scales.

## 4. Tween, input and sliver

- [ ] 4.1 Add the single device-offset function (design D7) and use it for the grid translate. Verify with unit tests that the offset is a whole device pixel at every tween step at scale 1.5. Verify that the existing `anim` tests pass unchanged.
- [ ] 4.2 Convert the pointer position to device pixels in the input controllers, then subtract the device offset. Verify with a test that a click on the centre of a cell at scale 1.5 maps to that cell, docked left and docked right.
- [ ] 4.3 Verify that the sliver beyond the grid shows the theme background on the side away from the docked edge. Add a draw test for both docking sides at a width that is not a multiple of the cell.

## 5. Documentation and integration

- [ ] 5.1 Update `README.md`: the GTK 4.12 minimum, the device-pixel cell size and the scale-change resize. Verify that the README states each of the three.
- [ ] 5.2 If the file list or the units changed, update the render layout text in `AGENTS.md`. Verify with `git diff` that only changed facts differ.
- [ ] 5.3 Run the panel on a fractional output at 1.25 and 1.5, and on a 2x output. Test with `GSK_RENDERER` set to `ngl`, `vulkan` and `cairo`. Make sure with a screenshot that no seam or grid is visible in a coloured block field. This check was waived for `gsk-render-nodes` and must not be waived here.
- [ ] 5.4 Run `cargo test`, `cargo clippy` and `openspec validate device-pixel-grid --strict`. Verify that all three pass.
