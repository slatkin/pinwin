# Spec Delta

## ADDED Requirements

### Requirement: The app's window becomes the panel
A panel SHALL hold at most one window of its app. When the panel holds no window, the next
top-level window that the app creates SHALL become the panel's window. The app SHALL see a
normal window with a fixed size, and SHALL NOT be told that it is a panel.

#### Scenario: First window
- **WHEN** the user runs `pinwin run --name notes -- myeditor` and `myeditor` opens its window
- **THEN** the window shows as the `notes` panel at the panel's edge

#### Scenario: Window comes back
- **WHEN** the app closes its panel window, keeps running, and later opens a new window
- **THEN** the new window shows as the panel with the panel's current settings

### Requirement: Other windows pass through
While the panel holds a window, every other top-level window of the app SHALL open as a normal
compositor window, as it would without pinwin. A window that opened this way SHALL stay a
normal window when the panel's window closes.

#### Scenario: Dialog
- **WHEN** the panel's app opens a file dialog as a new top-level window
- **THEN** the dialog opens as a normal compositor window and the panel does not change

#### Scenario: Passed-through window stays
- **WHEN** the app has its panel window and a second window, and it closes the panel window
- **THEN** the second window stays a normal compositor window

### Requirement: Empty panel shows nothing
While the panel holds no window, nothing of the panel SHALL show and no strip SHALL be
reserved. The panel keeps its settings and its name.

#### Scenario: App closes its window
- **WHEN** a pushing panel's app closes its panel window and keeps running
- **THEN** tiled windows take the panel's strip, and `pinwin status` reports the panel with no
  window

### Requirement: Popups of the panel window
Popups of the panel's window, such as menus and tooltips, SHALL show at the position the app
asks for relative to its window, as they would on a normal window. Popups of other windows
SHALL behave as they would without pinwin.

#### Scenario: Context menu
- **WHEN** the user right-clicks inside the panel window at a point and the app opens a menu
  there
- **THEN** the menu shows at that point, not at an offset

### Requirement: No decorations on the panel window
The panel's window SHALL show no title bar and no frame: the app SHALL be told that the
compositor draws its decorations, and the compositor draws none on a panel. Other windows of
the app SHALL get the same decorations as without pinwin.

#### Scenario: App with its own title bar
- **WHEN** an app that draws its own title bar on a normal window opens its window in a panel
- **THEN** the panel shows the app's content with no title bar

#### Scenario: Dialog keeps its decorations
- **WHEN** the same app opens a second window
- **THEN** that window has the decorations it has without pinwin

### Requirement: Panel lifetime follows the app
A panel SHALL exist from its `pinwin run` until the launched process has exited and the app
has no Wayland connection to the panel left. Then the panel SHALL end: its window goes away,
its strip is released and its name is free.

#### Scenario: App exits
- **WHEN** the panel's app exits
- **THEN** the panel ends and tiled windows take its strip

#### Scenario: App never connects
- **WHEN** the launched process exits without a Wayland connection
- **THEN** the panel ends

#### Scenario: App hands off to a running copy
- **WHEN** the launched app passes its launch to a copy of itself that runs outside pinwin and
  exits
- **THEN** the panel ends, and the window opens in the other copy as a normal window

#### Scenario: Launcher process forks
- **WHEN** the launched process starts a child that keeps the Wayland connection and then exits
- **THEN** the panel stays until the child closes its connection

### Requirement: Apply layout without restarting the app
A successful change of size, edge, gutters, reserve mode, layer, output or keyboard mode SHALL
keep the same app process and the same window. The app SHALL get a new window size only when
the panel's width or length changes.

#### Scenario: Change the edge
- **WHEN** a left panel moves to the right edge with the same size
- **THEN** the same window shows at the right edge, and the app gets no new size

#### Scenario: Change the size
- **WHEN** a panel's size changes from 640 to 800 pixels
- **THEN** the app gets one new window size of 800 pixels wide and keeps running

### Requirement: Output goes away
When the panel's output is removed, the panel SHALL move to the output that the compositor
chooses, with the same settings. The app SHALL keep running and SHALL NOT be asked to close.

#### Scenario: Monitor unplugged
- **WHEN** the panel is on DP-2 and DP-2 is unplugged
- **THEN** the panel shows on another output and the app keeps running

