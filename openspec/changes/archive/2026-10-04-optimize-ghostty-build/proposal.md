# Proposal

## Why

mbv issue slatkin/mbv#877: switching tabs in the pinned panel lags when the panel is
expanded (120 cols) but not collapsed (40 cols), and was fine on mbv v0.22.4. The reporter
ties it to artwork, hence expanded-only.

Reading the code (nothing has been run or measured):

- `src/render/images.rs` (premultiply, PNG decode, `ImageCache`) is functionally identical to
  the old C `src/images.c`; the Rust image code is not the regression.
- v0.22.4's mbv `crates/mbv-pinwin/build.rs` ran `zig build -Doptimize=ReleaseSafe`.
  `build_ghostty()` in `build.rs` runs `zig build -Demit-lib-vt --prefix ...` with no
  `-Doptimize`. In the pinned ghostty (`3a3047f6b62a791fd8b12d9f07a85b3d2160370b`),
  `src/build/Config.zig:79` uses `b.standardOptimizeOption(.{})`, which defaults to Debug.
- libghostty-vt does the kitty-graphics work (base64 decode, image storage, PNG callback
  plumbing, render-state update), which scales with pixel count. A Debug build fits
  "artwork, expanded only".
- Not confirmed: archive sizes do not discriminate (new 58.7 MB vs old ReleaseSafe 57.6 MB),
  and nobody has verified the new archive is Debug or measured a tab-switch frame.
- Other suspects that remain open: the Rust rewrite of renderer, ghostty_sys, pty and panel;
  the focus accent's per-switch cost was not examined. The GSK texture grid (PR #6) is only used during the width tween and is ruled
  out.

## What Changes

- `build_ghostty()` passes an explicit `-Doptimize=<mode>` to the ghostty zig build. Default
  matches the old build (`ReleaseSafe`); `ReleaseFast` is evaluated in task 3.
- The optimise mode is part of the `source-stamp` identity, so changing it rebuilds the
  cached archive instead of reusing a stale one.
- Verification is by the user, not agents: a subjective A/B of a tab switch in the expanded
  panel with artwork, before and after. Frame timing, if wanted later, is a separate change.

Closes slatkin/pinwin#7 (downstream: slatkin/mbv#877).

## Capabilities

### New Capabilities

_None._

### Modified Capabilities

- `pinwin-panel`: adds a requirement that the bundled libghostty-vt is built optimised.

## Impact

- **Code**: `build.rs` only.
- **Build**: first build after the change recompiles ghostty (the stamp changes).
- **Downstream**: mbv bumps its pinwin rev in `Cargo.toml` afterwards. The fix lives in
  pinwin, not mbv.
- **Non-goals**: no change to image code, the renderer or the tween.
