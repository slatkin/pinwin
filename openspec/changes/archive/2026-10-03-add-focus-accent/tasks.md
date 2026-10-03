# Tasks

Each row states its verify. Commands run from the repo root. This change folds already-shipped
behavior (`ef778e4`) into the spec and fixes the two `pe-r8` review notes; no niri session
work is required beyond what `ef778e4` already showed on screen.

## 1. Pure accent validation (review note: startup_valid untested)

- [x] 1.1 Move `PinwinAccent` from `src/pinwin.h` to `src/options.h` (beside `PinwinLayout`)
      and add `int pinwin_accent_validate(const PinwinAccent*)` to `src/options.c`: reject
      `enabled` outside 0/1, and when enabled reject `width` outside 1..=65535; an off accent
      is valid regardless of width. `startup_valid` in `src/pinwin_api.c` calls it in place of
      the inline accent checks. Verify: `zig build` succeeds and `rg -n "PinwinAccent"
      src/pinwin.h` finds nothing.
- [x] 1.2 Cover `pinwin_accent_validate` in `src/options_test.zig`: enabled with widths 1,
      65535 accepted and 0, 65536, negative rejected; `enabled` 0 accepted with any width;
      `enabled` 2 and -1 rejected. Verify: `zig build check` passes.

## 2. Host env parsing (review note: color rejected while off)

- [x] 2.1 In `host/main.c`, call `accent_color` and parse `PINWIN_ACCENT_WIDTH` only when
      `PINWIN_ACCENT` is on; when off, store zeroes for the colour and width 0. Verify:
      `zig build` succeeds and `PINWIN_ACCENT=off PINWIN_ACCENT_COLOR=zzz ./zig-out/bin/pinwin true`
      starts the panel instead of exiting 2 (kill it after the panel appears).

## 3. Spec

- [x] 3.1 Validate and archive this change so the accent scenarios land in
      `openspec/specs/pinwin-panel/spec.md`. Verify: `openspec validate add-focus-accent
      --strict` passes before archive.
