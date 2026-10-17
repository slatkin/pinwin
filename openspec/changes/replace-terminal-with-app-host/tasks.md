# Tasks

Live checks are run by the user. An agent prepares the build and the steps, and records the
user's answer word for word. If the user says a live result is not acceptable, the work
stops at that task and the design is reopened.

Each task that adds or removes a module also updates the module list in AGENTS.md.

## 1. Spike: prove the proxy mechanics live

The spike is a throwaway crate outside this repository (a copy of `wl-paper` changed as
below). Its code does not enter pinwin; only the findings do.

- [x] 1.1 Change the spike to anchor one edge plus both ends, set a width, set exclusive zone
  -1, and add a second transparent layer surface with the exclusive zone (design D4, D9). The
  user runs it with a GTK app and a terminal app, and checks: the window docks, tiled windows
  move beside it, right-click menus open at the pointer, a second window opens as a normal
  window, and `on-demand` keyboard focus works. Record the answer here.

  Recorded answer (the user's words, word for word):

  - paw (foot fork): "1. Yes" (docks, full height); "4. Yes" (second window normal);
    "5. Yes" (keyboard). On the tiled-window check: "A new window starts to the right of
    this terminal." Menus: "Paw does not have a menu".
  - galculator (GTK3), first run: "it works. it is not fully top to bottom like paw, the
    top has a gap. the bottom runs off the screen." Keyboard: "keyboard does work. it was
    a calculator, i had to press numbers lol".
  - galculator, rerun after the spike fix: "everything looks fine now".
  - code (Electron): "it did NOT work with code". Cause not known; parked for the viewport
    work in row 1.3.

  Not run: the right-click menu check was not run with an app that has menus (paw has no
  menu; the user did not report a menu test for galculator), and the second-window check
  ran only with paw, a terminal, not with a GUI app. The user did not report these two
  checks.
- [x] 1.2 Add hide and show by unmap with a null buffer, with commits stripped of `attach` and
  fake `wl_buffer.release` while hidden (D6). The user toggles 20 times with the app drawing,
  and checks: no protocol error, no wrong-size frame, the app redraws on show. Record the
  answer here.

  Recorded answer (the user's words, word for word):

  - galculator, first build: "show hide is fail. it hid it but then shows never showed it
    again. it would reserve space for it, but nothing appeared."
  - galculator, after the kept-buffer fix: "came back the second and third time, then:"
    followed by a GTK abort (Gdk-WARNING buffer_release_callback, then a
    cairo_surface_reference assertion, SIGABRT).
  - galculator, after the release-accounting fix: "the first test is pass all the way."
  - paw yes, first build: "second one fails on execute" (paw and kitty died before the
    first configure; `kitty yes` printed an EGL error).
  - paw yes, after the viewport fix: "the paw test is pass".
  - code: "nothing." (no window appeared; the cause was not found; parked for row 1.3).

  Kitty was not reported after the viewport fix.
- [x] 1.3 Add one grow and one shrink tween with the viewport crop and the piggyback-or-inject
  commit rule (D7, D8), with and without an app that sets its own viewport (a browser). The
  user checks: smooth width change, content stays at the docked edge, the app gets one size.
  Record the answer here, and update design.md where a finding changes a decision.

  Recorded answer (the user's words, word for word):

  - galculator, first tween build: "the tween is a bit of a fail. the expand if janky, it
    happens in like two frames, first half-expanded, then full. then the collapse is
    animated. ensure WM animation rules not interfering as well."
  - galculator, after the commit-time grow detection: "all is smooth".
  - paw yes, first build: died with `wp_viewport for this surface already exists` (the
    user pasted the log).
  - brave-origin: "it works, but interestingly when it expands it scales differently and
    looks blurrier". With fractional scale off: "not blurry at scale 1, all good at
    scale 1. however, i would prefer that it do support fractional scale".
  - After the fractional fix, about paw: "this does not seem to affect paw". (The sentence
    "no it looks blurry when launched in spike compared to normally launched" was about
    zeditor, see below.)
  - zeditor, first runs: "zeditor is the one that looks blurry when i launch it." Later
    the run died: "no, something tries to launch and there is brief reservation of space
    and it immediately ends". Cause: `zeditor` is a launcher that exits 0; not a proxy
    fault. Run as `zeditor --foreground`.
  - `zeditor --foreground`, after the syncobj and buffer-bound fixes: "yes to all" (it
    shows in the panel, the text is sharp, grow and shrink are smooth).
  - Final run with galculator, `paw yes` and meld: "all pass".
  - meld: "everything is bigger in spike with meld vs meld normally" (earlier run; no
    trace was taken). Resolved: the user answered "yes" to "does all pass include meld
    now looking the right size".
  - Single-instance apps (Brave, code, Firefox) hand a launch to a running copy and exit;
    the proxy never sees the window. The user: "hi motherfucker, if this panel isn't
    going to work if an existing binary is running then it needs to fuck off, spike
    failed" and "I AM NOT CLOSING EVERY BRAVE WINDOW". Tests must run beside the user's
    open apps.

  Section 1 is a spike outside this repository. There is no commit range, so there is no
  `reviewer` round for it.

## 2. Remove the terminal and the library

- [x] 2.1 Remove the terminal code. Move the rounding and crop-rect parts of
  `src/panel/wayland_side/crop.rs` to `src/proxy/crop.rs` (without `draw_wide`,
  `upload_wide`, `copy_crop` and their tests); keep `src/anim.rs`. Delete `src/term*`,
  `src/render*`, `src/pty*`, `src/fontconfig*`, `src/nerd_font.rs`, `src/panel*`,
  `src/settings.rs`, `src/lib.rs`, `examples/`, `.cargo/config.toml`,
  `tests/command_line_ipc.rs` (5.6 writes a new one) and the `[lib]` section. Leave a
  `main.rs` that prints usage and exits 2. Remove the deleted modules and the demo from
  AGENTS.md and README.md. Verify: `cargo build`, `cargo clippy --all-targets -- -D warnings`
  and `cargo nextest run` (the moved crop tests) pass, and
  `rg -l 'ghostty|libghostty' src Cargo.toml` finds nothing.
- [x] 2.2 Remove the dependencies listed in design D15, drop the Zig setup from
  `.github/workflows/build.yml` and AGENTS.md, change `Cargo.toml` `license` to
  `GPL-3.0-or-later`, replace `LICENSE` with the GPL-3.0 text, and update the description and
  keywords (no terminal, no niri). Verify:
  `cargo tree | rg -i 'ghostty|smithay|calloop|swash|fontconfig'` prints nothing,
  `cargo audit` passes, and `cargo metadata --format-version 1 --no-deps | rg GPL-3.0`
  matches.

## 3. Geometry for four edges and pixel sizes

- [x] 3.1 Create `src/geometry.rs` from `src/layout.rs` and `src/surfaces/gap.rs` (D9): `Edge`
  with four values, size as pixels or percent, four gutters, reserve mode, held gap. Delete the
  old modules. Verify: unit tests for the spec scenarios "Left docking", "Right docking",
  "Top docking", "Vertical insets", "Negative gutter", "Covering layout holds the gap" and
  "Side switch moves the gap" pass.
- [x] 3.2 Add validation (spec "Validate a change before applying it") and percent resolution
  (spec "Size values"). Verify: unit tests for "No visible length", "Too wide for the
  output", "Covering panel wider than the output", "Percent of the output", "Percent follows
  the output" and "Invalid size" pass.
- [x] 3.3 Adapt `src/proxy/crop.rs` to an `Edge`, so top and bottom panels crop the height.
  Verify: crop unit tests pass for all four edges, including a right and a bottom shrink that
  stays at the docked edge.

## 4. Names, config file and command-line parsing

- [x] 4.1 Create `src/name.rs` from the name rules in `src/instance.rs`, with the default name
  from the command's file name. Verify: unit tests for "Default name" and "Invalid default
  name" pass.
- [x] 4.2 Create `src/config.rs` (D13): the `[daemon]`, `[panel.defaults]` and
  `[panel.<name>]` tables, the built-in defaults and the precedence. Document the file and
  every key in README.md. Verify: unit tests for "No config file", "Invalid config file"
  (message has the line), "Per-name override", "Option wins" and "Defaults" pass, and every
  key in the README table exists in `config.rs`.
- [x] 4.3 Rewrite `src/cli.rs` for `run`, `daemon` and the client subcommands, with the
  options of spec "Run options" and "Command-line client". Document the options and
  subcommands in README.md, with the migration table of design "Migration Plan". Verify: unit
  tests for "Invalid option value", "No command", "Usage error" and an invalid `--focus-ring`
  (exit 2) pass, and every README subcommand exists in `cli.rs`.

## 5. IPC and daemon core

- [x] 5.1 Create `src/ipc.rs` (D13): request enum, reply and error kinds, line limit, client
  call with a bounded wait. Verify: unit tests for "Valid request", "Not JSON", "Unknown
  command" and an over-long line pass.
- [x] 5.2 Create the daemon socket (spec "Daemon socket", "One daemon for each Wayland
  display") from the bind and stale-file code in `src/instance.rs`; then delete
  `src/instance*`. Verify: tests for "Stale socket file", "Second daemon", "Two displays",
  "No runtime directory" and the 0600 mode pass, with the socket in a temp
  `XDG_RUNTIME_DIR`.
- [x] 5.3 Add the registry and command routing (D12): panel handles are a channel plus an
  eventfd; replies use a bounded wait. Verify: tests with a test thread in the panel's place
  pass for "Unknown panel", "Duplicate name", "Name is free again", "Status of all panels",
  "Status of one panel" and a panel that does not answer (error `internal`).
- [ ] 5.4 Add the event bus and `subscribe` (spec "Event stream"). Verify: tests for "Toggle
  events", "Slow subscriber" and "Silent client" pass.
- [ ] 5.5 Add daemon lifetime: `exit_when_idle`, `quit`, SIGINT and SIGTERM, socket removal,
  and `reload` (spec "Reload", "Runtime changes stay in memory"). Verify: tests for "Exit with
  the last panel", "Stay resident", "Quit with panels", "Reload a valid file" and "Reload an
  invalid file" pass.
- [ ] 5.6 Wire the client subcommands and exit codes (spec "Command-line exit codes").
  Write a new `tests/command_line_ipc.rs` that runs the binary against a daemon in a temp
  runtime directory. Verify: `cargo nextest run` passes "No daemon", "Unknown panel" and
  "Usage error" with exit codes 1, 1 and 2.

## 6. Proxy core: one panel thread per panel

- [ ] 6.1 Create `src/proxy.rs` (D1): one `State` with an acceptor, poll on `poll_fd`, the
  command eventfd and the pidfd, the whole loop under the panic guard. Add the layer-shell
  check at daemon start (D12). Verify: unit test that a panic in the loop ends only that
  panel's handle; live: the user runs `pinwin daemon` on a compositor without layer-shell (or
  with `WAYLAND_DISPLAY` set to a missing socket) and sees exit 1 with a message.
- [ ] 6.2 Add window routing (D4) with the slot state as a pure type, and the configure and
  ack mapping copied from wl-paper with a GPL-3.0 source note. Document in README.md how a
  panel picks its window, that other windows pass through, and that apps which hand a launch
  to a running copy do not enter a panel (for example Ghostty needs
  `--gtk-single-instance=false`). Verify: unit tests for the slot rule pass ("First window",
  "Window comes back", "Dialog", "Passed-through window stays", "Empty panel shows nothing");
  live: the user runs an app with two windows and confirms the same.
- [ ] 6.3 Add popups and per-window decorations (D5). Verify: unit tests for the popup anchor
  offset and the decoration routing pass; live: the user confirms "Context menu", "App with
  its own title bar" and "Dialog keeps its decorations".
- [ ] 6.4 Add the reserve surface and geometry apply: anchors, margins, size, layer and zones
  from `geometry.rs`, and output size from `zxdg_output_v1` (D9). Verify: live, the user
  confirms "Tiles move right", "Other monitors unaffected", "Covers an existing top bar
  without a bottom gap", "Top docking" and "Change the layer".
- [ ] 6.5 Add output handling (D11): `set-output`, unknown output, and a new layer surface on
  `closed`. Verify: unit test for "Unknown output"; live, the user confirms "Opens on the
  chosen output" and "Monitor unplugged".
- [ ] 6.6 Add "Apply layout without restarting the app": edge, size, gutters, reserve and layer
  changes on a live panel. Verify: live, the user confirms "Change the edge" (no new size for
  the app) and "Change the size" (one new size).

## 7. `pinwin run` and panel lifetime

- [ ] 7.1 Add the `create` request and the `run` flow (D2): resolve options, ask the daemon,
  set `WAYLAND_DISPLAY`, `exec`; exit 127 when `exec` fails. Verify: unit test of the
  environment and argument hand-over; live, the user confirms "Run an app" (working folder and
  environment kept) and "Command not found".
- [ ] 7.2 Add the daemon spawn with `setsid` and the bounded wait (D12). Verify: test that two
  `pinwin run` calls at once against a temp runtime directory leave one daemon ("Two first
  runs"); live, the user confirms "First run" (closing the terminal keeps the daemon).
- [ ] 7.3 Add panel lifetime from the pidfd and the connection count (D3). Verify: unit test
  of the end rule (exited and no connection); live, the user confirms "App exits", "App never
  connects", "App hands off to a running copy" and "Launcher process forks".

## 8. Show, hide, focus and keyboard modes

- [ ] 8.1 Add the commit coordinator (D7): dirty flag, queued requests, inject or piggyback.
  Verify: unit tests pass for a clean surface (inject), a dirty surface (queue until the app's
  commit) and two queued changes in order.
- [ ] 8.2 Add hide and show (D6) with buffer release while hidden, and `--hidden`. Verify:
  live, the user confirms "Hide", "Show a shown panel", "Hide releases the reservation",
  "Size untouched", "Change while hidden" and "Start hidden".
- [ ] 8.3 Add keyboard modes, `set-keyboard` and `focus`. Verify: live, the user confirms
  "Click to type", "Click away", "No focus steal on launch", "Opt-in keyboard focus",
  "Change mode at run time", "Show takes focus", "Show in none mode" and "Focus".

## 9. Focus ring

- [ ] 9.1 Add the ring as four desynchronized subsurfaces with shm buffers and empty input
  regions, shown on `wl_keyboard.enter` (D10). Verify: unit test of the four rectangles for
  each edge and width; live, the user confirms "Accent appears while focused" and "Accent
  disabled", and that clicks on the ring reach the app.

## 10. Size animation

- [ ] 10.1 Add the tween (D8): grow waits for the app's buffer (500 ms bound), shrink crops the
  old buffer, intermediate configures acked by the proxy, viewport composed with the app's
  own. Verify: unit tests for the viewport composition and the "one size per tween" rule;
  live, the user confirms "Animate on request", "Covering expand holds tiles still", "One new
  size for the app", "Grow waits for the app" and "Content does not slide" on a left and a
  top panel.
- [ ] 10.2 Add the end and interrupt rules (spec "Exact final width", "Interrupting an
  animation", "No intermediate width after a failure"), the 1000 ms clamp, `tween_ms` and the
  no-viewporter snap. Verify: unit tests for "Duration clamp", "Retarget", "Snap interrupts"
  and "Stalled frame clock" pass; live, the user confirms "Animation completes" and "Stop
  mid-animation".

## 11. Tray

- [ ] 11.1 Add `src/tray.rs` with ksni (`blocking`, `async-io`, no tokio) (D14): one item while
  panels exist, a submenu per panel, "Quit pinwin". Document the icon, its menu and that it
  hides without panels in README.md. Verify: unit test that the menu model lists panels in
  start order with the current state; the daemon runs and answers `pinwin status` with
  `DBUS_SESSION_BUS_ADDRESS` set to a missing socket ("No tray host"); live, the user
  confirms "Icon with panels", "No icon without panels", "Icon comes back", "Hide from the
  menu", "Menu follows the IPC", "Quit" and "Primary click".

## 12. Integration

- [ ] 12.1 Run all gates and the toolkit check: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo audit`, `cargo nextest run`,
  `make check-code-file-lines` and `ldd target/release/pinwin`. Verify: all gates pass, and
  `ldd` lists no GTK, GDK, GLib, GObject, GIO, Pango, Cairo, gdk-pixbuf or HarfBuzz library.
- [ ] 12.2 Live end-to-end check by the user: two panels (a terminal on the left, a GTK app on
  the top), toggles from a key binding, the tray menu, a size animation and `pinwin quit`.
  Record the answer here.

## Workflow follow-up

- Update the Purpose of `openspec/specs/pinwin-panel/spec.md` after archive: pinwin docks a
  Wayland app at one edge of an output.
- Archive the change after the review.
