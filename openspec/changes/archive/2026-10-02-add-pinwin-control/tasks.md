# Tasks

## 1. Split oversized sources (pure moves)

- [x] 1.1 Split `pinwin/src/glue.c` into `glue.c`, `render.c`, `images.c`, `pty.c`, `input.c`, `fontconfig.c` plus `glue_internal.h` for shared state, as in design D4, with no behaviour edits; add the new files to `build.zig`. Verify: `zig build` succeeds with `-Wall` clean and every C file under `pinwin/src/` is under 800 lines (`wc -l`).
- [x] 1.2 Split `pinwin/src/main.zig` into `main.zig`, `cells.zig`, `input.zig` as in design D4, with no behaviour edits. Verify: `zig build` succeeds and every `.zig` file is under 800 lines.
- [x] 1.3 Manual check after the split: `pinwin <command>` draws text and posters (kitty graphics), takes keys after a click, reports mouse clicks, resizes on Apply, and exits with the client's status. Commit the split on its own.

## 2. Checks and tooling

- [x] 2.1 Add a `check` step to `build.zig` that compiles `tools/check_options.c` with `options.c` and `tray.c` against the system libraries and runs it. Verify: `zig build check` prints `check_options: all passed`.
- [x] 2.2 Delete `tools/gen_nerd_tables.py`, keeping `src/nerd_font_tables.h`. Replace the README's `cc` check recipe with `zig build check`. Verify: no reference to `gen_nerd_tables` or the `cc` recipe is left (`rg`).

## 3. Launch options and tray

- [x] 3.1 Rework `commandArgv` in `main.zig`: consume leading `--no-tray` and `--`; reject any other leading argument starting with `--` with exit 2 and an error naming it; default the command to `$SHELL`, or `/bin/sh` if `SHELL` is empty or unset. Verify by hand: `pinwin --sideways <command>` exits 2 naming `--sideways`; `SHELL=/bin/bash pinwin` runs bash; `pinwin -- --odd` tries to run `--odd`.
- [x] 3.2 Pass `--no-tray` through `glue_init` and skip `tray_init()` when set. Verify by hand: `pinwin --no-tray htop` shows no tray entry while a tray host runs, and plain `pinwin htop` still shows one.
- [x] 3.3 Publish the tray icon by theme name (`pinwin` if the GTK icon theme has it, else `utilities-terminal`). Delete the `$HOME/pinwin.svg` loading, pixmap rasterising, `tray_argb_from_rgba` and their check cases. Verify: `zig build check` passes, and the tray shows `utilities-terminal` when no `pinwin` icon is installed.
- [x] 3.4 Update the README: usage (`pinwin [--no-tray] [--] [command...]`, `$SHELL` default, desktop-entry example `pinwin <command>`), the tray icon via the theme, and Wayland layer-shell only. Verify that the documented commands match the behaviour from 3.1–3.3.

## 4. Control API

- [x] 4.1 Add `control.c`/`control.h` with `pinwin_control_parse` (recognises exactly `options`; anything else is unknown). Add check cases for `options`, an unknown request and an empty line to `tools/check_options.c`, and add `control.c` to the build and check step. Verify: `zig build check` passes.
- [x] 4.2 Start a `GSocketService` on `$XDG_RUNTIME_DIR/pinwin/<pid>.sock` before `spawn_pty()`: create the directory with mode 0700, unlink any stale file at that path, restrict the socket to the user, read asynchronously up to 256 bytes or `\n`, reply `ok` or `error unknown request`, then close; `options` calls `pinwin_options_open()`. With no `XDG_RUNTIME_DIR`, or if binding fails, print a diagnostic and continue without a socket. Verify by hand: `printf 'options\n' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/pinwin/<pid>.sock` prints `ok` and opens the window; sending `resize 60` prints `error unknown request`; sending 1000 bytes with no newline leaves the panel running.
- [x] 4.3 In the child, between `forkpty` and `execvp`, set `PINWIN_SOCKET` to the socket path, or unset it when there is no socket. Unlink the socket file from `glue_exit`. Verify by hand: `pinwin sh -c 'echo $PINWIN_SOCKET; sleep 5'` shows the path; the file is gone after exit; `XDG_RUNTIME_DIR= PINWIN_SOCKET=/x pinwin sh -c 'env | grep PINWIN'` prints nothing.
- [x] 4.4 Document the control API in the README (socket path, `PINWIN_SOCKET`, the `options` request and its replies, a `socat` example). Verify that the documented example works as written.

## 5. Integration check

- [x] 5.1 Run `pinwin --no-tray <command>` under niri: no pinwin tray entry; `options` sent over `PINWIN_SOCKET` opens the options window; Apply resizes the panel without restarting the client; quitting the client exits pinwin and removes the socket.
