# pinwin

A layer-shell app host for Wayland compositors: it runs an app in a panel at
one edge of an output as a layer surface, reserves space from tiled windows
via a transparent reservation surface, and animates the panel's width between
layouts. Built for niri, on a direct Wayland client with no toolkit.

pinwin is a single Rust crate: `src/main.rs` is the `pinwin` binary. One
daemon owns every panel on a Wayland display; `pinwin run` starts an app in a
panel and the client subcommands control the daemon.

## Install

Building needs the Rust toolchain pinned in `rust-toolchain.toml`. Running
needs a Wayland compositor with wlr-layer-shell.

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
pinwin run [options] -- <command> [args...]   # start an app in a new panel
pinwin daemon                                 # start the daemon
pinwin <command> [args...]                    # send one request to the daemon
```

`pinwin run` asks the daemon for a new panel and starts one in the background
when none answers for the display. It sets `WAYLAND_DISPLAY` to the new
panel's display and replaces itself with the command, so the app keeps its
process ID, working folder and environment. A command that cannot run exits
127; a missing daemon or a daemon error exits 1; a bad command line exits 2.

`pinwin daemon` binds the display's socket and runs in the foreground until a
`quit` request, SIGINT or SIGTERM ends it. The daemon reads the config file
and requires a compositor with wlr-layer-shell before it creates anything.

pinwin reads `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`, `XDG_CONFIG_HOME` and
`HOME` for its socket and config file; it reads no `PINWIN_*` variable. The
app keeps every other variable unchanged.

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

## Tray

The daemon adds one StatusNotifierItem to a running tray host while at least
one panel exists, and removes it when the last panel ends. The number of
panels never changes the number of icons. The item uses the `pinwin` title
and the theme icon named `pinwin`, which the tray host shows from the user's
icon theme. pinwin does not install an icon file itself, so if the theme has
no icon with that name the host shows its own placeholder.

Its menu has one submenu for each panel, labelled with the panel name, in the
order the panels started. A panel submenu has a `Shown` check item that hides
or shows that panel, an edge choice of `left`, `right`, `top` and `bottom`,
and a reserve choice of `push` and `cover`; each item shows the panel's
current state. `Quit pinwin` follows the panel submenus and does the same as
the `quit` command. The menu is also a client of the daemon's IPC: a change
made by `pinwin`, a key binding or a script shows in the menu the next time
it opens, and the menu's own choices use the same commands.

A primary click on the icon changes nothing; the menu is the only way the
tray changes a panel.

A session with no D-Bus session bus or no tray host is not an error: the
daemon still runs, the panels work and the IPC answers. The daemon reports
the missing tray once and adds the icon when a tray host starts later.

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

Popups follow the window they belong to. A popup of the panel's window, such
as a context menu or a tooltip, opens where the app asks for it relative to
its window. A popup of another window behaves as it would without pinwin.

The panel's window carries no title bar and no frame: the app is told that
the compositor draws the decorations, and the compositor draws none on a
panel. Another window of the app gets the same decorations as without
pinwin, whether the compositor or the app draws them. The panel's window is
also cropped to the window geometry the app reports, so a client-side title
bar and drop shadow stay outside the panel; an app that manages its own
surface scaling through a viewport keeps that mapping and is not cropped.

An app that hands a launch to a running copy opens no window of its own, so it
puts nothing in the panel; the running copy keeps its own window. See "Known
limits".

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

## Daemon socket

The daemon listens on one Unix socket at
`$XDG_RUNTIME_DIR/pinwin/<display>.sock`, where `<display>` is the base name
of the Wayland display's socket. The socket file is readable and writable
only by its owner. A socket file left by a daemon that no longer runs does not
block a new daemon, and the daemon removes its socket file when it exits.

A second `pinwin daemon` for the same display prints a message and exits 0
without changing the running daemon. Daemons for different displays do not
interfere. With no `XDG_RUNTIME_DIR` the daemon prints a message, exits 1 and
creates nothing.

## Behaviour

- A panel is a layer-shell surface on its layer (default `overlay`) at one
  edge of its output, flush against the output edges at the ends of that edge
  except for the side gutters. It stays on its output and shows on every
  workspace of that output.
- A pushing panel reserves a strip at its docking edge of
  `near + width + far` pixels, so the compositor tiles windows beside it.
  Gutters are directional and may be negative; the reservation sum may not
  be, and must leave some output width for other windows. A covering panel
  uses the same insets and reserves nothing: it draws over the tiled windows.
- A layout change keeps the app process and its window. The app gets a new
  window size only when the panel's width or length changes, and an animated
  change sends exactly one new size for the whole animation. While the panel
  is hidden, a change is validated and stored with no animation, and the next
  show uses it.
- Keyboard interactivity is per panel and changes at run time. In `on-demand`
  mode the panel gets the keyboard when the user clicks inside it or a show
  or focus request shows it, and gives it up when the user clicks a compositor
  window; at open it takes no keyboard. In `exclusive` mode it has the
  keyboard while it shows. In `none` mode it never gets the keyboard.
  `focus` shows a hidden panel and gives it the keyboard in any mode except
  `none`; `focus` on a shown panel changes nothing, so to move the keyboard to
  a shown panel, hide it and show it again.
- While the panel holds keyboard focus and a focus ring is set, pinwin draws
  the ring around the whole panel window, over the app's content. With the
  ring `off`, pinwin draws nothing over the app.
- When the panel's output is removed, the panel moves to the output the
  compositor chooses, with the same settings, and the app keeps running.
- A panel ends when the launched process has exited and the app has no
  Wayland connection to the panel left. Its window goes away, its strip is
  released and its name is free.

## Architecture

`src/main.rs` is the `pinwin` binary: it dispatches `run`, `daemon` and the
client subcommands, and runs the daemon in the foreground. `src/cli.rs` is the
hand-written command-line parser; `src/name.rs` holds the panel name rules;
`src/config.rs` reads the config file. `src/ipc.rs`, with `src/ipc/options.rs`
and `src/ipc/client.rs`, is the wire format, the run options and the client.
`src/daemon.rs` owns the daemon socket, and `src/daemon/` holds the registry,
the panel handles, the event bus, the connections, the `create` request, the
lifetime and the daemon spawn. `src/proxy.rs` is the panel thread and
`src/proxy/` holds the window routing, the dock, the reserve surface, popups,
decorations, keyboard, the focus ring and the width tween. `src/tray.rs` is
the tray icon, and `src/run.rs` is the exec that becomes the app.
`src/geometry.rs` is the display-free geometry core, `src/anim.rs` eases the
width and `src/guard.rs` is the panic guard.

The behaviour spec lives in `openspec/specs/pinwin-panel/spec.md`; the
archived design decisions code comments cite as D-numbers are under
`openspec/changes/archive/`, the port's own are in
`openspec/changes/port-to-rust/design.md`, the Wayland rewrite's are in
`openspec/changes/replace-gtk-with-wayland/design.md`, and this app host's
are in `openspec/changes/replace-terminal-with-app-host/design.md`.

## Known limits

- Apps that hand their window to an already running copy, or that start it
  through a launcher that quits before the window connects, cannot be docked.
  The daemon reports an app it sees exit without a window: it logs a line
  naming the app and the panel, and `pinwin subscribe` receives a `no-window`
  event for the panel before its `removed` event. Run such an app so that it
  opens a window in the process `pinwin run` starts; Ghostty, for example,
  needs `--gtk-single-instance=false`.
- Wayland with wlr-layer-shell is required. An X11 session or a compositor
  without it (such as GNOME) makes the daemon print a message and exit 1 at
  start, and `pinwin run` print a message and exit 1 without running the
  command; there is no fallback to an ordinary window.
- X11 apps are not supported. An app under Xwayland connects to the Xwayland
  server, not to the panel socket, so it does not enter a panel.
- A panic inside one panel ends only that panel and closes its app's Wayland
  connections; the daemon and the other panels keep running. `panic =
  "unwind"` is fixed in every Cargo profile, so a panic cannot abort the
  daemon.
