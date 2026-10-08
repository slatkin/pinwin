# Design

## Context

See proposal.md for motivation. Observed state of the code (line numbers in
issue #24 have drifted; these are current):

- `gap::staged_publish` (`src/surfaces/gap.rs`) takes four `i32` metrics and
  calls `surfaces::metrics_valid`, which rebuilds `CellSize` / `OutputSize`
  with `::new`. Its only caller is `apply.rs::stage`, which already holds the
  typed values and unwraps them. Its `u16` column result is discarded.
- `HeldGap` has private `side` / `zone`. `reserve_gap` returns a
  `(Side, i32)` tuple; `reserve_gap_parts` rebuilds a `HeldGap` from two loose
  values only so it can call `reserve_gap`. Its callers are `apply.rs`
  (builds `gap_side` / `gap_zone` for `StagedTween`) and
  `crop::frame_geometry` (called from `TweenDraw::commit_plan`).
  `StagedTween`, `TweenDraw` and `TweenDraw::retarget` all carry the pair.
- `PublishOutcome` is a three-variant enum used by `panel/handshake.rs`,
  `wayland_side.rs` (`PanelCommand::Apply` reply) and `apply.rs`.
  `surfaces` is a `pub mod`, so it is currently reachable as
  `pinwin::surfaces::PublishOutcome`.
- `run_thread` calls `PanelState::headless` and then assigns `terminal`,
  `repaint`, `stale_grid_px`, `draw_offset`, `seat_links` and `render`.
  `PanelState.terminal` and `TweenRender.terminal` and `SeatLinks.terminal`
  are clones of one `Rc<RefCell<Terminal>>`. `run_loop` builds its pty source
  from `state.render.as_ref().and_then(..)` and carries a comment for the
  "unreachable" `None`. Tests rely on a terminal-less state: `grid_sink_at`
  degrades to a winsize-only push when `terminal` is `None`
  (`glue.rs::push_grid`), and about 20 test helpers call `headless`.
- `SeatLinks.draw_offset: Rc<dyn Fn() -> f64>` and `queue_draw: Rc<dyn Fn()>`
  are built in `glue::seat_links` as closures over `Rc<Cell<f64>>` and the
  `Rc<Cell<bool>>` repaint latch that `PanelState` holds as `draw_offset` and
  `repaint`.
- `capability_step(held, arriving)` has four cells. sctk 0.21.1
  (`src/seat/mod.rs:441-480`) stores `has_keyboard` / `has_pointer` and calls
  `new_capability` / `remove_capability` only when `keyboard != has_keyboard`
  (and likewise for the pointer), so `(true, true)` and `(false, false)`
  cannot occur from the toolkit.

## Goals / Non-Goals

**Goals:**
- Typed values flow from `apply.rs` through `staged_publish` and the tween
  path without being unwrapped and rebuilt.
- `PanelState` is valid from construction in production code.
- No behaviour change, no new tests of behaviour: existing tests keep their
  assertions.

**Non-Goals:**
- Restructuring `Sizing`, `TweenDriver` or `Session`.
- Changing the `Rc` sharing of the terminal between `PanelState`,
  `TweenRender` and `SeatLinks`. `TweenRender` is cloned per tween frame and
  `SeatLinks` is cloned into `SeatSide`, so both need their own handle.
- Touching `SeatObjects`' ownership of `WlKeyboard` / `WlPointer`.

## Decisions

**D1. `staged_publish(output: OutputSize, cell: CellSize, held: HeldGap,
layout: Layout) -> Option<(Layout, HeldGap)>`.** `metrics_valid` collapses to
`layout.validate(cell, output).is_ok()` inside it, and the function is deleted.
The degenerate-metrics assertions in `surfaces.rs`'s test move to the
constructors' own tests (`CellSize::new` / `OutputSize::new` returning `None`
for non-positive values); the bad-geometry cases (layout the monitor
refuses) move to `staged_publish` tests in `gap.rs`. Alternative considered:
keep `metrics_valid` as a typed wrapper — rejected, it would be a one-line
alias of `Layout::validate`.

