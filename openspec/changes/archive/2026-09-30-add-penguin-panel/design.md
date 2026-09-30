# Design

## Context

See proposal.md for why. Requirements are in `specs/penguin-panel/spec.md`.

Facts this design rests on (checked against source on 2026-09-29):

- **libghostty-vt** lives in ghostty-org/ghostty (main at `3a3047f`). A Zig consumer adds `ghostty` as a `build.zig.zon` dependency. Two surfaces exist:
  - Zig module `ghostty-vt` (`dep.module("ghostty-vt")`, `src/lib_vt.zig`): `Terminal`, `RenderState`, `kitty`, `input.encodeKey` / `encodeMouse` / `encodeFocus`. But its `Terminal.vtStream()` is documented as a *read-only* stream: it does not answer queries.
  - C library `ghostty-vt` (`dep.artifact("ghostty-vt-static")`, headers `include/ghostty/vt.h` + `vt/*.h`). The terminal has effect callbacks set with `ghostty_terminal_set`: `GHOSTTY_TERMINAL_OPT_WRITE_PTY` (all query/mode replies), `..._OPT_SIZE` (`CSI 14/16/18 t`), `..._OPT_DEVICE_ATTRIBUTES` (`CSI c`), `..._OPT_USERDATA`. Size: `ghostty_terminal_resize(t, cols, rows, cell_width_px, cell_height_px)`. Input: `ghostty_terminal_vt_write`. Rendering: `ghostty_render_state_new/update/get`, `..._row_iterator_new/next_dirty`, `..._row_cells_new/next/get`. Kitty graphics: enabled only when `GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT` is non-zero *and* a PNG decoder is installed with `ghostty_sys_set(GHOSTTY_SYS_OPT_DECODE_PNG, fn)`; placements via `ghostty_kitty_graphics_get`, `..._placement_iterator_new`, `..._placement_next`, `..._placement_pixel_size`, `..._placement_grid_size`, `..._placement_render_info`; pixels via `ghostty_kitty_graphics_image` + `..._image_get_multi` (always RGBA after decode). Key/mouse encoders: `include/ghostty/vt/key/encoder.h`, `mouse/encoder.h`, `focus.h`. Examples `example/c-vt-{effects,kitty-graphics,render,encode-key,encode-mouse,encode-focus,size-report}` exercise these.
  - ghostty's `build.zig.zon` sets `minimum_zig_version = "0.16.0"`. Zig is not installed on this machine yet.
