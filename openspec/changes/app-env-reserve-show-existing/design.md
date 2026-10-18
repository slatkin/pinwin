# Design

## Context

See proposal.md for the reasons. The specs in this change define the behavior: the
`specs/panel-control` and `specs/pinwin-panel` deltas.

Current code that this design changes:

- `src/daemon.rs`: `socket_path_from_env` builds `$XDG_RUNTIME_DIR/pinwin/<display>.sock`
  from `WAYLAND_DISPLAY`. Every client in `src/main.rs` (`run_app`, `send`, `subscribe`)
  calls it. The daemon binds with `DaemonSocket::bind_from_env`, which uses the same path.
- `src/main.rs`: `create_panel` sends `create`, and on a connect error starts a daemon with
  `daemon::spawn::start_and_wait(path)`. The started daemon inherits the caller's
  environment, so inside a panel it would take the panel's `WAYLAND_DISPLAY` as its display.
- `src/run.rs`: `Exec::new(command, display)` sets only `WAYLAND_DISPLAY`.
- `src/ipc.rs`: `Request::SetSize { panel, size, ms }`, `Request::Resize { panel, by, ms }`,
  `Request::Create { panel, pid, app, options }`. `Create` is sent only by `pinwin run`.
- `src/daemon/connection.rs`: the `Create` arm calls `proxy::spawn`. The registry refuses a
  name in use with `DuplicateName` (`src/daemon/lifetime.rs` `add_panel`), and `run_app` maps
  that kind to exit 2.
- `src/proxy/apply.rs`: `plan_change` builds the next `PanelSettings` from one request,
  validates the geometry, and `apply_plan` applies it and publishes one `geometry-changed`
  event. `Geometry` already carries the reserve mode, so one plan can change both fields.
- `src/proxy/tween.rs` and `src/geometry.rs`: `HeldGap::tweens_with_panel` decides if the strip
  follows the visible width (pushing target, strip differs from the resting strip).
  `Tween` uses `gap_tweening` for each frame and for the gap at the end
  (`resting_gap.reserve(self.gap_tweening, self.geometry, self.to)`).

## Goals / Non-Goals

**Goals:**

- No new IPC command. The new fields are optional, so a 0.4.0 request line still parses and
  does the same.
- The client takes `PINWIN_SOCKET` from its own environment; the daemon's socket path does
  not change.

**Non-Goals:**

- No daemon socket path from `PINWIN_SOCKET` for `pinwin daemon`.
- No check that `PINWIN_SOCKET` is inside `$XDG_RUNTIME_DIR/pinwin/`. The variable comes from
  the user's own environment, and the socket file is owner-only.
- No `PINWIN_PANEL` check by the client. The variable is for the app.

## Decisions

### D1. One client lookup function

Add `daemon::client_socket_path()` and a source tag:

- `PINWIN_SOCKET` set and not empty: an absolute path is `(path, Source::Variable)`; a
  relative path is a new `PathError::RelativeSocket` whose message names `PINWIN_SOCKET`.
- Otherwise: `socket_path_from_env()` as today, `(path, Source::Display)`.

`run_app`, `send` and `subscribe` call it. `DaemonSocket::bind_from_env` keeps
`socket_path_from_env`, so `pinwin daemon` does not read the variable. A pure
`client_socket_path_from(socket, runtime_dir, display)` takes the values as arguments, so the
unit tests do not change the process environment.

Alternative: put the variable inside `socket_path`. Rejected: the daemon would then read it
too.

### D2. The client sets the environment

`Exec::new` takes the panel name and the socket path together with the display, and sets
`WAYLAND_DISPLAY`, `PINWIN_PANEL` and `PINWIN_SOCKET`. `Command::env` replaces inherited
values, so a nested `pinwin run` gives its app its own values. The path is the one that
`pinwin run` used for `create`. It is absolute: either `PINWIN_SOCKET` (D1 refuses a relative
one) or `$XDG_RUNTIME_DIR/pinwin/...`. When `XDG_RUNTIME_DIR` is relative, `pinwin run` makes
the path absolute against its working folder before it sets the variable.