**D2. `reserve_gap` returns a `HeldGap`; `HeldGap` is stored, not its parts.**
The function already computes "the gap the reserve draws", so it returns that
type instead of a tuple, and `gap_tween_decision(gap.zone(), ..)` reads the
zone from it. `StagedTween.gap`, `TweenDraw.gap` and `retarget(.., gap, ..)`
carry one value; `frame_geometry` takes `gap: HeldGap`. `reserve_gap_parts`
is deleted. Tests that set the pair directly (`tween_draw/tests.rs`) build a
`HeldGap` with `start_held_gap` or, if they need an exact side and zone, a
`#[cfg(test)]` constructor next to the type. Alternative considered: a public
`HeldGap::new` — rejected, production code must only reach a `HeldGap`
through `start_held_gap`, `held_gap_after_publish` and `reserve_gap`, so an
arbitrary gap cannot be made outside those rules.

**D3. `PublishOutcome` moves to `src/panel/handshake.rs` as `pub(crate)`.**
That module owns the reply channel and `map_outcome`, and is what the
publish path hands the outcome to. `wayland_side.rs` and `apply.rs` import it
from there. `surfaces/publish.rs` and the `pub use` in `surfaces.rs` are
deleted, and `surfaces.rs`'s header stops mentioning the publish verdict.
This removes one public path (see the proposal's **BREAKING** note); no
in-repo code outside the crate names it. Alternative considered: keep a
`pub use` re-export — rejected, it would keep a module whose only reason to
exist is gone.

**D4. `PanelState` is built from a wiring bundle.** `PanelState::new(handshake,
poisoned, inner, startup, cell, wiring)` replaces `headless` as the one
constructor, where `wiring` carries the terminal, repaint latch,
`stale_grid_px`, `draw_offset` cell, `SeatLinks` and `TweenRender` that
`run_thread` builds from `BytePath`. The fields lose their `Option` (`render`
and `seat_links`) or their duplicate (`terminal` — read through
`render.terminal`). `run_loop` takes the terminal from `state.render` without
a `None` branch. Tests that call `headless` today move to `PanelState::new`
with a real display-free `Terminal` (the seat-handler fixture already builds
one with `Terminal::new` and `push_size`). The winsize-only degrade stays as
`push_grid(terminal: Option<&..>)`, which is a free function and keeps its own
unit test with `None`; `grid_sink_at` simply always passes `Some`.
Alternative considered: keep `headless` as a `#[cfg(test)]` constructor with
`Option` fields — rejected, the `Option`s would then exist only for tests and
production code would keep the unwraps. Fallback if moving the ~20 helpers
proves to alter a test's meaning: keep a `#[cfg(test)]` shim for those tests
only and say so in the task's notes; do not reintroduce `None` branches in
non-test code.

**D5. `SeatLinks.draw_offset: Rc<Cell<f64>>`, `queue_draw: Rc<Cell<bool>>`.**
`glue::seat_links` clones the cells `PanelState` holds instead of wrapping
them in closures. `pointer.rs` reads `links.draw_offset.get()`; `focus.rs`
calls `links.queue_draw.set(true)`. Tests that passed `Rc::new(|| 0.0)` pass a
fresh `Rc<Cell<f64>>`; the redraw-count test in `focus.rs` reads the latch
instead of a counter. Alternative considered: keep the closures for
test flexibility — rejected, the only closure body anywhere is the cell read.

**D6. Delete `capability_step`; `on_capability` matches on `arriving`.**
Arriving creates the object, leaving drops it. Because sctk reports only
changes, the `held` bookkeeping that guarded repeated arrivals is redundant.
A refused `get_keyboard` leaves the cell `None`, and a later removal then
drops nothing, which `release_*` already handles. `SeatObjects`' `Option`
fields stay: they own the protocol objects. `CapabilityStep` and the
"four cells" test go with it. If a later sctk bump changes the
change-only contract, the replacement is a one-line `held` guard; this is the
single lower-confidence item in the issue and the task for it re-reads the
pinned source before deleting.

## Risks / Trade-offs

- [D4 touches about 20 test helpers and `run_thread`'s startup order] →
  land it last, as its own commits, with the full `cargo test` and
  `cargo clippy --all-targets -- -D warnings` run before and after; the
  rest of the change does not depend on it.
- [A test relied on the terminal-less state to avoid building a `Terminal`]
  → use the fallback in D4 for that test only and record which one.
- [D6 relies on sctk's change-only guarantee] → verified against the pinned
  0.21.1 source in this planning step; the task re-checks the `Cargo.lock`
  version before deleting.
- [D3 removes a public path] → the proposal marks it **BREAKING**; the only
  in-tree users are inside the crate.
- No live-session verification applies: nothing here changes what the
  compositor or the seat sees, and agents do not touch the user's live niri
  session. The user runs any live check.
