# Proposal

Related: slatkin/mbv#864 (mbv-side consumer plan `pin-mbv-in-pinwin`), slatkin/pinwin#1 (prior art only).

## Why

mbv is adopting pinwin as an embedded in-process library instead of running it as a separate
program (mbv plan `pin-mbv-in-pinwin`, which defers the entire reshape here). Every surface that
exists only because pinwin was a program — its `main()`, env vars, argv, tray, options window,
config file and control socket — must go, and what remains becomes a static library with a small
C ABI that mbv links and drives from its own GTK thread. Doing this reshape in pinwin's own
openspec keeps the contract reviewable by both repos before either side builds on it.

## What Changes

- **Program surfaces removed**: `main()` in `pinwin/src/main.zig` with all env parsing (`COLS`,
  `GUTTER`, `PINWIN_DEBUG`, `PINWIN_KEYBOARD`, `PINWIN_FONT`, `PINWIN_FONT_SIZE`) and command
  argv (`--no-tray`, `--`); `pinwin/src/tray.c`/`tray.h` with the gio/dbusmenu-glib link lines;
  the GTK options window and GKeyFile persistence in `pinwin/src/options.c` (the pure layout
  core stays); the control socket in `pinwin/src/glue.c` plus `pinwin/src/control.c`/`control.h`
  and the `PINWIN_SOCKET` setenv in `pinwin/src/pty.c`.
- **New C ABI** (`pinwin/src/pinwin_api.h`): `pinwin_start` (host-supplied pty master fd, full
  layout, keyboard mode; spawns the GTK thread; synchronous result code), `pinwin_apply_layout`
  (synchronous validation result; posts to the GTK loop) and `pinwin_stop` (closes the panel,
  joins the thread).
- **No-exit contract**: every `exit()` path goes (`std.process.exit` in `main.zig`, `exit(status)`
  in `glue.c`'s `glue_exit`, `exit(1)` on `forkpty` failure in `pty.c`). A library that kills its
  host process is a defect, not a diagnostic.
- **`pty.c` without `fork`**: reads/writes the supplied master fd, applies winsize to it
  (`TIOCSWINSZ`); no child env (`TERM`/`COLORTERM`/`PINWIN_SOCKET` setenv removed — the host owns
  the child side).
- **`pinwin/build.zig` builds `libpinwin.a`** instead of an executable; `tools/check_options.c`
  is replaced by Zig unit tests under `zig build check`.
- **Specs**: `pinwin-panel` rewritten for the library form; `pinwin-tray-options` and
  `pinwin-control` REMOVED (surfaces gone; layout/validation rules fold into `pinwin-panel`).

## Capabilities

### Modified Capabilities
- `pinwin-panel`: rewritten — docking, reservation, gutters, keyboard focus, terminal features,
  font, size reports and layer-shell kept; command/env/install requirements dropped; ABI,
  result codes and the no-exit contract added; layout/validation rules folded in from the
  retired specs.

### Retired Capabilities
- `pinwin-tray-options`, `pinwin-control`: the surfaces are gone. Tray, options window, config
  file and socket requirements are REMOVED; layout parsing/geometry/validation rules move into
  `pinwin-panel`. Persistence moves to the host (mbv's `config.toml`); pinwin keeps no config.

## Impact

- `pinwin/src/` (deletions plus `pinwin_api.h` and the intake/threading rewire),
  `pinwin/build.zig` (static library, check step), `pinwin/tools/check_options.c` (deleted),
  `README.md` (library consumer docs).
- No new dependencies; gio and dbusmenu-glib leave the link lines.
- Consumer contract (mbv's `crates/mbv-pinwin` `build.rs`): link `zig-out/lib/libpinwin.a` **and**
  the installed `libghostty-vt` archive (design D7) against GTK4, gtk4-layer-shell, pango/cairo.
- Open questions for the planning review (design §Open Questions): a dev-only demo executable
  for manual checks, the fate of `pinwin.sh` and the `Makefile`, and the release/pinning story.
