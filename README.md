# pinwin

A GTK4 layer-shell terminal panel for Wayland compositors: it docks a terminal
running any command to a screen edge as a layer surface, reserves space from
tiled windows via a transparent reservation surface, and animates its width
between layouts. Built for niri.

## Build

Requires Zig 0.16 and the system GTK4, gtk4-layer-shell-0 and pangocairo; the
pinned libghostty-vt is fetched by the build.

```
zig build        # zig-out/bin/pinwin + the static archives consumers link
zig build demo   # dev-only ABI driver
zig build check  # unit tests
```

## Usage

```
pinwin [--] [command...]    # default command: $SHELL; needs a running niri
```

`COLS`, `GUTTER`, `PINWIN_KEYBOARD`, `PINWIN_ACCENT`, `PINWIN_ACCENT_COLOR`
and `PINWIN_ACCENT_WIDTH` tune the panel — see the header of `host/main.c`
for the full list. The demo (`zig-out/bin/pinwin-demo`) drives the ABI
directly; `e` toggles the animated width, `DEMO_DENSE=1` gives it a full,
busy grid to test animation against.

## More

The behaviour spec lives in `openspec/specs/pinwin-panel/spec.md`; the
architecture map and contribution guidance in `AGENTS.md`.
