# Tasks

Live checks are run by the user. An agent prepares the build and the steps, and records the
user's answer word for word.

Each task updates the README text and the `AGENTS.md` module entries that its change makes
wrong.

## 1. The strip's surface on the background layer

- [x] 1.1 Give `src/proxy/reserve.rs` one `RESERVE_LAYER` constant
  (`ZwlrLayerShellV1Layer::BACKGROUND`) and drop the `layer` parameter from `Reserve::create`,
  so the strip's surface can never be created on another layer (design D1). Verify: a unit test
  in `reserve.rs` asserts the constant is the background layer; `cargo nextest run` stays green.
- [x] 1.2 Drop the layer parameter from `start_reserve`, `recreate_reserve` and `dock_reserve`
  in `src/proxy/dock.rs`, and stop passing `layer_of(self.routing.layer())` in
  `src/proxy/routing/xdg_surface.rs` (design D1). Verify: the crate builds with no unused
  import or parameter; the re-dock paths (`set-output`, a `closed` strip) still create the
  strip on the background layer.
- [x] 1.3 Make `set_layer` change the panel window's layer only: delete `Reserve::set_layer`
  and its call in `dock::set_layer`, and update both doc comments (spec "Changing the layer
  leaves the strip in place"). Verify: `src/proxy/dock.rs`'s `set_layer` test still covers the
  re-dock on a version 1 bind; a live `set-layer bottom` leaves the strip on the background
  layer and the bar in place.
- [x] 1.4 Update the `src/proxy/reserve.rs` and `src/proxy/dock.rs` entries in `AGENTS.md`,
  and the README behaviour bullet on the reservation, to say that the strip is on the
  background layer whatever layer the panel draws on.

## 2. Checks

- [x] 2.1 Run the gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo nextest run` and `make check-code-file-lines`.
- [x] 2.2 Live check on umbriel, on a scratch runtime directory so the running daemon and its
  panel are untouched: symlink the session's `wayland-0` into the scratch directory, start the
  patched `pinwin daemon` there, run a 200 pixel left `pinwin run --reserve push` panel, and
  compare `umbriel layers --json` with the panel shown and hidden. Expect
  `noctalia-bar-Main` to keep x=500, w=2440 with the panel shown, the strip on the background
  layer with zone 200, and `umbriel outputs` to report a usable area of 3240x1405+200+35.
  End the test panel and daemon afterwards. Record the user's answer word for word.

## Live check record (agent-run, 2026-10-19)

The check ran on umbriel 0.1.0 (DP-1, 3440x1440) against a scratch runtime directory;
the user's daemon and its `mbv` panel were not touched.

- Before: `noctalia-bar-Main` on `top` at x=500, w=2440; usable area 3440x1405+0+35.
- A 200 px left pushing test panel from the patched build: the bar stayed at x=500,
  w=2440; its strip was on `background` with zone=200; usable area 3240x1405+200+35.
- `set-size` to 300 px with `ms` 200 (the expand in the report): the bar stayed at
  x=500, w=2440; the strip grew to zone=300; usable area 3140x1405+300+35.
- `set-layer` `bottom`: the panel window moved to the bottom layer, the strip stayed on
  `background` with zone=300, and the bar stayed at x=500, w=2440.
- Ending the app ended the panel and the daemon, and the bar stayed at x=500, w=2440.
