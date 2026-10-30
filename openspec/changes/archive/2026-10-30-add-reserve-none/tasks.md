# Tasks

## 1. Reserve mode and held strip

- [x] 1.1 Add `ReserveMode::None` with text `none` in `as_str` and `FromStr` (`src/geometry.rs`, `src/config.rs`), including the "push, cover or none" error text; verify parse and `as_str` round-trip tests pass.
- [x] 1.2 Make `HeldGap::start` and `after_publish` set a zero zone for `none`, and make `Geometry::validate` treat `none` like `cover`; verify new `src/geometry/tests.rs` cases pass: none start is zero, none after a 372 push is zero, none stays zero after a later none, push after none restores the strip, a none panel wider than the output is refused.

## 2. Animated change

- [x] 2.1 Make `final_gap` in `src/proxy/tween.rs` return a zero zone for a `none` target, and keep `tweens_with_panel` push-to-push only; verify tween tests for push to none and cover to none hold the strip on every frame before the last and give zero on the last frame.

## 3. Surfaces and docs

- [x] 3.1 Add `none` to the tray `RESERVES` list (`src/tray.rs`) and the `--reserve push|cover` usage strings in `src/cli.rs`; verify the tray menu test shows three reserve choices and the CLI usage tests pass.
- [x] 3.2 Update the `README.md` option tables (`--reserve`, `[panel.*] reserve`, `set-size`, `resize`, `set-reserve`, tray menu text) and the held-strip description in `AGENTS.md`; verify `rg "push\|cover" README.md AGENTS.md` shows no stale two-mode list.
- [x] 3.3 Run `cargo test` and the project's lint gates; verify they pass with the `none` cases included.

## Workflow follow-up

- The user runs the live check: a pushing panel gets `pinwin set-reserve <name> none` and tiled windows take the strip. Agents do not touch the live session.
- Archive the change after review.
