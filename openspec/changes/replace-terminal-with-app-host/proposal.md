# Proposal

## Why

pinwin has scope creep: most of its code re-implements a terminal emulator
(libghostty, renderer, fonts, pty, kitty images) to dock one terminal at a
screen edge. The useful part is the panel UX: edge placement, the reserved
strip, push or cover, gutters, the width tween and show/hide. The `wl-proxy`
crate makes it possible to give that UX to any Wayland app, so a terminal
panel can be a real Ghostty and pinwin can stop being a terminal.

## What Changes

- **BREAKING** pinwin no longer contains a terminal emulator. A terminal
  panel is a wrapped terminal app (for example Ghostty). The pty, renderer,
  font, kitty-image, pixel-snap and libghostty/Zig build requirements go
  away.
- **BREAKING** The `Panel` library API with its host-owned pty goes away.
  Programs control pinwin through its IPC.
- New: pinwin docks any Wayland app as an edge panel. A proxy built on the
  `wl-proxy` crate gives the app its own Wayland socket and turns the app's
  toplevel into a layer-shell surface. Code from `wl-paper` (in the
  wl-proxy repo) is copied for the toplevel swap, popups and decorations.
- New: one daemon owns all panels. `pinwin run` starts the daemon if it is
  not running.
- New: a full IPC (JSON lines on one daemon socket) for visibility,
  geometry, layer, reserve, keyboard mode, status and an event stream. The
  CLI, the tray and scripts are clients of the IPC.
- New: one tray icon (StatusNotifierItem) with a menu for each panel. The
  icon is hidden while no panels exist.
- New: a config file with panel defaults, per-name overrides and daemon
  preferences, including `exit_when_idle`.
- Changed: panel width is in pixels or a percent of the output, not in
  terminal columns.
- New: a panel can dock at any of the four edges (today only left and
  right).
- Kept: the optional focus ring, now drawn by pinwin over the hosted app.
- Kept: the width tween, done by the proxy with the same viewport-crop
  method as today, so the app redraws once per tween.
- **BREAKING** License changes from MIT to GPL-3.0.

## Capabilities

### New Capabilities

- `panel-control`: the daemon, its IPC (commands, replies, events), the
  `pinwin` CLI client, panel naming, the config file and preferences.
- `panel-tray`: the single StatusNotifierItem icon and its per-panel menu.

### Modified Capabilities

- `pinwin-panel`: changes from "dock a terminal" to "dock any Wayland app".
  Terminal, font, rendering, pty, library-API and build requirements are
  removed. Placement, reservation, push/cover, gutters, width tween and
  show/hide are rewritten for a hosted app with pixel widths. New
  requirements cover the xdg-to-layer swap, popups, decorations and which
  toplevels go into the panel.

## Impact

- Code: `src/term`, `src/render`, `src/pty`, `src/fontconfig`,
  `src/nerd_font.rs` and most of `src/panel` are removed. The tween crop
  plan (`src/panel/wayland_side/crop.rs`) and the instance-name rules
  (`src/instance.rs`) are reused or adapted.
- Dependencies: add `wl-proxy` and an SNI crate (for example `ksni`);
  remove libghostty-vt and the Zig build.
- Consumers: any program that used the `Panel` library moves to the IPC.
- Users: apps that hand launches to an already running instance (Ghostty
  without `--gtk-single-instance=false`, Firefox, Chromium, Electron) do
  not enter the panel. This is documented, and reported by the daemon
  when pinwin sees the app exit without a window.
- License: `Cargo.toml` and a new `LICENSE` file change to GPL-3.0.
