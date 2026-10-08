# Tasks

## 1. Visibility narrowing

- [x] 1.1 Narrow `pub mod wayland_side` to `pub(crate)` in `src/panel.rs` and verify `cargo build --all-targets` succeeds with no external use errors
- [x] 1.2 Narrow the `pub` children in `src/panel/wayland_side.rs` (`buffers`, `commands`, `crop`, `present`, `renderer`, `seat`, `sizing`, `tween`) to `pub(crate)` wherever the build still passes, and verify with `cargo build --all-targets`
- [x] 1.3 Triage any newly surfaced dead-code warnings: delete genuinely unused items or keep with a stated reason, adding no `allow` attributes, and verify `cargo clippy --all-targets -- -D warnings` is clean

## 2. `BytePath` named shape

- [x] 2.1 Convert `BytePath` in `src/panel/wayland_side/glue.rs` from an anonymous 4-tuple alias to a struct with named private fields built by `byte_path()`, updating use sites, and verify `cargo test pty::` and `cargo test panel::` pass
- [x] 2.2 Replace the split orphaned paragraphs with a single coherent doc block on the new shape, and verify `cargo doc --no-deps --document-private-items` builds without warnings

## 3. Comment rewrites: apply and tween

- [x] 3.1 Rewrite `row`/`dispatch` refs and `GTK side's`/`GTK publish's` prose in `src/panel/wayland_side/apply.rs` (incl. `apply/tests.rs`) as present-tense Wayland behaviour with `replace-gtk-with-wayland Dn` citations, deleting what the code makes obvious, and verify `cargo test panel::wayland_side::apply` passes
- [x] 3.2 Rewrite `row`/`dispatch` refs and GTK prose in `tween.rs`, `tween/tests.rs`, `tween_draw.rs`, `tween_draw/frames.rs`, `tween_draw/tests.rs`, `crop.rs`, and `crop/draw.rs`, and verify `cargo test panel::wayland_side::tween` and `cargo test panel::wayland_side::crop` pass

## 4. Comment rewrites: frame log, watchdog, commands, toggle

- [x] 4.1 Rewrite the module header and tick/stop-relay comments in `frame_log.rs` (log-format stability, not GTK history) and the GTK refs in `watchdog.rs`, `commands.rs`, and `toggle.rs`, and verify `cargo test panel::wayland_side` passes

## 5. Comment rewrites: seat and surfaces

- [x] 5.1 Rewrite the `GDK path's` controller/mouse/focus/key comments in `seat.rs` and `seat/` (`cursor.rs`, `focus.rs`, `keyboard.rs`, `pointer.rs`, `xkb.rs`) plus `state/seat_handlers.rs` as current Wayland seat behaviour, and verify `cargo test panel::wayland_side::seat` passes
- [x] 5.2 Rewrite the remaining `row`/`GTK` refs in `surfaces.rs`, `state/device.rs`, `state/handlers.rs`, `state/session.rs`, `buffers.rs`, `present.rs` (incl. tests), `renderer.rs` (incl. `font_setup.rs` and tests), `sizing.rs`, `state.rs`, `glue.rs`, `tests.rs`, and `src/surfaces/publish.rs`, and verify `cargo test panel::` passes

## 6. Comment rewrites: long tail

- [x] 6.1 Rewrite the `row`/`dispatch` refs and GTK/glib prose outside the panel thread (`handshake.rs`, `startup.rs`, `pty.rs`, `pty/calloop.rs` `glib twin`, `settings.rs` `focus socket`, `anim.rs`, `render/*`, `term/*`, `layout.rs`, `cli.rs`, `instance.rs`, `main.rs`), keeping sanctioned design citations, and verify `grep -rnE "rows? [0-9]+\\.[0-9]+|dispatch D[0-9]" src | wc -l` returns 0 and only sanctioned `replace-gtk-with-wayland`/`port-to-rust` citations remain for the prose pattern
- [x] 6.2 Reviewer diff-check: confirm no deleted comment carried timing/ordering knowledge that is not restated in the rewrite, and verify the full `cargo test` suite passes

## 7. Final gates

- [ ] 7.1 Run the gate set — `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `make check-code-file-lines` — and verify all three are clean
- [x] 7.2 Exercise the demo per AGENTS.md (`cargo run --example demo`; width animation both ways, focus accent) and verify launch, animation, accent, and exit-cleanup behave as before

## Workflow follow-up

- Archive the change after the project's review requirements are satisfied.
- Verify the archived result.
