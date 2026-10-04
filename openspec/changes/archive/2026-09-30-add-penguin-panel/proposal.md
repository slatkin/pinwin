# Proposal

## Why

`pinwin` fakes a docked client window: it opens a floating Ghostty window, rewrites the global niri `struts` in `~/.config/niri/woims/pin.kdl`, follows workspace switches over `niri msg --json event-stream`, and snaps the window back after drags. That approach leaves a gap on every output, needs `LEFT/RIGHT/TOP/BOTTOM` hand-synced with the user's layout, and leaves the gap behind after a `SIGKILL`. The wlr-layer-shell protocol gives a real dock: an exclusive zone on one output that the compositor releases when the surface goes away. libghostty-vt (Ghostty's terminal core, now with kitty graphics exposed) makes it practical to host the client in a small purpose-built terminal on such a surface.

## What Changes

- Add `penguin`, a new Zig program in `penguin/` (own `build.zig`), that opens a GTK4 layer-shell surface on the `top` layer, anchored to the left, top and bottom edges of the monitor niri has focused at startup.
- `penguin` reserves `panel width + GUTTER` pixels as its exclusive zone, so niri tiles windows to its right on that monitor only.
- `penguin` runs a command (default the client; arguments override, like `pinwin`) in a PTY, emulates the terminal with libghostty-vt, and draws text with Pango and kitty graphics images.
- `penguin` supports every terminal feature the client enables: alternate screen, truecolor and bold/italic/inverse, kitty keyboard protocol (disambiguate), SGR mouse capture, focus reporting, `CSI > 1 s` shift-escape, kitty graphics with query replies, `CSI 16 t` cell-size replies, and PTY size with pixel dimensions.
- Keyboard: `on-demand` layer-shell keyboard interactivity. Click `penguin` to type into it; click a niri window to leave.
- Settings: `COLS` (default 40) and `GUTTER` (default 12) environment variables, same as `pinwin`. No config file.
- `penguin` exits when its command exits; the compositor releases the reserved space. No files written, no niri config needed.
- `pinwin` (the bash script) is unchanged and remains supported.
- README gains a `penguin` section.

## Capabilities

### New Capabilities
- `penguin-panel`: docking a terminal running a command at the left edge of one monitor via layer-shell, its settings, keyboard/mouse focus behavior, the terminal features it guarantees, and its lifecycle.

### Modified Capabilities
None. `pinwin` has no specs and its behavior does not change.

## Impact

- New directory `penguin/` with Zig sources and `build.zig` / `build.zig.zon`.
- New build dependencies: Zig 0.16.x, libghostty-vt pinned to a specific ghostty-org/ghostty commit, GTK4, gtk4-layer-shell, Pango.
- Requires a compositor implementing wlr-layer-shell (niri does). Works on any such compositor; no niri config include.
- `pinwin`, `pin.kdl` and the niri `include` line are untouched.
- Out of scope for this change: a `penguin --focus` flag for keyboard focus without clicking, OSC 52 clipboard, a config file, following the focused monitor after startup.
