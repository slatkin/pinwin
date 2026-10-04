# Repository Guidelines

## Project Structure & Module Organization

libpinwin is the product: a Zig core (`src/main.zig` with `cells.zig`,
`input.zig`, `keys.zig`, `c.zig`) that owns the pinned libghostty-vt terminal
and exports the C ABI declared in `src/pinwin.h` (no GTK types there), plus a
C glue layer split by responsibility — `glue.c` (surfaces and layout),
`glue_anim.c` (width tween), `render.c` (drawing, tween frame cache, focus
accent), `images.c` (kitty image surfaces), `pty.c` (pty and grid), `input.c`
(GDK controllers), `fontconfig.c` (Ghostty config/theme), `options.c`
(GTK-free layout core) and `pinwin_api.c` (ABI and GTK-thread lifecycle).
`host/main.c` is the thin `pinwin` program; `demo/main.c` is a dev-only ABI
driver. `openspec/specs/pinwin-panel/spec.md` is the behaviour spec and
`openspec/changes/archive/` holds the design decisions that code comments
reference as D-numbers. `pinwin.sh` is the legacy pre-library script, kept for
reference. `README.md` documents installation, usage, behaviour, architecture
and known caveats.

## Build, Test, and Development Commands

`zig build` installs `zig-out/bin/pinwin` plus the static archives a consumer
links; `zig build demo` builds `zig-out/bin/pinwin-demo`; `zig build check`
runs the layout-core and ABI-contract unit tests. Requires Zig 0.16 and the
system GTK4, gtk4-layer-shell-0 and pangocairo; the pinned libghostty-vt is
fetched by the build. Live runs require niri. If touching the legacy
`pinwin.sh`, check it with `bash -n pinwin.sh`.

## Coding Style & Naming Conventions

C glue compiles as gnu11 with `-Wall`: four spaces, lowercase names for
functions and locals, uppercase for user settings and layout constants such as
`COLS`, `GUTTER` and the `PINWIN_*` environment names. Zig files are `zig fmt`
clean. Quote path and command arguments where expansion should remain one
argument. Keep comments focused on niri, GTK or timing behaviour that is not
obvious from the code, and keep the D-number references pointing at the
archived design they came from.

## Testing Guidelines

Run `zig build check` after any change; it covers geometry validation, the
tween step and the pty yield decision, plus the ABI contract test. Exercise
visual or behavioural changes with `zig build demo` in a running niri session
(`DEMO_DENSE=1` adds a full grid with kitty images and resize traffic). Verify
launch, width animation both ways, focus accent and cleanup when the launched
command exits. Keep gutters aligned with the user's niri layout when changing
defaults.

## Commit & Pull Request Guidelines

Always commit changes; never leave a dirty worktree. Commit as you go with
short, imperative subjects that describe the behaviour changed, such as
“Default GUTTER to 12” and “Keep the width tween smooth against a full grid
and pty traffic.” In a pull request, describe the user-visible behaviour, list
the checks performed, and mention any required niri configuration change.
Include screenshots only when a layout change is easier to assess visually.
