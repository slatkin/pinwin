# Tasks

Each task states its verify. `rg` runs from the repo root unless noted. Planning review
adjudicates OQ-a/OQ-b/OQ-c first; tasks assume the defaults recorded there (no demo exe,
delete the `Makefile` install target, README-carried pin notes).

## 1. Remove the program-only surfaces (design D1)

- [ ] 1.1 Delete `main()`/`envSetting`/`keyboardMode`/`commandArgv` in `pinwin/src/main.zig`;
  the file keeps the terminal core (`ensureTerminal`, effect callbacks, `pinwin_size`,
  `pinwin_pty_data`, frame/input protocol). Verify: `zig build` still compiles the Zig side
  and `rg -n "COLS|GUTTER|PINWIN_DEBUG|PINWIN_KEYBOARD|no_tray|commandArgv" pinwin/src/main.zig`
  finds nothing.
- [ ] 1.2 Delete `pinwin/src/tray.c`/`tray.h`; drop the `gio-2.0` and `dbusmenu-glib-0.4`
  link lines in `pinwin/build.zig`. Verify: `rg -n "tray|dbusmenu|gio-2.0" pinwin/src
  pinwin/build.zig` finds nothing outside this change's own docs.
- [ ] 1.3 In `pinwin/src/options.c`, delete the GTK options window (`pinwin_options_open`,
  its statics/callbacks) and the GKeyFile persistence (`pinwin_config_load`,
  `pinwin_config_save`, `PINWIN_CONFIG_*`); keep the pure layout core
  (`pinwin_parse_cols`, `pinwin_parse_gutter`, `pinwin_side_geometry`,
  `pinwin_layout_validate`, `pinwin_layout_default`). Verify: `rg -n "GKeyFile|
  pinwin_options_open|pinwin_config_" pinwin/src` finds nothing.
- [ ] 1.4 Delete the control socket in `pinwin/src/glue.c` (`control_start`,
  `on_control_incoming`, `control_read_done`/`control_reply`/`control_write_done`,
  `ControlConnection`, `g_control_socket_path`, `g_control_service`), delete
  `pinwin/src/control.c`/`control.h`, and delete the `PINWIN_SOCKET` setenv/unsetenv in
  `pinwin/src/pty.c`. Verify: `rg -n "PINWIN_SOCKET|control_start|ControlConnection|
  pinwin_control_parse" pinwin/src` finds nothing.
- [ ] 1.5 Delete the remaining env reads: `PINWIN_FONT`/`PINWIN_FONT_SIZE` overrides in
  `src/fontconfig.c`, `PINWIN_DEBUG` reads in `src/glue.c`, `src/options.c`, `src/render.c`.
  Ghostty-config file following (font/theme) stays. Verify: `rg -n "getenv|Environ.getPosix"
  pinwin/src` finds nothing.

## 2. No-exit contract (design D3)

- [ ] 2.1 Remove every exit path: `std.process.exit` in `main.zig` (ABI-boundary errors
  become result codes per D2; `pinwin_size`/`ensureTerminal` failure is reported — `pinwin_size`
  gains an `int` return, callers keep the previous grid); `exit(status)` in `glue_exit()`
  (delete the function with the fork observation); `exit(1)` on `forkpty` failure (no fork
  remains). Verify: `rg -n "\<exit\(|_exit\(|std\.process\.exit" pinwin/src` finds nothing,
  and `rg -n "WEXITSTATUS|WTERMSIG" pinwin/src` finds nothing.

## 3. Layout intake rewire (design D4)

- [ ] 3.1 Rework `glue_init` off `(cols, gutter, keyboard, no_tray)`: it takes the stored
  startup layout + keyboard mode, keeps theme/font metrics, the layer-shell check and
  `GtkApplication` setup; `on_activate` applies the startup layout directly with no
  `pinwin_layout_default` call and no `load_saved_layout` branch (monitor resolution for
  the reservation stays). Verify: `rg -n "g_gutter|g_no_tray|g_argv|load_saved_layout|
  no_tray" pinwin/src` finds nothing.
- [ ] 3.2 Rewire `glue_publish_layout` / `glue_layout_metrics` / `glue_current_layout` onto
  the ABI path (D5): keep only what `pinwin_apply_layout` needs, delete the dead
  options-window bridges in `options.h` alongside them. Verify: `zig build` clean and
  `rg -n "glue_current_layout|glue_layout_metrics" pinwin/src` shows only ABI-path users.

## 4. ABI and threading (design D2, D5, D6)

