# panel-control Specification

## Purpose
Lets a user and their scripts start, find and control pinwin panels: one daemon owns every
panel on a Wayland display, and the `pinwin` command, the tray and scripts control the
panels through the daemon's IPC socket and a config file.

## Requirements

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
the new panel's socket, `PINWIN_PANEL` set to the panel's name and `PINWIN_SOCKET` set to the
absolute path of the socket of the daemon that owns the panel, and with the caller's working
folder and all other environment variables unchanged. These three values SHALL replace values
that the caller's environment has. The process ID of the command is the process ID of
`pinwin run`.

#### Scenario: Run an app
- **WHEN** the user runs `pinwin run --name notes -- myeditor ~/notes.txt` from `~/work`
- **THEN** `myeditor` runs with the working folder `~/work`, the caller's environment, a
  `WAYLAND_DISPLAY` for the `notes` panel, `PINWIN_PANEL=notes` and a `PINWIN_SOCKET` that
  names the daemon's socket, and its first window shows as the panel

#### Scenario: Command not found
- **WHEN** the user runs `pinwin run -- no-such-program`
- **THEN** pinwin prints a message, exits 127, and the `no-such-program` panel ends

#### Scenario: Run from inside a panel
- **WHEN** the app of panel `notes` runs `pinwin run --name chat -- mychat`
- **THEN** the `chat` panel is a panel of the same daemon, and `mychat` runs with
  `PINWIN_PANEL=chat` and the `WAYLAND_DISPLAY` of the `chat` panel

#### Scenario: Process outside pinwin
- **WHEN** a process starts in a session without `pinwin run`
- **THEN** it has no `PINWIN_PANEL` from pinwin

### Requirement: Run starts the daemon
When no daemon answers for the display, `pinwin run` SHALL start one in the background,
detached from the caller's terminal and session, and SHALL wait a bounded time for it to
report that it is ready. The started daemon SHALL report ready once it accepts requests, or
report its error message when it cannot start. When the daemon reports an error or ends
before it is ready, `pinwin run` SHALL print the daemon's message and exit 1 at once,
without running the command. When the daemon reports nothing in the bounded time,
`pinwin run` SHALL print a message and exit 1 without running the command. Two
`pinwin run` calls at the same time SHALL result in one daemon. When `PINWIN_SOCKET` sets
the socket and no daemon answers there, `pinwin run` SHALL print a message, exit 1, start
no daemon and run nothing.

#### Scenario: First run
- **WHEN** no daemon runs and the user runs `pinwin run --name notes -- myeditor`
- **THEN** a daemon starts, the `notes` panel shows `myeditor`, and closing the caller's
  terminal does not end the daemon

#### Scenario: Two first runs
- **WHEN** no daemon runs and two `pinwin run` calls with different names start at the same
  time
- **THEN** one daemon runs and it owns both panels

#### Scenario: Daemon cannot start
- **WHEN** no daemon runs, the config file is invalid, and the user runs `pinwin run -- myeditor`
- **THEN** `pinwin run` prints the config file's error with its file and line, exits 1
  without waiting for the bounded time, and does not run `myeditor`

#### Scenario: No daemon at the set socket
- **WHEN** `PINWIN_SOCKET` names a path where no daemon answers and the user runs
  `pinwin run --name chat -- mychat`
- **THEN** pinwin prints a message, exits 1, no daemon starts and `mychat` does not run

### Requirement: Run options
`pinwin run` SHALL accept `--name <name>`, `--edge <left|right|top|bottom>`,
`--size <size>`, `--reserve <push|cover|none>`, `--layer <background|bottom|top|overlay>`,
`--keyboard <none|on-demand|exclusive>`, `--output <output name>`, `--hidden`,
`--focus-ring <off|#RRGGBB[:width]>`, `--gutter-top <pixels>`, `--gutter-bottom <pixels>`,
`--gutter-left <pixels>`, `--gutter-right <pixels>` and `--show-existing`. A gutter value
SHALL be a whole number of logical pixels and MAY be negative. `--` SHALL end the options. A
missing command, an unknown option or an invalid value SHALL make pinwin print a message and
exit 2.

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

