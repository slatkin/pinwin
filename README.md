# pinwin

A layer-shell terminal panel for Wayland compositors: it docks a terminal
running any command to a screen edge as a layer surface, reserves space from
tiled windows via a transparent reservation surface, and animates its width
between layouts. Built for niri, on a direct Wayland client with no toolkit.

pinwin is a single Rust crate: `src/main.rs` is the `pinwin` binary and
`src/lib.rs` is a library you can depend on. The pre-port C and Zig sources
are removed with the port.

## Install

Building needs Zig 0.16 on PATH: `build.rs` fetches the pinned libghostty-vt
commit and builds ghostty's own static VT library with `zig build` (a cold
cache also needs `git` and network access). The system needs libxkbcommon
and fontconfig, and running needs a Wayland compositor with wlr-layer-shell.
Fractional output scale comes from the fractional-scale protocol; without it
the panel uses the integer buffer scale. A width tween crops its cached
buffer through viewporter, and copies the crop into a fresh buffer when the
compositor lacks it. Set
`PINWIN_GHOSTTY_SRC=<dir>`
to build against an existing ghostty checkout at the pinned commit instead of
fetching; a checkout at any other commit is rejected.

```sh
cargo install --path .          # installs the `pinwin` binary
cargo build --release           # or run target/release/pinwin
cargo test                      # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo build --examples          # build the dev-only demo example
```

`cargo run --example demo` builds and runs the demo, a dev-only driver that
creates its own pty pair and never gets installed. Its keys, typed in the
terminal that launched the demo:

- `<enter>` toggles the side and width and applies live (resize/re-dock)
- `e` animated width toggle 40 <-> 120 columns, same side (200 ms); press it
  again mid-tween to interrupt the animation
- `p` the same toggle through the plain snap apply
- `c` animated cover toggle: pushing 40 columns vs covering 120 on the same
  side, so the reservation holds and the tiles stay put through the
  excursion
- `t` the `Panel::toggle` hide/show: hiding unmaps the panel and releases
  the reservation, showing draws the grid again; any layout key applied
  while hidden is stored and comes back at the show
- `i` vertical-inset toggle: top and bottom gutters 0 <-> 40 px
- `g` right-gutter toggle: 12 <-> -24 px, so the panel's edge moves past
  the output edge
- `b` applies a rejected layout and expects `InvalidLayout`
- `q` stops the panel and exits

`DEMO_KEYBOARD=on-demand|exclusive|none` fixes the keyboard mode at start
(default `on-demand`, so the launching terminal keeps its stdin; `t` toggles
in every mode); `DEMO_ZONE=reserve|overlay` mirrors `PINWIN_ZONE`, the
push/cover class the start applies; `DEMO_ACCENT=on|off` mirrors
`PINWIN_ACCENT`; `DEMO_DENSE=1` swaps the shell child for a dense stand-in
with a full grid, kitty images and resize traffic to test animation against.
The environment reaches the library as it does for the host, so
`PINWIN_FRAMELOG=1` records one summary line per tween.

## Use as a Cargo dependency

Depend on the checkout:

```toml
[dependencies]
pinwin = { path = "../pinwin" }
```

The library exposes a `Panel` handle. `Panel::start` takes a host-owned pty
master fd, a full `Layout` and a `Keyboard` mode, plus an optional `Accent`,
and returns once the panel is on screen or has failed. Layouts push by
default; `Layout::covering` opts a layout into covering the tiled windows
while the reservation stays where the last pushing layout put it.
`apply_layout` and `apply_layout_animated` are methods on the handle;
`apply_layout_animated` clamps its duration to 1000 ms. `toggle` hides a
shown panel and shows a hidden one; hiding unmaps the panel and releases
its reservation, and the terminal and the child keep running while it is
hidden. `show` shows a hidden panel and leaves a shown one unchanged; on
a shown panel it returns `Ok` and changes nothing on screen.
Dropping the handle closes the panel and cancels any running
animation. The library never closes the fd and never exits the host.

```rust
use std::num::NonZeroU16;
use std::os::fd::RawFd;

use pinwin::layout::{Accent, Keyboard, Layout, Side};
use pinwin::panel::Startup;
use pinwin::{Panel, PinwinError};

fn run(fd: RawFd) -> Result<(), PinwinError> {
    let layout = Layout::new(
        Side::Left,
        NonZeroU16::new(60).expect("60 columns is non-zero"),
        0, 0, 0, 12,
    );
    let startup = Startup::new(
        fd,
        layout,
        Keyboard::OnDemand,
        Some(Accent::new([0xda, 0xbc, 0x7f], NonZeroU16::new(2).unwrap())),
    );

    let panel = Panel::start(startup)?;

    // Re-dock right; then widen from 60 to 120 columns over 200 ms.
    panel.apply_layout(Layout::new(Side::Right, NonZeroU16::new(60).unwrap(), 0, 0, 0, 8))?;
    panel.apply_layout_animated(
        Layout::new(Side::Right, NonZeroU16::new(120).unwrap(), 0, 0, 0, 8),
        200,
    )?;

    drop(panel); // stops the panel
    Ok(())
}
```

