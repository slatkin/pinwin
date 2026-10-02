# Tasks

Each task states its verify. `rg` runs from the repo root unless noted. The design's open
questions are adjudicated (OQ-a: dev-only demo executable; OQ-b: delete the `Makefile`
install target, `pinwin.sh` untouched; OQ-c: README notes plus an annotated `library-abi`
tag). Section order keeps the build green and each verify true at its own step: the library
flip (1.1) precedes the entry-point deletion, the control socket (1.4) precedes the options
window it called, and the no-exit audit (§4) is the final sweep over the finished library.

## 1. Library build and removal of the program-only surfaces (design D1, D7)

- [x] 1.1 Flip `pinwin/build.zig` to the library form: `addStaticLibrary` + `installArtifact` produce `zig-out/lib/libpinwin.a`, the pinned ghostty dependency's `ghostty-vt-static` artifact installs as `zig-out/lib/libghostty-vt.a`, and the `gio-2.0` and `dbusmenu-glib-0.4` link lines, the C `check` step and `pinwin/tools/check_options.c` are deleted (the layout core's Zig tests return in 6.1). Verify: from `pinwin/`, `zig build` yields `zig-out/lib/libpinwin.a` and `zig-out/lib/libghostty-vt.a`, no `zig-out/bin/pinwin` exists, and `rg -n "gio-2.0|dbusmenu|check_options" pinwin/build.zig pinwin/tools` finds nothing.
- [x] 1.2 Delete `main()`/`envSetting`/`keyboardMode`/`commandArgv` in `pinwin/src/main.zig`; the file keeps the terminal core (`ensureTerminal`, effect callbacks, `pinwin_size`, `pinwin_pty_data`, frame/input protocol). Verify: from `pinwin/`, `zig build` still compiles the static library and `rg -n "COLS|GUTTER|PINWIN_DEBUG|PINWIN_KEYBOARD|no_tray|commandArgv" pinwin/src/main.zig` finds nothing.
- [x] 1.3 Delete `pinwin/src/tray.c`/`tray.h`. Verify: `rg -n "tray|dbusmenu" pinwin/src pinwin/build.zig` finds nothing outside this change's own docs.
- [x] 1.4 Delete the control socket in `pinwin/src/glue.c` (`control_start`, `on_control_incoming`, `control_read_done`/`control_reply`/`control_write_done`, `ControlConnection`, `g_control_socket_path`, `g_control_service`) together with its `pinwin_options_open` call site, delete `pinwin/src/control.c`/`control.h`, and delete the `PINWIN_SOCKET` setenv/unsetenv in `pinwin/src/pty.c`. Verify: `rg -n "PINWIN_SOCKET|control_start|ControlConnection|pinwin_control_parse" pinwin/src` finds nothing.
- [x] 1.5 In `pinwin/src/options.c`, delete the GTK options window (`pinwin_options_open`, its statics/callbacks) and the GKeyFile persistence (`pinwin_config_load`/`pinwin_config_save`, `PINWIN_CONFIG_*`), and in `pinwin/src/glue.c` delete `load_saved_layout` and the saved-layout branch of `resolve_layout_monitor` (monitor resolution itself stays); delete the dead declarations from `pinwin/src/options.h`. Keep the pure layout core (`pinwin_parse_cols`, `pinwin_parse_gutter`, `pinwin_side_geometry`, `pinwin_layout_validate`, `pinwin_layout_default`). Verify: `rg -n "GKeyFile|pinwin_options_open|pinwin_config_|load_saved_layout" pinwin/src` finds nothing.
- [x] 1.6 Delete the remaining pinwin env reads: `PINWIN_FONT`/`PINWIN_FONT_SIZE` overrides in `pinwin/src/fontconfig.c` and `PINWIN_DEBUG` reads in `pinwin/src/glue.c` and `pinwin/src/render.c`. Ghostty-config file following (font/theme) stays. Verify: `rg -n "PINWIN_FONT|PINWIN_DEBUG|getenv\(\"PINWIN_KEYBOARD\"\)|Environ.getPosix" pinwin/src` finds nothing. (`TERMINFO`/`TERMINFO_DIRS` in `fontconfig.c` go with `terminfo_exists` in 3.3.)

