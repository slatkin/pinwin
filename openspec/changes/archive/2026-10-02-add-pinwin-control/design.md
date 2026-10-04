# Design

## Context

pinwin is Zig 0.16 (`main.zig`, `keys.zig`) plus C: `glue.c` (GTK app, layer-shell surfaces, PTY,
rendering, input), `options.c` (layout logic, config), and `tray.c` (StatusNotifierItem plus
dbusmenu). Facts the design relies on:

- `glue_init` already returns failure with a diagnostic when `gtk_layer_is_supported()` is false,
  so the Wayland-only requirement needs no new code.
- `pinwin_options_open()` already presents the single options window and reuses it if it is open.
  A control request only has to call it.
- `commandArgv` in `main.zig` treats every argument as the command and defaults to the client.
- `tray.c` rasterises `$HOME/pinwin.svg` into SNI pixmaps (`tray_argb_from_rgba`, covered by the
  check program) and falls back to a theme icon.
- The child is spawned in `spawn_pty()` with `forkpty`. `setenv("TERM", ...)` runs between fork
  and `execvp`.
- The check program `tools/check_options.c` is compiled by hand with the `cc` line in the README.

## Goals / Non-Goals

**Goals:**
- The smallest control API a host app needs: open the options window.
- Source files under 800 lines and checks runnable from `zig build`, so the code can move into the client's repository
  unchanged.

**Non-Goals:**
- No control requests beyond `options` (no resize, side or hide over the socket).
- No fallback window when layer-shell is missing, and no X11 support.
- No change to rendering, terminal emulation, fonts, keyboard handling or layout behaviour, apart
  from moving code between files.
- No tray artwork.

## Decisions

**D1. Control socket on the GTK main loop.** Use `GSocketService` (gio is already linked) bound to
`$XDG_RUNTIME_DIR/pinwin/<pid>.sock`. It starts in the activate path before `spawn_pty()`, so the
child can get `PINWIN_SOCKET` between `forkpty` and `execvp`, next to the existing `setenv("TERM",
...)`. When there is no socket, the child runs `unsetenv("PINWIN_SOCKET")` instead.

Each connection is read asynchronously, up to 256 bytes or the first `\n`. The request is parsed,
the reply written, and the connection closed. Parsing is a pure function in a new
`control.c`/`control.h` (`pinwin_control_parse(const char *, size_t)` returning an enum), so the
check program can cover it without GTK.

The socket file is unlinked from the exit path that already handles the child's exit
(`glue_exit`). Before binding, any existing file at its own path is unlinked: it can only be left
over from a killed instance whose PID was reused.

Alternatives considered:
- A D-Bus method on the SNI object. Rejected: the object doesn't exist under `--no-tray`, and the
  host would have to speak D-Bus.
- `SIGUSR1`. Rejected: no reply, no room for a second request, and a host's helper processes are
  not pinwin's parent.

**D2. Option parsing in `main.zig`.** `commandArgv` consumes leading pinwin options (`--no-tray`
and `--`) and rejects any other leading argument that starts with `--` (exit 2, matching
`envSetting`'s error style). The remaining arguments are the command. With no command it runs
`$SHELL`, or `/bin/sh` if `SHELL` is empty or unset. `--no-tray` reaches C as a new `glue_init`
parameter, which skips `tray_init()`.

**D3. Tray icon by theme name.** `tray.c` publishes `IconName` instead of pixmaps: `pinwin` when
the GTK icon theme has it, otherwise `utilities-terminal`. The `$HOME/pinwin.svg` loading, the
pixmap rasterising and `tray_argb_from_rgba` are deleted, together with their check cases.
Alternative: keep rasterising an installed SVG. Rejected: there is no artwork, and a name lets the
tray host theme the icon.

**D4. File splits along responsibilities.** These are pure moves, with no behaviour edits in the
same commit.
- `glue.c` becomes:
  - `glue.c`: app activation, layer-shell surfaces, layout application, init/start/exit.
  - `render.c`: cell metrics, text, sprite and nerd-font drawing.
  - `images.c`: kitty image surfaces and cache.
  - `pty.c`: spawn, read, write, resize, child exit, socket environment.
  - `input.c`: GDK controllers for key, mouse, scroll and focus.
  - `fontconfig.c`: Ghostty config, theme colours, terminfo check.
- State shared between these files goes into one internal header, `glue_internal.h`.
- `main.zig` becomes:
  - `main.zig`: entry, environment, argv, terminal callbacks.
  - `cells.zig`: frame and cell iteration, grapheme merging.
  - `input.zig`: key, mouse, scroll and focus encoding.

The target is under 800 lines per file, with natural seams over an even split.

**D5. `zig build check`.** `build.zig` gains a `check` step that compiles `tools/check_options.c`
with `options.c`, `tray.c` and `control.c` against the same system libraries, then runs it. The
README's `cc` recipe is replaced by `zig build check`. `tools/gen_nerd_tables.py` is deleted: it is
a one-off generator, and the generated header is the source of truth.

## Risks / Trade-offs

- [The split changes behaviour by accident] → the split is in its own commit as pure moves, and
  the manual check (task group 4) covers drawing, input, images, resize and options after it.
- [Bare `pinwin` no longer runs the client] → this is intended. The README and desktop-entry examples
  change to `pinwin <command>`.
- [A user's `$HOME/pinwin.svg` stops being used] → the README says to install an icon named
  `pinwin` into the icon theme instead.
- [A blocking request handler would freeze the panel] → all socket I/O uses gio async calls on the
  main loop, with no blocking reads.
