# Design

## Context

pinwin today (`eaefd7f`): Zig 0.16 `main.zig` over libghostty-vt (pinned in
`pinwin/build.zig.zon`: ghostty `3a3047f…`, hash `ghostty-1.3.2-dev-…`) plus C glue on GTK4,
gtk4-layer-shell, pango/cairo, gio (control socket, tray) and dbusmenu-glib (tray menu).
Verified against the sources (all paths under `pinwin/`):

- Entry/env/argv: `src/main.zig` — `pub fn main(init: Minimal)` reads `COLS`/`GUTTER` via
  `envSetting` (exits 2 on bad values), `PINWIN_KEYBOARD` via `keyboardMode` (exit 2),
  `PINWIN_DEBUG` for the Ghostty log, and `commandArgv` (`--no-tray`, `--`, unknown `--…`
  exits 2; default `$SHELL`/`/bin/sh`).
- Exit paths: `std.process.exit` in `main.zig` (`pinwin_size`/`ensureTerminal` failure,
  `glue_init` failure, argv/env errors); `exit(status)` in `glue.c`'s `glue_exit()`; `exit(1)`
  on `forkpty` failure plus `setenv("TERM")`/`setenv("COLORTERM",…)`/`setenv("PINWIN_SOCKET",…)`
  in the fork child in `src/pty.c` (`spawn_pty`, `on_child_exit` via `g_child_watch_add`).
- Control socket: lives in `src/glue.c` (`control_start`, `ControlConnection`,
  `on_control_incoming`, `g_control_socket_path`, ~100 lines); `src/control.c`/`control.h`
  is only the 16-line `pinwin_control_parse` helper.
- Layout intake: `glue_init(cols, gutter, keyboard, no_tray)` (`pinwin/src/pinwin.h`) stores the
  launch baseline; `on_activate` builds `pinwin_layout_default(g_cols, g_gutter)`; the saved
  GKeyFile layout loads at first draw via `load_saved_layout`/`resolve_layout_monitor`
  (`glue.c`); `glue_publish_layout` / `glue_layout_metrics` / `glue_current_layout` bridge the
  options window.
- GTK-thread state: `g_monitor`, `g_cell_w`/`g_cell_h` (set in `render.c`'s `cell_metrics_update`
  and `resolve_layout_monitor`), read by `glue_publish_layout` validation.
- Remaining env reads: `PINWIN_FONT`/`PINWIN_FONT_SIZE` in `src/fontconfig.c`
  (`font_config_load`), `PINWIN_DEBUG` in `glue.c` (`glue_init`), `options.c`
  (`on_options_destroy`), `render.c` (draw debug). Font *config* following (Ghostty
  `font-family`/`font-size`, theme colours) is file-based, not env, and stays.
- Checks: `pinwin/tools/check_options.c` compiled by the `check` step in `pinwin/build.zig`
  against `options.c` + `control.c` + `tray.c`, linked to gtk4/gio/dbusmenu-glib.

## Goals / Non-Goals

**Goals:** a linkable `libpinwin.a` with a three-call C ABI; no program-only surface left; no
exit path left; layout fully supplied by the host; geometry contracts covered by `zig build check`.

**Non-Goals:** no tray/ctrl/options-window replacements; no host-side wiring (mbv's
`crates/mbv-pinwin`, settings, start-up live in the mbv plan); no X11 or non-layer-shell
fallback (unchanged); no rendering/terminal/font/layout-behaviour changes beyond the moves and
rewires listed here.

## Decisions

**D1. Remove the program-only surfaces.**
- Delete `main()`/`envSetting`/`keyboardMode`/`commandArgv` in `src/main.zig` (the file keeps
  the terminal core: `ensureTerminal`, effect callbacks, `pinwin_size`, `pinwin_pty_data`,
  frame/input protocol). Zig entry becomes the library root instead of an executable.