## 2. Layout intake rewire (design D4)

- [x] 2.1 Rework `glue_init` off `(cols, gutter, keyboard, no_tray)`: it takes the stored startup layout + keyboard mode, keeps theme/font metrics, the layer-shell check and `GtkApplication` setup; `on_activate` applies the startup layout directly with no `pinwin_layout_default` call. `pinwin/src/pinwin.h` loses `glue_start`, `glue_exit` and the old `glue_init` signature, and `pinwin/src/glue.c` their definitions (the ABI in 3.1 replaces them). Verify: `rg -n "g_gutter|g_no_tray|g_argv|no_tray|glue_start|glue_exit" pinwin/src` finds nothing.
- [x] 2.2 Rewire `glue_publish_layout` / `glue_layout_metrics` / `glue_current_layout` onto the ABI path (design D5): keep only what `pinwin_apply_layout` needs, delete the dead options-window bridges in `options.h` alongside them. Verify: `zig build` clean and `rg -n "glue_current_layout|glue_layout_metrics" pinwin/src` shows only ABI-path users.

## 3. ABI and threading (design D2, D5, D6)

- [x] 3.1 Add `pinwin/src/pinwin_api.h` (`PinwinStartup`, `PinwinLayout` reuse, keyboard mode, `pinwin_start`/`pinwin_apply_layout`/`pinwin_stop`, result codes, the never-from-the-GTK-thread contract) and its implementation: startup handshake with a synchronous result, GTK thread owning the `GtkApplication` main loop. Verify: a C consumer compiling against the header alone (`cc -fsyntax-only`) succeeds.
- [x] 3.2 Implement two-phase `pinwin_apply_layout`: Phase 1 pure checks on the caller thread (side, cols 1..=65535, checked geometry — no GTK); Phase 2 validate-against-live-metrics plus publish via `g_main_context_invoke` with a bounded wait; not-started → `PINWIN_ERR_NOT_RUNNING` without blocking. Verify: contract test in 6.2 plus `rg -n "g_main_context_invoke" pinwin/src` shows the single crossing.
- [x] 3.3 Rework `pinwin/src/pty.c` onto the supplied master fd: non-blocking attach, initial `TIOCSWINSZ`, existing read/write/resize paths unchanged; delete `forkpty`, `g_child_watch_add`/`on_child_exit`, `g_argv`, all child setenv, and `terminfo_exists` in `pinwin/src/fontconfig.c` (dead with `spawn_pty` gone; this also removes the last `TERMINFO`/`TERMINFO_DIRS` getenv). Hangup drops the pty source without touching process lifetime. Verify: `rg -n "forkpty|g_child_watch_add|on_child_exit|setenv|g_argv|terminfo_exists|TERMINFO" pinwin/src` finds nothing, and `rg -n "getenv|Environ.getPosix" pinwin/src` finds nothing.
- [x] 3.4 Dev-only demo executable (design OQ-a, adjudicated): `zig build demo` builds `zig-out/bin/pinwin-demo` from a small dev-only root that drives the C ABI with a canned layout over a pty pair it creates itself; never installed, and the default `zig build` does not build it. Verify: from `pinwin/`, `zig build` produces no `zig-out/bin/pinwin-demo`, `zig build demo` does, and `rg -n "pinwin-demo" pinwin/build.zig` shows it only under the `demo` step.

## 4. No-exit contract (design D3)

