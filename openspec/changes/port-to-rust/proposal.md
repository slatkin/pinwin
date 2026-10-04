# Proposal

## Why

pinwin is written in Zig and C only because Zig cannot import the GTK4 headers. All GTK,
layer-shell, Pango, cairo, GdkPixbuf and PTY work sits in C glue behind a hand-kept interface
(`pinwin.h`), and the library is published through a C ABI whose structs every consumer must
mirror by hand. Rust is the preferred language, and every one of those libraries has
maintained Rust bindings. A Rust port removes all of the C, all of the Zig and both
hand-kept interfaces.

## What Changes

- **BREAKING** pinwin becomes a single Rust crate that is both a library (`src/lib.rs`, used
  as a Cargo dependency) and the standalone `pinwin` binary (`src/main.rs`, replacing
  `host/main.c`).
- **BREAKING** The C ABI is removed with no replacement C surface: `pinwin_api.h`,
  `pinwin_start` / `pinwin_apply_layout` / `pinwin_apply_layout_animated` / `pinwin_stop`,
  `PinwinStartup` and `PINWIN_ERR_*`.
- The library API becomes a Rust `Panel` handle:
  - Starting the panel returns `Result<Panel, PinwinError>`.
  - Applying a layout, plain or animated, is a method on the handle.
  - Dropping the handle stops the panel.
  - Unknown sides, unknown keyboard modes and zero columns cannot be expressed in the
    argument types, so they no longer need runtime rejection. Validating a layout against the
    live monitor remains a runtime error.
- All C sources (`src/*.c`, `src/*.h`) and Zig sources (`src/*.zig`, `build.zig`,
  `build.zig.zon`) are deleted. GTK, layer-shell, Pango, cairo, GdkPixbuf and the PTY are
  driven through their Rust crates.
- libghostty-vt is reached through Rust bindings. A spike decides between the existing
  `libghostty-vt` crate and pinwin's own extern declarations against a pinned ghostty commit.
  The criterion is whether the crate exposes everything the panel uses: effect callbacks,
  render state, encoders and kitty graphics.
- `demo/main.c` becomes a Cargo example.
- `zig build check` is replaced by `cargo test`, which covers the same layout-validation and
  API-contract checks.
- Panel behaviour is unchanged: surfaces, reservation, font, rendering, terminal query
  replies, input protocols, kitty graphics, focus accent, SIGWINCH and animated width.

## Capabilities

### New Capabilities

_None._

### Modified Capabilities

- `pinwin-panel`: requirements are restated as follows; all other behavioural requirements
  carry over unchanged.
  - Requirements that name the C entry points, `PinwinStartup` or `PINWIN_ERR_*` codes are
    restated against the Rust `Panel` API and `PinwinError`.
  - The requirement exposing the C ABI is replaced by requirements for the crate's library
    API and the `pinwin` binary.
  - Requirements that reject unknown sides, unknown keyboard modes or out-of-range columns at
    runtime are restated as type guarantees.
  - The never-terminate-the-host requirement is extended to panics, which must not cross the
    API.

## Impact

- **Code**:
  - Removed: every file under `src/`, `host/` and `demo/`, plus `build.zig`, `build.zig.zon`
    and `compile_flags.txt`.
  - Added: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, the remaining `src/*.rs` modules and
    `examples/`.
  - Rewritten for the Cargo build: `AGENTS.md` and `README.md`.
- **Dependencies**: adds the gtk4, gtk4-layer-shell, pango, pangocairo, cairo-rs and
  gdk-pixbuf crates, plus libghostty-vt bindings. Building ghostty still invokes Zig at build
  time, but pinwin itself contains no Zig code.
- **API**: any existing C consumer of `libpinwin.a` loses that interface.
