# Design

## Context

See proposal.md for the motivation. The specs are in `specs/pinwin-panel/spec.md`.

Two niri facts shape the approach. Both come from niri's source.

- `update_keyboard_focus` gives an `Exclusive` surface focus because of its mode, and niri computes this again on each update. An `OnDemand` surface gets focus only from `layer_shell_on_demand_focus`. niri sets that field after a click on the surface, or after the first map of an `OnDemand` surface (`handlers/layer_shell.rs`). An exclusive grab never sets it.
- niri handles xdg-activation only for windows in its layout, and niri IPC has no action that focuses a layer surface.

Kitten panels (`--toggle-visibility`) and noctalia panels on niri get focus from the same first-map rule.

pinwin has two layer surfaces. The panel window holds the terminal. A separate reserve window (`pinwin-reserve`) holds the exclusive zone. `Surfaces::on_map` in `src/surfaces.rs` runs on each map of the panel window. It sets `latch`, creates the reserve window and connects the scale notify handler. `latch` means "first-draw monitor resolution pending". `resolve_monitor` clears it on the first draw and then fires the start handshake hook (`start_result`). Today, the panel window maps only once.

The library's GTK side runs on its own thread. `Panel::apply_layout` posts a closure to the GTK main context and waits for a bounded reply. In the binary, the main thread blocks in `wait_for_child` until the child exits.

## Goals / Non-Goals

**Goals:**

- A focus request that works on niri, with no change to the keyboard mode and no exit key.
- The library stays free of any transport, so each host chooses its own IPC.

**Non-Goals:**

- Focus on compositors that do not focus a newly mapped `OnDemand` surface.
- More IPC commands than focus. The protocol leaves room for them, but this change adds none.
- Focus release on request. The normal `on-demand` rules cover release.

## Decisions

### Remap the panel window as `OnDemand`

The library hides the panel window and presents it again. The compositor sees an unmap and a new map, and it focuses the surface.

Alternatives:

- Switch to `Exclusive`, then back to `OnDemand` after focus arrives. This fails on niri, because the switch back loses focus (see Context).
- Stay `Exclusive` until a release event. Escape breaks terminal programs such as vim and fzf. A toggle needs a second key press, and a click on another window does not release an `Exclusive` surface on niri.
- `hyprland_focus_grab_v1`, which noctalia uses. niri does not implement it.

### Make the map work run once

`on_map` creates a reserve window and connects a scale handler. A remap must not create a second reserve window or a second handler. `latch` cannot record the first map, because `resolve_monitor` clears it after the first draw. The reserve window records it: if `self.reserve` is already `Some`, `on_map` returns early, before it sets `latch`. So a remap does not run `resolve_monitor` again, and the start handshake hook does not fire a second time. The reserve window stays mapped during the remap, so the exclusive zone, and the tiled windows, do not change.

### `Panel::request_focus` follows the `apply_layout` pattern

The method posts a closure to the GTK thread and waits for a bounded reply. It shares the poisoned check, the `NotRunning` check and the `Internal` timeout with `apply_via_inner`. The closure reads the startup keyboard mode. For `none` and `exclusive` it does nothing. For `on-demand` it remaps. A new error variant for the other modes was rejected, because the host chose the mode itself.

### The binary uses a Unix socket per name

The socket path is `$XDG_RUNTIME_DIR/pinwin/$WAYLAND_DISPLAY-<name>.sock`. The directory gets mode 0700. A socket gives the client a reply. If no host answers, `pinwin --focus` can exit 1. A signal gives no reply and reaches every instance.

The protocol is one line each way. The client writes `focus\n`. The host replies `ok\n`. If `request_focus` fails, the host replies `error\n`. The client waits a bounded time for the reply.

Startup order in the host:

1. Parse `PINWIN_NAME` and make sure that it is valid, with the other settings, before the fork.
2. Try to connect to the path. If the connect succeeds, a live host uses the name, so exit 2.
3. If the connect fails, remove any stale file and bind.
4. Start the panel, then start the listener.

The listener is a standard-library thread that calls `request_focus` through a shared reference. The main thread keeps `wait_for_child`. `std::thread::scope` keeps the borrow of the `Panel` valid. The main thread removes the socket file after the child exits, and that also ends the accept loop.

The name check allows `[A-Za-z0-9_-]` and 1..=64 characters. This keeps the path safe and inside the 108-byte limit of a socket address.

Alternatives:

- A signal. It gives no reply and no name.
- D-Bus. It adds a dependency and a bus for one command.

## Risks / Trade-offs

- [The remap blinks for one frame] → The terminal state and the render cache survive the hide. The first task checks the result on niri. If the blink is visible, the task records it and the user decides.
- [Other compositors can ignore the remap] → The non-goal says so. The call still returns `Ok`, because the library cannot see the compositor's choice.
- [A connect-then-bind race between two hosts with one name] → Two hosts that start in the same instant can both pass the connect check. The second bind then fails, and that host exits 2. So the bind error is the final check.
- [SIGKILL leaves a socket file] → The next host finds that the connect fails and removes the file.

## Open Questions

None.
