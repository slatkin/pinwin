# Tasks

## 1. Layout push/cover choice

- [x] 1.1 Add the push/cover flag to `Layout` in `src/layout.rs` with a builder-style opt-in, keeping `Layout::new` pushing, and verify `cargo test layout` passes.
- [x] 1.2 Split `Layout::validate` by choice (pushing keeps today's gap checks; covering checks panel fits plus the vertical row, never the gap) with unit cases for covering-too-wide, covering vertical, and rejected-covering-leaves-held-gap-untouched, and verify `cargo test layout` passes.
- [x] 1.3 Document the choice on `Layout` (pushing default, covering opt-in) and verify `cargo doc --no-deps` builds without warnings.

## 2. Held-gap surfaces

- [x] 2.1 Store the held (side, zone) gap in `Surfaces`, update it only on pushing publishes and side switches, and draw the reserve surface from it while the panel follows the current layout, and verify `cargo test surfaces` passes.
- [x] 2.2 Admit push/cover-only differences in the animate eligibility and tween the gap only for pushing targets with a differing strip (panel always tweens), keeping snap rules, and verify the animation unit tests pass.
- [x] 2.3 Validate the staged layout in `publish` before mutating applied layout or held gap, default a covering start to an empty held strip, and verify `cargo test surfaces panel` passes.

## 3. API docs and demo exercise

- [x] 3.1 Document the per-size choice on the `Panel` apply methods in `src/panel/mod.rs` and verify `cargo doc --no-deps` builds without warnings.
- [x] 3.2 Add a demo cover toggle (pushing narrow vs covering wide) in `examples/demo.rs` and verify `cargo build --examples` succeeds.
- [x] 3.3 Update `README.md` Behaviour (push vs cover, same-side stillness, side-switch moves) and verify the documented demo toggle runs as written.

## 4. Integration checks

- [x] 4.1 Run `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`, and verify all three are clean.
- [x] 4.2 In a running niri session with the demo, verify covering expand/shrink on one side moves no tiles, side switch moves the gap, and a covering start reserves nothing.
