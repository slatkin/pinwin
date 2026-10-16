# Design

## Context

See proposal.md for the reasons. The specs in this change define the behavior:
`specs/panel-control`, `specs/panel-tray` and the `specs/pinwin-panel` delta.

Current code that this design reuses or replaces:

- The panel thread (`src/panel/wayland_side/`) has two layer surfaces: the visible panel
  (exclusive zone -1, anchored to its edge and both ends) and a transparent "reserve" surface
  that carries the exclusive zone. The held-gap rule (cover keeps the last pushing strip) is
  in `src/surfaces/gap.rs`. The new design keeps the two-surface model.
- The width tween crops a wide buffer with `wp_viewport` each frame
  (`src/panel/wayland_side/crop.rs`) and steps with `src/anim.rs`. The new design keeps the
  method but applies it to the app's buffer.
- `src/instance.rs` has the name rules (1..=64 of `[A-Za-z0-9_-]`), the socket bind with
  stale-file removal, and bounded socket waits. The new daemon socket reuses these parts.
- `src/guard.rs` contains panics. `Cargo.toml` already sets `panic = "unwind"`.

wl-proxy facts that shape the design (checked in the wl-proxy source, version 0.1.5):

- A `State` holds one upstream compositor connection. It can serve several client
  connections (`State::add_client`, `State::create_acceptor`), and it exposes a `poll_fd`
  for an external poll loop. Objects are `Rc`, so one `State` stays on one thread.
- The proxy can send requests on objects the client created, create its own objects on the
  upstream connection, and send its own events to the client.
- `wl-paper` (`apps/wl-paper/src/paper.rs`, GPL-3.0) turns every `xdg_toplevel` into a layer
  surface, fixes popup positions, and forces server-side decorations. It anchors all four
  edges and sets no exclusive zone.

## Goals / Non-Goals

**Goals:**

- One process, the daemon, owns all panels on a display. A failure in one panel ends only
  that panel.
- The proxy changes only what it must: the panel window's role, popups of that window,
  decorations of that window, and the surface state the tween, the ring and hide need.
  Everything else passes through.
- Keep the pure geometry, gap and tween logic testable without a compositor, as today.

**Non-Goals:**

- No handling of apps that pass a launch to a copy that runs outside pinwin. The README
  documents it.
- No X11 apps. An app under Xwayland connects to the Xwayland server, not to the panel
  socket, so it does not enter a panel.
- No public library API. The crate becomes a binary only.
- No per-panel gutter commands in the IPC. Gutters come from options and the config file.

## Decisions

### D1. One `State` and one thread for each panel

Each panel has one thread with one wl-proxy `State`: one upstream connection, one acceptor
(the panel socket), and every client connection of the app. The thread polls the state's
`poll_fd`, an eventfd for daemon commands, and the app's pidfd (D3).

Why: the panel's window, its reserve surface, its ring and its outputs are then objects of
one connection on one thread. The tween can change the panel, the reserve and the crop in one
frame with no cross-thread work. The "window goes into an empty panel" rule needs no lock.

Alternative: `SimpleProxy`, one thread and one upstream connection per client connection.
Rejected: an app with two connections would have two upstream connections, and the slot rule
and the reserve would need shared state across threads.