- **Ghostty's own GTK app** binds GTK4 through `gobject` (zig-gobject generated bindings) and gtk4-layer-shell through its own `pkg/gtk4-layer-shell`.
- **Installed system libraries**: GTK 4.22.5, gtk4-layer-shell 1.3.0, pangocairo 1.58.2, `/usr/include/pty.h`.
- **niri** (`src/handlers/layer_shell.rs`, `new_layer_surface`): a layer surface created without an output goes to `layout.active_output()`, the focused monitor. niri's `update_keyboard_focus` gives an `on-demand` surface keyboard focus when clicked and clears it when a window is clicked.
- **mbv** (`src/app/infra/terminal.rs`, `crates/mbv-images/src/protocol.rs`) enables alternate screen, mouse capture, `CSI > 1 s`, focus change, kitty keyboard `DISAMBIGUATE_ESCAPE_CODES`, and probes images with ratatui-image `Picker::from_query_stdio` (kitty query + `CSI 16 t`, falling back to half-blocks without replies; wrong cell size clipped images in mbv #654).

## Goals / Non-Goals

**Goals:**
- One Zig program, `penguin/`, built with `zig build`, linking system GTK4 / gtk4-layer-shell / pangocairo and libghostty-vt from a pinned ghostty commit.
- Every query mbv sends gets the reply Ghostty itself would give.

**Non-Goals:**
- `penguin --focus` as a flag: the same capability was added during implementation as the `PENGUIN_KEYBOARD` environment variable (see D4), which is also what the verification of keyboard input uses.
- OSC 52 clipboard, scrollback UI, selection/copy, a config file of its own, theme/colour import from the Ghostty config, following the focused monitor after startup, ligatures/shaping beyond what Pango does per cell.

## Decisions

### D1. Layout: `penguin/` subdirectory with its own `build.zig` / `build.zig.zon`
Keeps `pinwin` (the repo-root script) untouched and gives `zig build` its own root. Output: `penguin/zig-out/bin/penguin`.
Alternative: `build.zig` at repo root. Rejected: mixes the Zig build with the bash script's files.

### D2. libghostty-vt through its C API, not the Zig module
`exe_mod.linkLibrary(dep.artifact("ghostty-vt-static"))` plus `@cImport(@cInclude("ghostty/vt.h"))`. Static linkage keeps `make install` independent of Zig's build-cache runpath and an incompatible system `libghostty-vt.so`. The C API is where the query-reply callbacks (`WRITE_PTY`, `SIZE`, `DEVICE_ATTRIBUTES`) and the kitty placement iterator are documented, exercised by examples, and used by ghostling. The Zig module's `vtStream()` is read-only, so replies would have to be rebuilt by hand.
Alternative: Zig module `ghostty-vt`. Rejected for v1; revisit if the C API lags a needed feature.
Pin: `.ghostty = .{ .url = "https://github.com/ghostty-org/ghostty/archive/<commit>.tar.gz", .hash = ... }`, commit chosen in task 1.

### D3. GTK4 + gtk4-layer-shell + pangocairo via a C glue file

**Updated after task 1.2 (spike).** The original decision was `@cImport` of the system headers. That does not work: Zig 0.16's translate-c cannot consume the GTK4 header chain and crashes the compiler (SEGV). Two independent causes, both reproduced with a minimal TU:

- `G_GNUC_BEGIN_IGNORE_DEPRECATIONS` expands to `_Pragma("GCC diagnostic push")` at statement position in `glib/gutils.h` and `glib/gthread.h`; translate-c reports `unknown type name 'pragma'` (9588 errors before the next cause was reached).
- Once the `_Pragma` uses are stripped by pre-including `glib/gmacros.h` and redefining the macro, the compiler crashes (signal SEGV) instead of reporting an error.

The replacement: `src/glue.c` (compiled by the Zig build with `addCSourceFile`, so it goes through the real C frontend) owns every GTK, Pango, cairo, GdkPixbuf and PTY call, and `src/penguin.h` is a GTK-free interface of plain integers, structs and function pointers that Zig *can* `@cImport` (`src/penguin.h` + `src/penguin.h`'s `glue_*`/`penguin_*` split). `src/compile_flags.txt` carries the pkg-config include paths for clangd.

Alternative considered and rejected: ghostty's `gobject` (zig-gobject) dependency. Rejected for the same reason as before (a large generated-bindings dependency for one window), now with the added cost of leaning on a binding whose GTK surface we would have to learn by reading generated code.

### D4. Surface setup
The visible `penguin` surface uses the `OVERLAY` layer, anchors LEFT/TOP/BOTTOM with zero margins, and sets exclusive zone `-1`: it covers the full output height, including Noctalia's top bar. A separate transparent, 1-pixel-wide `penguin-reserve` surface on the `BOTTOM` layer reserves `COLS * cell_w + GUTTER` at the left edge so tiled windows still stay to the right. Both surfaces are released when the process exits. Keyboard mode on the visible panel comes from `PENGUIN_KEYBOARD` (default `ON_DEMAND`, or `EXCLUSIVE`/`NONE`). Neither surface sets a monitor explicitly (niri chooses the focused output). Panel width = `COLS * cell_w`; rows = `floor(height / cell_h)`, recomputed on resize. The last row's background extends through any leftover pixels at the bottom, so no dark strip remains below the status row. `pinwin`'s `TOP`/`BOTTOM` inset existed only to mirror niri struts; penguin doesn't read niri's layout.

### D5. Cell metrics and drawing
Font: the first `font-family` and the `font-size` from the Ghostty config (`$XDG_CONFIG_HOME/ghostty/config`, default `~/.config/ghostty/config`), so the panel matches the user's terminal; `PENGUIN_FONT` and `PENGUIN_FONT_SIZE` override them, and `"monospace 11"` is the fallback when the config has no font. `cell_w` = rounded approximate digit width, `cell_h` = rounded ascent + descent, from `pango_font_metrics`. The same integers go to `gtk_layer_set_exclusive_zone`, `ghostty_terminal_resize`, `TIOCSWINSZ` and the size callback, so every reported pixel size matches what's drawn. Each cell is drawn at an absolute position with its baseline pinned to the row's, so a glyph that comes from a fallback font (box drawing, nerd icons, emoji) stays on its line. Block elements (U+2580-U+259F), braille (U+2800-U+28FF), corner triangles (U+25E2-U+25E5) and powerline separators (U+E0B0-U+E0B3) are drawn as geometry rather than as the font's glyphs; the geometry is ported from Ghostty's sprites (`src/font/sprite/draw/*.zig`). U+2594 (upper one-eighth block, used by the seek bar) rounds its height up to whole pixels: 3 pixels at Pango's 19-pixel cell height, matching Ghostty's 3-pixel bar rather than a translucent or thin edge. Nerd Font glyphs (the table's codepoints) are normalised the way Ghostty does it: `src/nerd_font_tables.h` is generated from the pinned commit's `src/font/nerd_font_tables.zig`, `src/glue.c` ports `Glyph.RenderOptions.Constraint` (scale rule, padding, relative geometry, alignment, against Ghostty's grid metrics), and a symbol may use a second cell when the cell to its right is empty (`renderer/cell.zig`'s constraintWidth rule). A wide cell spans two columns and the spacer cell after it is not drawn. libghostty-vt leaves an emoji modifier, a variation selector or the base after a zero-width joiner in their own cells, so a cluster arrives as two or three cells; the cell stream joins them back into one cell (skipping the wide-glyph spacer cells in between) before drawing, as Ghostty does.
Drawing: `GtkDrawingArea` draw func with cairo. Each frame: `ghostty_render_state_update`, walk **every** row to fill cell backgrounds, rewind the captured row iterator, then walk again to draw glyphs (bold/italic from the style), followed by cursor and kitty placements. Painting backgrounds first preserves Nerd Font glyphs that extend into the next cell. Constrained Pango glyphs explicitly reset cairo's current point after the transform, so a preceding cell cannot shift them left (the playback pause glyph must remain at column 1, not column 0).

**Updated after implementation (tasks 2.2/3.1).** The dirty-row walk was dropped. The draw callback repaints the whole widget (background fill included), so a row the render state does not report as dirty would be left blank rather than keeping its previous pixels; GTK presents the whole widget on each `queue_draw`, so nothing survives from the previous frame. Dirty state is still used to skip a frame entirely (plus a forced frame after every resize), which is the cheap part of the optimisation. 2,800 cells per frame is fine at 40 columns.
Images: on each frame read placements with the iterator and `..._placement_render_info`; convert the RGBA pixels to a cached `cairo_image_surface` (ARGB32, premultiplied) keyed by the unsigned 32-bit image id (IDs can exceed `INT32_MAX`); paint scaled to the placement's pixel rect.
PNG decode callback (`GHOSTTY_SYS_OPT_DECODE_PNG`): decode with `GdkPixbufLoader` into RGBA.
Alternative: `GtkSnapshot` + `GdkTexture`. Deferred: cairo is enough at 40 columns; switch if redraw cost shows up.

### D6. PTY and event loop
`forkpty()` with the initial `winsize` (cols, rows, xpixel, ypixel), child `execvp`s the command with `TERM=xterm-ghostty` if that terminfo is installed, else `xterm-256color`. Parent sets the PTY fd non-blocking and watches it with `g_unix_fd_add`: read → `ghostty_terminal_vt_write` → `gtk_widget_queue_draw`. `WRITE_PTY` callback writes the bytes to the PTY fd. `g_child_watch_add` on the child pid: on exit, `exit()` with the child's status. Everything runs on the GTK main thread; no extra threads.

### D7. Query callbacks
- `WRITE_PTY`: write to the PTY fd.
- `SIZE`: fill cols, rows, `cell_w`, `cell_h` (answers `CSI 16 t`).
- `DEVICE_ATTRIBUTES`: fill the same primary attributes Ghostty reports (exact values copied from Ghostty's GTK app in task 1). ratatui-image uses the DA1 reply as the end marker of its probe.
- Kitty storage limit: 320 MB (Ghostty's default `image-storage-limit`).

### D8. Input
- Keys: `GtkEventControllerKey` `key-pressed` / `key-released` → build a `GhosttyKeyEvent` (action, physical key from the hardware keycode, mods from `GdkModifierType`, UTF-8 text from the keyval) → key encoder with the terminal's current kitty keyboard flags and modes → write result to the PTY. The keycode → `GhosttyKey` table follows Ghostty's GTK apprt.
- Mouse: `GtkGestureClick` (all buttons), `GtkEventControllerMotion`, `GtkEventControllerScroll` → pixel position → mouse encoder (which converts to cells and respects the enabled mouse mode, SGR format, and shift-escape) → PTY.
- Focus: `GtkEventControllerFocus` `enter`/`leave` on the window → focus encoder → PTY, only when the program enabled focus reporting (mode 1004).

### D9. Settings parsing
`COLS` (default 40), `GUTTER` (default 0), `PENGUIN_KEYBOARD` read from the process environment and `std.fmt.parseInt(u32, ...)`; `COLS == 0` or a parse error → message naming the variable on stderr, exit 2, before `gtk_init`. `PENGUIN_FONT` and `PENGUIN_FONT_SIZE` are read in `glue.c` together with the Ghostty config (design D5) and have no invalid case: a non-numeric or non-positive `PENGUIN_FONT_SIZE` is ignored in favour of the configured size.

## Risks / Trade-offs

- [libghostty-vt API signatures still change] → pin one ghostty commit in `build.zig.zon`; bump deliberately.
- [Physical-key mapping gaps break kitty keyboard encoding for some keys] → copy Ghostty's GTK keycode table instead of writing one; manual check of Escape, arrows, Enter, Tab, and modified letters in mbv.
- [Cell metrics rounding mismatches Pango's actual glyph advance] → draw every grapheme at `col * cell_w` (never let Pango advance across cells), so the grid is exactly what's reported.
- [Rendering all dirty cells through Pango layouts is slow] → acceptable at 40×~90 cells; profile only if redraws visibly lag.
- [Exclusive zone stacks with niri's own left strut] → visible gap = `GUTTER` + the niri layout's left strut (24 in the user's `woims/layout.kdl`). Documented in the README; users can set `GUTTER=0`.
- [OSC 52 unavailable] → mbv doesn't use it; not needed for v1.
- [Zig 0.16 not installed] → install step in tasks; `pinwin` users are unaffected.

## Migration Plan

None. `penguin` is additive; `pinwin`, `pin.kdl` and the niri `include` line stay as they are. Removing `penguin` means deleting `penguin/` and its installed binary.

## Open Questions

- Exact DA1 attribute values Ghostty reports (copy from Ghostty source in task 1.3; doesn't change the approach).
- Whether ratatui-image's kitty transmission is PNG (`f=100`) or raw RGBA (`f=32`); both are handled once the PNG decoder is installed, so this only affects which path gets exercised.
