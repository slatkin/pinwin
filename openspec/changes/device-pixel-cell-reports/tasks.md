# Tasks

## 1. The device-size arithmetic (pure, display-free)

- [ ] 1.1 Add the device-size conversion next to the existing scale helpers: given a logical cell (`CellSize`) and a `FractionalScale`, produce the reported device cell (`scale_dimension` per dimension) and the drawn grid's device size from a logical grid size; unit-test the 1.8/9 px case (16 / 648), the scale-1 identity, and the non-exact products against `FractionalScale::scale_dimension` (`cargo test -p pinwin`).
- [ ] 1.2 Unit-test the size-report mapping as a pure function: cols/rows pass through unchanged, `CSI 14 t` grid pixels and `CSI 16 t` cell pixels come from the device conversion, at scale 1 and 1.8 (`cargo test`).

## 2. The size report answers device pixels

- [ ] 2.1 Add the shared scale note to the terminal's callback context (an `Rc<Cell<u32>>` of 1/120 units, defaulting to 120) and thread a handle to it through the terminal construction the panel thread already assembles; verify the existing term tests still pass (`cargo test term`).
- [ ] 2.2 Make `size_report` answer from the note per decision D1/D3 (logical ctx values × scale, rounded; cols/rows unchanged) and have the panel thread update the note wherever it calls `sync_renderer_scale`; add a test that a scale-note update changes a subsequent report's pixel fields and nothing else (`cargo test`).
- [ ] 2.3 Verify the mouse encoder path is untouched: the encoder still receives the logical `ctx` cell and the pointer→cell mapping tests hold (`cargo test` in `term` and `wayland_side::seat`).

## 3. The pty winsize carries device pixels

- [ ] 3.1 Extend the pty push (`apply_pty_size` and its `grid_sink` caller) with the session's resolved scale so `ws_xpixel`/`ws_ypixel` are the drawn grid's device size (D1/D4); unit-test the push decision display-free through the existing apply seams (`apply_against` / `configure_grid` observers) at scale 1 and 1.8, cols/rows unchanged (`cargo test`).
- [ ] 3.2 Re-push the winsize from the scale-note handlers (`note_integer_scale`, `note_preferred_scale`) when the device pixel size changed, SIGWINCH included, and unit-test the change-only re-push plus the no-change no-op (`cargo test`).

## 4. Integration checks

- [ ] 4.1 Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo nextest run`; fix findings without lint suppression (`make check-code-file-lines` before pushing).
- [ ] 4.2 Live niri check: set the panel's output to a fractional scale (1.8), run the demo with `DEMO_DENSE=1` and mbv pinned, and verify `CSI 16 t` / `TIOCGWINSZ` report device pixels (the `/tmp/probe_winsize.sh` probe), posters render sharp next to the same host in ghostty, mouse cell mapping still hits the right cells, and text/accent are unchanged; restore the scale afterwards.
