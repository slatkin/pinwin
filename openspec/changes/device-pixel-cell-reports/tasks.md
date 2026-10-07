# Tasks

## 1. Experiment before implementation (B2 gate)

- [ ] 1.1 Prove the premise with an ad hoc experiment, instant feedback: temporarily make `size_report` answer device pixels (hardcoded 1.8 multiply of the ctx cell), rebuild, run the demo's dense child (`DEMO_DENSE=1`) on the laptop's 1.8 output with a kitty image sized from the `CSI 16 t` reply, and record in the proposal's Experiment result section: the reported cell size, the transmitted bitmap's pixel size, and whether the image renders sharp side-by-side with the same image in ghostty. Revert the hardcode. If the image is not sharp, stop: the premise is wrong, and the remaining tasks wait for re-planning.
- [ ] 1.2 Move the cell-size probe out of `/tmp` into the repo (`scripts/probe-winsize.sh`, the TIOCGWINSZ + CSI 16 t reader the live checks cite) and verify it runs against a started panel (`bash -n` plus one live run).

## 2. Device-pixel reports

- [ ] 2.1 Add the shared scale note to the terminal's callback context (an `Rc<Cell<u32>>` of 1/120 units, defaulting to 120), thread a handle to it through the panel thread's terminal assembly, make `size_report` answer the ctx cell times the note (rounded, D1/D3), and have the thread update the note wherever it calls `sync_renderer_scale`; unit-test the report mapping through the callback at scale 1 and 1.8 — cell pixels scale, cols/rows unchanged (`cargo test`).
- [ ] 2.2 Extend the pty push (`apply_pty_size` via its `grid_sink` caller) with the session's resolved scale so it passes the device cell to `apply_winsize` (the existing `cols × cell_w` product then yields the device pixel fields, D1/D4), and re-push from the scale-note handlers (`note_integer_scale`, `note_preferred_scale`) when the device pixel size changed — fd from `startup`, the current grid re-derived from `Sizing`'s live columns and last configure height, pushed through the same sink; unit-test the push at scale 1 and 1.8, the change-only re-push and the no-change no-op (`cargo test`).
- [ ] 2.3 Verify the mouse encoder path is untouched: the encoder still receives the logical `ctx` cell and the pointer→cell mapping tests hold (`cargo test` in `term` and `wayland_side::seat`).

## 3. Integration checks

- [ ] 3.1 Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo nextest run`; fix findings without lint suppression (`make check-code-file-lines` before pushing).
- [ ] 3.2 Live niri check at a fractional scale using `scripts/probe-winsize.sh`: `CSI 16 t` and `TIOCGWINSZ` report device pixels; with `DEMO_DENSE=1`, images sized from the report render sharp next to ghostty; mouse cell mapping still hits the right cells; text and accent are unchanged; restore the scale afterwards.