A host that wants `pinwin --toggle [name]` and `pinwin --show [name]` to
reach its panel opts into the instance socket: bind an `InstanceSocket`
for a name, then hand it to the start with `Startup::with_instance`. The
bind needs no panel, so a duplicate name fails with the typed `Duplicate`
error before any surface opens, and a socket file left by a killed
instance does not block a new bind. A start without a socket listens on
nothing. A panel started with a socket answers `toggle` and `show`
requests until the handle drops; dropping it removes the socket file, so
the name is free at once. From another process, `instance::send` sends
one `Request` to the named instance on this display; its `SendError`
tells a missing runtime directory or display apart from no listening
instance, a refused request, and a missing answer.

```rust
use std::num::NonZeroU16;
use std::os::fd::RawFd;

use pinwin::Panel;
use pinwin::instance::{InstanceName, InstanceSocket, Request};
use pinwin::layout::{Keyboard, Layout, Side};
use pinwin::panel::Startup;

fn run(fd: RawFd) -> Result<(), Box<dyn std::error::Error>> {
    let name = InstanceName::parse("notes")?;
    // Bind first, before any surface opens: a live owner of the name fails
    // here with the typed duplicate error.
    let socket = InstanceSocket::bind(&name)?;
    let layout = Layout::new(
        Side::Left,
        NonZeroU16::new(60).expect("60 columns is non-zero"),
        0,
        0,
        0,
        12,
    );
    let startup = Startup::new(fd, layout, Keyboard::OnDemand, None).with_instance(socket);
    let panel = Panel::start(startup)?;

    // Show a hidden panel; a shown one stays as it is.
    panel.show()?;
    // The same request from another process is one client call.
    pinwin::instance::send(&name, Request::Show)?;

    drop(panel); // removes the socket file; the name is free at once
    Ok(())
}
```

`PinwinError` is `#[non_exhaustive]` and implements `std::error::Error`:

- `InvalidLayout` — the monitor cannot hold the layout (overflow, a
  negative or too-wide pushing reservation, a covering panel wider than the
  output, no complete row). The applied layout is unchanged.
- `InvalidFd` — the pty fd is not an open descriptor; nothing opened.
- `NoDisplay` — no Wayland display, or the compositor lacks wlr-layer-shell.
- `AlreadyRunning` — at most one panel exists per process.
- `NotRunning` — the handle's panel is no longer live.
- `Internal` — a caught panic, a wedged panel thread, or a terminal grid that
  could not be allocated.

The layout types keep their invariants in their fields, so an unknown side, a
zero or oversized column count, a non-positive cell/output size and an accent
width outside 1..=65535 cannot be expressed (pass `None` for "no accent").

## Usage

```text
pinwin [--] [command...]    # default command: $SHELL, else /bin/sh; needs niri
pinwin --toggle [name]      # hide or show the named running panel, then exit
```

The binary owns the pty and the child: it sets `TERM=xterm-256color` and
`COLORTERM=truecolor` for the child, forwards SIGINT and SIGTERM to it as
SIGHUP, and exits with the child's exit status (1 if the child did not exit
normally). `--` ends option parsing; an unknown `--option` exits 2 with a
message before any surface opens.

Environment:

| Variable | Values | Default |
| --- | --- | --- |
| `COLS` | 1..=65535 | 40 |
| `GUTTER` | 0..=65535 (the right gutter) | 0 |
| `PINWIN_KEYBOARD` | `on-demand`, `exclusive`, `none` | `on-demand` |
| `PINWIN_ZONE` | `reserve`, `overlay` | `reserve` |
| `PINWIN_ACCENT` | `on`, `off` | `on` |
| `PINWIN_ACCENT_COLOR` | `#RRGGBB` or `RRGGBB` | `#dabc7f` |
| `PINWIN_ACCENT_WIDTH` | 1..=65535 (read only when the accent is on) | 1 |
| `PINWIN_NAME` | 1..=64 characters from `A-Za-z0-9_-` | `default` |

An invalid value exits 2 with a message on stderr before any surface opens.
Only the binary reads these; the library reads no pinwin-owned configuration.

## Toggle request

Every instance reads `PINWIN_NAME`. The default name is `default`. A name has
1..=64 characters from `A-Za-z0-9_-`. While its command runs, the instance
listens on a Unix socket under `$XDG_RUNTIME_DIR/pinwin/`, named for the
Wayland display and the name. A second instance with a name that a live
instance already uses prints a message, exits 2 and opens nothing. A socket
file left by a killed instance does not block a new host.

