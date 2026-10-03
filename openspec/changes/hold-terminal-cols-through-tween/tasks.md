# Tasks

Each row states its verify. Commands run from the repo root. This change is verified by
`zig build` and `zig build check`; the tween behavior itself needs a live GTK window and is
the user's live check (the hermetic harness cannot reach it without faking coverage — see
1.3).

## 1. Previous-hold tween (design D1, D2)

- [x] 1.1 Carry the pending target in the tween: add `PinwinLayout pending` to `Anim` in
      `src/glue_anim.c`; `glue_anim_begin` takes the target layout and sets `pending` and
      `to_px` from it; add `glue_anim_retarget` (update `pending`/`to_px` without restarting
      the timing), `glue_anim_target_cols`, and `glue_anim_end` (stop + commit, no-op without
      a tween); declare them in `src/glue_internal.h`. Verify: `zig build` succeeds. Result:
      `zig build` clean.
- [x] 1.2 Stop publishing the target at tween start in `glue_publish_layout` (`src/glue.c`):
      the animate branches leave `g_layout`/`g_cols` at the applied values and hand the target
      to the tween; the snap branches keep setting both; the `GLUE_ERR_TERMINAL` snap-back
      snaps from a held tween to the requested layout; `glue_close_surfaces` ends the tween
      (commit) instead of only cancelling it. Verify: `zig build` succeeds and
      `rg -n "g_cols = layout->cols" src/glue.c` shows it only on snap paths. Result:
      `zig build` clean; the assignment appears on the snap branch, the no-column-change
      branch and the held-tween snap-back only.
- [x] 1.3 Commit through the non-animated tail: `anim_finish` (tick and watchdog) sets
      `g_layout`/`g_cols` from the pending target and runs `glue_apply_geometry()` +
      `apply_size()` exactly once. Verify: `zig build` succeeds. Result: `zig build` clean
      (`anim_commit`, shared by `anim_finish` and `glue_anim_end`).

## 2. Verification

- [x] 2.1 `zig build check` passes (the hermetic ABI harness still passes unchanged; the
      hold/commit paths need a live GTK window and are not faked there). Result: PASS.
- [ ] 2.2 Live check (the user, under niri with mbv pinned to this SHA): Ctrl+e toggles
      animate smoothly with no jerk and no image flash; the child sees no winsize change
      during the tween and exactly one at the end; interrupting mid-tween (rapid toggles, a
      plain apply, a stop) leaves the panel at the last requested layout with no residual
      offset. Verify: observed.