### Requirement: The daemon survives bad input
No app, client request, compositor error or panic inside a panel SHALL end the daemon or
another panel. A failure inside one panel SHALL end only that panel, which closes its app's
Wayland connections. A panel that does not answer a request within a bounded time SHALL make
that request fail with error kind `internal`.

#### Scenario: Panic in one panel
- **WHEN** code that serves one panel panics
- **THEN** that panel ends, other panels keep working, and the daemon answers requests

#### Scenario: App protocol error
- **WHEN** a panel's app sends a request that breaks the Wayland protocol
- **THEN** that app's connection closes, and other panels keep working

### Requirement: Validate a change before applying it
A change to a pushing panel SHALL fail with error kind `invalid_layout` on arithmetic
overflow, a negative reservation, a visible length below one pixel, or a reservation that
leaves no output space for other windows. A change to a covering panel SHALL fail on
arithmetic overflow, a panel wider than the output, or a visible length below one pixel; it
SHALL NOT fail because of the held reservation. A failed change SHALL leave the panel's
settings, window and reservation unchanged.

#### Scenario: No visible length
- **WHEN** a left panel gets Top and Bottom gutters that leave less than one pixel of height
- **THEN** the request fails with `invalid_layout` and nothing changes

#### Scenario: Too wide for the output
- **WHEN** a pushing panel gets a size whose reservation leaves no output width for other
  windows
- **THEN** the request fails with `invalid_layout` and nothing changes

#### Scenario: Covering panel wider than the output
- **WHEN** a covering panel gets a size wider than its output
- **THEN** the request fails with `invalid_layout` and the panel and reservation are unchanged


### Requirement: No intermediate width after a failure
A rejected change SHALL NOT leave the panel at an intermediate width and SHALL leave the
panel's settings unchanged. If an animation cannot start, the panel SHALL snap to the requested
state.

#### Scenario: Invalid change
- **WHEN** a client requests, animated, a size that fails validation
- **THEN** the request fails with `invalid_layout` and the width and settings are unchanged


### Requirement: Show and hide a panel
`toggle`, `show`, `hide` and `focus` SHALL change whether the panel shows. A panel shows at
start unless `hidden` is set. `show` on a shown panel and `hide` on a hidden panel SHALL succeed
and change nothing. Hiding SHALL remove the panel from the screen and release the strip; a
running animation first ends at its target. Showing SHALL put the window back and restore the
strip. The app SHALL keep running and connected while hidden and SHALL NOT be asked to close.

#### Scenario: Hide
- **WHEN** the panel shows and a client toggles it
- **THEN** the panel leaves the screen, keys go to a compositor window, and the app keeps
  running

#### Scenario: Show a shown panel
- **WHEN** the panel shows and a client sends `show`
- **THEN** the reply is a success, the panel stays on screen, and the reservation does not
  change

#### Scenario: Hide releases the reservation
- **WHEN** a pushing panel shows and a client toggles it twice
- **THEN** tiled windows take the strip after the first toggle and move back beside the panel
  after the second

### Requirement: Size and keyboard when a panel shows
Neither hiding nor showing SHALL give the app a new window size, and no frame SHALL show the
panel at another size. While the panel is hidden, a change SHALL be validated and stored with
no animation, and the next show uses it. In `on-demand` mode, `show` and `toggle` SHALL give
the panel the keyboard as a click does. `focus` SHALL show the panel and give it the keyboard
in any mode except `none`.

#### Scenario: Show takes focus
- **WHEN** an `on-demand` panel is hidden, a tiled window has the keyboard, and a client
  toggles the panel
- **THEN** the panel shows and the next typed key goes to the app

#### Scenario: Show in none mode
- **WHEN** a `none` panel is hidden and a client toggles it
- **THEN** the panel shows and the keyboard stays with the window that had it

#### Scenario: Size untouched
- **WHEN** a client toggles a panel twice
- **THEN** the app gets no new window size and no frame shows the panel at another size

#### Scenario: Change while hidden
- **WHEN** the panel is hidden and a client sends `set-size` with a larger size and `ms` 200
- **THEN** the reply is a success, nothing shows, and the next show shows the panel at the new
  size with no animation

## MODIFIED Requirements

