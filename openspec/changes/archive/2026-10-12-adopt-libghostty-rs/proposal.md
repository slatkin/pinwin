# Proposal

## Why

pinwin maintains its own FFI to libghostty-vt: about 2,150 hand-written lines in
`src/ghostty_sys/` and a 188-line `build.rs` that fetches and builds ghostty `3a3047f6`. The
libghostty-rs crates (`libghostty-vt`, `libghostty-vt-sys`) provide generated bindings, a safe
API, and the same Zig build. The port-to-rust spike (D2, 2026-10-04) rejected the crate. Its
ghostty pin was 1,404 commits behind ours, it had no row viewport Y, and its constructor ABI did
not match our pin. Two upstream PRs, now merged, removed those blockers. Uzaaft/libghostty-rs#84 moved
the pin to ghostty `3425025`, which is 66 commits ahead of ours. Uzaaft/libghostty-rs#85 added
`RowIteration::viewport_y`. Tracking issue: slatkin/pinwin#18.

## What Changes

- pinwin depends on `libghostty-vt` (git dependency, pinned by `rev`) at the first master
  commit that contains both #84 and #85. Implementation does not start before both PRs merge.
- `src/term/` and its `cells/`, `input.rs` and `callbacks.rs` call the crate's safe API
  (`Terminal`, `RenderState`, `RowIterator`, `CellIterator`, `kitty::graphics`, `key`,
  `mouse`, `focus`) instead of `ghostty_sys`.
- `src/ghostty_sys/` is deleted, together with its layout tests against the pinned headers.
- `build.rs` is deleted. The crate's own build script fetches and builds ghostty.
  `PINWIN_GHOSTTY_SRC` is replaced by the crate's `GHOSTTY_SOURCE_DIR`.
- The Zig optimize mode (`ReleaseSafe`) and CPU floor (`x86_64_v2`) move to a checked-in
  `.cargo/config.toml` `[env]` table that the crate's build script reads.
- The ghostty pin becomes the crate's pin. pinwin no longer chooses its own ghostty commit.
- `rust-version` rises from 1.88 to 1.90, the crate's minimum.
- No change to panel behavior, the public `Panel` API, or the `pinwin` binary.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: the "Optimised terminal library build" requirement names `build.rs` and the
  `source-stamp` identity, which this change removes. The requirement is restated against the
  crate's build: the mode is still explicit, never Debug, and changing it still rebuilds.

## Impact

- Code: `src/term/**`, `src/ghostty_sys/` (removed), `build.rs` (removed), `Cargo.toml`,
  new `.cargo/config.toml`, `scripts/check-code-file-lines.sh` (the `ghostty_sys` exclusion
  goes away), `AGENTS.md` and `README.md` (build and layout sections).
- Dependencies: adds `libghostty-vt` and, through it, `libghostty-vt-sys`. Build requirements
  stay Zig 0.16, `git` and network on a cold cache.
- Risk: the crate states that its API is unstable. Its `extern "C"` callback trampolines do
  not catch panics, so a panic in a callback closure aborts the host unless pinwin guards the
  closure body (port-to-rust D5).
- Supersedes port-to-rust D2 (own FFI). The D2 "no mixing" rule still holds: all of it comes
  from the crate.
