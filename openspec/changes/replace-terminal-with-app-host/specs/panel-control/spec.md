# Spec Delta

## Purpose

Lets a user and their scripts start, find and control pinwin panels: one daemon owns every
panel on a Wayland display, and the `pinwin` command, the tray and scripts control the
panels through the daemon's IPC socket and a config file.

## ADDED Requirements

### Requirement: One daemon for each Wayland display
pinwin SHALL run one daemon for each Wayland display. The daemon owns every panel on that
display. `pinwin daemon` starts it in the foreground. A second daemon for the same display
SHALL exit 0 with a message and SHALL NOT change the daemon that runs. Daemons for different
displays SHALL NOT interfere.

#### Scenario: Start the daemon
- **WHEN** no daemon runs for the display and the user runs `pinwin daemon`
- **THEN** the daemon starts and answers `pinwin status`

#### Scenario: Second daemon
- **WHEN** a daemon runs for the display and the user runs `pinwin daemon` again
- **THEN** the second process prints a message and exits 0, and the first daemon is unchanged

#### Scenario: Two displays
- **WHEN** one daemon runs for `wayland-1` and the user starts one for `wayland-2`
- **THEN** both run, and each answers only for its own display

### Requirement: Daemon socket
The daemon SHALL listen on one Unix socket at `$XDG_RUNTIME_DIR/pinwin/<display>.sock`, where
`<display>` is the base name of the display's socket. The socket file SHALL be readable and
writable only by its owner. A socket file left by a daemon that no longer runs SHALL NOT
block a new daemon. When the daemon exits, it SHALL remove its socket file.

#### Scenario: Stale socket file
- **WHEN** a daemon was killed with SIGKILL and the user starts a new daemon
- **THEN** the new daemon starts and answers requests

#### Scenario: No runtime directory
- **WHEN** `XDG_RUNTIME_DIR` is not set and the user runs `pinwin daemon`
- **THEN** pinwin prints a message, exits 1 and creates nothing

### Requirement: Daemon lifetime preference
The `exit_when_idle` preference SHALL control the daemon's lifetime. When it is true, the
daemon SHALL exit after its last panel ends. When it is false, the daemon SHALL stay running
with no panels until a `quit` request or a signal. The default is true. A daemon that starts
with `exit_when_idle` true and no panel SHALL NOT exit before its first panel has ended.

#### Scenario: Exit with the last panel
- **WHEN** `exit_when_idle` is true and the last panel's app exits
- **THEN** the daemon exits and removes its socket file

#### Scenario: Stay resident
- **WHEN** `exit_when_idle` is false and the last panel's app exits
- **THEN** the daemon keeps running and answers `pinwin status` with no panels

### Requirement: Quit
A `quit` request SHALL end every panel and then end the daemon. Ending a panel closes the
Wayland connections of its app. SIGINT and SIGTERM to the daemon SHALL do the same as `quit`.

#### Scenario: Quit with panels
- **WHEN** two panels exist and a client sends `quit`
- **THEN** the client gets a success reply, both apps lose their Wayland connection, and the
  daemon exits and removes its socket file

### Requirement: Run an app in a panel
`pinwin run [options] -- <command> [args...]` SHALL ask the daemon for a new panel, then
replace its own process with the command. The command SHALL run with `WAYLAND_DISPLAY` set to
the new panel's socket, and with the caller's working folder and all other environment
variables unchanged. The process ID of the command is the process ID of `pinwin run`.

#### Scenario: Run an app
- **WHEN** the user runs `pinwin run --name notes -- myeditor ~/notes.txt` from `~/work`
- **THEN** `myeditor` runs with the working folder `~/work`, the caller's environment and a
  `WAYLAND_DISPLAY` for the `notes` panel, and its first window shows as the panel

#### Scenario: Command not found
- **WHEN** the user runs `pinwin run -- no-such-program`
- **THEN** pinwin prints a message, exits 127, and the `no-such-program` panel ends

### Requirement: Run starts the daemon
When no daemon answers for the display, `pinwin run` SHALL start one in the background,
detached from the caller's terminal and session, and SHALL wait a bounded time for it to
answer. If the daemon does not answer in that time, `pinwin run` SHALL print a message and
exit 1 without running the command. Two `pinwin run` calls at the same time SHALL result in
one daemon.

#### Scenario: First run
- **WHEN** no daemon runs and the user runs `pinwin run --name notes -- myeditor`
- **THEN** a daemon starts, the `notes` panel shows `myeditor`, and closing the caller's
  terminal does not end the daemon

#### Scenario: Two first runs
- **WHEN** no daemon runs and two `pinwin run` calls with different names start at the same
  time
- **THEN** one daemon runs and it owns both panels