#### Scenario: Gutter option wins over the file
- **WHEN** the user runs `pinwin run --name notes --gutter-left 12 -- myeditor` and
  `[panel.notes]` sets `gutter_left = 4`
- **THEN** the panel starts 12 pixels from the left output edge

#### Scenario: Reserve none at start
- **WHEN** the user runs `pinwin run --name notes --reserve none -- myeditor`
- **THEN** the `notes` panel starts with reserve mode `none` and reserves nothing

### Requirement: Panel names
Each panel SHALL have a name of 1..=64 characters from `[A-Za-z0-9_-]`. Without `--name`, the
name SHALL be the file name of the command. A name SHALL be unique among the daemon's panels.
An invalid name SHALL make `pinwin run` print a message and exit 2 without running the
command. A name that a panel of the daemon already uses SHALL make `pinwin run` print a
message and exit 2 without running the command, unless `--show-existing` is given.

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
`set-size`, `resize`, `set-gutters`, `set-output`, `set-layer`, `set-reserve` and
`set-keyboard`. A command with an invalid value SHALL get error kind `invalid_value`. A
geometry the output cannot hold SHALL get error kind `invalid_layout`. Neither error SHALL
change the panel.

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

#### Scenario: Focus on a shown panel
- **WHEN** the `notes` panel is shown and a client sends `{"cmd":"focus","panel":"notes"}`
- **THEN** the reply is a success and the panel and the keyboard focus do not change

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
(`no-window`), and each SHALL name the panel. A client that does not read SHALL NOT block the
daemon or other clients; the
daemon SHALL close the connection of a client that falls too far behind.

#### Scenario: Toggle events
- **WHEN** a client has subscribed and another client toggles `notes` twice
- **THEN** the subscriber receives a `hidden` event and then a `shown` event for `notes`

#### Scenario: App never opens a window
- **WHEN** a client has subscribed and the app of panel `notes` exits without ever having
  connected to the panel
- **THEN** the subscriber receives a `no-window` event for `notes` and then a `removed`
  event for `notes`

#### Scenario: A panel that had a connection gets no notice
- **WHEN** a client has subscribed and the app of panel `notes`, which had a connection to
  the panel, exits
- **THEN** the subscriber receives a `removed` event for `notes` and no `no-window` event

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
`set-size <name> <size> [--reserve push|cover|none] [--ms N]`,
`resize <name> <amount> [--reserve push|cover|none] [--ms N]`,
`set-gutters <name> [--top N] [--bottom N] [--left N] [--right N]`,
`set-output <name> <output>`, `set-layer <name> <layer>`, `set-reserve <name> <mode>`,
`set-keyboard <name> <mode>`, `status [name]`, `subscribe`, `quit` and `reload`.

#### Scenario: Toggle from a key binding
- **WHEN** a compositor key binding runs `pinwin toggle notes` twice
- **THEN** the first run hides the `notes` panel, the second shows it, and each exits 0

#### Scenario: Subscribe prints events
- **WHEN** the user runs `pinwin subscribe` and then toggles a panel
- **THEN** `pinwin subscribe` prints one JSON line for the event

#### Scenario: Size with a reserve mode
- **WHEN** the user runs `pinwin set-size notes 1100px --reserve cover --ms 200`
- **THEN** pinwin sends one `set-size` request with the size, the reserve mode and the
  duration, and exits 0 on its success reply

#### Scenario: Invalid reserve option
- **WHEN** the user runs `pinwin set-size notes 1100px --reserve middle`
- **THEN** pinwin prints a message, exits 2 and sends nothing

#### Scenario: Gutter command
- **WHEN** the user runs `pinwin set-gutters notes --top 8 --bottom 8`
- **THEN** pinwin sends one `set-gutters` request with both gutters and exits 0 on its
  success reply

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

### Requirement: Client socket lookup
Every client use of the daemon socket, `pinwin run` included, SHALL use the path in
`PINWIN_SOCKET` when the variable is set and not empty. Otherwise it SHALL use the path of
requirement "Daemon socket" from `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`. An empty
`PINWIN_SOCKET` SHALL be the same as an unset one. A `PINWIN_SOCKET` that is not an absolute
path SHALL make the client print a message and exit 1 without sending a request.
`pinwin daemon` SHALL NOT read `PINWIN_SOCKET`.