- Delete `src/tray.c`/`tray.h`; drop the `gio-2.0` and `dbusmenu-glib-0.4` link lines in
  `pinwin/build.zig` (D2 of the archived control change confirms both exist only for the tray;
  gio's socket use in `glue.c` goes with the socket).
- In `src/options.c`, delete the GTK options window (`pinwin_options_open` and its statics /
  callbacks) and the GKeyFile persistence (`pinwin_config_load`/`pinwin_config_save`,
  `PINWIN_CONFIG_*`); keep the pure layout core (`pinwin_parse_cols`,
  `pinwin_parse_gutter`, `pinwin_side_geometry`, `pinwin_layout_validate`,
  `pinwin_layout_default` — retained as the pure baseline constructor for tests).
- Delete the control socket in `src/glue.c` (`control_start`, `on_control_incoming`,
  `control_read_done`/`control_reply`/`control_write_done`, `ControlConnection`,
  `g_control_socket_path`, `g_control_service`) and `src/control.c`/`control.h`; delete the
  `PINWIN_SOCKET` setenv/unsetenv in `src/pty.c`.
- Delete all env parsing: `COLS`, `GUTTER`, `PINWIN_DEBUG`, `PINWIN_KEYBOARD` (`main.zig`,
  `glue.c`, `options.c`, `render.c` debug paths) and `PINWIN_FONT`/`PINWIN_FONT_SIZE`
  (`fontconfig.c` overrides; Ghostty-config following stays — it is file-based panel
  appearance, not a program surface).

**D2. New C ABI (`pinwin/src/pinwin_api.h`, owned data only).**
- Types reuse `PinwinLayout` from `options.h` (side, cols, four gutters) and the
  `PINWIN_KEYBOARD_*` constants from `pinwin.h`:
  `typedef struct { int32_t master_fd; PinwinLayout layout; int32_t keyboard_mode; }
  PinwinStartup;`
- `int pinwin_start(const PinwinStartup*)` — validates args purely (bad fd, bad side, cols
  outside 1..=65535 → `PINWIN_ERR_INVALID`), stores the startup, spawns the GTK thread, and
  blocks on a start-up handshake until the thread reports GTK/layer-shell init success or
  failure (`PINWIN_ERR_NO_DISPLAY`). Second call while running → `PINWIN_ERR_ALREADY_RUNNING`.
- `int pinwin_apply_layout(const PinwinLayout*)` — two-phase validation (D5); posts the
  publish to the GTK loop; synchronous result code.
- `void pinwin_stop(void)` — closes panel + reservation surfaces, stops the GTK loop, joins
  the thread; no-op when not running. Hangup on the master fd stops the pty source without
  touching process lifetime; the host observes the hangup on its own side and calls
  `pinwin_stop`.
- Result codes (`PINWIN_OK 0`, `PINWIN_ERR_INVALID 1`, `PINWIN_ERR_ALREADY_RUNNING 2`,
  `PINWIN_ERR_NOT_RUNNING 3`, `PINWIN_ERR_NO_DISPLAY 4`, `PINWIN_ERR_INTERNAL 5`) are the
  only error channel: no stderr diagnostics on library paths, no `exit` (D3).

**D3. The library MUST NEVER call `exit()`.**
- Remove: `std.process.exit` ×7 in `main.zig` (env/argv errors become `PINWIN_ERR_INVALID`
  at the ABI boundary; `ensureTerminal` failure inside `pinwin_size` is reported to the
  caller — `pinwin_size` gains an `int` return — and surfaces as `PINWIN_ERR_INTERNAL`);
  `exit(status)` in `glue_exit()` (child-exit observation goes away with the fork; the
  function itself is deleted); `exit(1)` on `forkpty` failure (no fork remains).
- Audit rule: after the work, `rg -n "\<exit\(|_exit\(|std\.process\.exit" pinwin/src`
  finds nothing. `WEXITSTATUS`/`WTERMSIG` must not appear either (no child to wait on).
- Rationale: any of these kills the host process (mbv). This is a hard contract and a
  spec requirement, not style.

**D4. Layout intake restructure.**
- `glue_init` loses `(cols, gutter, no_tray)`: the ABI passes a full `PinwinLayout` up
  front, there is no saved config, and there is no tray. Its remaining job (theme/font
  metrics load, layer-shell check, `GtkApplication` setup) takes `(const PinwinLayout*,
  keyboard_mode)` or equivalent; exact signature is implementation detail.
- `on_activate` applies the stored startup layout directly — no `pinwin_layout_default` at
  startup, no `load_saved_layout`, no `resolve_layout_monitor` config branch (monitor
  resolution itself stays: the reservation must still pin to the panel's original monitor).
- `glue_publish_layout` keeps its validate-then-apply shape but is now driven by
  `pinwin_apply_layout` (D5) instead of the options window; `glue_layout_metrics` /
  `glue_current_layout` lose their options-window callers — keep only what the ABI path
  needs, delete the rest rather than leaving dead bridges.

**D5. Cross-thread contract: two-phase validation with synchronous invoke-and-wait.**
- `pinwin_apply_layout` runs on the host's non-GTK thread and must return a synchronous
  result, but metrics-dependent validation needs GTK-thread state (`g_monitor`,
  `g_cell_w`/`g_cell_h`).
- Phase 1 (caller thread, no GTK needed): pure checks — known side, cols 1..=65535,
  `pinwin_side_geometry` checked arithmetic. Failure → `PINWIN_ERR_INVALID` before any
  GTK interaction (this is what the contract test exercises).
- Phase 2 (GTK thread): the layout plus `layout->cols` is handed to the GTK main context
  with `g_main_context_invoke` (or equivalent) and the caller waits on a `GMutex`/`GCond`
  (bounded wait); there `pinwin_layout_validate` runs against live `g_cell_*`/monitor
  geometry and the valid layout publishes exactly like today's Apply path. Not started →
  `PINWIN_ERR_NOT_RUNNING` without blocking.
- Rejected alternative: an atomically-published metrics snapshot validated on the caller
  thread. It returns faster but validates against potentially stale output geometry (a
  monitor change between snapshot and publish can mis-reserve tiles), and any re-validation
  on the GTK thread would make the already-returned code a lie. Invoke-and-wait keeps one
  validation, on live metrics, with the exact code returned.
- Contract: `pinwin_apply_layout` MUST NOT be called from the GTK thread (it would
  deadlock the wait); `pinwin_stop` from the GTK thread is likewise forbidden. Documented
  in `pinwin_api.h`.

**D6. `pty.c` on the supplied master fd.**
- No `forkpty`, no `g_child_watch_add`/`on_child_exit`, no `g_argv`. At start the GTK thread
  takes the supplied fd non-blocking, applies the initial winsize (`TIOCSWINSZ` with the
  grid + pixel sizes, as `glue_pty_resize` does today), and installs the existing
  `on_pty_readable` source on it. `glue_pty_write`/`glue_pty_resize` are otherwise unchanged.
- No child env: `TERM`/`COLORTERM`/`PINWIN_SOCKET` setenv removed — the host owns the child
  side of the pty and sets whatever the TUI needs.
- `apply_size` keeps its grid/winsize logic; the `spawn_pty` first-shot branch becomes
  "attach to the supplied fd".

**D7. `build.zig` produces `libpinwin.a`; libghostty-vt is linked separately.**
- `b.addStaticLibrary(.{ .name = "pinwin", .root_module = … })` + `installArtifact` replaces
  `addExecutable`; `zig build` no longer yields a runnable panel.
- Zig static-library artifacts do **not** merge linked static archives, so these are
  different claims and the change states it plainly: `libpinwin.a` does NOT bundle
  libghostty-vt. `build.zig` additionally installs the ghostty static archive next to it
  (`zig-out/lib/libghostty-vt.a`, via the existing `ghostty` lazy dependency's
  `ghostty-vt-static` artifact), so the consumer links **both** archives plus
  GTK4/gtk4-layer-shell/pango-cairo. The mbv `build.rs` contract in the proposal depends
  on both files existing in `zig-out/lib/`.
- The `check` step compiles the Zig unit tests (D8), not C: `tools/check_options.c` is
  deleted and its gtk4/gio/dbusmenu link lines go with it.

**D8. Tests: Zig unit tests under `zig build check`.**
- Layout-core contracts against `options.h`: strict cols parsing (1..=65535, no sign/space;
  0, 65536, `12x`, empty rejected), strict gutter parsing (optional leading `-`, digits
  only, int32 bounds), checked side geometry (overflow → 0), `pinwin_layout_validate`
  result codes (each `PINWIN_GEOM_*` reachable: bad side, overflow, negative reservation,
  no row, no width, bad metrics).
- Contract test: `pinwin_apply_layout` with an invalid layout (bad side / cols 0) returns
  `PINWIN_ERR_INVALID`, distinct from `PINWIN_ERR_NOT_RUNNING`, before any GTK surface
  exists — i.e. exercising Phase 1 (D5) with the GTK thread never started.
- Manual checks that previously ran `zig-out/bin/pinwin` move to the demo-executable
  question (OQ-a): automated tests deliberately do not open surfaces.

## Risks / Trade-offs

- [GTK on a spawned thread] `GtkApplication`/`g_application_run` run on the spawned thread,
  not the process main thread. GTK tolerates this when all GTK calls stay on that thread
  (true today: everything already funnels through the main loop); the ABI boundary is the
  only crossing and goes through `g_main_context_invoke`. The start-up handshake (D2) keeps
  `pinwin_start` synchronous despite the spawn.
- [`pinwin_size` signature change] Zig `export fn` gains an int return; its C callers
  (`apply_size`) handle failure by keeping the previous grid — terminal allocation failure
  must degrade, never exit (D3).
- [Invoke-and-wait stalls] a wedged GTK loop stalls the host thread calling
  `pinwin_apply_layout`. Bounded by using a timeout that degrades to `PINWIN_ERR_INTERNAL`
  rather than blocking forever; normal operation is one main-loop turn.
- [Two archives, not one] consumers must link both `libpinwin.a` and `libghostty-vt.a`
  (D7). Accepted: bundling would mean re-archiving third-party objects into our artifact
  and lying about provenance; two documented files is the honest shape.

## Open questions (for the planning review to adjudicate)

- **OQ-a. Dev-only demo executable?** Today's manual checks run `zig-out/bin/pinwin` under
  niri. With no executable, how are the panel's manual checks performed? Candidate: a
  `demo` executable in `pinwin/tools/` (or a `zig build demo`), explicitly dev-only,
  never installed, driving the ABI with a canned layout — or drop it and test exclusively
  through mbv. Tasks carry the no-demo default; review picks.
- **OQ-b. `pinwin.sh` and the `Makefile`?** `pinwin.sh` (Ghostty-based, independent of the
  Zig panel) is untouched by this change either way. The `Makefile`'s `install` target
  installs the executable that no longer exists: delete the target, retarget it at
  `zig build` for dev convenience, or keep a `demo` install? Tasks assume delete-the-target
  (plus deleting the now-dead `install-from-checkout` spec requirement); review confirms.
- **OQ-c. Release/pinning story?** mbv pins a revision (today `eaefd7f`); after this change
  the pin must name a revision carrying the library ABI. Tag, branch, or bare revision —
  and where the "minimum consumer Zig/GTK versions" note lives (README vs a version
  header). Tasks assume README documents the pinned ghostty commit + Zig 0.16 minimum;
  review confirms.