### Requirement: Run options
`pinwin run` SHALL accept `--name <name>`, `--edge <left|right|top|bottom>`,
`--size <size>`, `--reserve <push|cover>`, `--layer <background|bottom|top|overlay>`,
`--keyboard <none|on-demand|exclusive>`, `--output <output name>`, `--hidden` and
`--focus-ring <off|#RRGGBB[:width]>`. `--` SHALL end the options. A missing command, an
unknown option or an invalid value SHALL make pinwin print a message and exit 2.

#### Scenario: Options apply to the new panel
- **WHEN** the user runs `pinwin run --name notes --edge right --size 30% -- myeditor`
- **THEN** the `notes` panel docks at the right edge with a width of 30% of its output

#### Scenario: Invalid option value
- **WHEN** the user runs `pinwin run --edge middle -- myeditor`
- **THEN** pinwin prints a message, exits 2, and no panel is created

#### Scenario: No command
- **WHEN** the user runs `pinwin run --name notes`
- **THEN** pinwin prints a message and exits 2

#### Scenario: Start hidden
- **WHEN** the user runs `pinwin run --name notes --hidden -- myeditor`
- **THEN** the app runs, the panel does not show, and a later `pinwin show notes` shows it

### Requirement: Panel names
Each panel SHALL have a name of 1..=64 characters from `[A-Za-z0-9_-]`. Without `--name`, the
name SHALL be the file name of the command. A name SHALL be unique among the daemon's panels.
An invalid name, or a name that a panel of the daemon already uses, SHALL make `pinwin run`
print a message and exit 2 without running the command.

#### Scenario: Default name
- **WHEN** the user runs `pinwin run -- /usr/bin/myeditor`
- **THEN** the panel's name is `myeditor`

#### Scenario: Invalid default name
- **WHEN** the user runs `pinwin run -- my.app`
- **THEN** pinwin prints a message that names `--name`, exits 2 and runs nothing

#### Scenario: Duplicate name
- **WHEN** a panel named `notes` exists and the user runs `pinwin run --name notes -- other`
- **THEN** pinwin prints a message, exits 2, runs nothing, and the `notes` panel is unchanged

#### Scenario: Name is free again
- **WHEN** the `notes` panel has ended and the user runs `pinwin run --name notes -- myeditor`
- **THEN** the new panel starts with the name `notes`

### Requirement: Request and reply format
A client SHALL send each request as one JSON object on one line, with a `cmd` field and,
for a panel command, a `panel` field with the panel name. The daemon SHALL answer each request
with one JSON object on one line: `{"ok":true,...}` or
`{"ok":false,"error":{"kind":...,"message":...}}`. A line that is not a valid request SHALL
get an error reply with kind `invalid_request` and SHALL change nothing.

#### Scenario: Valid request
- **WHEN** a client sends `{"cmd":"toggle","panel":"notes"}` and the `notes` panel exists
- **THEN** the daemon toggles the panel and replies `{"ok":true}` on one line

#### Scenario: Not JSON
- **WHEN** a client sends `toggle notes`
- **THEN** the daemon replies with error kind `invalid_request` and no panel changes

#### Scenario: Unknown panel
- **WHEN** a client sends `{"cmd":"show","panel":"nope"}` and no panel has that name
- **THEN** the daemon replies with error kind `no_panel`

#### Scenario: Unknown command
- **WHEN** a client sends `{"cmd":"restart-app","panel":"notes"}`
- **THEN** the daemon replies with error kind `invalid_request` and no panel changes

### Requirement: Panel commands
The daemon SHALL accept these panel commands: `toggle`, `show`, `hide`, `focus`, `set-edge`,
`set-size`, `resize`, `set-output`, `set-layer`, `set-reserve` and `set-keyboard`. A command
with an invalid value SHALL get error kind `invalid_value`. A geometry the output cannot hold
SHALL get error kind `invalid_layout`. Neither error SHALL change the panel.

#### Scenario: Set the size
- **WHEN** a client sends `{"cmd":"set-size","panel":"notes","size":"640px"}`
- **THEN** the panel's width becomes 640 pixels and the reply is a success

#### Scenario: Relative resize
- **WHEN** the `notes` panel is 640 pixels wide and a client sends
  `{"cmd":"resize","panel":"notes","by":"-40px"}`
- **THEN** the panel's width becomes 600 pixels

#### Scenario: Invalid value
- **WHEN** a client sends `{"cmd":"set-edge","panel":"notes","edge":"middle"}`
- **THEN** the reply has error kind `invalid_value` and the panel does not change

#### Scenario: Too large for the output
- **WHEN** a client sends a `set-size` for a pushing panel that would leave no output space
  for other windows
- **THEN** the reply has error kind `invalid_layout` and the panel does not change

