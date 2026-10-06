# pinwin

A GTK4 layer-shell terminal panel for Wayland compositors: it docks a terminal
running any command to a screen edge as a layer surface, reserves space from
tiled windows via a transparent reservation surface, and animates its width
between layouts. Built for niri.

pinwin is a single Rust crate: `src/main.rs` is the `pinwin` binary and
`src/lib.rs` is a library you can depend on. The pre-port C and Zig sources
are removed with the port.

## Install

Building needs Zig 0.16 on PATH: `build.rs` fetches the pinned libghostty-vt
commit and builds ghostty's own static VT library with `zig build` (a cold
cache also needs `git` and network access). The system GTK4,
gtk4-layer-shell-0 and pangocairo are required. GTK 4.12 is the API floor;
fractional output scale with the default renderer needs GTK 4.14 (before
4.13.6 the default GL renderer reports an integer scale on Wayland). Set
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
creates its own pty pair and never gets installed. In a running niri session
it toggles the side and width (`<enter>`, `e`, `p`) and the push/cover choice
(`c` animates pushing 40 columns vs covering 120 on the same side, so the
reservation and the tiles stay still through the excursion); `DEMO_DENSE=1`
gives it a full, busy grid with kitty images and resize traffic to test
animation against.

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
`apply_layout_animated` clamps its duration to 1000 ms.
`request_focus(&ActivationToken::new(token)?)` asks the compositor to give
the panel keyboard focus with an xdg-activation token; `ActivationToken` is
re-exported from the crate root and validates the token before any request
is made. Until the panel thread replaces the GTK thread (rows 8.1 and 8.2
of the `replace-gtk-with-wayland` change), the request is accepted and does
nothing: no activation request reaches the compositor, so the panel does
not get the keyboard this way.
<!-- remove when replace-gtk-with-wayland row 8.1 lands -->
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
    let startup = Startup {
        fd,
        layout,
        keyboard: Keyboard::OnDemand,
        accent: Some(Accent::new([0xda, 0xbc, 0x7f], NonZeroU16::new(2).unwrap())),
    };

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

`PinwinError` is `#[non_exhaustive]` and implements `std::error::Error`:

- `InvalidLayout` — the monitor cannot hold the layout (overflow, a
  negative or too-wide pushing reservation, a covering panel wider than the
  output, no complete row). The applied layout is unchanged.
- `InvalidFd` — the pty fd is not an open descriptor; nothing opened.
- `NoDisplay` — no GTK display, or the compositor lacks wlr-layer-shell.
- `AlreadyRunning` — at most one panel exists per process.
- `NotRunning` — the handle's panel is no longer live.
- `Internal` — a caught panic, a wedged GTK side, or a terminal grid that
  could not be allocated.

The layout types keep their invariants in their fields, so an unknown side, a
zero or oversized column count, a non-positive cell/output size and an accent
width outside 1..=65535 cannot be expressed (pass `None` for "no accent").

## Usage

```text
pinwin [--] [command...]    # default command: $SHELL, else /bin/sh; needs niri
pinwin --focus [name]       # ask the named running panel for focus, then exit;
                            # needs XDG_ACTIVATION_TOKEN, exits 2 without it
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
| `PINWIN_ACCENT` | `on`, `off` | `on` |
| `PINWIN_ACCENT_COLOR` | `#RRGGBB` or `RRGGBB` | `#dabc7f` |
| `PINWIN_ACCENT_WIDTH` | 1..=65535 (read only when the accent is on) | 1 |
| `PINWIN_NAME` | 1..=64 characters from `A-Za-z0-9_-` | `default` |

An invalid value exits 2 with a message on stderr before any surface opens.
Only the binary reads these; the library reads no pinwin-owned configuration.

## Focus request

Every instance reads `PINWIN_NAME`. The default name is `default`. A name has
1..=64 characters from `A-Za-z0-9_-`. While its command runs, the instance
listens on a Unix socket under `$XDG_RUNTIME_DIR/pinwin/`, named for the
Wayland display and the name. A second instance with a name that a live
instance already uses prints a message, exits 2 and opens nothing. A socket
file left by a killed instance does not block a new host.