`pinwin --toggle [name]` asks the instance with that name to hide its shown
panel or show its hidden one, then exits. `pinwin --show [name]` asks it to
show its hidden panel and leaves a shown one unchanged, then exits. Both
reach any panel bound to that name, whether the `pinwin` program or a
library host started it. The name defaults to `default`.
The client exits 0 when the named instance accepts the request. With no
instance on that name it prints a message and exits 1. A bad name prints a
message and exits 2 in both places: in `PINWIN_NAME`, where the host opens
nothing, and after `--toggle` or `--show`. The client reads no environment besides the
display.

`PINWIN_ZONE` chooses what a toggle moves. With `reserve` (the default) the
panel starts with a pushing layout and reserves its strip, so tiled windows
sit beside it and take the strip back while it is hidden. With `overlay` it
starts with a covering layout, reserves nothing, draws over the tiles and
moves no window on a toggle; `overlay` is kitty's `--exclusive-zone=0`.

Showing the panel maps a new layer surface, so focus on show depends on the
compositor focusing a newly mapped surface. Niri does this in `on-demand`
and `exclusive` mode; in `none` mode the panel only appears. Clicking a
shown panel also gives it focus in `on-demand` mode.

In niri, bind a key to the client:

```kdl
Mod+P { spawn "pinwin" "--toggle"; }
```

## Migrating from `request_focus`

Version 0.2.0 replaces GTK with a direct Wayland client. A host program that
used 0.1.0 changes these things:

- **`Panel::request_focus` is gone; call `Panel::toggle` or `Panel::show`.**
  A toggle hides a shown panel and shows a hidden one; a show shows a hidden
  panel and leaves a shown one unchanged. Focus is the effect of showing:
  niri focuses a newly mapped `on-demand` or `exclusive` layer surface. The
  calls are not focus requests, so a host that needs "focus the panel, never
  hide it" calls `Panel::show`. The panel is shown at start.
- **`pinwin --focus [name]` is gone; run `pinwin --toggle [name]`.** It uses
  the same socket and the same `PINWIN_NAME`. Rebind the niri key:
  `Mod+P { spawn "pinwin" "--toggle"; }`. Hosts that send the request
  themselves send the line `toggle\n` to the instance socket instead of
  `focus\n`.
- **Hiding no longer resizes anything.** The terminal grid and the pty window
  size stay as they were, and the child gets no `SIGWINCH`. The child keeps
  running while the panel is hidden. This is the fix for the flash of a
  different layout (issue #19).
- **Hiding releases the reserved strip and showing restores it.** To keep
  tiled windows still on a toggle, start with `PINWIN_ZONE=overlay` (binary)
  or a `Layout::covering` layout (library). With `overlay` the panel draws
  over the tiles and a toggle moves no window.
- **Apply while hidden is stored.** `apply_layout` and `apply_layout_animated`
  validate the layout, store it with no animation and return; the next show
  uses it.
- **`gtk-enable-animations` is no longer read.** To apply a layout without
  animation, pass a duration of 0 to `apply_layout_animated`, or call
  `apply_layout`.
- **No GTK at run time.** The library and the binary link no GTK, GLib,
  Pango, Cairo or gdk-pixbuf library; the `pinwin` binary links 13 shared
  libraries in place of 113. The system needs libwayland, libxkbcommon and
  fontconfig.
- **`Startup` is built with `Startup::new(fd, layout, keyboard, accent)`.** Its
  fields are private; read them through the accessors of the same names.
- **Unchanged:** `Panel::start`, `Layout`, `Keyboard`, `Accent`,
  `PinwinError` and the `COLS`, `GUTTER`, `PINWIN_KEYBOARD`, `PINWIN_ACCENT*`
  and `PINWIN_NAME` variables.

## Migrating to 0.3.0

Version 0.3.0 moves the instance socket into the library and breaks one
host type:

- **`Startup` is no longer `Copy` — nor `Clone`.** It can own a bound
  instance socket, an owned fd with one owner, so the startup moves into
  `Panel::start`. Hosts that copied or cloned it keep one value and move it.
- **The instance socket is opt-in and new.** Bind it with
  `InstanceSocket::bind(&name)` and hand it to the start with
  `Startup::with_instance`; a start without a socket listens on nothing, as
  before. A panel started with a socket answers `toggle` and `show` on it
  until the handle drops, so `pinwin --toggle [name]` and
  `pinwin --show [name]` reach library-hosted panels. `Panel::show` and
  `instance::send` are new with the socket: see "Use as a Cargo dependency"
  for the shape.

## Behaviour

- The panel is a layer-shell surface on the `overlay` layer, flush against the
  top and bottom edges of its monitor and spanning its full height. It docks
  left or right and stays on its original monitor across workspace switches,
  focus moves and side changes.
- A pushing layout reserves a strip at its docking edge of
  `left + panel width + right` pixels, so the compositor tiles windows
  beside it. Gutters are directional and may be negative; the reservation
  sum may not be, and must leave some output width for other windows.
- Each layout is either pushing or covering: `Layout::new` pushes and
  `Layout::covering` opts in, per size. The reservation rules for both
  choices — where the strip sits, what a side switch does — are stated once
  on `pinwin::layout::Coverage` in the library docs.
- Keyboard interactivity is fixed at start: `on-demand` (the default) takes
  focus after a click, or when a toggle shows it on a compositor that
  focuses a newly mapped surface; `exclusive` takes it immediately, `none`
  never does. While focused, the panel draws its own focus accent in the
  configured colour and width, because the compositor draws no focus ring on
  layer surfaces.
- Applying a layout updates the columns, all four gutters, the docking side
  and the push/cover choice as one operation, follows the reservation rules
  above, and resizes the existing terminal grid and pty winsize without
  recreating the terminal or touching the child. An animated apply changes
  the width continuously when only the column count or the push/cover choice
  differs; a side switch or a left or right gutter change never animates.
  A duration of 0 is the only user-controlled way to snap an animateable
  apply — the desktop animation setting has no effect, because the library
  reads no desktop setting — and an apply also snaps when the animation
  cannot present its frames. During a covering animation the
  reservation holds still while the panel width tweens.
- The font and theme follow the user's Ghostty config (font family and size,
  default background/foreground); with no config the panel falls back to
  `monospace 11`. A missing or unreadable config never prevents opening.