- [x] 4.1 Remove every exit path as the final sweep: `std.process.exit` in `main.zig` (ABI-boundary errors became result codes in 3.1; `pinwin_size`/`ensureTerminal` failure is reported — `pinwin_size` gains an `int` return, callers keep the previous grid), and any remaining `exit(` in the C sources (the fork, `on_child_exit` and `glue_exit` were deleted in 2.1/3.3). Verify: `rg -n "\<exit\(|_exit\(|std\.process\.exit" pinwin/src` finds nothing, and `rg -n "WEXITSTATUS|WTERMSIG" pinwin/src` finds nothing.

## 5. Build leftovers (design OQ-b)

- [x] 5.1 Delete the `Makefile` `install` target (the executable it installed no longer exists). `pinwin.sh` untouched. Verify: `make -n install` fails and `git status` shows `pinwin.sh` unmodified.

## 6. Tests (design D8)

- [x] 6.1 Add the layout-core Zig unit tests under a `check` step in `pinwin/build.zig`: strict cols parsing (1..=65535; 0/65536/`12x`/empty rejected), strict gutter parsing (optional `-`, digits only, int32 bounds), checked side geometry (overflow → 0), and every `PINWIN_GEOM_*` validation code reachable. Verify from `pinwin/`: `zig build check` passes.
- [x] 6.2 Contract test (in `zig build check`): `pinwin_apply_layout` with an invalid layout returns `PINWIN_ERR_INVALID` — distinct from `PINWIN_ERR_NOT_RUNNING` — with the GTK thread never started. Verify: `zig build check` passes, and the test fails if Phase 1 (design D5) is bypassed.

## 7. Specs, docs, release

- [x] 7.1 Spec deltas: rewrite `pinwin-panel` for the library form (docking, reservation, gutters, keyboard focus, terminal features, font, size reports, layer-shell kept; command/env/install/coexists requirements dropped; ABI + result codes + no-exit contract added; layout/validation rules folded in); REMOVED deltas deleting `pinwin-tray-options` and `pinwin-control` (persistence moves to the host; nothing folds back). Drops the `install-from-checkout` and `coexists-with-pinwin` requirements: the program no longer installs, and the library never touches `pin.kdl` or `~/.local/bin/pinwin` paths. Verify: `openspec validate add-library-abi --strict` passes.
- [x] 7.2 Rewrite `README.md` for the library: consumer build/link recipe (both archives + system libs), the ABI with result codes, the no-exit contract, layout-ownership notes (host supplies the full layout every start; no env, no config file), the pinned ghostty commit + Zig 0.16 minimum (design OQ-c). No `pinwin <command>` usage, no socket/tray/options-window/control-API sections survive. Verify: `rg -n "PINWIN_SOCKET|no-tray|zig-out/bin/pinwin|Options\.\.\." README.md` finds nothing.
- [x] 7.3 Manual check under niri via the demo (3.4). Verified mechanically (commands in
  `/tmp/verify2.log`, screenshots pixel-diffed): demo run with scripted stdin — `pinwin_start = 0`,
  layer surfaces `pinwin` + `pinwin-reserve` present in `niri msg layers` while running; live
  `pinwin_apply_layout` re-docks and widens 40→48 cols (`= 0`, left-strip mean 43→34 px, std 38→5);
  invalid layout returns 1 and leaves the frame byte-identical (739 px diff = cursor blink);
  `pinwin_stop` removes both surfaces (`niri msg layers` count 0, desktop pixels restored);
  panel failure: `WAYLAND_DISPLAY=does-not-exist` → `pinwin_start = 4` (`PINWIN_ERR_NO_DISPLAY`),
  host exits 1 cleanly; child hangup: `kill -9` the fish child mid-run → host keeps running and
  `q` still stops cleanly. Keyboard: keys typed in the panel render at the fish prompt (observed
  on-screen; the ABI replay contract test in `zig build check` covers the underlying path).
- [x] 7.4 Tag the library-form revision (annotated tag `library-abi` on the landing commit) and make `README.md` name it for consumers, alongside the ghostty pin and the Zig minimum (design OQ-c). Verify: `git tag -l library-abi` lists it and `rg -n "library-abi" README.md` finds the consumer note.
