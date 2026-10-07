## ADDED Requirements

### Requirement: Instance socket opt-in for library hosts
The library SHALL let a host bind an instance socket for a name on its Wayland display, then
pass the bound socket to the start. A name has 1..=64 characters from `[A-Za-z0-9_-]`. The
bind SHALL need no panel, so a host can bind before any surface opens. A start without a bound
socket SHALL listen on nothing.

#### Scenario: No socket without opt-in
- **WHEN** a host starts a panel without a bound socket
- **THEN** no socket file is created, and a client call to any name reports that no instance
  is listening

#### Scenario: Library host answers a toggle
- **WHEN** a host binds the name `notes`, starts a panel with that socket, and another process
  sends `toggle` to `notes`
- **THEN** the panel hides and the client call returns `Ok`

### Requirement: Instance bind errors
If a live instance on the same display already uses the name, the bind SHALL fail with a
typed duplicate error and leave that instance unchanged. A socket file left by an instance
that no longer runs SHALL NOT block the bind. Different names and different displays SHALL
NOT interfere. A missing runtime directory, or a display name that cannot be used in a path,
SHALL fail the bind with a typed error and create nothing.

#### Scenario: Duplicate bind
- **WHEN** a panel bound to `notes` runs and a second host binds `notes` on the same display
- **THEN** the second bind returns the duplicate error before that host opens any surface,
  and the first panel is unchanged

#### Scenario: Stale socket file
- **WHEN** a host bound to `notes` was killed with SIGKILL and a new host binds `notes`
- **THEN** the bind succeeds and the new panel answers requests

### Requirement: Serving instance requests
A panel started with a bound socket SHALL pass `toggle` requests to its toggle and `show`
requests to its show. It SHALL refuse any other request without changing the panel. A client
that stalls or floods SHALL NOT block later requests or the host.

#### Scenario: Library host answers a show
- **WHEN** a host's panel bound to `notes` is hidden and another process sends `show` to
  `notes` twice
- **THEN** the first request shows the panel, the second leaves it shown, and both calls
  return `Ok`

#### Scenario: Unknown request
- **WHEN** a process writes `hide\n` to the socket of a panel bound to `notes`
- **THEN** the panel answers with an error and does not change

### Requirement: Instance socket lifetime
Dropping the handle SHALL remove the socket file and SHALL NOT wait for the listener to end.
After the drop, no request reaches the old panel. If a start with a bound socket fails, the
library SHALL close the socket and remove its file.

#### Scenario: Drop does not wait on the listener
- **WHEN** a host drops a handle whose panel serves a socket
- **THEN** the drop returns within the bounded teardown wait, the socket file is gone, and
  the host process can exit at once

#### Scenario: Failed start releases the name
- **WHEN** a host binds `notes` and the start fails with `NoDisplay`
- **THEN** the socket file is gone, and a new bind of `notes` succeeds

### Requirement: Instance client call
The library SHALL offer a client call that sends `toggle` or `show` to a named instance on the
current display. The call SHALL return `Result` and SHALL NOT block indefinitely. Its error
SHALL tell four cases apart: the environment gives no usable socket path; no instance with
that name is listening; the instance refused or failed the request; no valid answer came in
time.

#### Scenario: Client finds no instance
- **WHEN** nothing is bound to `notes` and a process sends `show` to `notes`
- **THEN** the client call returns the "no instance listening" error, distinct from a refused
  request or a missing answer

#### Scenario: Request fails on the host
- **WHEN** the panel bound to `notes` has ended and a process sends `toggle` to `notes`
- **THEN** the client call returns the "refused or failed" error

## MODIFIED Requirements

### Requirement: Show and hide on request
The `Panel` handle SHALL offer a toggle and a show. The host can call either from any thread,
and both return `Result<(), PinwinError>`. A toggle hides a shown panel and shows a hidden
one. A show shows a hidden panel and leaves a shown panel unchanged; on a shown panel it SHALL
return `Ok` and change nothing on screen. The panel is shown at start.

Hiding SHALL unmap the panel surface and release the held reservation. A width animation in
progress SHALL end at its target layout first. The terminal and the pty keep running while
the panel is hidden, and the host's child keeps receiving input from the pty.

Showing SHALL map the panel surface again, draw the current grid and restore the held
reservation. In `on-demand` mode, the panel SHALL map with `on-demand` keyboard
interactivity, so a compositor that focuses a newly mapped `on-demand` surface gives it the
keyboard without a click. In `exclusive` mode, the panel maps as `exclusive`. In `none` mode,
the panel only appears. After a show, the panel keeps and gives up focus by the rules of its
keyboard mode.

Neither hiding nor showing SHALL change the terminal grid or the pty window size, and neither
SHALL raise a `SIGWINCH`. No frame SHALL show the panel at another size. Two cases are
exempt. A width animation that a hide ends runs its deferred grid resize as it would at its
own end. If the output's height changed while the panel was hidden, the configure that a show
receives resizes the grid and the pty like any other configure with a new height. While the panel is
hidden, a layout apply SHALL validate and store the layout as it does when shown, with no
animation and nothing on screen. The next show uses that layout.

