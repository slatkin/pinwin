# Proposal

## Why

An app in a panel cannot control its own panel well today. A `pinwin` command that the app
runs looks for the daemon socket of the panel's display, where no daemon listens, so it
exits 1. The app also cannot know that it runs in a panel, or the name of its panel. A
change of width and reserve mode needs two requests, so a refused second request leaves the
panel in a mixed state. A launcher that wants "start it, or show it if it runs" must call
`status` first and can race with a second launch. Issue #55 asks for these three additions
for 0.4.1.

## What Changes

- `pinwin run` sets `PINWIN_PANEL=<name>` and `PINWIN_SOCKET=<path of the daemon socket>`
  for the command, together with `WAYLAND_DISPLAY`. A nested `pinwin run` replaces the
  inherited values with those of its own panel.
- Every client use of the daemon socket, `pinwin run` included, uses `PINWIN_SOCKET` when it
  is set and not empty, before `WAYLAND_DISPLAY`. A relative path is an error (exit 1). When
  `PINWIN_SOCKET` is set and no daemon answers, `pinwin run` exits 1 and starts no daemon.
  `pinwin daemon` does not read the variable.
- **BREAKING** (small): a client with `PINWIN_SOCKET` set in its environment now uses that
  path instead of the path from `WAYLAND_DISPLAY`. Without the variable, nothing changes.
- `set-size` and `resize` accept an optional `reserve` field (`push` or `cover`). CLI:
  `pinwin set-size <name> <size> [--reserve push|cover] [--ms N]`, and the same for
  `resize`. The daemon validates the size and the reserve mode together; a refusal changes
  nothing. A request sends one `geometry-changed` event. Without `reserve`, the request is as in
  0.4.0.
- `pinwin run --show-existing`: when a panel with the name exists, the daemon shows it (as
  `show` does), the other run options do not change it, and `pinwin run` exits 0 and runs
  nothing. The check is done in the daemon, so two launches at the same time give one panel.
  Without the flag, a duplicate name exits 2 as in 0.4.0.
- The README names `PINWIN_SOCKET` and `PINWIN_PANEL`, `--reserve` and `--show-existing`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `panel-control`: the run environment, the client socket lookup, the run start of the
  daemon, `--show-existing`, the `reserve` field of `set-size` and `resize`, and the CLI
  grammar.
- `pinwin-panel`: new scenarios for an animated change of width and reserve mode back to a
  pushing state (the strip that equals the held strip, and a new strip). The rule does not
  change.

## Impact

- Code: `src/daemon.rs` (socket lookup), `src/main.rs` (`run_app`, `create_panel`,
  clients), `src/run.rs` (environment), `src/cli.rs` (`--reserve`, `--show-existing`),
  `src/ipc.rs` and `src/ipc/reply.rs` (request fields, reply of an existing panel),
  `src/daemon/connection.rs` (create or show), `src/proxy/apply.rs` (one plan for size
  plus reserve).
- Tests: `src/cli/tests.rs`, `src/ipc/tests.rs`, `src/proxy/apply/tests.rs`,
  `src/daemon/tests.rs`, `tests/command_line_ipc.rs`.
- Docs: `README.md`, the module entries in `AGENTS.md`.
- No new dependency. No library API. Release: 0.4.1.
