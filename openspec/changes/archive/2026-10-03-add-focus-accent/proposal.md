# Proposal

## Why

The panel is a layer-shell surface, and niri draws its focus ring and border only around
layout windows — so while the panel holds keyboard focus, nothing on screen says so. The
panel must mark focus itself. The accent shipped ad hoc in `ef778e4` (pushed to
`origin/main`; the client's `pinned-panel-focus-accent` change pins that SHA). This change folds
the shipped behavior into the spec and addresses the two review notes from `pe-r8`:
`startup_valid`'s accent checks are untestable as static inline code, and the host parses
`PINWIN_ACCENT_COLOR` even when `PINWIN_ACCENT=off`.

## What Changes

- **Spec**: MODIFIED "Keyboard focus by clicking" in `pinwin-panel` — the focused panel
  SHALL draw a focus accent strip on its workspace-facing edge in the configured colour and
  width, draw nothing when the accent is disabled, and `pinwin_start` SHALL reject an
  invalid accent with `PINWIN_ERR_INVALID`.
- **Pure validation**: `PinwinAccent` moves from `src/pinwin.h` to `src/options.h` (beside
  `PinwinLayout`) and gains `pinwin_accent_validate()` in `src/options.c`, exercised by the
  Zig unit tests like `pinwin_layout_validate`; `startup_valid` in `src/pinwin_api.c` calls
  it. No ABI-struct change: `PinwinStartup` keeps the shape `ef778e4` gave it.
- **Host**: `PINWIN_ACCENT_COLOR` and `PINWIN_ACCENT_WIDTH` are parsed only when
  `PINWIN_ACCENT` is on; an off accent ignores (and does not reject) the other two.

## Capabilities

### Modified Capabilities
- `pinwin-panel`: MODIFIED "Keyboard focus by clicking" to require the focus accent and its
  validation.

## Impact

- `src/options.h`, `src/options.c`, `src/options_test.zig`, `src/pinwin.h`,
  `src/pinwin_api.c`, `host/main.c`.
- No new dependencies; no `PinwinStartup` shape change, so the client's pin of `ef778e4` stays
  valid (the follow-up commits add symbols, not fields).
