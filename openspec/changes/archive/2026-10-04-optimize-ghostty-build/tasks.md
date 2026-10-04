## 1. Baseline (user runs; agents do not run mbv, pinwin or niri)

- [x] 1.1 With the current build, switch tabs in the expanded (120 col) panel with artwork and note how it feels (subjective A/B baseline). `PINWIN_FRAMELOG` is tween-only and is not extended here.

## 2. Build change

- [x] 2.1 `build.rs`: add an optimise-mode constant (`ReleaseSafe`) and pass `-Doptimize=<it>` in `build_ghostty()`.
- [x] 2.2 Include the mode in the `identity` string written to `source-stamp`.
- [x] 2.3 `cargo build` succeeds; a second build does not rebuild ghostty; changing the constant does.

## 3. Verification (user)

- [x] 3.1 Repeat the 1.1 tab switch with the new build and compare against the baseline.
- [x] 3.2 If still slow, try `ReleaseFast`; if it is not faster, keep `ReleaseSafe`.
- [x] 3.3 If the lag persists, reopen the other suspects (renderer, ghostty_sys, pty, panel, focus accent).

## 4. Release

- [x] 4.1 Land with "Closes #7" (slatkin/pinwin#7); mbv bumps the pinwin rev in `Cargo.toml`, citing slatkin/pinwin#7 and slatkin/mbv#877, and closes #877 when 3.1 confirms.

Note: whether the current archive is Debug is not checked separately; size/nm checks do not discriminate, and 2.x plus the A/B tests the hypothesis.
