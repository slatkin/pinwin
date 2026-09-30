# Proposal

## Why

Penguin currently fixes its terminal to the left edge and accepts only a launch-time extra gutter. A tray entry lets the user adjust spacing and docking without restarting mbv, and remember those choices across launches.

## What Changes

- Show one system tray entry per running penguin instance using `$HOME/penguin.svg`; right-click exposes `Options...`.
- Open an options window containing top, bottom, left and right gutters in pixels, a Left/Right docking selector, and Apply and Close controls.
- Interpret gutters as literal screen directions: top/bottom inset the panel vertically; left/right surround the panel horizontally regardless of docking side.
- Apply validates, saves and updates both the visible panel and its reserved space without replacing the PTY or child process. Editing alone has no effect; closing discards unapplied edits.
- Persist layout settings in `$XDG_CONFIG_HOME/penguin/config`, defaulting to `~/.config/penguin/config`.
- Preserve existing launch behavior without a saved config: left docking, zero top/bottom/left gutters, and the existing `GUTTER` value as the right gutter. Keep `COLS`, font, keyboard and command settings unchanged.
- Do not make tray availability a requirement for running the terminal.

## Capabilities

### New Capabilities

- `penguin-tray-options`: tray access, staged layout editing, live left/right docking and four directional gutters, persistent settings and failure behavior.

### Modified Capabilities

None in the durable spec inventory (currently empty). The completed but unarchived `add-penguin-panel` change contains `penguin-panel` requirements for left-only, flush/full-height docking, environment-only settings and no disk writes. This change intentionally supersedes those restrictions for penguin layout settings; it does not edit that historical change. Its command lifecycle, terminal rendering and pinwin independence remain intact. Reconcile those restrictions when promoting both changes into durable specs.

## Impact

- C GTK integration in `penguin/src/glue.c`, with small focused C modules for settings/UI/tray as needed rather than adding unrelated code to the renderer.
- `penguin/src/penguin.h` and the `main.zig` startup boundary only if required for passing layout settings; retain the GTK-free Zig interface.
- `penguin/build.zig`: explicitly link GIO and the GTK-independent `dbusmenu-glib-0.4` library for a host-rendered tray menu; no GTK3 indicator library.
- README: layout semantics, precedence, persistence, tray-host requirement and installation dependency.
- Session D-Bus StatusNotifierItem registration and a per-user config file. No niri configuration, daemon, autostart, command restart, hide/show action or changes to `pinwin`.