Consequence: a protocol error that the compositor raises for one of the app's connections
ends the shared upstream connection, so the whole panel ends. That matches the spec ("that
app's connection closes"); the panel belongs to one app.

### D2. The caller runs the app; `pinwin run` execs

`pinwin run` connects to the daemon socket and sends `create` with the resolved name, the
run options and its own process ID. The daemon replies with the panel's `WAYLAND_DISPLAY`.
`pinwin run` sets it and calls `exec`. The daemon does not own the app process, so there is
no `restart-app`.

The `create` command is internal to `pinwin run`; it is not a listed IPC command for scripts.

### D3. Panel lifetime from a pidfd plus the connection count

The daemon opens a pidfd for the process ID that `pinwin run` sent. After `exec`, that is the
app's process. The panel ends when the pidfd reports exit and the state has no client
connection. This covers: the app exits; `exec` fails (pidfd exits, no connection); a launcher
that forks a child which keeps the connection; and an app that hands off to a running copy.

Alternative: end the panel when the last connection closes. Rejected: a panel whose app has
not connected yet would have no end, and an app that reconnects would lose its panel.

### D4. Window routing: the empty-slot rule

The proxy handles `xdg_wm_base.get_xdg_surface` for every client, as wl-paper does, and
holds the `xdg_surface` back from the compositor until the role is known.

- `get_toplevel` while the panel slot is empty: the slot takes this window. The proxy creates
  a `zwlr_layer_surface_v1` for the same `wl_surface` (wl-paper lines 366-404) with pinwin's
  anchors, margins, size, layer and keyboard mode, and keeps the `xdg_toplevel` local.
- `get_toplevel` while the slot is full: the proxy forwards the `xdg_surface` and the
  `xdg_toplevel` to the compositor unchanged. The window is a normal window.
- Destroy of the slot's `xdg_toplevel`: the proxy destroys the layer surface, empties the
  slot and releases the strip.

The layer surface's `configure` becomes an `xdg_toplevel.configure` with the tiled states
plus `xdg_surface.configure` with the same serial (wl-paper lines 483-520). The app's
`ack_configure` goes to the layer surface. The proxy acks configures that it does not forward
itself (D8).

### D5. Popups and decorations, per window

Popups use wl-paper's code (lines 406-451 and 527-546): a popup whose parent is the panel
window becomes a layer-surface popup with the anchor rectangle moved by the window geometry.
Popups of passed-through windows are forwarded unchanged.

Decorations differ from wl-paper. pinwin keeps the compositor's real decoration globals and
handles each decoration object by its toplevel:

- The panel window: the proxy answers locally with server-side mode, so the app draws no
  title bar, and the compositor draws none on a layer surface.
- A passed-through window: the proxy forwards the decoration object to the compositor.

If the compositor has no `zxdg_decoration_manager_v1`, the proxy offers a synthetic one and
answers client-side for passed-through windows, which is what the app does without the
global.

Edge case: an app can create the decoration object before the window enters or leaves the
slot. The proxy routes by the toplevel's state at the time of the request and, for the panel
window, answers again on each `set_mode`.

### D6. Hide keeps the layer surface and unmaps it

Hide commits a null buffer on the panel surface and moves the reserve's zone to 0. While
hidden, the proxy forwards the app's commits without their `attach`, and sends the app a
`wl_buffer.release` for each buffer it removed, so the app does not run out of buffers. Frame
callbacks then do not fire, so the app stops drawing, as a hidden window does.

Show commits the surface with no buffer. The compositor sends a new configure, which the
proxy forwards with the same size. The app draws and the surface maps. The reserve gets its
zone back in the same frame.

Alternative: destroy the layer surface and create a new one on show. Rejected: it needs a
role change on the app's surface, and compositors differ on reusing a role.

### D7. Proxy commits: piggyback or inject

The tween, the ring position, hide and show change state on the app's surface, and a
`wl_surface.commit` applies all pending state. The proxy must not commit state that the app
has not finished.

Rule: the proxy marks the app's surface as dirty on any request on it after the app's last
commit. When the proxy needs a commit:

- surface clean: the proxy sends its requests and its own commit;
- surface dirty: the proxy queues its requests and sends them just before it forwards the
  app's next commit.

The pure part (the dirty flag and the queue) is unit tested.

### D8. Tween on the app's buffer

The tween keeps today's method and the `anim.rs` step:

- Grow: the proxy configures the app once to the target width. It holds the current visible
  width until the app commits a buffer for that configure (bounded at 500 ms; then it
  snaps). Then each frame crops the docked-edge part of the wide buffer.
- Shrink: the proxy keeps the app's current buffer and crops it down each frame. At the end it
  configures the app to the target width.
- Each frame sets the layer surface size, the margins, the reserve zone (when the gap moves),
  and the viewport source and destination, then commits (D7).
- During a tween the proxy acks the compositor's intermediate configures itself and does not
  forward them, so the app gets one new size for the whole tween.

Viewport ownership: only one `wp_viewport` can exist for a surface. The proxy owns the
upstream viewport of the panel surface. If the app creates one, the proxy keeps it local and
stores its source and destination. Each frame, the proxy composes the crop with the app's
values: the visible fraction of the destination width maps to the same fraction of the
source width, taken from the docked edge. Without an app viewport, the source is the
surface's own size, which is the configured size.

No `wp_viewporter`: the change snaps (spec).

`crop.rs` keeps its exact-rational rounding but loses the grid painter parts (`draw_wide`,
`upload_wide`, `copy_crop`): the app draws the wide buffer, and there is no no-viewporter
copy.

### D9. Geometry in pixels for four edges

`src/layout.rs` and `src/surfaces/gap.rs` become one pure geometry module: an `Edge`
(left, right, top, bottom), a resolved width in logical pixels, four gutters and a reserve
mode. It computes the anchors, the margins, the layer size, the reserve zone and the held gap,
and it validates (spec "Validate a change before applying it"). Percent sizes resolve against
the output's logical size, which the proxy reads from `zxdg_output_v1` (or `wl_output` mode and
scale when it is missing) on the panel's upstream connection.