### Requirement: Keyboard focus by clicking
The panel SHALL have one keyboard mode: `none`, `on-demand` or `exclusive`. In `on-demand`
mode, the panel gets the keyboard when the user clicks inside it or a show or focus request
shows it, and gives it up when the user clicks a compositor window. At open, an `on-demand`
panel SHALL NOT take the keyboard. In `exclusive` mode, the panel has the keyboard while it
shows. In `none` mode, it never gets the keyboard. `set-keyboard` changes the mode at run
time.

The panel has an optional focus ring: absent, or a colour with a width of 1..=65535 pixels.
While the panel holds keyboard focus and the ring is set, pinwin SHALL draw the ring around the
whole panel window, over the app's content. Without a ring, pinwin SHALL draw nothing over
the app. A focus ring value outside these forms SHALL be an invalid value.

#### Scenario: Click to type
- **WHEN** an `on-demand` panel shows and the user clicks inside it and types `j`
- **THEN** the app receives the `j` key

#### Scenario: Click away
- **WHEN** the panel has keyboard focus and the user clicks a tiled window
- **THEN** keys go to the tiled window, not the panel

#### Scenario: No focus steal on launch
- **WHEN** a panel starts in `on-demand` mode
- **THEN** keyboard input stays with the window that had it

#### Scenario: Opt-in keyboard focus
- **WHEN** a panel starts in `exclusive` mode
- **THEN** at open, the panel has the keyboard without a click

#### Scenario: Change mode at run time
- **WHEN** an `on-demand` panel gets `set-keyboard none` and the user clicks inside it
- **THEN** keys stay with the window that had them

#### Scenario: Invalid keyboard mode
- **WHEN** the user runs `pinwin run --keyboard sometimes -- myeditor`
- **THEN** pinwin prints a message, exits 2 and creates no panel

#### Scenario: Accent appears while focused
- **WHEN** the panel gains keyboard focus with a focus ring of `#dabc7f` and width 2
- **THEN** a 2 pixel `#dabc7f` outline shows around the whole panel window, and it goes away
  when focus moves to a compositor window

#### Scenario: Accent disabled
- **WHEN** the panel's focus ring is `off`
- **THEN** pinwin draws nothing over the app, focused or not

#### Scenario: Invalid accent
- **WHEN** the user runs `pinwin run --focus-ring '#dabc7f:0' -- myeditor`
- **THEN** pinwin prints a message, exits 2 and creates no panel

### Requirement: Wayland layer-shell is required
pinwin SHALL run only on a Wayland compositor that implements wlr-layer-shell. When the
compositor lacks it, or no Wayland compositor answers, the daemon SHALL print a message and
exit 1 at start, and `pinwin run` SHALL print a message and exit 1 without running the
command. pinwin SHALL NOT fall back to a normal window.

#### Scenario: No layer-shell
- **WHEN** the user runs `pinwin run -- myeditor` on a compositor without wlr-layer-shell
- **THEN** pinwin prints a message, exits 1, and `myeditor` does not run

### Requirement: Directional gutters and docking geometry
Gutters SHALL use screen directions: Left, Right, Top and Bottom, in logical pixels. The near
gutter is the one on the panel's edge, the far gutter the opposite one, and the other two are
side gutters. A pushing panel SHALL be inset from its edge by the near gutter and reserve
`near + width + far` pixels at that edge. Side gutters SHALL inset the panel's length from the
output edges at its ends. A covering panel SHALL use the same insets and SHALL NOT change the
reserved strip. The reserved strip SHALL cover the output's full length even when side gutters
inset the panel.

A gutter MAY be negative: then the panel extends beyond its output edge, or the strip ends
before the panel's far side. A pushing reservation SHALL NOT be negative. An edge change SHALL
move the reservation to the new edge, sized by the last pushing size setting resolved on the
new edge. Compositor struts stay additive and SHALL NOT be edited.

#### Scenario: Left docking
- **WHEN** the panel width is 320 pixels, Left is 8, Right is 12 and the edge is left
- **THEN** the panel starts 8 pixels from the left output edge and reserves 340 pixels at that
  edge, before any compositor struts

#### Scenario: Right docking
- **WHEN** the same values apply with the edge right
- **THEN** the panel ends 12 pixels before the right output edge and reserves 340 pixels at
  that edge, and releases its left reservation

#### Scenario: Top docking
- **WHEN** the panel width is 400 pixels, Top is 10, Bottom is 6, Left is 20, Right is 30 and
  the edge is top
