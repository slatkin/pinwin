# Tasks

## 1. Layout core: columns as a sixth setting

- [x] 1.1 Add `int32_t cols` to `PenguinLayout` (options.h/options.c); extend `penguin_layout_default` to take the launch column count and set it; update the two existing callers in glue.c to pass `g_cols`. Verify: `zig build` in `penguin/` compiles clean.
- [x] 1.2 Add `penguin_parse_cols` (digits only, no sign, ≥ 1, ≤ 65535 — the PTY winsize column field) alongside `penguin_parse_gutter`. Verify: it is compiled by the GTK-free check tool.
- [x] 1.3 Extend `penguin/tools/check_options.c` with parse cases (valid, zero, negative-with-sign, non-numeric, > 65535) and `penguin_layout_default` coverage. Verify: build and run the check per README's Checks section; it prints `check_options: all passed`.

## 2. Config persistence

- [x] 2.1 Extend `penguin_config_load`/`penguin_config_save` to read and write `cols=` under `[layout]`; a missing key leaves the launch `COLS` in place, a malformed one makes the whole saved layout fall back (existing whole-file rule). Verify: check-tool config round-trip cases (save with cols → load returns it; absent key → cols untouched; malformed cols → fallback), run in the isolated `XDG_CONFIG_HOME` the tool already uses.

## 3. Apply path in glue.c

- [x] 3.1 Change `glue_publish_layout` to validate against the staged `layout->cols` instead of the current `g_cols`, then set `g_cols` on success. Verify: build clean; reasoning check that a rejected Apply leaves `g_cols` untouched.
- [x] 3.2 Resize the visible panel on publish: set the drawing area's width size request to the new `cols * cell_w` (GTK4 natural-size path; no `gtk_window_resize`) and keep `apply_layout_surfaces()` for margins and the reservation. At init, keep the same width derivation as today's `gtk_window_set_default_size`. Verify: `zig build` clean.
- [x] 3.3 Extend `apply_size()` to fire `penguin_size` + `glue_pty_resize` when `g_cols` changed since the last applied grid, not only on row changes; cols stay the input and are never re-derived from the allocated width. Verify: build clean; trace the width-only-Apply path by inspection against design D4.

## 4. Options window

- [x] 4.1 Add the labeled `Columns` numeric field as the first control, initialized from the instance's applied column count (`glue_layout_metrics`), staged like the gutter fields. Verify: build clean.
- [x] 4.2 Wire Apply: parse the field with `penguin_parse_cols`, build the layout with the staged cols, publish, save; on any failure show an error naming the offending field and leave the applied layout, column count and config untouched. Verify: build clean.
- [x] 4.3 Extend `check_options.c` validation cases for staged cols: too-wide reservation (`PENGUIN_GEOM_ERR_NO_WIDTH`), and geometry that passes with the new count. Verify: check tool prints `check_options: all passed`.

## 5. Documentation

- [x] 5.1 Update `README.md`: options-window description lists the Columns field; the "COLS, font and keyboard settings are never touched by an Apply" statements become "font, keyboard and the command"; document config precedence (saved `cols` > `COLS` env > 40) next to the existing gutter precedence text. Verify: README reads consistent with the spec deltas; no stale "never changes COLS" claims remain.

## 6. Live verification (manual checklist)

- [x] 6.1 With the user's permission (live-session rule): launch penguin, change Columns via the options window, Apply, and confirm the panel width, reserved strip, tile reflow and terminal grid columns all follow; restart penguin and confirm the saved column count is restored; delete the config and confirm launch `COLS` applies. Verify: each observable checked in the running session and recorded in the task notes.
  - Notes (verified live 2026-09-30, fresh `zig-out` build, `PENGUIN_DEBUG=1`, isolated `XDG_CONFIG_HOME` so the user's real config was never touched, child reporting `stty size`): launch `COLS=60` → grid `75 60`, panel draw `540` px (= 60 × 9 px cell). Apply Columns 96 → config `cols=96`, grid `96`, draw `864`; Apply 120 → grid `120`, draw `1080`; child PID unchanged (no respawn). Reservation/reflow: a tile's niri `tile_size` width went `855` → `783` when the panel grew 216 px (24 cols × 9), i.e. the working area shrank by exactly the reservation delta. Restart with `COLS=60` and saved `cols=120` → restored `1080` / `120` cols. Config deleted + `COLS=60` → `540` / `60` cols and launch wrote no config.