### D10. Focus ring as four subsurfaces

The ring is four `wl_subsurface`s of the app's surface, created by the proxy, placed above the
app, each with an `wl_shm` buffer of the ring colour and an empty input region, so clicks go to
the app. They are desynchronized, so showing or hiding the ring needs no commit on the app's
surface. Their positions change only with the panel size, which already needs a commit (D7).
The proxy sees `wl_keyboard.enter` and `leave` for the panel surface on its way to the app,
and shows the ring on enter.

Alternative: one subsurface with a frame-shaped buffer. Rejected: a buffer the size of the
panel for a 1 pixel line.

### D11. Output changes

The layer surface is created with no output unless `output` is set, so the compositor
chooses. `set-output` destroys and creates the layer surface on the new output; the app gets
a configure, and a new size only when a percent size resolves differently. When the
compositor sends `closed` (for example, the output was removed), the proxy creates a new layer
surface with no output instead of forwarding a close to the app.

### D12. Daemon structure

- Main thread: accepts IPC connections on the daemon socket and handles SIGINT and SIGTERM
  with a signalfd.
- One thread for each IPC connection, with bounded reads and writes, as in `instance.rs`.
  A request line is limited to 64 KiB.
- One thread for each panel (D1). The daemon sends a command on a channel, wakes the thread
  with an eventfd, and waits for the reply with a bounded wait; no reply gives error `internal`.
- One tray thread (D14).
- A registry `Mutex<BTreeMap<PanelName, PanelHandle>>` in start order, and an event bus: each
  subscriber has a bounded queue, and a full queue closes that subscriber.

`pinwin run` starts a daemon with `pinwin daemon` in a new session (`setsid`), with stdin and
stdout on `/dev/null`, and waits up to 2 s for the socket to answer. Two daemons that start at
the same time race on the socket bind; the loser sees a live daemon and exits 0, and both
`pinwin run` calls connect to the winner.

The daemon checks layer-shell once at start with a short upstream connection, so it can
exit 1 before any panel exists.

### D13. Wire format and config with serde

IPC: JSON lines with `serde` and `serde_json`. Requests are a tagged enum on `cmd`. Replies
are `{"ok":true,...}` or `{"ok":false,"error":{"kind","message"}}`. Error kinds:
`invalid_request`, `invalid_value`, `invalid_layout`, `no_panel`, `duplicate_name`,
`invalid_config`, `internal`.