- **THEN** the panel starts 10 pixels below the top output edge, is 400 pixels high, starts
  20 pixels from the left output edge, ends 30 pixels before the right output edge, and
  reserves 416 pixels at the top edge across the full output width

#### Scenario: Vertical insets
- **WHEN** a left panel has Top 24 and Bottom 10
- **THEN** the panel begins 24 pixels below the output top and ends 10 pixels above its bottom,
  and the reservation stays a full-height strip

#### Scenario: Negative gutter
- **WHEN** the panel width is 320 pixels, Left is -40, Right is 12 and the edge is left
- **THEN** the panel starts 40 pixels beyond the left output edge and the reservation is 292
  pixels

#### Scenario: Covering layout holds the gap
- **WHEN** the gap is 340 pixels on the left from a pushing layout and the panel changes to
  covering with a width of 1000 pixels on the same edge
- **THEN** the panel is 1000 pixels wide and the reserved strip stays 340 pixels on the left

#### Scenario: Side switch moves the gap
- **WHEN** a covering panel holds a gap on the right from the pushing size `25%` and moves to
  the left edge
- **THEN** the reservation moves to the left edge at 25% of the output width

### Requirement: Dock at one edge of the panel's monitor
The panel SHALL be a layer-shell surface on its layer (default `overlay`), flush against its
edge and both output edges at the ends of that edge, along the output's full length minus the
side gutters, even when a bar reserves an end. A panel's width is its size across its edge;
its length runs along the edge. The panel SHALL stay on its output, show on every workspace of
that output, and SHALL NOT show on other outputs.

#### Scenario: Opens on the chosen output
- **WHEN** the user runs `pinwin run --output DP-2 -- myeditor`
- **THEN** the panel shows on DP-2 and nothing shows on DP-1

#### Scenario: Opens on the focused monitor
- **WHEN** no output is set and DP-2 has focus
- **THEN** the panel shows on the output the compositor chooses for new layer surfaces

#### Scenario: Unknown output
- **WHEN** a client sends `set-output` with an output name that does not exist
- **THEN** the request fails with `invalid_value` and the panel does not move

#### Scenario: Covers an existing top bar without a bottom gap
- **WHEN** a bar reserves the top edge and a left panel has Top 0
- **THEN** the panel covers that bar across its own width and reaches the output's bottom edge

#### Scenario: Visible across workspace switches
- **WHEN** the panel shows and the user switches to another workspace on the same output
- **THEN** the panel stays at its edge without moving or flickering

#### Scenario: Cannot be moved
- **WHEN** the user tries to drag or move the panel with the compositor's window actions
- **THEN** the panel stays at its edge

#### Scenario: Change the layer
- **WHEN** a client sends `set-layer` with `bottom` for a shown panel
- **THEN** the panel shows below normal windows and keeps its window

### Requirement: Reserve space so tiles start beside the panel
The panel SHALL hold one reservation: a strip at one edge of its output. A pushing panel sets
the strip to its edge with the size `near + width + far`, so the compositor places tiled
windows beside it. A covering panel leaves the strip as the last pushing setting made it, so
tiled windows do not move while the panel draws over them. A panel that has never pushed
SHALL reserve nothing. While the panel is hidden or holds no window, the strip SHALL be
released; showing a window SHALL restore it. The strip SHALL apply only to the panel's output.

#### Scenario: Tiles move right
- **WHEN** a panel docks left with width 320, Left 0 and Right 12
- **THEN** tiled windows' left edge moves to at least 332 pixels from the output's left edge

#### Scenario: Other monitors unaffected
- **WHEN** the panel shows on DP-2
- **THEN** tiled windows on DP-1 keep their position

#### Scenario: Covering start reserves nothing
- **WHEN** a panel starts with `--reserve cover`
- **THEN** tiled windows stay where they are until the panel first pushes

#### Scenario: Covering expand holds the strip
- **WHEN** the held strip is 332 pixels on the left and the panel changes to covering at a
  larger width on the same edge
- **THEN** tiled windows keep their position and the panel extends past the strip

#### Scenario: Hidden panel reserves nothing
- **WHEN** the held strip is 332 pixels on the left and the panel hides
- **THEN** tiled windows take the strip, and they move back beside it when the panel shows

