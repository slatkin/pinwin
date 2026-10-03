# Tasks

Each row states its verify. Commands run from the repo root. Manual niri checks follow
`AGENTS.md` (`bash -n` is for `pinwin.sh` only; this change is verified by `zig build` and
`zig build check`). OQ-2 is resolved first because it decides whether the reservation can be
tweened as designed.

## 1. Spike the open risk (design OQ-2)

- [ ] 1.1 With a throwaway local edit (not committed), call `gtk_layer_set_exclusive_zone` on `g_reserve` from a `g_timeout` at ~16 ms and observe via `zig build demo` under niri whether tiled windows reflow each step. Record the result in design OQ-2 and revert the edit. Verify: `git diff --stat` is empty after the revert and design.md OQ-2 is updated with the finding.

## 2. Tween core (design D2, D4, D5, D6)

- [ ] 2.1 Add `src/glue_anim.c` with the static state struct, ease-out cubic, tick callback, watchdog, retarget, cancel and finish; declare its entry points in `src/glue_internal.h`; add the file to the `.files` list in `build.zig`. Verify: `zig build` succeeds and `rg -n "g_malloc|g_new|malloc" src/glue_anim.c` finds nothing.
- [ ] 2.2 Add `panel_px()` and use it in `apply_panel_width` and `apply_layout_surfaces` in `src/glue.c` in place of `g_cols * g_cell_w`. Verify: `zig build` succeeds and `rg -n "g_cols \* g_cell_w" src/glue.c` shows only `panel_px()` and `glue_init`.
- [ ] 2.3 Add the `gtk-enable-animations` check (design D6) and the animate-eligibility test (side and left/right gutters unchanged, cols differ, duration > 0) in `src/glue_anim.c`. Verify: `zig build` succeeds.
- [ ] 2.4 Call `glue_anim_cancel()` from `glue_close_surfaces` in `src/glue.c`. Verify: `rg -n "glue_anim_cancel" src` shows the definition and this call site.

## 3. ABI (design D1)

- [ ] 3.1 Add `uint32_t duration_ms` to `ApplyRequest` and a duration parameter to `glue_publish_layout` (`src/glue_internal.h`, `src/glue.c`, `src/pinwin_api.c`); `pinwin_apply_layout` passes 0. Verify: `zig build` succeeds and `rg -n "pinwin_apply_layout\(" src/pinwin_api.c` shows the unchanged structural-check path.
- [ ] 3.2 Add `pinwin_apply_layout_animated(const PinwinLayout*, uint32_t duration_ms)` and `#define PINWIN_ANIM_DEFAULT_MS 200` to `src/pinwin_api.h` and `src/pinwin_api.c`, clamping the duration to 1000 and sharing Phase 1 with `pinwin_apply_layout`. Verify: `cc -fsyntax-only -Isrc` on a one-line C file including `pinwin_api.h` succeeds, and `git diff -U0 src/options.h src/pinwin_api.h` shows no change to `PinwinLayout` or `PinwinStartup`.
- [ ] 3.3 Extend `src/pinwin_api_test.zig` for the new symbol: null and invalid layouts return `PINWIN_ERR_INVALID`, a valid layout without a running panel returns `PINWIN_ERR_NOT_RUNNING`, and `duration_ms == 0` is accepted like the snap call. Verify: `zig build check` passes.

## 4. Terminal content behavior (design D3)

- [ ] 4.1 In `src/render.c` `on_draw`, translate cell drawing by `width − g_cols * g_cell_w` for a right dock (left dock unchanged), keeping the background fill over the full width. Verify: `zig build` succeeds; the visual check is in 6.1.
- [ ] 4.2 In `src/input.c`, apply the same offset to mouse coordinates (resolving design OQ-5). Verify: `zig build` succeeds; the click check is in 6.1.

## 5. Release bookkeeping (design D7)

- [ ] 5.1 Bump `.version` to `0.2.0` in `build.zig.zon`. Verify: `rg -n "\.version" build.zig.zon` shows `0.2.0` and `zig build` succeeds.
- [ ] 5.2 Add the pinwin-panel delta to the main spec at archive time and record the tag decision (design OQ-1). Verify: `openspec validate add-animated-width --strict` passes.

## 6. Manual verification under niri (AGENTS.md)

- [ ] 6.1 Using `zig build demo`, apply a collapse and an expand through `pinwin_apply_layout_animated` on both left and right docks: motion is continuous, tiled windows reflow alongside, the final width is exactly `cols * cell_w`, the drawn grid is anchored to the docked edge, and clicks land on the intended cells. Verify: observed; note results in the PR description.
- [ ] 6.2 Interrupt mid-animation with a second animated apply, a collapse immediately followed by an expand, a plain `pinwin_apply_layout`, and `pinwin_stop`: each ends at the last requested layout (or torn down) with no residual offset. Verify: observed; note results in the PR description.
- [ ] 6.3 Set `gtk-enable-animations` to false (`gsettings set org.gnome.desktop.interface enable-animations false` or equivalent) and confirm the animated call snaps. Verify: observed.
- [ ] 6.4 Record the later benchmarks listed in design (on_draw frame time, client reflow cost, dropped frames). Verify: a short note exists, or the item is explicitly deferred in the PR description.