#### Scenario: Focus
- **WHEN** the `notes` panel is hidden and a client sends `{"cmd":"focus","panel":"notes"}`
- **THEN** the panel shows and gets the keyboard

### Requirement: Size values
A size SHALL be a whole number of logical pixels (`640px`, or a JSON number) or a percent of
the output (`30%`) from 1 to 100. A percent SHALL be of the output's width for a left or right
panel and of its height for a top or bottom panel. A percent that is not a whole number of pixels
SHALL round down, with a minimum of 1 pixel. A `resize` amount SHALL use the same forms
with a sign. The daemon SHALL resolve a percent again when the panel's output or edge changes.

#### Scenario: Percent of the output
- **WHEN** a left panel on a 2560 pixel wide output has the size `25%`
- **THEN** the panel is 640 pixels wide

#### Scenario: Percent follows the output
- **WHEN** a panel with the size `25%` moves to a 1920 pixel wide output
- **THEN** the panel becomes 480 pixels wide

#### Scenario: Invalid size
- **WHEN** a client sends the size `0px`, `150%` or `wide`
- **THEN** the reply has error kind `invalid_value`

### Requirement: Animated size changes
`set-size` and `resize` SHALL accept an optional `ms` field, the duration of the size
animation. Without `ms`, the duration SHALL be the `tween_ms` setting of the panel. `ms` of 0
SHALL change the size in one step.

#### Scenario: Animate with the default duration
- **WHEN** `tween_ms` is 200 and a client sends a `set-size` without `ms`
- **THEN** the size changes over about 200 ms

#### Scenario: Snap
- **WHEN** a client sends a `set-size` with `"ms":0`
- **THEN** the size changes in one step

### Requirement: Status
A `status` request SHALL reply with the daemon's process ID and a list of its panels. Each
panel entry SHALL include its name, the app's process ID, whether it is shown, whether it holds
a window, its edge, size, resolved width in pixels, reserve mode, layer, keyboard mode and
output name. `status` with a `panel` field SHALL reply with that panel only.

#### Scenario: Status of all panels
- **WHEN** panels `notes` and `chat` exist and a client sends `{"cmd":"status"}`
- **THEN** the reply lists both panels with their fields

#### Scenario: Status of one panel
- **WHEN** a client sends `{"cmd":"status","panel":"notes"}`
- **THEN** the reply has only the `notes` panel

### Requirement: Event stream
A `subscribe` request SHALL make the daemon send a success reply and then one JSON line for
each event on that connection until the client closes it. Events SHALL cover a panel added,
removed, shown or hidden, a panel's geometry changed and a panel that ends with no connection
(`no_window`), and each SHALL name the panel. A client that does not read SHALL NOT block the
daemon or other clients; the
daemon SHALL close the connection of a client that falls too far behind.

#### Scenario: Toggle events
- **WHEN** a client has subscribed and another client toggles `notes` twice
- **THEN** the subscriber receives a `hidden` event and then a `shown` event for `notes`

#### Scenario: App never opens a window
- **WHEN** a client has subscribed and the app of panel `notes` exits without ever having
  connected to the panel
- **THEN** the subscriber receives a `no_window` event for `notes` and then a `removed`
  event for `notes`

#### Scenario: A panel that had a connection gets no notice
- **WHEN** a client has subscribed and the app of panel `notes`, which had a connection to
  the panel, exits
- **THEN** the subscriber receives a `removed` event for `notes` and no `no_window` event

#### Scenario: Slow subscriber
- **WHEN** a subscriber stops reading and many events happen
- **THEN** other requests are still answered, and the daemon closes the slow subscriber

### Requirement: Clients cannot block the daemon
A client that connects and sends nothing, sends a partial line or stops reading its reply SHALL
NOT block requests from other clients or any panel. A request line longer than 64 KiB SHALL get
error kind `invalid_request` and the daemon SHALL close that connection.

#### Scenario: Silent client
- **WHEN** one client connects and sends nothing, and another client sends `toggle`
- **THEN** the second client gets its reply at once

### Requirement: Command-line client
The `pinwin` command SHALL offer one subcommand for each request: `toggle <name>`,
`show <name>`, `hide <name>`, `focus <name>`, `set-edge <name> <edge>`,
`set-size <name> <size> [--ms N]`, `resize <name> <amount> [--ms N]`,
`set-output <name> <output>`, `set-layer <name> <layer>`, `set-reserve <name> <mode>`,
`set-keyboard <name> <mode>`, `status [name]`, `subscribe`, `quit` and `reload`.

#### Scenario: Toggle from a key binding
- **WHEN** a compositor key binding runs `pinwin toggle notes` twice
- **THEN** the first run hides the `notes` panel, the second shows it, and each exits 0