### Requirement: Animated width transition
An animated change, with a duration above 0 ms, that changes only the panel's width and
reserve mode (same edge and gutters) SHALL change the visible width continuously over the
duration, driven by the compositor's frame callbacks. The strip SHALL move with the panel only
when the target pushes and its strip differs from the held one. The duration SHALL be clamped
to 1000 ms. The app's content SHALL stay against the panel's edge in every frame. pinwin SHALL
NOT read any desktop animation setting.

#### Scenario: Animate on request
- **WHEN** a pushing panel gets `set-size` from 400 to 600 pixels with `ms` 200
- **THEN** the panel and the strip grow through intermediate widths over about 200 ms and tiled
  windows reflow alongside

#### Scenario: Covering expand holds tiles still
- **WHEN** a panel changes to covering at a larger width with `ms` 200
- **THEN** the panel grows over about 200 ms while the strip holds still and tiled windows do
  not move

#### Scenario: One new size for the app
- **WHEN** an animated change runs from 400 to 600 pixels
- **THEN** the app gets exactly one new window size, 600 pixels wide, for the whole animation

#### Scenario: Grow waits for the app
- **WHEN** an animated grow starts
- **THEN** the panel stays at its current width until the app has drawn at the target width,
  and snaps to the target if the app has not drawn within 500 ms

#### Scenario: Content does not slide
- **WHEN** a right panel shrinks with an animation
- **THEN** the app's content stays against the right edge in every frame

#### Scenario: Snap on zero duration
- **WHEN** a change has `ms` 0
- **THEN** it applies in one step

#### Scenario: Snap when animations are disabled
- **WHEN** the panel's `tween_ms` is 0 and a client sends `set-size` without `ms`
- **THEN** the change applies in one step

#### Scenario: Desktop animation setting has no effect
- **WHEN** the GTK `gtk-enable-animations` setting is false and a change has `ms` 200
- **THEN** the panel width animates over about 200 ms

#### Scenario: No viewport support
- **WHEN** the compositor does not offer the viewporter protocol and a change has `ms` 200
- **THEN** it applies in one step

#### Scenario: Other fields snap
- **WHEN** a change moves the edge or changes a gutter
- **THEN** it applies in one step with no animation

#### Scenario: Duration clamp
- **WHEN** a change has `ms` 60000
- **THEN** the animation lasts at most 1000 ms

### Requirement: Exact final width
When an animation ends, the panel's width SHALL equal the target width. A pushing target SHALL
leave the reservation at `near + width + far`; a covering target SHALL leave it at the held
strip, with no offset left from intermediate frames. The final state SHALL be the same as a
change with `ms` 0 to the same target.

#### Scenario: Animation completes
- **WHEN** an animated pushing change to 600 pixels ends
- **THEN** the panel is exactly 600 pixels wide and the reservation matches

#### Scenario: Covering animation completes
- **WHEN** an animated covering change to 1000 pixels ends over a 400 pixel gap
- **THEN** the panel is exactly 1000 pixels wide and the reservation still equals the 400 pixel
  strip

#### Scenario: Stalled frame clock
- **WHEN** the compositor sends no frame callbacks for the duration plus 100 ms
- **THEN** the panel snaps to the exact target width

### Requirement: Interrupting an animation
A change made while an animation runs SHALL win over it. An animated change SHALL continue from
the current animated width. A change with `ms` 0, a hide, or the end of the panel SHALL stop the
animation; a hide or a change with `ms` 0 leaves the panel at the requested state.

#### Scenario: Retarget
- **WHEN** a shrink animation is half done and a client requests a larger size with `ms` 200
- **THEN** the width continues from its current value toward the larger size

#### Scenario: Snap interrupts
- **WHEN** a client sends a change with `ms` 0 during an animation
- **THEN** the panel snaps to that change and no later animation frame changes it

#### Scenario: Stop mid-animation
- **WHEN** the app exits during an animation
- **THEN** no panel surface remains and no animation frame runs afterwards

## REMOVED Requirements

### Requirement: Show and hide on request
**Reason**: Replaced by "Show and hide a panel"; the old scenarios name columns, terminal allocation or the pty, which are gone.
**Migration**: None needed.

### Requirement: No mid-state on failure
**Reason**: Replaced by "No intermediate width after a failure"; the old scenarios name columns, terminal allocation or the pty, which are gone.
**Migration**: None needed.

### Requirement: Validate before applying
**Reason**: Replaced by "Validate a change before applying it"; the old scenarios name columns, terminal allocation or the pty, which are gone.
**Migration**: None needed.

