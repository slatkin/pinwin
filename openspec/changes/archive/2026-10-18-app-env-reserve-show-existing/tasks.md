# Tasks

Live checks are run by the user. An agent prepares the build and the steps, and records the
user's answer word for word.

Each task updates the README text and the `AGENTS.md` module entries that its change makes
wrong.

## 1. Run environment and client socket lookup

- [x] 1.1 Add `daemon::client_socket_path()` with its pure form and source tag, and
  `PathError::RelativeSocket` (design D1). Use it in `run_app`, `send` and `subscribe`;
  `bind_from_env` keeps `socket_path_from_env`. Update the README sentence about the
  variables pinwin reads, and the `src/daemon.rs` entry in `AGENTS.md`. Verify: unit tests in
  the `src/daemon.rs` tests for a set variable, an empty one (same as unset), a relative one
  (error that names `PINWIN_SOCKET`) and an unset one; and a `tests/command_line_ipc.rs`
  test where `PINWIN_SOCKET` names the running daemon and `WAYLAND_DISPLAY` names another
  display, and `pinwin status` exits 0 (spec "Variable wins over the display",
  "Relative path", "Daemon ignores the variable").
- [x] 1.2 Set `PINWIN_PANEL` and `PINWIN_SOCKET` in `Exec::new` together with
  `WAYLAND_DISPLAY`, with an absolute socket path (D2). Document both variables in the README
  section on `pinwin run`. Verify: a `tests/command_line_ipc.rs` test runs
  `pinwin run --name notes -- env` against the test daemon, reads its output, and checks `PINWIN_PANEL=notes` and a `PINWIN_SOCKET` equal to the daemon's socket path, with
  inherited values of both variables replaced (spec "Run an app", "Run from inside a panel").
- [x] 1.3 Do not start a daemon when the path comes from `PINWIN_SOCKET` (D3). Verify: a
  `tests/command_line_ipc.rs` test with `PINWIN_SOCKET` set to a path where nothing listens:
  `pinwin run` exits 1, prints a message, runs nothing, and no daemon socket appears in the
  test's runtime directory (spec "No daemon at the set socket").

## 2. Size and reserve in one request

- [x] 2.1 Add the optional `reserve` field to `set-size` and `resize` in `src/ipc.rs`, and
  `--reserve push|cover` to both subcommands in `src/cli.rs` (D4). Update the README command
  table rows for `set-size` and `resize`. Verify: `src/ipc/tests.rs` parses lines with and
  without `reserve` and refuses `"reserve":"middle"` with `invalid_value`; a 0.4.0 line
  without the field round-trips unchanged; `src/cli/tests.rs` builds the request from
  `set-size notes 1100px --reserve cover --ms 200` and refuses `--reserve middle` with exit 2
  (spec "Size with a reserve mode", "Invalid reserve option", "Invalid mode").
- [x] 2.2 Apply the size and the reserve mode as one plan in `plan_change` (D4). Verify:
  `src/proxy/apply/tests.rs` cases: push 400 to cover 1100 is one plan with both fields; a
  combined change whose pushing strip leaves no output space is refused with
  `invalid_layout` and the settings stay; a `reserve` equal to the current mode gives the
  same plan as no `reserve`; `resize -700px` with `push` from cover 1100 gives push 400
  (spec "Covering expand in one request", "Refused together", "Same reserve mode",
  "Relative resize with a mode"). One `geometry-changed` event per request is checked with
  the bus in the same tests (spec "One event").
- [x] 2.3 Hold the strip in a change from covering to pushing, and compute the end gap from
  the target mode (D5). Verify: `src/geometry.rs` and `src/proxy/tween.rs` unit tests: cover
  1100 with held 400 to push 400 keeps the strip at 400 in every frame and at the end; cover
  1100 with held 400 to push 500 keeps 400 in every frame and ends at 500; push 400 to push
  600 still moves the strip with the panel; a covering target still ends at the held strip
  (spec "Collapse back to the held strip", "Collapse to a new pushing width",
  "Animate on request", "Covering animation completes").

## 3. Show an existing panel

- [x] 3.1 Add `--show-existing` to `pinwin run` and `show_existing` to the `create` request,
  the `existing` reply, and the create-or-show order of D6 in `src/daemon/connection.rs`;
  `run_app` exits 0 on `existing` without exec. Document the flag in the README section on
  `pinwin run`, and update the `src/main.rs`, `src/ipc.rs` and `src/daemon/connection.rs`
  entries in `AGENTS.md`. Verify: `src/cli/tests.rs` parses the flag; `src/ipc/tests.rs`
  round-trips `create` with and without it; `tests/command_line_ipc.rs` tests: a hidden
  panel shows and the second `pinwin run` exits 0 without running its command; a shown panel
  stays; `--edge`, `--size` and `--hidden` do not change the existing panel's `status`; no
  panel with the name starts a new one; two launches at the same time give one panel and
  both exit 0; `--show-existing -- my.app` exits 2 (spec "Show an existing panel",
  "One decision for a duplicate launch").