#### Scenario: Subscribe prints events
- **WHEN** the user runs `pinwin subscribe` and then toggles a panel
- **THEN** `pinwin subscribe` prints one JSON line for the event

### Requirement: Command-line exit codes
A client subcommand SHALL exit 0 on a success reply. It SHALL print the error message and exit
1 when no daemon answers, when the daemon replies with an error, or when no reply comes in a
bounded time. It SHALL exit 2 for an unknown subcommand, a missing argument or an invalid
argument. `pinwin status` SHALL print the reply's JSON on stdout.

#### Scenario: No daemon
- **WHEN** no daemon runs and the user runs `pinwin toggle notes`
- **THEN** pinwin prints a message and exits 1

#### Scenario: Unknown panel
- **WHEN** the daemon has no panel `nope` and the user runs `pinwin show nope`
- **THEN** pinwin prints the daemon's message and exits 1

#### Scenario: Usage error
- **WHEN** the user runs `pinwin frobnicate`
- **THEN** pinwin prints a message and exits 2

### Requirement: Config file
The daemon SHALL read `$XDG_CONFIG_HOME/pinwin/config.toml` (default
`~/.config/pinwin/config.toml`) when it starts. The file has a `[daemon]` table, a
`[panel.defaults]` table and one `[panel.<name>]` table per panel name. A missing file SHALL
mean built-in defaults. An invalid file SHALL make the daemon print a message that names the
file and line, and exit 2.

#### Scenario: No config file
- **WHEN** no config file exists and the user runs `pinwin run -- myeditor`
- **THEN** the panel uses the built-in defaults

#### Scenario: Invalid config file
- **WHEN** the config file sets `edge = "middle"` in `[panel.defaults]` and the daemon starts
- **THEN** the daemon prints a message with the file and line, exits 2, and `pinwin run`
  reports that the daemon did not start

### Requirement: Panel settings and precedence
A panel table SHALL accept `edge`, `size`, `reserve`, `layer`, `keyboard`, `output`,
`hidden`, `focus_ring`, `tween_ms` and the gutters `gutter_left`, `gutter_right`,
`gutter_top` and `gutter_bottom`. For each setting, a `pinwin run` option SHALL win over the
`[panel.<name>]` value, which SHALL win over `[panel.defaults]`, which SHALL win over the
built-in default.

#### Scenario: Per-name override
- **WHEN** `[panel.defaults]` sets `edge = "left"`, `[panel.chat]` sets `edge = "right"`, and
  the user runs `pinwin run --name chat -- mychat`
- **THEN** the `chat` panel docks right

#### Scenario: Option wins
- **WHEN** `[panel.chat]` sets `size = "30%"` and the user runs
  `pinwin run --name chat --size 500px -- mychat`
- **THEN** the `chat` panel is 500 pixels wide

### Requirement: Built-in defaults
Without other settings, a panel SHALL dock at the left edge with the size `25%`, push tiled
windows, use the `overlay` layer and `on-demand` keyboard mode, open on the output the
compositor chooses, show at start, have all gutters 0, have `tween_ms` 200 and draw a
`#dabc7f` focus ring 1 pixel wide.

#### Scenario: Defaults
- **WHEN** no config file exists and the user runs `pinwin run -- myeditor` on a 2560 pixel
  wide output
- **THEN** a 640 pixel wide panel docks at the left edge, tiled windows move right, and a
  1 pixel `#dabc7f` ring shows while the panel has the keyboard

### Requirement: Runtime changes stay in memory
A command that changes a panel SHALL NOT write the config file. When a panel ends, its runtime
changes SHALL be lost. The next panel with the same name SHALL start from the options and the
config file again.

#### Scenario: Not saved
- **WHEN** the user runs `pinwin set-edge notes right`, ends the `notes` app, and runs it again
  with no `--edge` while the config does not set the edge
- **THEN** the new `notes` panel docks at the built-in default edge, and the config file is
  unchanged

### Requirement: Reload
A `reload` request SHALL read the config file again. If the file is valid, the daemon SHALL use
it for panels that start after the reload and for the daemon preferences. Panels that exist
SHALL NOT change. If the file is invalid, the daemon SHALL keep the previous config and reply
with error kind `invalid_config` and the file and line.

#### Scenario: Reload a valid file
- **WHEN** the user changes `[panel.defaults] size` to `40%` and runs `pinwin reload`
- **THEN** the reply is a success, existing panels keep their size, and the next new panel is
  40% wide

#### Scenario: Reload an invalid file
- **WHEN** the config file has a syntax error and the user runs `pinwin reload`
- **THEN** pinwin prints the message with the file and line and exits 1, and the daemon keeps
  the previous config
