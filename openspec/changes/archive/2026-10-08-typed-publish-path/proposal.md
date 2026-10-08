# Proposal

## Why

Issue #24 collects the simplifications skipped from the `/simplify` pass on
PR #20. The publish path already has the types it needs (`CellSize`,
`OutputSize`, `HeldGap`) but unwraps them into raw `i32`s and rebuilds them
one call later, so the same degenerate-metrics check runs twice and the
gap crosses a layer as separate `Side` and `i32` values. Separately, the
panel thread's startup builds a half-empty `PanelState` and patches six
fields afterwards, which forces "no terminal" branches that can never run in
production. These are all code-shape costs: each makes the next reader prove
a state is unreachable instead of the types ruling it out.

## What Changes

- `staged_publish` takes `OutputSize` and `CellSize` and returns
  `Option<(Layout, HeldGap)>` (the unused `cols` goes). `metrics_valid` in
  `src/surfaces.rs` is deleted; its degenerate-metrics cases are covered where
  the types are built.
- `HeldGap` is stored in `StagedTween`, `TweenDraw` and passed to
  `crop::frame_geometry` instead of a `gap_side` / `gap_zone` pair.
  `reserve_gap` becomes `pub(crate)`, returns the drawn gap as a `HeldGap`,
  and the `reserve_gap_parts` wrapper is deleted.
- `PublishOutcome` moves from `src/surfaces/publish.rs` to
  `src/panel/handshake.rs`, which owns its reply channel and `map_outcome`.
  `src/surfaces/publish.rs` is deleted. **BREAKING**:
  `pinwin::surfaces::PublishOutcome` leaves the crate's public API
  (`pub mod surfaces` exports it today); nothing in the repository, the
  `pinwin` binary or the examples names it outside the crate.
- `PanelState` is built whole. The six fields `run_thread` overwrites after
  `PanelState::headless` (`terminal`, `repaint`, `stale_grid_px`,
  `draw_offset`, `seat_links`, `render`) become constructor inputs and stop
  being `Option` where `None` was only the test state. The terminal has one
  owner on `PanelState` (`render.terminal`), the redundant `state.terminal`
  field and `run_loop`'s "no terminal (unreachable)" branches go.
  `SeatLinks.draw_offset` and `queue_draw` become the `Rc<Cell<f64>>` and
  `Rc<Cell<bool>>` `PanelState` already holds instead of `Rc<dyn Fn>`
  wrappers around them.
- `capability_step` and its four-cell table are deleted, and the seat
  handler decides create or drop from `arriving` alone. `SeatObjects` stays,
  because it owns the keyboard and pointer objects that must outlive the
  event. The `smithay-client-toolkit` 0.21.1 source was checked: `SeatData`
  tracks `has_keyboard` / `has_pointer` and calls `new_capability` /
  `remove_capability` only when a capability actually changes
  (`seat/mod.rs:452-470`), so the `(held, arriving)` cells `(true, true)` and
  `(false, false)` cannot reach the handler.

No runtime behaviour changes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This is a pure refactor, so the change sets `skip_specs: true` in
`.openspec.yaml`.

## Impact

- `src/surfaces.rs`, `src/surfaces/gap.rs`, `src/surfaces/publish.rs`
  (deleted), `src/panel/handshake.rs`.
- `src/panel/wayland_side.rs` (`run_thread`, `run_loop`),
  `wayland_side/state.rs`, `apply.rs`, `crop.rs`, `tween_draw.rs`,
  `glue.rs`, `present.rs`, `toggle.rs`, `seat.rs`, `seat/pointer.rs`,
  `seat/focus.rs`, `state/seat_handlers.rs`, `state/session.rs`, and the test
  modules that build `PanelState::headless`, `SeatLinks` and `StagedTween`.
- Public API: one removed re-export, `surfaces::PublishOutcome` (above).
- Sizing: the issue's items 1–3 are small; item 4 is the large part and the
  main regression risk, so the tasks order it last and verify it on its own.