## 4. Gates and live check

- [x] 4.1 Run the gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo audit`, `cargo nextest run` and `make check-code-file-lines`. Verify:
  all pass.
- [x] 4.2 Live check, run by the user: a pushing panel 400 px wide; then
  `pinwin set-size <name> 1100px --reserve cover --ms 200` (tiles do not move);
  `pinwin set-size <name> 400px --reserve push --ms 200` (tiles do not move);
  expand again, then `pinwin set-size <name> 500px --reserve push --ms 200` (tiles move once,
  at the end); from a terminal panel, `pinwin status` and `pinwin run --name <name>
  --show-existing -- <app>` after a hide. Record the user's answer here.
  User's answer: "live checks are pass".

## 5. Gutters

- [x] 5.1 Add `--gutter-top N`, `--gutter-bottom N`, `--gutter-left N` and
  `--gutter-right N` to `pinwin run` (design D7): parse each as an `i32` in `src/cli.rs`,
  add the four fields to `WireOptions` in `src/ipc/options.rs` and copy them in
  `wire_options` and `parse`, so the `create` request carries them and the existing
  `config::resolve` precedence makes an option win over `[panel.<name>]` and
  `[panel.defaults]`. Update the README run-options table and the `src/cli.rs`,
  `src/ipc/options.rs` and `src/config.rs` entries in `AGENTS.md`. Verify: `src/cli/tests.rs`
  parses `--gutter-left -12 --gutter-top 8` into `PanelOptions` and refuses
  `--gutter-left 1.5` with exit 2; `src/ipc/tests.rs` round-trips a `create` with all four
  gutter fields and with none (absent, as 0.4.0), and refuses a fractional gutter with
  `invalid_value`; `src/config/tests.rs` checks that the option wins over the config file
  for a gutter (spec "Run options", "Panel settings and precedence", "Option wins").
- [x] 5.2 Add the `set-gutters` command (design D7): `Request::SetGutters` and
  `RawRequest::SetGutters` in `src/ipc.rs`, the `set-gutters` subcommand in `src/cli.rs`,
  and the `plan_change` arm with its `is_layout_request` entry in `src/proxy/apply.rs`; add
  the `Gutters` accessors in `src/geometry.rs` and replace the apply module comment that says
  gutters have no command. A request with no gutter field, or a non-integer one, is
  `invalid_value`; the plan validates through the existing `set-size` path, applies with no
  animation, and a hidden panel stores the settings. Update the README command table and the
  `src/ipc.rs`, `src/cli.rs`, `src/geometry.rs` and `src/proxy/apply.rs` entries in
  `AGENTS.md`. Verify: `src/ipc/tests.rs` parses a line with one, several and no gutter
  fields (the last `invalid_value`) and refuses `"top":1.5` with `invalid_value`;
  `src/cli/tests.rs` builds `set-gutters notes --top 8 --bottom 8` and refuses no option or
  `--top 1.5` with exit 2; `src/proxy/apply/tests.rs` cases: a left panel with Top 0 gets Top
  8 and Bottom 8 as one plan with the new margins and no animation; a pushing left panel
  whose `right` leaves no output width is refused with `invalid_layout` and the settings and
  gap stay; a hidden panel stores the new gutters and its next show uses them; a near-gutter
  change sends no new app size while a side-gutter change sends exactly one (spec "Set
  gutters", "Panel commands", "Change while hidden", "Other fields snap", "Validate a
  change before applying it").
- [x] 5.3 Run the gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo audit`, `cargo nextest run` and `make check-code-file-lines`. Verify:
  all pass.
- [x] 5.4 Live check, run by the user: a running panel with Top 0; then
  `pinwin set-gutters <name> --top 8 --bottom 8` (the panel's edges move in one step and the
  app keeps its window); a pushing panel and `pinwin set-gutters <name> --right <large>`
  (the reply is `invalid_layout` and nothing moves); `pinwin run --name <name>
  --gutter-left 12 -- <app>` while `[panel.<name>]` sets `gutter_left = 4` (the panel starts
  12 pixels from the left edge). Record the user's answer here.
  User's answer: "live checks are pass".

## Workflow follow-up

- Archive the change after the review.
- Release 0.4.1 with the release workflow.
