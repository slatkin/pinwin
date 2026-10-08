# Tasks

## 1. Typed `staged_publish`

- [x] 1.1 Change `staged_publish` in `src/surfaces/gap.rs` to take `OutputSize` and `CellSize` and return `Option<(Layout, HeldGap)>`; update `apply.rs::stage`; verify `cargo test surfaces::gap` and `cargo test panel::wayland_side::apply` pass
- [x] 1.2 Delete `metrics_valid` and its `use` in `src/surfaces.rs`/`gap.rs`; move the bad-geometry assertions to `staged_publish` tests and confirm `CellSize::new`/`OutputSize::new` already test non-positive inputs (add them in `src/layout.rs` if not); verify `cargo test layout::` and `cargo test surfaces::` pass

## 2. `HeldGap` through the tween path

- [x] 2.1 Make `reserve_gap` `pub(crate)` returning `HeldGap`, delete `reserve_gap_parts`, and update its `gap.rs` tests; verify `cargo test surfaces::gap` passes
- [x] 2.2 Store a `HeldGap` in `StagedTween` and `TweenDraw` (including `retarget`) and pass it to `crop::frame_geometry`; update `apply.rs`, `crop.rs`, `tween_draw.rs` and their tests; verify `cargo test panel::wayland_side::{apply,crop,tween_draw,tween}` pass
- [x] 2.3 Remove the `gap_side`/`gap_zone` doc references and the `reserve_gap_parts` mentions in comments; verify `rg "reserve_gap_parts|gap_side|gap_zone" src` returns nothing

## 3. Move `PublishOutcome`

- [x] 3.1 Move `PublishOutcome` into `src/panel/handshake.rs` as `pub(crate)`, delete `src/surfaces/publish.rs` and its `pub use`/`mod`, update imports in `wayland_side.rs`, `apply.rs`, `toggle.rs` and tests, and fix the `surfaces.rs` header; verify `cargo build --all-targets` and `cargo test panel::` pass and `rg "surfaces::PublishOutcome" src examples` returns nothing

## 4. Seat plumbing

- [x] 4.1 Re-read `smithay-client-toolkit` in `Cargo.lock` and its `SeatData` dispatch (`src/seat/mod.rs`, `Capabilities` event) to confirm `new_capability`/`remove_capability` fire only on change; then delete `capability_step`/`CapabilityStep` and its test, matching `on_capability` on `arriving`; verify `cargo test panel::wayland_side::state::seat_handlers` passes
- [x] 4.2 Change `SeatLinks.draw_offset` to `Rc<Cell<f64>>` and `queue_draw` to `Rc<Cell<bool>>`, build them in `glue::seat_links` from `PanelState`'s cells, and update `pointer.rs`, `focus.rs`, `seat.rs` and the test fixtures; verify `cargo test panel::wayland_side::seat` and `cargo test panel::wayland_side::glue` pass

## 5. Build `PanelState` whole

- [x] 5.1 Record the baseline: run `cargo test` and `cargo clippy --all-targets -- -D warnings` and note the pass counts
- [x] 5.2 Replace `PanelState::headless` with `PanelState::new` taking the wiring bundle (terminal, repaint, `stale_grid_px`, `draw_offset`, `SeatLinks`, `TweenRender`); make `render` and `seat_links` non-`Option`, drop the `PanelState.terminal` field in favour of `render.terminal`, and rewrite `run_thread` to build the state in one call; verify `cargo build --all-targets` passes
- [ ] 5.3 Delete the "no terminal (unreachable)" branches in `run_loop` and the `ok_or(BindFailure::Internal)` on `seat_links` in `state/session.rs`; verify `rg "unreachable on a production thread|seat_links.clone\(\).ok_or" src` returns nothing
- [ ] 5.4 Move every `headless`/`headless_state` test helper to `PanelState::new` with a display-free `Terminal`; keep `push_grid`'s winsize-only degrade covered by a direct `None` test; where a test cannot move without changing its meaning, keep a `#[cfg(test)]` shim for it alone and list it in the commit message; verify `cargo test` matches the 5.1 count
- [ ] 5.5 Final gate: `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo doc --no-deps --document-private-items` pass with no new `allow` attributes, and `openspec validate typed-publish-path` passes

## Workflow follow-up

- Close issue #24 with the commit range once the change is archived.
- Any live niri check is run by the user; agents do not touch the live session.
