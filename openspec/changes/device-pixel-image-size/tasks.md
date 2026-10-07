# Tasks

## 1. Experiment

- [x] 1.1 Prove the fix with an ad hoc experiment (done 2026-10-07, recorded in the proposal's Experiment result): at scale 1.8 the placeholder image 576×324 is drawn 320×180 and the user confirmed it fits.
- [x] 1.2 Discard the uncommitted `EXPERIMENT` patch in the worktree and verify `git status` shows only `openspec/` changes (done at planning time).

## 2. Image size at the resolved scale

- [ ] 2.1 Add the device→logical conversion (D2) next to `virtual_rect` and unit-test it at scale 1 (identity) and 1.8 (576→320, 324→180, rounding half up) (`cargo nextest run images`).
- [ ] 2.2 Thread the scale note from `Terminal::image_next` into `images::image_next` (D1) and divide the virtual placement's destination size; update `virtual_placement_uses_the_origin_cell_box` and add a 1.8 case asserting 320×180 with the source rect still 576×324.
- [ ] 2.3 Declare the placement `COLUMNS`/`ROWS` selectors (10, 11) in `src/ghostty_sys/kitty.rs`, read them per viewport placement, and divide `pixel_width`/`pixel_height` only when both are 0 (D3); unit-test the decision for both-zero, one-given and both-given placements at 1.8 (`cargo nextest run images`).

## 3. Integration checks

- [ ] 3.1 Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo nextest run` and `make check-code-file-lines`; fix findings without lint suppression.
- [ ] 3.2 (user-run) On the laptop at scale 1.8, run the host with the built binary and confirm the image fits the panel without covering the rows below; at scale 1 confirm images are unchanged.
