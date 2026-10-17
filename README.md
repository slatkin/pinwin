# pinwin

A layer-shell terminal panel for Wayland compositors: it docks a terminal
running any command to a screen edge as a layer surface, reserves space from
tiled windows via a transparent reservation surface, and animates its width
between layouts. Built for niri, on a direct Wayland client with no toolkit.

pinwin is a single Rust crate: `src/main.rs` is the `pinwin` binary. The
pre-port C and Zig sources are removed with the port.

## Install

Building needs the Rust toolchain pinned in `rust-toolchain.toml`. The system
needs libxkbcommon and fontconfig, and running needs a Wayland compositor with
wlr-layer-shell.

```sh
cargo install --path .          # installs the `pinwin` binary
cargo build --release           # or run target/release/pinwin
cargo test                      # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The library API is removed: the crate is binary-only now, with no `Panel`,
`Startup`, `PinwinError` or layout/instance types to depend on.

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

## Commands

`pinwin run` starts an app in a new panel and starts the daemon first when no
daemon answers for the display. `pinwin daemon` starts the daemon in the
foreground. The other subcommands send one request to the daemon and exit:
0 on success, 1 when the daemon does not answer or replies with an error, and
2 for an unknown subcommand, a missing argument or an invalid argument.

| Command | Meaning |
| --- | --- |
| `run` | Start an app in a new panel, with the run options below. |
| `daemon` | Start the daemon for the display in the foreground. |
| `toggle` | Hide a shown panel or show a hidden one. |
| `show` | Show a hidden panel and leave a shown one unchanged. |
| `hide` | Hide a shown panel. |
| `focus` | Show a hidden panel and give it the keyboard. |
| `set-edge` | Move a panel to another edge. |
| `set-size` | Set a panel's size, optionally with `--ms <N>`. |
| `resize` | Change a panel's size by a signed amount, optionally with `--ms <N>`. |
| `set-output` | Move a panel to another output. |
| `set-layer` | Change the layer-shell layer of a panel. |
| `set-reserve` | Change whether a panel pushes or covers tiled windows. |
| `set-keyboard` | Change the keyboard interactivity of a panel. |
| `status` | Print the daemon's reply as JSON, for all panels or one name. |
| `subscribe` | Print one JSON line for each event until the connection closes. |
| `quit` | End every panel and then the daemon. |
| `reload` | Read the config file again. |

## Run options

`pinwin run [options] -- <command> [args...]` starts the command in a panel.
`--` ends the options, so a command argument that starts with `--` stays an
argument. The first word that is not an option also starts the command. The
panel name is `--name`, else the file name of the command.

| Option | Value | Default | Meaning |
| --- | --- | --- | --- |
| `--name` | 1..=64 characters from `[A-Za-z0-9_-]` | the command's file name | The panel name. |
| `--edge` | `left`, `right`, `top`, `bottom` | `left` | The output edge the panel docks to. |
| `--size` | pixels as `640px` or a percent as `30%` | `25%` | The size across the docking edge. |
| `--reserve` | `push`, `cover` | `push` | `push` reserves the panel's strip; `cover` draws over tiled windows. |
| `--layer` | `background`, `bottom`, `top`, `overlay` | `overlay` | The layer-shell layer. |
| `--keyboard` | `none`, `on-demand`, `exclusive` | `on-demand` | The keyboard interactivity. |
| `--output` | an output name | the compositor chooses | The output the panel opens on. |
| `--hidden` | none (a flag) | shown | Start hidden. |
| `--focus-ring` | `off`, `#RRGGBB` or `#RRGGBB:<width>` | `#dabc7f`, 1 pixel | The ring the panel draws while it has the keyboard; `<width>` is in pixels and at least 1. |

The options win over the config file for the values they set: the option wins
over `[panel.<name>]`, which wins over `[panel.defaults]`, which wins over the
built-in default.

## Window picking

A panel holds one window of its app. The first top-level window the app opens
becomes the panel's window, docked at the panel's edge with the panel's
settings. The panel takes a new window the same way after its window closes,
so an app that closes its window and opens another one later gets the panel
again with the panel's current settings.

While the panel holds a window, every other top-level window of the app opens
as a normal compositor window, as it would without pinwin. A window that
opened this way stays a normal window when the panel's window closes: the
panel does not take it over.

An app that hands a launch to a running copy opens no window of its own, so it
puts nothing in the panel; the running copy keeps its own window. Start such
an app so that it opens a window in this process; Ghostty, for example, needs
`--gtk-single-instance=false`.

## Migrating to 0.4.0

Version 0.4.0 replaces the terminal and its environment variables with a
daemon, `pinwin run` and the client subcommands. A command line that used the
old form changes as follows.