Config: TOML with `serde` and `toml`, with `deny_unknown_fields`, so a typo is an error with a
line. The CLI parser stays hand-written, as `src/cli.rs` is today.

### D14. Tray with ksni, created and dropped with panels

The tray uses `ksni` with the `blocking` feature and `async-io` instead of tokio. The daemon
starts the tray service when the first panel appears and shuts it down when the last panel
ends, so no icon shows without panels. ksni re-registers when a tray host appears later, and
a missing session bus is logged and ignored. Menu actions call the same command functions as
the IPC.

### D15. Crate layout after the change

```
src/main.rs       subcommand dispatch
src/cli.rs        argument parsing (hand-written)
src/ipc.rs        wire types, client call
src/daemon.rs     registry, listener, events, lifetime, spawn
src/config.rs     file, defaults, precedence
src/geometry.rs   edges, sizes, gutters, gap, validation (pure)
src/anim.rs       tween step (kept)
src/proxy.rs      panel thread: State loop, routing, popups, decorations
src/proxy/        tween.rs, crop.rs, ring.rs, reserve.rs, commit.rs, output.rs
src/tray.rs       ksni tray
src/name.rs       panel name rules (from instance.rs)
src/guard.rs      panic guard (kept)
```

Removed: `src/term*`, `src/render*`, `src/pty*`, `src/fontconfig*`, `src/nerd_font.rs`,
`src/panel*`, `src/surfaces*`, `src/layout.rs`, `src/settings.rs`, `src/instance*`,
`src/lib.rs`, `examples/` and `.cargo/config.toml` (Zig settings).

Dependencies removed: `libghostty-vt`, `smithay-client-toolkit`, `wayland-client`,
`wayland-protocols`, `calloop`, `calloop-wayland-source`, `xkbcommon`, `tiny-skia`, `swash`,
`fontconfig`, `yeslogic-fontconfig-sys`, `png`, `num-traits`. Added: `wl-proxy` (features
`suite-wayland-protocols`, `suite-wlr-protocols`), `ksni`, `serde`, `serde_json`, `toml`.
`libc` stays (pidfd, eventfd, signalfd, `setsid`, `exec`).

## Risks / Trade-offs

- [The swap from window to layer surface fails on the user's compositor: popups, exclusive
  zones, or the on-demand keyboard] → Task group 1 is a spike that the user tests live before
  the main work starts.
- [An app sends surface state through an extension that the dirty flag does not see, and an
  injected commit applies it early (D7)] → The flag covers all `wl_surface` requests and the
  intercepted viewport; the spike tests the tween with a GTK app and a browser.
- [An app ignores the configured size (D8 assumes the surface size equals it)] → The tween
  crops the configured width; a wrong-size app shows a wrong crop only during a tween. Risk
  accepted; the final state is correct.
- [A hidden app keeps a buffer that the proxy released early (D6)] → The proxy releases only
  buffers that it removed from a commit; the compositor never saw them.
- [One compositor error ends all connections of a panel (D1)] → Accepted; all of them belong
  to one app.
- [ksni or zbus pulls in GLib, against "No toolkit libraries"] → A task checks the linked
  libraries with `ldd`.
- [Daemon crash ends every app] → `panic = "unwind"` stays; each panel thread and each IPC
  thread runs under the panic guard.

## Migration Plan

This is a breaking release. Nothing uses pinwin between this change and the release,
so there is no transition. Rollback is the previous release. The README gets a migration
table: `pinwin cmd` to `pinwin run -- ghostty -e cmd`, `COLS` to `--size`, `GUTTER` to the
gutter settings, `PINWIN_*` to options, `pinwin --toggle name` to `pinwin toggle name`.

## Open Questions

- The default icon for the tray. A themed icon name is enough for the first release.
