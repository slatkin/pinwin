# Tasks

## 1. Gate

- [x] 1.1 Make sure that Uzaaft/libghostty-rs#84 and #85 are both merged into master (`gh pr view 84 -R Uzaaft/libghostty-rs --json state,mergeCommit` and the same for 85). If either PR is not merged, stop and report. Record the first master commit that contains both PRs as `<sha>` in this task. At `<sha>`, make sure that `crates/libghostty-vt-sys/build.rs` pins a ghostty commit at or past `3a3047f6` (`gh api repos/ghostty-org/ghostty/compare/3a3047f6b62a791fd8b12d9f07a85b3d2160370b...<pin>` reports `ahead` or `identical`). Make sure that `RowIteration::viewport_y` and `kitty::graphics::set_png_decoder` exist at `<sha>`. If any check fails, stop and report. Verified 2026-10-09: `<sha>` = `e1e145f22bace96593ecaa348c09d874635ef637`, pin `3425025e585a3403340d4dc6d65132f42e05e605` (`ahead` by 66), both APIs present.

## 2. Golden output before the port

- [ ] 2.1 Add a test in `src/term/cells/` that records the golden output (design A7). It feeds a fixed VT byte stream through `term::Terminal`. The stream holds wide characters and styled text (bold, italic, inverse, underline). It also holds 24-bit and palette colors, enough lines to scroll, a cursor at a known cell, and one kitty PNG placement. Use a test `PngDecoder` that returns a fixed 2x2 RGBA image. The test writes every `Cell`, the cursor, the colors and every `Image` as text and compares the result with a checked-in fixture file. Generate the fixture from the current FFI and commit it before any port code. Make sure that `cargo nextest run` passes.

## 3. Dependency and build

- [ ] 3.1 Add `libghostty-vt` as a git dependency at `rev = "<sha>"` with default features and without `png` (design A1). Raise `rust-version` to 1.90. Add `.cargo/config.toml` with `[env]` entries `LIBGHOSTTY_VT_SYS_OPTIMIZE = "ReleaseSafe"` and `LIBGHOSTTY_VT_SYS_CPU = "x86_64_v2"` (design A2). Delete `build.rs` and the `build = "build.rs"` line and its comment from `Cargo.toml`. Make sure that `cargo build -vv` shows `-Doptimize=ReleaseSafe` and `-Dcpu=x86_64_v2` in the zig command line for the dev and the release profile. Change the optimize value, rebuild, and make sure that zig runs again. If the crate does not build yet with `ghostty_sys` still present, fold this task into task 4.1 and do not commit it on its own.
- [ ] 3.2 If the CI workflow `.github/workflows/build.yml` references `build.rs` or `PINWIN_GHOSTTY_SRC`, update it, including the cache key. Make sure that CI passes on the branch.

## 4. Port `src/term/`

- [ ] 4.1 Port terminal creation, resize, `vt_write` and the effect callbacks in `src/term/mod.rs` and `src/term/callbacks.rs` to the crate (design A5). Use `on_pty_write`, `on_size`, `on_device_attributes` and `set_kitty_image_storage_limit`. Share state through closure captures instead of the userdata pointer. Remove the lazy init path and the `GHOSTTY_REJECTED` sentinel, and map a failed `Terminal::new` to `PinwinError::Internal`. Keep early pty buffering. Make sure that the existing `term` tests pass.
- [ ] 4.2 Run the body of every closure handed to the crate through `guard::guard_default` with the terminal's poison latch (design A3). Add one test per closure (`on_pty_write`, `on_size`, `on_device_attributes`, the PNG forwarder). Each test makes the closure panic. Then it makes sure that the process does not abort, that `Terminal::poisoned()` is true, and that later calls do nothing.
- [ ] 4.3 Port the PNG decode hook to one `DecodePng` forwarder installed with `set_png_decoder` inside a `Once` (design A4). Keep `DECODE_CONTEXTS` routing and the RGBA length check, and allocate output with `Bytes::new_with_alloc`. Make sure that `decode_png_rejects_a_buffer_that_disagrees_with_the_dimensions` still passes, adapted to the forwarder.
- [ ] 4.4 Port `src/term/cells/mod.rs` to `RenderState`, `RowIterator` and `CellIterator`, with one `update()` per pass (design A5). Read the row Y with `RowIteration::viewport_y`. Read cursor and colors through the snapshot methods. Clear dirty state with the snapshot-level call. Make sure that the golden test from task 2.1 passes, apart from the image section.
- [ ] 4.5 Port `src/term/cells/images.rs` to `kitty::graphics` (`Graphics`, `Image`, `PlacementIterator`, `placement_render_info`). Make sure that the golden test passes in full and that the existing image tests pass.
- [ ] 4.6 Port `src/term/input.rs` and `src/term/keys.rs` to `key`, `mouse` and `focus` (design A6). Use `encode` with a stack buffer and one exact-size retry on `OutOfSpace`, never `encode_to_vec`. If the codepoint is 0, skip `set_unshifted_codepoint`. Make sure that the existing input tests pass, including the kitty disambiguation, SGR mouse and focus report cases.

## 5. Remove the old FFI and update docs

- [ ] 5.1 Delete `src/ghostty_sys/` and `pub mod ghostty_sys` in `src/lib.rs`, and update the module comment there (design A8). Remove the raw-handle accessors from `term::Terminal` or make them private. Remove the `src/ghostty_sys/*` exclusion from `scripts/check-code-file-lines.sh`. Make sure that `make check-code-file-lines` passes.
- [ ] 5.2 Update `AGENTS.md` and `README.md`: the module list, the build section, the FFI layout tests line, and the `ghostty_sys` line-count exception. In the build section, `GHOSTTY_SOURCE_DIR` replaces `PINWIN_GHOSTTY_SRC`, and `.cargo/config.toml` holds the optimize and CPU values. Update code comments that cite port-to-rust D2 as the reason for own FFI so that they cite this change. Make sure that `rg -n 'ghostty_sys|PINWIN_GHOSTTY_SRC|build\.rs'` finds no stale references outside `openspec/changes/archive/`.

## 6. Acceptance

- [ ] 6.1 Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo audit`, `cargo nextest run`, `make check-code-file-lines` and `openspec validate adopt-libghostty-rs --strict`. Make sure that all of them pass.
- [ ] 6.2 Run `cargo run --example demo` with `DEMO_DENSE=1` in niri, as a user task. Make sure that text, colors, the cursor, kitty images, the width animation both ways, mouse clicks and focus reports behave as before.
- [ ] 6.3 Close slatkin/pinwin#18 with a link to the merge commit, sync the delta into `openspec/specs/pinwin-panel/spec.md`, and archive the change.
