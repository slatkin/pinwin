# Proposal

## Why

The panel in `on-demand` mode gets keyboard focus only from a click. niri has no action that focuses a layer surface. niri also ignores xdg-activation for layer surfaces. So a global hotkey cannot move focus to the panel (issue #15). The `exclusive` and `none` modes do not solve this.

## What Changes

- The library adds `Panel::request_focus()`. The host calls it from any thread, the same way as `apply_layout`. pinwin unmaps the panel surface and maps it again as `on-demand`. niri, like other compositors, gives focus to a newly mapped `on-demand` surface. Kitten panels, noctalia and launchers use the same behavior. The reserve surface stays mapped, so the gap does not move. The user releases focus in the normal `on-demand` way, for example with a click on another window.
- If the panel started with `none` or `exclusive`, `request_focus()` returns `Ok(())` and does nothing. After the panel ends, the call returns `NotRunning`. After a panic, it returns `Internal`.
- The library owns no transport. Each host chooses its own IPC and its own hotkey binding.
- The `pinwin` program is the reference host. It adds a command-line IPC over a Unix socket:
  - The host reads `PINWIN_NAME` (default `default`) and listens on `$XDG_RUNTIME_DIR/pinwin/$WAYLAND_DISPLAY-<name>.sock`.
  - `pinwin --focus [name]` connects, asks for focus, and exits 0. If no host answers, it exits 1 with a message.
  - A name contains only `[A-Za-z0-9_-]`, with at most 64 characters. A bad name exits 2, the same as the other environment errors.
  - If a live host already uses the name on the display, the second host exits 2 before it opens a surface. A stale socket file (connect fails) is removed and bound again. The host removes its socket on exit.
- Example niri bind: `Mod+P { spawn "pinwin" "--focus"; }`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: the `on-demand` keyboard requirement adds focus on request. The `Panel` handle requirement adds `request_focus()`. The standalone program requirement adds `PINWIN_NAME`, the socket listener and the `--focus` client.

## Impact

- `src/panel.rs`: the new handle method, posted to the GTK thread like `apply_layout`.
- `src/surfaces.rs`: the remap of the panel surface only.
- `src/main.rs`: `PINWIN_NAME` parsing, the `--focus` client mode, the socket listener on a scoped standard-library thread, and socket cleanup.
- No new dependencies. The socket uses the standard library.
- Risk: the remap can blink for one frame. The first task checks the remap on niri.