- [ ] 4.1 Add `pinwin/src/pinwin_api.h` (`PinwinStartup`, `PinwinLayout` reuse, keyboard
  mode, `pinwin_start`/`pinwin_apply_layout`/`pinwin_stop`, result codes, the
  never-from-the-GTK-thread contract) and its implementation: startup handshake with a
  synchronous result, GTK thread owning the `GtkApplication` main loop. Verify: a C
  consumer compiling against the header alone (`cc -fsyntax-only`) succeeds.
- [ ] 4.2 Implement two-phase `pinwin_apply_layout`: Phase 1 pure checks on the caller
  thread (side, cols 1..=65535, checked geometry — no GTK); Phase 2 validate-against-live-
  metrics plus publish via `g_main_context_invoke` with a bounded wait; not-started →
  `PINWIN_ERR_NOT_RUNNING` without blocking. Verify: contract test in 6.2 plus
  `rg -n "g_main_context_invoke" pinwin/src` shows the single crossing.
- [ ] 4.3 Rework `pinwin/src/pty.c` onto the supplied master fd: non-blocking attach,
  initial `TIOCSWINSZ`, existing read/write/resize paths unchanged; delete `forkpty`,
  `g_child_watch_add`/`on_child_exit`, `g_argv`, all child setenv. Hangup drops the pty
  source without touching process lifetime. Verify: `rg -n "forkpty|g_child_watch_add|
  on_child_exit|setenv|g_argv" pinwin/src` finds nothing.

## 5. Build (design D7)

- [ ] 5.1 `pinwin/build.zig` builds `libpinwin.a` (`addStaticLibrary` + `installArtifact`)
  instead of an executable; installs `libghostty-vt.a` next to it from the existing pinned
  ghostty lazy dependency; deletes the C `check` step's gtk4/gio/dbusmenu links with
  `tools/check_options.c` (gone in 6.1). Verify from `pinwin/`: `zig build` yields
  `zig-out/lib/libpinwin.a` and `zig-out/lib/libghostty-vt.a`, and no `zig-out/bin/pinwin`
  exists.
- [ ] 5.2 Delete the `Makefile` `install` target (OQ-b default; the executable it installed
  no longer exists). `pinwin.sh` untouched. Verify: `make -n install` fails and `git status`
  shows `pinwin.sh` unmodified.

## 6. Tests (design D8)

- [ ] 6.1 Delete `pinwin/tools/check_options.c`; add Zig unit tests under `zig build check`
  covering the layout-core contracts: strict cols parsing (1..=65535; 0/65536/`12x`/empty
  rejected), strict gutter parsing (optional `-`, digits only, int32 bounds), checked side
  geometry (overflow → 0), and every `PINWIN_GEOM_*` validation code reachable. Verify from
  `pinwin/`: `zig build check` passes.
- [ ] 6.2 Contract test (in `zig build check`): `pinwin_apply_layout` with an invalid layout
  returns `PINWIN_ERR_INVALID` — distinct from `PINWIN_ERR_NOT_RUNNING` — with the GTK
  thread never started. Verify: `zig build check` passes, and the test fails if Phase 1
  (D5) is bypassed.

## 7. Specs and docs

- [ ] 7.1 Spec deltas: rewrite `pinwin-panel` for the library form (docking, reservation,
  gutters, keyboard focus, terminal features, font, size reports, layer-shell kept;
  command/env/install/coexists requirements dropped; ABI + result codes + no-exit contract
  added; layout/validation rules folded in); REMOVED deltas deleting `pinwin-tray-options`
  and `pinwin-control` (persistence moves to the host; nothing folds back). Drops the
  `install-from-checkout` and `coexists-with-pinwin` requirements: the program no longer
  installs, and the library never touches `pin.kdl` or `~/.local/bin/pinwin` paths.
  Verify: `openspec validate --strict` (or `openspec change validate add-library-abi
  --strict`) passes.
- [ ] 7.2 Rewrite `README.md` for the library: consumer build/link recipe (both archives +
  system libs), the ABI with result codes, the no-exit contract, layout-ownership notes
  (host supplies the full layout every start; no env, no config file), the pinned ghostty
  commit + Zig 0.16 minimum (OQ-c default). No `pinwin <command>` usage, no socket/tray/
  options-window/control-API sections survive. Verify: `rg -n "PINWIN_SOCKET|no-tray|
  zig-out/bin/pinwin|Options\.\.\." README.md` finds nothing.
- [ ] 7.3 Manual check under niri against the OQ-a outcome (default: through the mbv
  consumer, or a review-approved demo exe): panel docks with the host layout, live
  `pinwin_apply_layout` resizes/re-docks, `pinwin_stop` removes the panel, host survives a
  rejected layout and a panel failure. Record the commands used.