| Before | After |
| --- | --- |
| `pinwin cmd` | `pinwin run -- ghostty -e cmd` |
| `pinwin -- cmd` | `pinwin run -- cmd` |
| `COLS` | `--size`, for example `--size 640px` |
| `GUTTER` | the `gutter_*` settings in the config file |
| `PINWIN_KEYBOARD` | `--keyboard` |
| `PINWIN_ZONE` | `--reserve` |
| `PINWIN_ACCENT*` | `--focus-ring` |
| `PINWIN_NAME` | `--name` |
| `pinwin --toggle name` | `pinwin toggle name` |
| `pinwin --show name` | `pinwin show name` |
| `pinwin --focus name` | `pinwin focus name` |

## Configuration

The daemon reads `$XDG_CONFIG_HOME/pinwin/config.toml` (default
`~/.config/pinwin/config.toml`) when it starts, and `pinwin reload` reads it
again. A missing file means the built-in defaults. An invalid file, including
an unknown key, makes the daemon print a message with the file and the line:
at start it exits 2, while `pinwin reload` exits 1, the daemon keeps the
previous config and the reply is `invalid_config`. A `[panel.<name>]` key
must follow the panel name rules, so `[panel."my.app"]` is an error;
`defaults` names the defaults table, so a panel cannot be named `defaults`.

The file has a `[daemon]` table, a `[panel.defaults]` table that applies to
every panel and one `[panel.<name>]` table for the panel with that name. For
each setting the `pinwin run` option wins over `[panel.<name>]`, which wins
over `[panel.defaults]`, which wins over the built-in default.

```toml
[daemon]
exit_when_idle = true

[panel.defaults]
edge = "left"
size = "25%"

[panel.chat]
edge = "right"
```

| Table | Key | Type | Default | Meaning |
| --- | --- | --- | --- | --- |
| `[daemon]` | `exit_when_idle` | boolean | `true` | Exit after the last panel ends; `false` keeps the daemon running with no panels. |
| `[panel.*]` | `edge` | string: `left`, `right`, `top`, `bottom` | `"left"` | The output edge the panel docks to. |
| `[panel.*]` | `size` | string: pixels as `640px` or a percent as `30%` | `"25%"` | The size across the docking edge; a percent is of the output's width on a side edge and of its height on a top or bottom edge. |
| `[panel.*]` | `reserve` | string: `push`, `cover` | `"push"` | `push` reserves the panel's strip so tiled windows start beside it; `cover` draws over the tiled windows. |
| `[panel.*]` | `layer` | string: `background`, `bottom`, `top`, `overlay` | `"overlay"` | The layer-shell layer the panel draws in. |
| `[panel.*]` | `keyboard` | string: `none`, `on-demand`, `exclusive` | `"on-demand"` | The keyboard interactivity of the panel's layer surface. |
| `[panel.*]` | `output` | string: an output name | none, the compositor chooses | The output the panel opens on. |
| `[panel.*]` | `hidden` | boolean | `false` | Start hidden instead of shown. |
| `[panel.*]` | `focus_ring` | string: `off`, `#RRGGBB` or `#RRGGBB:<width>` | `"#dabc7f"` | The focus ring the panel draws while it has the keyboard, or `off`; `<width>` is in pixels, at least 1, and defaults to 1. |
| `[panel.*]` | `tween_ms` | integer | `200` | The size animation duration in milliseconds; `0` changes the size in one step. |
| `[panel.*]` | `gutter_left` | integer | `0` | Pixels between the panel and the output's left edge; may be negative. |
| `[panel.*]` | `gutter_right` | integer | `0` | Pixels between the panel and the output's right edge; may be negative. |
| `[panel.*]` | `gutter_top` | integer | `0` | Pixels between the panel and the output's top edge; may be negative. |
| `[panel.*]` | `gutter_bottom` | integer | `0` | Pixels between the panel and the output's bottom edge; may be negative. |

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

`src/main.rs` is the `pinwin` binary; the daemon, the IPC client and the
option parser arrive with the panel-control rows, so it prints the usage and
exits 2 until then. `src/cli.rs` is the hand-written parser for `run`,
`daemon` and the client subcommands, compiled by `tests/cli.rs` until the
binary dispatches them; `src/instance.rs` and
`src/instance/` hold the instance socket (identity, bind, listener and
client); `src/geometry.rs` is the display-free geometry core (edges, sizes,
gutters, the reserved strip and the held gap), compiled from `src/main.rs`;
`src/proxy/crop.rs` is the width tween's crop plan, compiled by
`tests/tween_crop.rs` until the proxy owns it; `src/anim.rs` eases the width;
`src/guard.rs` is the panic guard.

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
