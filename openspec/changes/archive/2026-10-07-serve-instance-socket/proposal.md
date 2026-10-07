# Proposal

## Why

Only the `pinwin` program serves the instance socket today. A host that embeds the library
through `Panel::start` listens on nothing, so `pinwin --toggle` cannot reach its panel, and the
host would have to rebuild the socket path rules, the duplicate check and the line protocol
itself (issue #30). The handle also offers only a toggle. A host that wants "show the panel if
it is hidden, otherwise do nothing" has no way to ask for that, because nothing reports
whether the panel is shown.

## What Changes

- The library takes over the instance socket. The name type, the socket path, the duplicate
  check, stale-file replacement, the listener and the client move from the binary into a public
  library module. The behavior, the name rules and the protocol stay the same.
- A library host opts in. It binds the instance socket for a name before it starts the panel.
  The bind reports a duplicate as a typed error before any surface opens. Then the host passes
  the bound socket to the start. A start without a socket listens on nothing, as in 0.2.
- A panel started with a socket serves `toggle` and `show` requests until the handle drops.
  The drop removes the socket file and never waits on the listener.
- **BREAKING**: `Startup` is no longer `Copy`, because it can own the bound socket.
- A new `Panel::show()` shows a hidden panel and leaves a shown one unchanged.
- A new `show\n` request line, and `pinwin --show [name]` on the command line.
- A public client call sends `toggle` or `show` to a named instance. Its error type tells "no
  instance with that name" apart from a failed or missing answer.
- The `pinwin` program uses the library's socket instead of its own listener thread.
- The crate version moves to 0.3.0.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: the "Show and hide on request" requirement gains a show request and drops
  "the library owns no transport". "Toggle from the command line" gains `--show` and covers
  panels that library hosts start. A new requirement states the library's instance socket.

## Impact

- `src/ipc.rs` moves into the library as a public module. `src/main.rs` and `src/cli.rs` keep
  only argument parsing, exit codes and the message text.
- `src/panel.rs`, `src/panel/startup.rs`, `src/panel/wayland_side.rs` and the toggle code get
  the show command and the listener thread.
- Public API: a new instance module (name, bind, request, client call and their errors),
  `Startup::with_instance`, `Panel::show`. `Startup` loses `Copy`.
- README: the library section documents the socket opt-in and `--show`.
- No new dependencies.