#### Scenario: App controls its own panel
- **WHEN** the app of panel `notes` runs `pinwin set-size notes 800px`
- **THEN** the request goes to the daemon that owns `notes`, and the panel becomes 800
  pixels wide

#### Scenario: Variable wins over the display
- **WHEN** `PINWIN_SOCKET` names the socket of a running daemon and `WAYLAND_DISPLAY` names
  a display with no daemon, and the user runs `pinwin status`
- **THEN** pinwin prints the reply of the daemon at `PINWIN_SOCKET` and exits 0

#### Scenario: Empty variable
- **WHEN** `PINWIN_SOCKET` is set to an empty value and the user runs `pinwin status`
- **THEN** pinwin uses the socket of `WAYLAND_DISPLAY`, as when the variable is unset

#### Scenario: Relative path
- **WHEN** `PINWIN_SOCKET` is `pinwin/wayland-1.sock` and the user runs `pinwin toggle notes`
- **THEN** pinwin prints a message that names `PINWIN_SOCKET`, exits 1 and sends nothing

#### Scenario: Daemon ignores the variable
- **WHEN** `PINWIN_SOCKET` names another path and the user runs `pinwin daemon`
- **THEN** the daemon binds the socket of requirement "Daemon socket" for its display

### Requirement: Show an existing panel
`pinwin run --show-existing` SHALL, when the daemon already has a panel with the name, show
that panel as `show` does and exit 0 without running the command. The other run options
SHALL NOT change the existing panel. When the daemon has no panel with the name,
`--show-existing` SHALL NOT change what `pinwin run` does.

#### Scenario: Show a hidden panel
- **WHEN** panel `notes` exists and is hidden, and the user runs
  `pinwin run --name notes --show-existing -- myeditor`
- **THEN** the `notes` panel shows, `myeditor` does not run, and pinwin exits 0

#### Scenario: Panel already shows
- **WHEN** panel `notes` exists and shows, and the user runs
  `pinwin run --name notes --show-existing -- myeditor`
- **THEN** the panel does not change, `myeditor` does not run, and pinwin exits 0

#### Scenario: Options do not change the existing panel
- **WHEN** panel `notes` exists at the left edge, 400 pixels wide, and the user runs
  `pinwin run --name notes --show-existing --edge right --size 800px --hidden -- myeditor`
- **THEN** the `notes` panel shows at the left edge, 400 pixels wide, and `myeditor` does
  not run

#### Scenario: No panel with the name
- **WHEN** no panel `notes` exists and the user runs
  `pinwin run --name notes --show-existing -- myeditor`
- **THEN** a new `notes` panel starts with `myeditor`, as without the flag

#### Scenario: Invalid name
- **WHEN** the user runs `pinwin run --show-existing -- my.app`
- **THEN** pinwin prints a message that names `--name`, exits 2 and runs nothing

### Requirement: Size and reserve in one request
`set-size` and `resize` SHALL accept an optional `reserve` field, `push`, `cover` or `none`.
The daemon SHALL validate the new size and the new reserve mode together, with the rules for the
new reserve mode, and SHALL apply them as one change with the request's `ms` or the panel's
`tween_ms`. A refusal SHALL change neither the size nor the reserve mode. A `reserve` equal
to the panel's mode SHALL change only the size. A request without `reserve` SHALL behave as
before.

#### Scenario: Covering expand in one request
- **WHEN** a pushing panel 400 pixels wide gets
  `{"cmd":"set-size","panel":"notes","size":"1100px","reserve":"cover","ms":200}`
- **THEN** the panel grows to 1100 pixels over about 200 ms, the strip stays at 400 pixels,
  tiled windows do not move, and `status` reports the size `1100px` and the mode `cover`

#### Scenario: Refused together
- **WHEN** a covering panel gets a `set-size` with `reserve` `push` whose strip leaves no
  output space for other windows
- **THEN** the reply has error kind `invalid_layout`, and the size and the reserve mode do not
  change

#### Scenario: Same reserve mode
- **WHEN** a covering panel gets `set-size` with `"reserve":"cover"` and a new size
- **THEN** only the size changes, as with a `set-size` without `reserve`

