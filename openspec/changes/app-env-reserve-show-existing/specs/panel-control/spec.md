# Spec Delta

## ADDED Requirements

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
`set-size` and `resize` SHALL accept an optional `reserve` field, `push` or `cover`. The
daemon SHALL validate the new size and the new reserve mode together, with the rules for the
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

## MODIFIED Requirements

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
`--size <size>`, `--reserve <push|cover>`, `--layer <background|bottom|top|overlay>`,
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

### Requirement: Command-line client
The `pinwin` command SHALL offer one subcommand for each request: `toggle <name>`,
`show <name>`, `hide <name>`, `focus <name>`, `set-edge <name> <edge>`,
`set-size <name> <size> [--reserve push|cover] [--ms N]`,
`resize <name> <amount> [--reserve push|cover] [--ms N]`,
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
