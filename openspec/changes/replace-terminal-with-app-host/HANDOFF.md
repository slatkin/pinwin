# Handoff: replace-terminal-with-app-host

Status on 2026-10-14: the change is scaffolded only. No proposal, specs,
design or tasks exist. This file records the decisions from the explore
session. Continue with `/opsx:explore` (scratchpad question below) or
`/opsx:propose`.

## Goal

pinwin stops being a terminal emulator. pinwin becomes a host that docks
any Wayland app as an edge panel. A terminal panel is a wrapped real
Ghostty. The reason: the current pinwin (about 35k lines) mostly
re-implements a terminal (libghostty, renderer, fonts, pty, kitty images).
The new pinwin keeps only the panel UX.

## Mechanism

A Wayland proxy, built on the `wl-proxy` Rust crate:

- Crate: https://crates.io/crates/wl-proxy (source:
  https://github.com/mahkoh/wl-proxy, MIT/Apache).
- Reference app: `wl-paper`. It is NOT its own repo. It is the folder
  `apps/wl-paper` in the wl-proxy repo:
  https://github.com/mahkoh/wl-proxy/tree/master/apps/wl-paper (GPL-3.0).
- The proxy gives the app its own `WAYLAND_DISPLAY` socket. It passes most
  messages through. It swallows the app's `xdg_toplevel` and creates a
  `zwlr_layer_surface_v1` upstream for the same `wl_surface`. It maps
  configure and ack serials both ways, sends `xdg_popup` through
  `layer_surface.get_popup` (with positioner offset correction), and forces
  server-side decorations.
- wl-paper code to copy: `apps/wl-paper/src/paper.rs` (602 lines). Toplevel
  swap at about lines 366-404, popups at 406-444, configure mapping at
  467-519, decoration override at 555-580.
- wl-paper anchors all four edges, sends `set_size(0,0)` and sets no
  exclusive zone. pinwin must instead anchor one edge plus top and bottom,
  set the width and set the exclusive zone (reserve).
- `wl-proxy/src/simple.rs` (`SimpleProxy::run`) shows the per-client model:
  one thread and one `State` (own upstream connection) per client. Objects
  are `Rc`, so they cannot leave their thread.

## Decisions (confirmed by the user)

1. pinwin hosts any app through the wl-proxy crate. Copy wl-paper code as
   needed.
2. License: pinwin moves from MIT (`Cargo.toml` line 7) to GPL-3.0. The
   user wants GPL-3.0 for all projects from now on.
3. One change. It adds the app host and removes the terminal requirements
   from `openspec/specs/pinwin-panel` in the same change. No staged
   transition: nothing uses pinwin between changes.
4. One daemon owns all panels. One wl-proxy thread per panel. The daemon
   sends commands to a panel thread over a channel plus a wake fd.
5. One tray icon (StatusNotifierItem over D-Bus, for example the `ksni`
   crate). The user's bar is Noctalia, which has an SNI tray. Hide the icon
   when no panels exist. The user hates tray clutter.
6. A full IPC, not only toggle/show. JSON lines on one daemon socket. The
   CLI, the tray menu, keybinds and scripts are all clients. Users with no
   bar use the IPC. Draft command set: `toggle`, `show`, `hide`,
   `set-edge`, `set-size`, `resize`, `set-output`, `set-layer`,
   `set-reserve`, `set-keyboard`, `focus`, `list-windows`, `set-policy`,
   `status`, `subscribe` (events), `quit`, `restart-app`, `reload`.
7. A config file (for example `~/.config/pinwin/config.toml`) with
   `[panel.defaults]` and per-name `[panel.<name>]` overrides. Precedence:
   launch flags, then per-name, then defaults. Runtime IPC changes do not
   write back to the file.
8. Daemon lifetime is a preference: `exit_when_idle` (true: exit with the
   last panel; false: stay resident).
9. Width is in pixels or percent. Cell-exact width goes away.
10. The width tween stays, as a proxy add-on. It reuses the viewport-crop
    method of `src/panel/wayland_side/crop.rs`. Grow: configure the app
    once to the target width, then crop the wide buffer each frame. Shrink:
    keep the old buffer, crop down each frame, configure the app to the
    target at the end. The app redraws once per tween. No change to
    wl-proxy is necessary.
11. Single-instance apps (Ghostty without `--gtk-single-instance=false`,
    Firefox, Chromium, Electron) can escape the proxy. This is the user's
    problem. Document it once, do nothing more.

## Proposed, not yet confirmed

- The caller runs the app: `pinwin run --name <n> -- <cmd>` asks the
  daemon for a panel socket, sets `WAYLAND_DISPLAY`, and runs the command
  itself. The app keeps the caller's working folder and environment.
- `pinwin run` starts the daemon if it is not running.
- Toplevel policy: the first toplevel goes into the panel; later toplevels
  (dialogs, file pickers) pass through as normal windows.
- Reuse of `src/instance.rs` (name newtype, socket bind, bounded waits):
  copy, cargo feature, or shared crate. The IPC wire format changes to
  JSON lines, so much of it changes anyway.

## Open questions

1. Umbriel scratchpad (the user wants to explore this next). The user runs
   the Umbriel compositor (docs: https://docs.noctalia.dev/umbriel/), not
   niri. Umbriel has scratchpad and pinned windows plus a local IPC. Find
   out: can a scratchpad or pinned window reserve an edge strip so tiled
   windows shrink, or does it only float on top? Can it animate? This
   decides how much of pinwin is still needed on Umbriel.
2. Umbriel support for layer-shell popups and exclusive zones with its
   scrolling layout is not verified. A live test with plain wl-paper on
   Umbriel answers it. The user runs all live tests.
3. Tween risks to design for: an app with its own `wp_viewport` (Firefox,
   Chromium) needs our crop combined with its source and destination;
   injected commits must not commit the app's half-finished pending state;
   with no viewporter the width snaps.
4. Daemon crash ends every wrapped app's Wayland connection. Do not use
   `panic = "abort"`; the daemon must survive bad input.

## Rules for the next agent

- Never touch the live session (compositor, running apps). The user runs
  every live test.
- pinwin is independent of its consumers. Do not design for mbv.
- Ask before writing any artifact beyond what the user asked for.
- Commits are the user's; never add a Claude co-author trailer.
