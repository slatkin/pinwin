# pinwin

A GTK4 layer-shell terminal panel for Wayland compositors. It docks a terminal
running any command to a screen edge as a layer surface, reserves space from
tiled windows via a transparent reservation surface, and can animate its width
between layouts. Built for niri; needs any compositor supporting
wlr-layer-shell.

## Build

Requires Zig 0.16 and the system libraries GTK4 (which pulls gdk-pixbuf),
gtk4-layer-shell-0 and pangocairo. The pinned libghostty-vt (and its vendored
SIMD archives) is fetched by the build itself.

- `zig build` — installs `zig-out/bin/pinwin` and the static archives a C
  consumer links (`lib/libpinwin.a`, `lib/libghostty-vt.a`, `lib/libsimdutf.a`).
- `zig build demo` — builds the dev-only driver `zig-out/bin/pinwin-demo`.
- `zig build check` — runs the layout-core and ABI-contract unit tests.

Live behaviour needs a running niri session.

## Usage

```
pinwin [--] [command...]        # default command: $SHELL
```

Environment:

- `COLS` — panel width in terminal columns (default 40)
- `GUTTER` — gap between the panel and the first tile, px (default 0)
- `PINWIN_KEYBOARD` — `on-demand` (default), `exclusive` or `none`
- `PINWIN_ACCENT` — `on` or `off`; draw the focus accent
- `PINWIN_ACCENT_COLOR` — `#RRGGBB` (default matches the Ghostty theme accent)
- `PINWIN_ACCENT_WIDTH` — accent stroke width in px (default 1)

The demo reads single-letter commands on its own stdin (`e` animated width
toggle, `p` plain snap apply, `b` rejected layout, `q` quit);
`DEMO_DENSE=1` swaps the shell child for a dense stand-in (full text grid,
kitty images, resize traffic) for animation testing.

## Behaviour

- The panel is a layer-shell `overlay` surface; a second, transparent
  `bottom`-layer surface carries an exclusive zone of gutters + panel, so
  tiled windows reflow without any compositor configuration.
- Keyboard follows `on-demand` semantics by default: the panel takes the
  keyboard on click and gives it back when you click a tiled window.
- While the panel holds keyboard focus it draws the focus accent: a stroke
  around the whole window in the configured colour and width (layer surfaces
  get no compositor focus ring).
- Width changes animate (ease-out, 200 ms by default) through
  `pinwin_apply_layout_animated`: the visible width tweens frame by frame while
  the terminal grid resize lands once, after the tween's final frame; pty
  drainage is bounded per dispatch while a tween runs.
- The host owns the pty and the child process; the library raises SIGWINCH on
  every successful resize.

## Architecture

The deliverable is libpinwin: a Zig core with the terminal state and a C glue
layer with everything GTK, split along the pinned libghostty-vt API.

- `src/main.zig` (with `cells.zig`, `input.zig`, `keys.zig`, `c.zig`) owns the
  libghostty-vt terminal, cell iteration, kitty image placements and key/mouse
  encoding, and exports the C ABI declared in `src/pinwin.h` (deliberately
  free of GTK types so Zig can import it).
- `src/pinwin_api.c` — the public ABI: argument validation, the GTK thread's
  lifecycle, and the bounded invoke-and-wait apply handshake.
- `src/glue.c` — layer-shell surfaces (visible panel plus the transparent
  reservation), layout validation and application.
- `src/glue_anim.c` — the animated width transition: one tick callback, a
  watchdog, and the tween state.
- `src/render.c` — drawing: Pango cell text, sprite ranges, cursor, the tween
  frame cache (one grid render per tween, blitted at the dock offset), and the
  focus accent.
- `src/images.c` — cairo surfaces for kitty image placements and the PNG
  decode hook.
- `src/pty.c` — pty attach/read/write, winsize resize and grid application,
  with the tween-time drain budget.
- `src/input.c` — GDK key, mouse, scroll and focus controllers.
- `src/fontconfig.c` — fonts, metrics and theme colours read from the Ghostty
  config.
- `src/options.c` — the GTK-free layout core (geometry validation, tween step,
  pty yield decision), unit-tested by `src/options_test.zig`;
  `src/pinwin_api_test.zig` covers the ABI contract.
- `host/main.c` — the `pinwin` program: a thin host over the C ABI that owns
  the pty, the child's environment and the process lifetime.
- `demo/main.c` — dev-only ABI driver.
- `openspec/specs/pinwin-panel/spec.md` is the behaviour spec; the design
  decisions referenced as D-numbers in code comments live in
  `openspec/changes/archive/`.
- `pinwin.sh` is the legacy pre-library approach (a Ghostty window plus niri
  struts and window rules); libpinwin's layer-shell reservation supersedes it,
  and it is kept for reference.

## Known caveats

- Consumers pin a commit and mirror the ABI structs by hand: mbv's
  `crates/mbv-pinwin/src/ffi.rs` must match `PinwinStartup`'s field layout
  (the focus accent added a field; a consumer pinned before it needs its
  mirror updated or `pinwin_start` reads garbage and refuses the layout).
- Width animation assumes the host repaints through the library's frame
  protocol; a child that never redraws shows a stale grid until the tween's
  end, by design.