If the panel is no longer live, the toggle and the show SHALL return
`Err(PinwinError::NotRunning)` without blocking. If the panel thread does not answer within a
bounded time, they SHALL return `Err(PinwinError::Internal)`, so the host thread never blocks
indefinitely. Requests from other processes reach the panel only through the instance socket
the host opted into.

#### Scenario: Show takes focus
- **WHEN** the panel runs in `on-demand` mode on niri, is hidden, a tiled window has the
  keyboard, and the host toggles
- **THEN** the call returns `Ok`, the panel appears, and the next typed key goes to the pty
  master. If the accent is enabled, it appears.

#### Scenario: Hide
- **WHEN** the panel is shown and the host toggles
- **THEN** the call returns `Ok`, the panel leaves the screen, and keys go to a compositor
  window

#### Scenario: Show a hidden panel
- **WHEN** the panel is hidden and the host calls show
- **THEN** the call returns `Ok` and the panel appears as it does after a toggle from hidden

#### Scenario: Show a shown panel
- **WHEN** the panel is shown and the host calls show
- **THEN** the call returns `Ok`, the panel stays on screen, no surface is unmapped or
  remapped, and the reservation does not change

#### Scenario: Grid and pty untouched
- **WHEN** the host's child draws a layout that depends on the terminal size and the host
  toggles twice
- **THEN** the child observes no window size change and receives no `SIGWINCH`, and no frame
  shows the panel drawn at another size

#### Scenario: Show in none mode
- **WHEN** the panel runs in `none` mode, is hidden, and the host toggles
- **THEN** the panel appears and keyboard input stays with the window that had it

#### Scenario: Hide releases the reservation
- **WHEN** the panel pushes tiled windows and the host toggles twice
- **THEN** the tiled windows take the panel's strip after the first toggle and move back
  beside the panel after the second

#### Scenario: Apply while hidden
- **WHEN** the panel is hidden and the host applies a layout with more columns, animated with
  a duration of 200 ms
- **THEN** the call returns `Ok`, nothing appears, and the next toggle shows the panel at the
  new width with no animation

#### Scenario: Toggle on a dead panel
- **WHEN** the panel's thread ended on its own and the host toggles or shows
- **THEN** the call returns `Err(PinwinError::NotRunning)` without blocking

### Requirement: Toggle from the command line
The `pinwin` program SHALL accept toggle and show requests for its panel from other processes
on the same Wayland display, through the library's instance socket. The program SHALL read
`PINWIN_NAME`, with the default `default`. A name has 1..=64 characters from
`[A-Za-z0-9_-]`. Another process SHALL request a toggle with `pinwin --toggle [name]` and a
show with `pinwin --show [name]`, and the name defaults to `default`. Both reach any panel
bound to that name, whether the `pinwin` program or a library host started it.

If the named instance accepts the request, the client SHALL exit 0. If no instance with that
name answers on the display, or the instance refuses the request, the client SHALL print a
message on stderr and exit 1. A name in `PINWIN_NAME` or after `--toggle` or `--show` can be
invalid. Then `pinwin` SHALL print a message on stderr and exit 2, and the host SHALL open
nothing.

A live instance on the same display can already use the name. Then a second host SHALL print
a message and exit 2 before any surface opens. A name from an instance that no longer runs
SHALL NOT block a new host. Different names and different displays SHALL NOT interfere.

#### Scenario: Toggle the default instance
- **WHEN** `pinwin htop` runs in `on-demand` mode on niri and a compositor key binding runs
  `pinwin --toggle` twice
- **THEN** the first run hides the panel, the second shows it with keyboard focus, and the
  client exits 0 each time

#### Scenario: Toggle a named instance
- **WHEN** `PINWIN_NAME=notes pinwin nvim` and `pinwin htop` both run, and a compositor key
  binding runs `pinwin --toggle notes`
- **THEN** the `notes` panel hides, the other panel does not change, and the client exits 0

#### Scenario: Show from the command line
- **WHEN** `pinwin htop` runs and the user runs `pinwin --show` twice, then `pinwin --toggle`,
  then `pinwin --show`
- **THEN** the panel stays shown after both shows, hides on the toggle, appears again on the
  last show, and each client exits 0

#### Scenario: Toggle a library-hosted panel
- **WHEN** a library host binds the name `notes` and starts its panel, and a key binding runs
  `pinwin --toggle notes`
- **THEN** that panel hides and the client exits 0

#### Scenario: No instance
- **WHEN** no instance with the name `notes` runs and a key binding runs
  `pinwin --toggle notes` or `pinwin --show notes`
- **THEN** the client prints a message and exits 1

#### Scenario: Invalid name
- **WHEN** `PINWIN_NAME=a/b` is set, or the user runs `pinwin --toggle a/b` or
  `pinwin --show a/b`
- **THEN** `pinwin` prints a message, exits 2 and opens nothing

#### Scenario: Duplicate name
- **WHEN** `pinwin htop` runs and the user starts a second `pinwin` with no `PINWIN_NAME`
- **THEN** the second `pinwin` prints a message, exits 2 and opens nothing, and the first
  panel is unchanged

#### Scenario: Instance that crashed
- **WHEN** a `pinwin` named `notes` was killed with SIGKILL and the user starts a new one
  with the same name
- **THEN** the new `pinwin` starts normally and answers `pinwin --toggle notes`
