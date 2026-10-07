# Design

## Context

`src/ipc.rs` belongs to the binary crate. It holds `InstanceName`, `socket_path`,
`bind_instance_socket` (bind first; probe a taken path; unlink only a dead owner), the bounded
line I/O, `serve_toggle_requests` and `toggle_client`. `src/main.rs` binds before `forkpty`,
while the process is still single-threaded, starts the panel, and then runs the listener on a
scoped thread until the child exits (`serve_toggle_until_exit`). The library owns no transport,
and the spec says so ("Show and hide on request").

On the panel thread, `PanelState::toggle` branches on `Visibility` (`wayland_side/toggle.rs`).
`PanelThread::toggle` posts `PanelCommand::Toggle` and waits for a bounded reply.

## Goals / Non-Goals

**Goals:**

- One implementation of the name rules, the path rules, the bind and the protocol, in the
  library, used by both the binary and library hosts.
- A host keeps no IPC or shown state of its own.

**Non-Goals:**

- A shown-state query, a `hide` request, or requests with arguments.
- More than one socket per panel, or a socket without a panel.
- Any change to the bind rules, the path layout, the timeouts or the reply lines.

## Decisions

### 1. Bind is a separate step; the start takes the bound socket

`pinwin::instance::InstanceSocket::bind(&InstanceName)` reads `XDG_RUNTIME_DIR` and
`WAYLAND_DISPLAY`, binds, and returns the socket or an `InstanceError`. Then
`Startup::with_instance(socket)` hands it to `Panel::start`.

The alternative was a name on `Startup`, with `Panel::start` binding and returning a new
`PinwinError::Duplicate`. It loses for two reasons. The binary must detect a duplicate before
`forkpty`, while it is single-threaded and has no child to hang up. A start that binds would
force the binary to fork after the panel thread exists. Also, a host can check the name before
it does any other startup work. The duplicate stays a typed error, `InstanceError::Duplicate`,
returned before any surface opens.

`Startup` loses `Copy`, because the socket is an owned fd. `Panel::start` takes the socket out
of the startup before it builds the `StartCommand`, so the panel thread never sees it.

### 2. A listener thread that the panel owns, detached on drop

After the handshake succeeds, `Panel::start` spawns one thread, `pinwin-instance`. It runs
the existing accept loop over a clone of the panel's `Arc<Inner>` and a clone of the command
sender. It does not borrow `Panel`, so the thread can outlive a drop. `Panel` keeps an
`Arc<AtomicBool>` shutdown flag and the `SocketFile`.

`Drop` sets the flag and removes the socket file, then runs the existing teardown. It does not
join. The thread notices the flag within one accept poll (100 ms) and closes the listener fd.
A request that is in flight when the drop happens finds `live` cleared and answers `error`.
Process exit never waits on a detached thread.

If the thread spawn fails, the start drops the new panel, which removes the file, and returns
`Internal`. If the start fails, dropping the unused socket removes its file (`SocketFile` owns
the path).

The alternative was a calloop source on the panel thread. It needs no extra thread, but then
socket reads, even bounded ones, stall rendering for up to 500 ms per slow client.

### 3. Requests are an enum; the protocol stays line-based

`pub enum Request { Toggle, Show }` maps to `toggle\n` and `show\n`. The listener parses
the line to a `Request` and calls a callback with it. The peer-gone check stays in front of
every request. A late `show` would be harmless, but one rule is simpler than two.

### 4. The client call and its error

`pinwin::instance::send(&InstanceName, Request) -> Result<(), SendError>`, where `SendError`
is `#[non_exhaustive]`:

- `Environment(PathError)`: no usable socket path.
- `NotListening`: the connect failed with `ENOENT` or `ECONNREFUSED`.
- `Refused`: the instance answered `error\n`.
- `NoAnswer`: any other connect error, a timeout, or an invalid reply.

The binary maps `Environment` to exit 2 and every other error to exit 1. It prints the same
message text as today. `--show` reuses the `--toggle` parsing, with its own label in the
name error.

### 5. Typed errors replace the binary's strings

The library returns typed errors, `InvalidName`, `PathError` and `InstanceError`, each with a
`Display` and `std::error::Error`. The binary adds the `pinwin: PINWIN_NAME:` and
`pinwin: --toggle:` prefixes. The library takes no context label.

### 6. `Panel::show` is a new posted command

`PanelCommand::Show` calls `PanelState::show()` only when the panel is `Hidden`. When it is
`Shown`, the command answers at once with no commit. It has the same guard and the same
order of checks (poisoned, then live, then a bounded wait) as the toggle, through a shared
helper, so the two cannot drift apart.

### 7. Module placement

`src/ipc.rs` moves to `src/instance.rs` (`pub mod instance` in `lib.rs`), and its tests move
to `src/instance/tests.rs`. The module is about 410 lines today and about 550–600 after this
change, under the 800-line limit that `scripts/check-code-file-lines.sh` enforces, so it is not
split. The binary keeps no `ipc` module.

The environment-reading entry points (`InstanceSocket::bind`, `send`) are thin wrappers over
crate-private path-taking ones (`bind_at`, `send_to`). Tests use the path-taking ones with the
existing `temp_dir` helper, so they never touch the process environment.

## Risks / Trade-offs

- [`Startup` loses `Copy`, which breaks hosts that copy it] → The crate version goes to
  0.3.0, and the README notes the change.
- [A detached thread holds the listener fd for up to 100 ms after a drop] → The file is
  removed first, so a new bind of the same name succeeds at once. The old fd only refuses.
- [A request lands between the drop's flag and the thread's exit] → `live` is cleared by the
  teardown, so the request answers `error` and never touches the next panel. A new panel has
  its own `Inner`.

## Migration Plan

This is a library break: `Startup` is no longer `Copy`, and the public module is new. The
release is tag 0.3.0. To roll back, pin 0.2.0.
