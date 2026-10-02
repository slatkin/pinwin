# Proposal

Issue: #1

## Why

mbv wants to run inside pinwin with a single tray icon (mbv's), and offer pinwin's options from
that tray (slatkin/mbv#864). Today the only way to reach the options window is pinwin's own tray
entry, so a host app cannot drive the panel. pinwin's code is also about to move into the mbv repo,
which caps source files at 800 lines and does not allow bespoke scripts.

## What Changes

- **Control API**: each pinwin instance listens on a Unix socket at
  `$XDG_RUNTIME_DIR/pinwin/<pid>.sock` and exports the path to the child as `PINWIN_SOCKET`. The
  protocol is one request line and one reply line. The only request is `options`, which opens the
  options window exactly as the tray's `Options...` does.
- **`--no-tray`**: pinwin does not register its own tray entry. The options window stays reachable
  through the control API.
- **Option parsing**: pinwin's own options come before the command, `--` ends them, and an unknown
  leading `--…` argument is an error.
- **Default command**: with no command, pinwin runs `$SHELL` (`/bin/sh` if unset) instead of `mbv`.
- **Tray icon from the icon theme**: `pinwin`, falling back to `utilities-terminal`, instead of
  `$HOME/pinwin.svg`.
- **Wayland-only** is written down as a requirement. The code already exits with a diagnostic when
  layer-shell is unavailable.
- **Ready for import into mbv**: split `glue.c` (1637 lines) and `main.zig` (952 lines) along
  responsibility seams to stay under 800 lines each; add a `zig build check` step that builds and
  runs the check program; delete `tools/gen_nerd_tables.py` and keep its generated header.

## Capabilities

### New Capabilities
- `pinwin-control`: the per-instance control socket, `PINWIN_SOCKET` in the child environment, and
  the line protocol (`options`).

### Modified Capabilities
- `pinwin-panel`: the launch requirement changes (options before the command, `--`, `$SHELL`
  default); the Wayland layer-shell requirement is added.
- `pinwin-tray-options`: the tray icon comes from the icon theme; `--no-tray` suppresses the tray;
  the options window can also be opened by a control request.

## Impact

- `pinwin/src/` (new `control.c`/`control.h`; `glue.c` and `main.zig` split into several files;
  `tray.c` icon), `pinwin/build.zig` (new sources, `check` step), `pinwin/tools/check_options.c`
  (control parsing cases), README.
- No new dependencies: the socket uses gio, which is already linked.
- `pinwin.sh` is unaffected.
- Behaviour change for anyone relying on bare `pinwin` running `mbv`: they must now run
  `pinwin mbv`.
