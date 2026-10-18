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

- A 0.4.0 request line still parses and does the same: the new fields are optional, and
  `set-gutters` is an added command (D7).
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

### D7. Gutters as run options and one live command

`PanelOptions` already has `gutter_left`, `gutter_right`, `gutter_top` and
`gutter_bottom` (`Option<i32>`), and `config::resolve` already merges them, so the file and
the built-in defaults need no change. Two gaps keep them out of `pinwin run` and the IPC:
`src/cli.rs` has no `--gutter-*` arms, and `src/ipc/options.rs` `WireOptions` has no gutter
fields, so a `create` request drops them.

Run options:

- Add `--gutter-top N`, `--gutter-bottom N`, `--gutter-left N` and `--gutter-right N` to
  `run_option` in `src/cli.rs`. Each value parses as an `i32`; a negative value is valid and
  a non-integer is a usage error (exit 2).
- Add the four fields to `WireOptions` with `skip_serializing_if` for an absent field, and
  copy them in `wire_options` and `parse`. The existing `config::resolve` precedence then
  makes a run option win over `[panel.<name>]`, which wins over `[panel.defaults]` and the
  built-in `0`.

Wire command:

- `Request::SetGutters { panel, top, bottom, left, right }`, with each gutter an
  `Option<i32>` and `skip_serializing_if` for an absent field. The JSON names are `top`,
  `bottom`, `left` and `right`: `{"cmd":"set-gutters","panel":"notes","top":8,
  "bottom":8}`.
- `RawRequest::SetGutters` keeps each present field as `Option<Number>`, so a fractional or
  over-`i32` value is `invalid_value` of the known command, not a serde failure of the whole
  line; `whole` plus `i32::try_from` parses it. A line with no gutter field is
  `invalid_value` (the command is known, but its value is incomplete), with a message that
  names `top`, `bottom`, `left` and `right`.
- Add `set-gutters` to `cli::SUBCOMMANDS`, which a test checks against the request `cmd`
  names.

CLI:

- `pinwin set-gutters <name> [--top N] [--bottom N] [--left N] [--right N]`. The options
  may come in any order after the name; a repeated option takes its last value, as
  `--reserve` and `--ms` do. At least one option is required: without one the command is a
  usage error (exit 2) and sends nothing. A non-integer value is also exit 2.

Apply:

- Add `Request::SetGutters` to `is_layout_request` and a `plan_change` arm that builds
  `PanelSettings { gutters: Gutters::new(left, right, top, bottom), ..current.clone() }`,
  taking each absent field from `current.gutters()`. Add `Gutters::left()`, `right()`,
  `top()` and `bottom()` accessors to `src/geometry.rs` for that merge; they change no
  behaviour.
- `request_duration` returns `None` for `SetGutters`, so the plan never animates, even when
  the panel has a `tween_ms`. There is no `ms` field on the command.
- The plan goes through the same `geometry.validate(output)` as `set-size`; a refusal
  returns before `apply_plan`, so the settings, window and reservation stay unchanged. A
  successful apply stores the settings and publishes one `geometry-changed`; a request that
  names the current gutters is a no-change and publishes none, as the other layout
  commands.
- `needs_new_app_size` already gives the item's app-size rule: a near or far gutter change
  leaves the length alone and sends no new app size, while a side-gutter change (Top or
  Bottom on a side panel, Left or Right on a top or bottom panel) changes the length and
  sends exactly one new size.
- A hidden panel needs no special case: `apply::answer` validates against the known output
  and stores the settings, `publish_gap` uses `apply_gap_hidden`, and the next show re-sends
  the layer state, as for a hidden `set-size` (spec "Change while hidden"). A panel whose
  output size is not known yet is refused with `invalid_layout`, as every layout command.
- Replace the `src/proxy/apply.rs` module comment that says gutters have no command of their
  own; the module now applies `set-gutters` with the other layout commands.

No new IPC command is needed for the run options: they ride the existing `create` request.
A 0.4.1 daemon accepts the new fields, and a 0.4.0 request line without them still parses.

## Risks / Trade-offs

- [A script runs with a stale `PINWIN_SOCKET` from a panel that ended with its daemon] → the
  client exits 1 with a message that names the variable. A `pinwin run` does not start a
  daemon on the wrong display.
- [Step 3 of D6 starts a panel thread that is then refused] → this is the 0.4.0 cost of a
  duplicate name, and only a race causes it.
- [D5 changes the end gap code for every tween] → the unit tests of `tween.rs` and
  `geometry.rs` keep the push-to-push and covering cases, and new cases cover cover-to-push
  with an equal and a different strip.
- [A `set-gutters` field is absent on the wire] → an absent field keeps the panel's current
  gutter, and only a request with all four absent is refused, so a client can change one
  gutter without sending the other three.

## Migration Plan

Additions only. A 0.4.0 client works with a 0.4.1 daemon, and the other way, as long as the
new fields are not sent. Rollback is the 0.4.0 binary.