Alternative: the daemon reports its socket path in the `create` reply. Rejected: the client
already has the path, and the reply stays as it is.

### D3. No daemon start for a set socket

`create_panel` takes the source from D1. On a connect error with `Source::Variable`, it
returns a message ("no pinwin daemon answers at the PINWIN_SOCKET path ...") and starts no
daemon, so `run_app` exits 1. With `Source::Display`, it does what it does today. Because
the socket path comes from the outer display, a daemon that was restarted binds the same path,
and the variable stays valid.

### D4. Optional `reserve` on `set-size` and `resize`

`RawRequest::SetSize` and `RawRequest::Resize` get `reserve: Option<String>`, parsed with the
`setting("reserve", ...)` grammar (`invalid_value` for a bad value). `Request::SetSize` and
`Request::Resize` get `reserve: Option<ReserveMode>` with `skip_serializing_if`. In
`plan_change`, the `SetSize` and `Resize` arms set `size` and, when given, `reserve` on the
next settings. The rest of the plan does not change: one validation of the combined
geometry, one `apply_plan`, one `geometry-changed` event. A `reserve` equal to the current
mode gives the same plan as no `reserve`.

The CLI parses `--reserve push|cover` for `set-size` and `resize` beside `--ms`, in any order
after the positional arguments.

### D5. Hold the strip in a change from covering to pushing

The spec rule A: the strip follows the visible width only when the panel pushed before the
change. Two parts:

1. `HeldGap::tweens_with_panel` gets the current reserve mode and returns `false` when it is
   `cover`. The plan passes `current.reserve()`.
2. The gap at the end is not tied to `gap_tweening`. A pushing target always ends at
   `geometry.strip_at(to)` and a covering target at the resting gap (spec "Exact final
   width"). `Tween` keeps `gap_tweening` for the frames and computes the end gap from the
   target mode. The grow handshake does the same, so a snap of an unanswered grow lands on
   the same state.

With the strip equal to the held strip (the main collapse), nothing moves before or at the
end, as in 0.4.0. A retarget during such a tween keeps the held strip, because the panel
still covers when the retarget starts.

Alternative: move the strip from the held strip to the target strip in step with the
animation. Rejected by the requester: more code for a rare case.

### D6. `--show-existing` decided in the daemon

`Request::Create` gets `show_existing: bool` (`skip_serializing_if` false; absent on the wire
means false). In `connection.rs`, the `Create` arm with `show_existing`:

1. Route `Show { panel }` through the registry. A success reply becomes `Reply::existing()`
   (`{"ok":true,"existing":true}`), and no panel thread starts.
2. On `no_panel`, call `proxy::spawn` as today.
3. If the spawn is refused with `DuplicateName` (another launch won the race between steps 1
   and 2), route `Show` again and answer as in step 1.

`run_app` exits 0 on a reply with `existing` and does not exec. The options of `create` are
not applied to an existing panel, because step 1 sends only `show`. `--hidden` together with
`--show-existing` applies only to a new panel.

Alternative: the client sends `status` and then `show` or `create`. Rejected: two launches
can race.

## Risks / Trade-offs

- [A script runs with a stale `PINWIN_SOCKET` from a panel that ended with its daemon] → the
  client exits 1 with a message that names the variable. A `pinwin run` does not start a
  daemon on the wrong display.
- [Step 3 of D6 starts a panel thread that is then refused] → this is the 0.4.0 cost of a
  duplicate name, and only a race causes it.
- [D5 changes the end gap code for every tween] → the unit tests of `tween.rs` and
  `geometry.rs` keep the push-to-push and covering cases, and new cases cover cover-to-push
  with an equal and a different strip.

## Migration Plan

Additions only. A 0.4.0 client works with a 0.4.1 daemon, and the other way, as long as the
new fields are not sent. Rollback is the 0.4.0 binary.
