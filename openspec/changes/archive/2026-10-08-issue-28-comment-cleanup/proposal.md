# Proposal

## Why

Issue #28 tracks comment rot skipped from the PR #20 simplify pass: about
400 comments cite archived task-list rows (`row 6.2`) and dispatch decisions
(`dispatch D7`) that will rot once the change archives, and a second layer of
comments still describes the removed GTK/glib/GDK path (`the GTK side's
dispatch_apply`, `the GDK path's on_mouse`, `the glib twin`, `focus socket`).
AGENTS.md wants comments about niri, Wayland or timing behaviour with
D-number references to designs, so both layers violate the project's own
comment standard. Separately, `panel::wayland_side` is still `pub mod` while
nothing outside `src/panel.rs` uses it, which silences dead-code detection
across the panel thread, pty, anim and surfaces modules.

## What Changes

- Rewrite the ~408 `row X.Y` / `dispatch DN` comment references (30 files,
  concentrated in `src/panel/wayland_side/`) into AGENTS.md-standard comments:
  niri, Wayland or timing behaviour that is not obvious from the code, with
  D-number references to the design that introduced the behaviour
  (`replace-gtk-with-wayland Dn`, `port-to-rust Dn`).
- Rewrite the ~171 prose-style GTK/GDK/glib references (`GTK side's`,
  `GTK path's`, `GTK publish's`, `GDK path's`, `glib twin`, `focus socket`)
  to describe current Wayland behaviour; keep the 103 sanctioned
  `replace-gtk-with-wayland` design citations and the `GTK-free` adjective
  where they already meet the standard.
- Fix the `BytePath` docs in `src/panel/wayland_side/glue.rs`: the 4-tuple
  return is documented by an orphaned paragraph above the alias; give the
  tuple fields names (a struct or named-field return) or a single coherent
  doc block on the alias. (The issue's `gridsink`/`Option` half is stale:
  no gridsink item exists anymore.)
- Narrow `pub mod wayland_side` in `src/panel.rs` to `pub(crate)`, and narrow
  the `pub` items inside `src/panel/wayland_side.rs` (`buffers`, `commands`,
  `crop`, `present`, `renderer`, `seat`, `sizing`, `tween`) to `pub(crate)`
  where they are not needed outside the crate, so dead code fires again.
  **BREAKING**: `pinwin::panel::wayland_side` and its public children leave
  the crate's public API; external users cannot name those paths anymore.
  No runtime behaviour changes.

## Capabilities

### New Capabilities

None: this change introduces no system behaviour.

### Modified Capabilities

None: no requirement in `openspec/specs/pinwin-panel/spec.md` changes. This
is a pure comment and visibility cleanup with no observable behaviour change,
so the change opts out of specs via `skip_specs: true` in `.openspec.yaml`.

## Impact

- `src/` comments only (30 files, mostly `src/panel/wayland_side/`), plus the
  `BytePath` shape in `src/panel/wayland_side/glue.rs` (internal,
  `pub(crate)` already).
- Public API surface shrinks: `panel::wayland_side` becomes crate-internal.
  The `pinwin` binary, `examples/demo/` and all tests use only `Panel`,
  `Startup`, `PinwinError`, so no caller changes are expected; verification
  is `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --all -- --check` and `make check-code-file-lines`.
- No dependency, config, niri-configuration or wire-protocol changes.
