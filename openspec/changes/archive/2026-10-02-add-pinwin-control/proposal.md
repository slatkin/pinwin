# Proposal

Issue: #1

## Why

Today the only way to reach pinwin's options window is pinwin's own tray entry, so a host app
cannot drive a running panel and pinwin always registers a tray entry. pinwin needs a control
API that opens the options window on request, plus a way to run without pinwin's own tray entry.
The code also splits along responsibility seams to keep every source file under 800 lines and to
drop its bespoke table script.

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
- **Source split and checks**: split `glue.c` (1637 lines) and `main.zig` (952 lines) along
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