- The terminal supports the kitty keyboard protocol, SGR mouse reporting,
  focus in/out reports, 24-bit/256-colour text and the kitty graphics protocol,
  including answering the graphics query and drawing transmitted images.
- The library raises `SIGWINCH` in the host process after every successful
  winsize update and never terminates the host: bad arguments, bad layouts,
  a missing display, a missing config and panics inside the library are error
  values or degraded states, never `exit()` or an unwinding panic.

## Architecture

`src/lib.rs` re-exports the public `Panel` API. `src/panel.rs` and
`src/panel/` own the public handle, the start handshake and the apply
replies, and `src/panel/wayland_side/` runs the panel thread: one
smithay-client-toolkit connection with a calloop event loop per start, the
two layer-shell surfaces, the shared-memory buffers, the seat, the width
tween and the frame present step. `src/layout.rs` is the display-free
geometry core; `src/term.rs` and `src/term/` wrap the pinned libghostty-vt
terminal and its cells, keys and input encoders; `src/render/` is the
toolkit-free CPU painter — tiny-skia fills the canvas, swash shapes and
rasterizes the text, fontconfig resolves the fonts, and the kitty image pass
decodes PNGs; `src/surfaces.rs` and `src/surfaces/` hold the pure
layout-validation and held-gap rules that the panel thread applies against
its own Wayland surfaces; `src/anim.rs` eases the width; `src/pty.rs` and
`src/pty/` drive the host-supplied fd as a calloop source;
`src/fontconfig.rs` reads the Ghostty font and theme; `src/nerd_font.rs`
is a generated glyph table; `src/guard.rs`
is the panic guard; `src/ghostty_sys.rs` and `src/ghostty_sys/` are the
hand-written FFI to the pinned libghostty-vt. The `pinwin` host program
`src/main.rs` owns the pty and the child's process, and binds the instance
socket that the panel serves; its pure parts are `src/cli.rs` (arguments,
`--toggle`, `--show`) and `src/settings.rs` (the environment contract). The
socket itself (identity, bind, listener and client) lives in the library, in
`src/instance.rs`. `build.rs` fetches and builds that pinned commit.

The behaviour spec lives in `openspec/specs/pinwin-panel/spec.md`; the
archived design decisions code comments cite as D-numbers are under
`openspec/changes/archive/`, the port's own are in
`openspec/changes/port-to-rust/design.md`, and the Wayland rewrite's are in
`openspec/changes/replace-gtk-with-wayland/design.md`.

## Known caveats

- Wayland with wlr-layer-shell is required. An X11 session or a compositor
  without it (such as GNOME) fails with `NoDisplay`; there is no fallback to an
  ordinary window.
- One panel per process. A second start while a handle is alive fails with
  `AlreadyRunning`.
- Each start runs its own panel thread, and the thread ends when the handle
  drops. There is no process-lifetime parked thread.
- The library never closes the pty master fd; the host owns its lifetime.
- `panic = "unwind"` is fixed in every Cargo profile. Setting `panic = "abort"`
  would let a panic abort the host instead of being caught at the API
  boundary.
- Building needs Zig 0.16 and, on a cold cache, `git` and network access.
  pinwin's FFI targets one pinned ghostty commit; bumping it is a deliberate
  change to `build.rs` and `src/ghostty_sys/`.