`pinwin --focus [name]` asks the instance with that name for keyboard focus
and exits. The name defaults to `default`. The client exits 0 when the named
instance accepts the request. With no instance on that name it prints a
message and exits 1. A bad name prints a message and exits 2 in both places:
in `PINWIN_NAME`, where the host opens nothing, and after `--focus`.

The client reads the activation token from `XDG_ACTIVATION_TOKEN`, which the
compositor sets for a process it launches from a key binding. A token is
1..=255 bytes of visible ASCII. With the variable unset, or with a value
that is not a valid token, the client prints a message and exits 2 without
contacting any instance. Until the GTK libraries are unlinked (rows 8.1 and
8.2), the variable is always unset for a real `pinwin --focus`: the GTK
libraries consume `XDG_ACTIVATION_TOKEN` before `main` runs, so the client
always exits 2.
<!-- remove when replace-gtk-with-wayland row 8.1 lands --> The host passes the token to its panel's focus
request, which asks the compositor to focus the panel through xdg-activation.
Until the panel thread replaces the GTK thread (rows 8.1 and 8.2 of the
`replace-gtk-with-wayland` change), that request is accepted and does
nothing: the compositor sees no activation, so the panel does not get the
keyboard this way.
<!-- remove when replace-gtk-with-wayland row 8.1 lands -->

On-demand focus through xdg-activation needs a compositor that honours the
request for layer surfaces. Stock niri ignores it for layer surfaces; a niri
patch — one added branch in niri's `request_activation`, branch
`spike/xdg-activation-layer-focus` — enables it and is pending upstream, not
merged. Click focus works on every compositor. A stale token — one the
compositor already used, or one that is too old — does nothing: the
compositor's choice is invisible to the client, so the request still exits
0.

In niri, bind a key to the client:

```kdl
Mod+P { spawn "pinwin" "--focus"; }
```

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
  focus only after a click, `exclusive` takes it immediately, `none` never
  does. While focused, the panel draws its own focus accent in the configured
  colour and width, because the compositor draws no focus ring on layer
  surfaces.
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

`src/lib.rs` re-exports the public `Panel` API. `src/panel/` owns the
`pinwin-gtk` thread (spawned once, parked between panels) and the start
handshake and apply replies; `src/layout.rs` is the GTK-free geometry core;
`src/term/` wraps the pinned libghostty-vt terminal and its cells, keys and
input encoders; `src/render/` paints frames through the GSK render-node
snapshot path with a cairo fallback, plus the focus accent and kitty image
surfaces; `src/surfaces/` builds the layer-shell panel
and reservation surfaces; `src/input/` wires the GDK controllers;
`src/anim.rs` eases the width; `src/pty.rs` drives the host-supplied fd;
`src/fontconfig.rs` reads the Ghostty font and theme; `src/nerd_font.rs` is a
generated glyph table; `src/guard.rs` is the panic guard; `src/ghostty_sys/`
is the hand-written FFI to the pinned libghostty-vt. The `pinwin` host
program `src/main.rs` owns the pty, the child's process and the focus
socket; its pure parts are `src/cli.rs` (arguments, `--focus`), `src/ipc.rs`
(the focus socket's identity, bind, listener and client) and
`src/settings.rs` (the environment contract). `build.rs` fetches and
builds that pinned commit.

The behaviour spec lives in `openspec/specs/pinwin-panel/spec.md`; the
archived design decisions code comments cite as D-numbers are under
`openspec/changes/archive/`, and the port's own are in
`openspec/changes/port-to-rust/design.md`.

## Known caveats

- Wayland with wlr-layer-shell is required. An X11 session or a compositor
  without it (such as GNOME) fails with `NoDisplay`; there is no fallback to an
  ordinary window.
- One panel per process. A second start while a handle is alive fails with
  `AlreadyRunning`.
- The library's GTK thread is process-lifetime and is never joined, so it
  stays parked for a later start. GTK can only be initialised once per
  process; the parked thread owns that initialisation.
- The library never closes the pty master fd; the host owns its lifetime.
- `panic = "unwind"` is fixed in every Cargo profile. Setting `panic = "abort"`
  would let a panic abort the host instead of being caught at the API
  boundary.
- Building needs Zig 0.16 and, on a cold cache, `git` and network access.
  pinwin's FFI targets one pinned ghostty commit; bumping it is a deliberate
  change to `build.rs` and `src/ghostty_sys/`.