#### Scenario: Relative resize with a mode
- **WHEN** a covering panel 1100 pixels wide gets
  `{"cmd":"resize","panel":"notes","by":"-700px","reserve":"push"}`
- **THEN** the panel becomes 400 pixels wide and pushes

#### Scenario: Invalid mode
- **WHEN** a client sends a `set-size` with `"reserve":"middle"`
- **THEN** the reply has error kind `invalid_value` and the panel does not change

#### Scenario: Release the strip of a running panel
- **WHEN** a pushing panel with a 400 pixel strip gets `{"cmd":"set-reserve","panel":"notes","reserve":"none"}`
- **THEN** the reply is a success, the strip becomes zero, and `status` reports the mode `none`
### Requirement: Set gutters
The `set-gutters` command SHALL change one or more of the panel's four gutters and SHALL
validate the result with requirement "Validate a change before applying it". It SHALL apply
the change in one step, with no animation, keeping the app's window size unless the panel's
length changes. A refusal SHALL change neither the gutters nor the reservation. A change
while the panel is hidden SHALL be validated and stored for the next show.

#### Scenario: Set the side gutters
- **WHEN** a left panel with Top 0 gets
  `pinwin set-gutters notes --top 8 --bottom 8`
- **THEN** the panel's top edge is 8 pixels below the output's top edge, its bottom edge 8
  pixels above the output's bottom edge, and the reply is a success

#### Scenario: Refused gutter change
- **WHEN** a pushing left panel gets `set-gutters` with a `right` value that leaves no output
  width for other windows
- **THEN** the reply has error kind `invalid_layout`, and the gutters and the reservation do
  not change

#### Scenario: No gutter option
- **WHEN** the user runs `pinwin set-gutters notes`
- **THEN** pinwin prints a message, exits 2 and sends nothing

#### Scenario: Hidden panel stores the gutters
- **WHEN** the panel is hidden and a client sends `set-gutters` with a new gutter
- **THEN** the reply is a success, nothing shows, and the next show uses the new gutter

#### Scenario: Gutter change snaps
- **WHEN** a shown panel with `tween_ms` 200 gets `set-gutters` with a new gutter
- **THEN** the panel moves to the new gutter in one step with no animation

#### Scenario: App size follows the length
- **WHEN** a left panel gets `set-gutters` with a new `top` and `bottom`, and then with a new
  `left`
- **THEN** the first change sends the app one new window size and the second sends none

### Requirement: Gutter request fields
A `set-gutters` request SHALL carry `top`, `bottom`, `left` and `right` as optional whole
numbers of logical pixels; a negative value is valid. At least one field SHALL be present,
and a missing field SHALL keep the panel's current gutter. A request with no gutter field,
or a field that is not a whole number, SHALL get error kind `invalid_value`.

#### Scenario: One gutter
- **WHEN** a client sends `{"cmd":"set-gutters","panel":"notes","left":-12}`
- **THEN** the panel's left gutter becomes -12 pixels, its other gutters stay, and the reply
  is a success

#### Scenario: No gutter field
- **WHEN** a client sends `{"cmd":"set-gutters","panel":"notes"}`
- **THEN** the reply has error kind `invalid_value` and the panel does not change

#### Scenario: Non-integer gutter
- **WHEN** a client sends `{"cmd":"set-gutters","panel":"notes","top":1.5}`
- **THEN** the reply has error kind `invalid_value` and the panel does not change

### Requirement: One decision for a duplicate launch
The daemon SHALL decide between a new panel and the existing one for
`pinwin run --show-existing`. Two such calls with the same name at the same time SHALL give
one panel, and both calls SHALL exit 0.

#### Scenario: Two launches at the same time
- **WHEN** no panel `notes` exists and two `pinwin run --name notes --show-existing` calls
  start at the same time
- **THEN** one `notes` panel exists, one command runs in it, and both calls exit 0

### Requirement: One event for size and reserve
A successful `set-size` or `resize` with `reserve` SHALL send one `geometry-changed` event
for the panel, after the request is validated, so a subscriber never sees the new size
without the new reserve mode.

#### Scenario: One event
- **WHEN** a client has subscribed and another client sends a `set-size` with a new size and
  a new `reserve`
- **THEN** the subscriber receives one `geometry-changed` event for the panel