### Requirement: Font follows the Ghostty config
**Reason**: pinwin no longer draws text. The hosted app chooses its own font.
**Migration**: Set the font in the hosted app, for example in the Ghostty config.

### Requirement: Correct size reports
**Reason**: pinwin no longer has a terminal or a pty. A hosted terminal answers its own size
queries.
**Migration**: Run a terminal app in the panel, for example `pinwin run -- ghostty`.

### Requirement: Terminal features for the host's child
**Reason**: pinwin no longer contains a terminal emulator.
**Migration**: Run a terminal app that has these features in the panel.

### Requirement: Apply layout without restarting the terminal
**Reason**: Replaced by "Apply layout without restarting the app".
**Migration**: None needed; the same rule applies to the hosted app.

### Requirement: Host-owned pty
**Reason**: pinwin no longer owns or reads a pty.
**Migration**: Run a terminal app in the panel; it owns its pty.

### Requirement: The library never exits
**Reason**: The library API is removed. Replaced by "The daemon survives bad input".
**Migration**: Control panels through the daemon's IPC.

### Requirement: No pinwin-owned configuration
**Reason**: pinwin now has a config file and run options (see `panel-control`).
**Migration**: Put panel settings in `~/.config/pinwin/config.toml` or pass them to
`pinwin run`.

### Requirement: Winsize updates signal the host
**Reason**: pinwin no longer owns a pty, so it sends no `SIGWINCH`.
**Migration**: A hosted terminal signals its own child on resize.

### Requirement: Terminal grid during a width animation
**Reason**: pinwin no longer has a terminal grid. "Animated width transition" now covers the
hosted app's content.
**Migration**: None needed.

### Requirement: Library Panel API
**Reason**: The library API with its host-owned pty is removed.
**Migration**: Start a panel with `pinwin run` and control it through the IPC.

### Requirement: The pinwin binary
**Reason**: The `pinwin` command is now the daemon and IPC client described in
`panel-control`. `COLS`, `GUTTER` and the `PINWIN_*` variables are gone.
**Migration**: Use `pinwin run [options] -- <command>`; `--size` replaces `COLS`, the gutter
settings replace `GUTTER`, and `--reserve`, `--keyboard` and `--focus-ring` replace the
`PINWIN_*` variables.

### Requirement: Optimised terminal library build
**Reason**: pinwin no longer builds libghostty-vt or needs Zig.
**Migration**: None needed.

### Requirement: Seamless cell grid at any output scale
**Reason**: pinwin no longer draws a cell grid.
**Migration**: None needed.

### Requirement: Cell text on the device pixel lattice
**Reason**: pinwin no longer draws text.
**Migration**: None needed.

### Requirement: Key repeat follows the compositor
**Reason**: The hosted app gets keys from the compositor directly and does its own key repeat.
**Migration**: None needed.

### Requirement: Pointer cursor over the panel
**Reason**: The hosted app sets its own pointer cursor.
**Migration**: None needed.

### Requirement: Toggle from the command line
**Reason**: Replaced by the `pinwin toggle <name>` and `pinwin show <name>` subcommands of
`panel-control`. `PINWIN_NAME`, `--toggle` and `--show` are gone.
**Migration**: Use `pinwin run --name <name>` and `pinwin toggle <name>`.

### Requirement: Instance socket opt-in for library hosts
**Reason**: The library API and the per-panel instance socket are removed. One daemon socket
serves all panels.
**Migration**: Use the daemon's IPC (`panel-control`).

### Requirement: Instance bind errors
**Reason**: The per-panel instance socket is removed. "Panel names" and "Daemon socket" in
`panel-control` replace these rules.
**Migration**: None needed.

### Requirement: Serving instance requests
**Reason**: The per-panel instance socket is removed. The daemon serves all requests.
**Migration**: Send requests to the daemon socket.

### Requirement: Instance socket lifetime
**Reason**: The per-panel instance socket is removed.
**Migration**: None needed.

### Requirement: Instance client call
**Reason**: The library client call is removed.
**Migration**: Use the `pinwin` subcommands or write JSON lines to the daemon socket.

### Requirement: Kitty images drawn at device resolution
**Reason**: pinwin no longer draws images. A hosted terminal draws its own kitty images.
**Migration**: None needed.
