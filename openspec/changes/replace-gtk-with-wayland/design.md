# Design

## Context

See proposal.md for the motivation. The requirements are in `specs/pinwin-panel/spec.md`.

pinwin has two layer surfaces today. The panel window holds the terminal. A separate reserve
window holds the exclusive zone, so a covering layout can draw over tiled windows while the
gap stays put (overlay-expand). `Surfaces::publish` in `src/surfaces.rs` validates a layout,
stages it, starts or cancels the width tween and applies the geometry. The tween defers the
grid resize to its end and draws the old grid from a retained cache meanwhile.

The library's GTK side runs on a process-lifetime `pinwin-gtk` thread. `Panel` posts each
apply, focus request and teardown to the GTK main context and waits for a bounded reply
(`src/panel/handshake.rs`). The pty read source is a GLib unix fd source in `src/pty.rs`.

Several parts already avoid GTK and carry over unchanged: `src/layout.rs`, `src/term/`,
`src/surfaces/gap.rs`, `src/render/snap.rs`, `src/fontconfig.rs` and the tween step in
`src/anim.rs`. `src/term/` includes the XKB keycode and keyval tables in `src/term/keys.rs`.
`src/fontconfig.rs` is the Ghostty config reader. Its parser carries over, but its one GLib
call, `gtk4::glib::user_config_dir`, becomes a `$XDG_CONFIG_HOME` lookup with the `~/.config`
default. GDK keyvals and X keysyms share one numbering, so the key tables keep working
with xkbcommon keysyms.

Three niri facts shape the approach. All three come from niri's source.

- `layer_shell_handle_commit` (`handlers/layer_shell.rs`) gives keyboard focus to a layer
  surface that maps with `on-demand` interactivity. A later change of interactivity on a
  mapped surface does not grant focus.
- `request_activation` (`handlers/mod.rs`) looks only for layout windows and unmapped
  windows. A token aimed at a layer surface does nothing.
- `spawn` (`utils/spawning.rs`) puts an activation token in the child's
  `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID`. A key binding that spawns
  `pinwin --focus` therefore hands the client a token that niri issued.

## Goals / Non-Goals

**Goals:**

- pinwin owns every commit of its surfaces, so no toolkit step can unmap, resize or redraw
  them behind its back.
- The pure cores and their tests stay as they are. The rewrite replaces the GTK edges only.
- The public `Panel` API keeps its shape. Only `request_focus` changes.

**Non-Goals:**

- A GPU renderer. Decision 6 names the measurement that opens one as a separate change.
- An input method, a clipboard or primary selection. pinwin uses none of them today.
- More than one panel per process.
- A fallback remap for compositors that ignore xdg-activation for layer surfaces. The user
  chose activation only.

## Decisions

### 1. A direct Wayland client on smithay-client-toolkit

The panel thread opens its own Wayland connection with smithay-client-toolkit and runs a
calloop event loop. The toolkit covers the registry, `wl_compositor`, `wl_shm` buffer pools,
wlr-layer-shell, outputs with xdg-output, the seat with xkbcommon and key repeat,
xdg-activation and presentation time. The fractional-scale, viewporter and cursor-shape
protocols come from wayland-protocols and bind through the same registry.

`wl_compositor`, `wl_shm` and `zwlr_layer_shell_v1` are required. If any is missing, or the
connection fails, the start returns `PinwinError::NoDisplay`. The other globals are optional.
For each optional global, decisions 3, 5, 7 and 8 say what changes in its absence.

Alternatives:

- Keep GTK and fix the remap. The remap is one symptom. The size floor, the pixel grid, the
  frame timing and the library footprint stay.
- winit. It has no layer-shell support.
- Raw wayland-client without the toolkit. That repeats the registry, seat, keymap and repeat
  code that the toolkit already provides and tests.

### 2. One panel thread per start, and a channel instead of a main context

Each `Panel::start` spawns a thread that owns the connection, the surfaces, the terminal and
the pty source. The host posts apply, focus and teardown commands through a calloop channel,
and the bounded replies in `src/panel/handshake.rs` stay as they are. Drop posts the teardown,
waits for its bounded reply and returns. The thread then ends on its own. The single-instance
guard and its `AlreadyRunning` error stay.

GTK forced the process-lifetime thread, because GTK initializes once per process. A Wayland
connection has no such limit, so the parked thread and the per-start `GtkApplication` go
away. The pty fd becomes a calloop source, and the bounded drain during a tween stays.

The shared panic guard (`src/guard.rs`) wraps every calloop callback and every Wayland event
handler, as it wraps every GTK closure today. If the connection closes, the thread marks the
panel dead, so later calls return `NotRunning`.

### 3. Surfaces, outputs and the size authority

The panel is a layer surface on the `overlay` layer, anchored to the top, the bottom and the
docked side. Its exclusive zone is -1, so other zones do not push it. It requests its width
with `set_size` and its insets with margins. The reserve stays a separate layer surface. It
carries the exclusive zone, takes no input and shows a transparent buffer one pixel wide.

The panel surface is created with no output, so the compositor places it on the focused
output. The first `wl_surface.enter` names that output. The reserve is then created on the
same output, and the output's logical size from xdg-output replaces `gdk::Monitor::geometry`
for validation. The start handshake completes at that point, like the first-draw resolution
today.

The pty size has one source: the layout's column count and the height in the latest
configure. Rows are that height divided by the cell height. The grid and the pty change only
on a layout apply or a configure with a new height. No interim allocation exists, so nothing
else can reach the pty. A focus request touches neither input.

In `on-demand` mode, the panel maps with no keyboard interactivity and switches to
`on-demand` in the commit after its first buffer. niri grants focus only on the map itself,
so the launch takes no focus. `exclusive` mode maps as `exclusive`.

### 4. Focus through xdg-activation

`ActivationToken` is a newtype with a fallible constructor. It accepts 1..=255 bytes of
visible ASCII. `Panel::request_focus(&self, token: ActivationToken)` keeps the
`NotRunning`, `Internal` and keyboard-mode guards of today. In `on-demand` mode, the panel
thread calls `xdg_activation_v1.activate` with the token and the panel's `wl_surface`. If the
compositor lacks xdg-activation, the request does nothing. The call returns `Ok(())` once the
activation is sent, because the compositor's choice is invisible to the client.

`remap_for_focus`, the run-once map guard and the second-map handling go away. The panel
never unmaps.

The focus socket request becomes `focus <token>\n`. `pinwin --focus` reads
`XDG_ACTIVATION_TOKEN` and builds the token before it connects. A missing or invalid token
exits 2 without contacting the host. The listener parses the token from the request line and
answers `error\n` for a request without one. The request size bound grows to fit the longest
token.

The niri patch adds one branch to `request_activation`. When no layout window matches, it
looks for a mapped layer surface with that `wl_surface` and `on-demand` interactivity. For a
token without the urgency-only marker, it sets `layer_shell_on_demand_focus` to that surface
and queues a redraw. That is the same state a click sets in `focus_layer_surface_if_on_demand`.
A token with the urgency-only marker does nothing for a layer surface, because a layer
surface has no urgency state.

Alternatives:

- A Wayland-level remap, alone or as a fallback. It still unmaps, so a frame without the
  panel stays possible. The user chose activation only.
- A simulated click through the virtual pointer protocol. It moves the cursor and sends a
  click to the hosted program.
- Two panel surfaces that swap. It costs a second surface and shared drawing state to hide a
  remap.

### 5. CPU rendering into shared memory at device size

Each frame draws into a `wl_shm` buffer whose size is the logical size times the
fractional scale, rounded. The viewporter destination is the logical size. Without the
fractional-scale protocol, the scale is the integer preferred buffer scale, or 1. A pool of
two buffers lets the painter draw the next frame while the compositor reads the last one.

tiny-skia fills the rectangles, draws the powerline paths, strokes the one line sprite and
scales the kitty images. The buffers use `ARGB8888`, which every compositor supports. In
memory that order is blue, green, red, alpha. tiny-skia blends channels without regard to
their meaning. So the painter swaps red and blue once, at cache time. That covers each theme
colour, image and colour glyph. No per-frame conversion pass runs.

The snapping rules carry over through `OutputScale` in `src/render/snap.rs`. Cells stay in
logical pixels, and each edge snaps to a device pixel at draw time, as `snap-grid-edges`
decided. `device-pixel-grid` stays parked. A text origin is the cell's snapped corner plus a
snapped offset inside the cell. That meets the new requirement "Cell text on the device pixel
lattice".

The painter draws only the rows that changed, using the dirty state of libghostty-vt's render
state. If the pinned version reports only a whole-frame flag, the painter redraws the whole
grid on a change. A narrow panel fits that inside the frame budget, and task 10.3 measures it.

One painter draws every frame. The cairo fallback, the GSK snapshot path, the retained
texture node and `src/render/parity.rs` go away.

### 6. Text with swash and fontconfig

fontconfig resolves the Ghostty `font-family` to a font file, as Pango does through
fontconfig today. It also finds a fallback font for a code point the primary font lacks. The
painter caches each fallback per code point. A missing family resolves to fontconfig's
`monospace`, which keeps the "No Ghostty config" scenario true.

swash shapes each cell's grapheme cluster and rasterizes the glyphs at the device size, with
hinting on and grayscale antialiasing, the Ghostty defaults on Linux. It renders colour
bitmaps and layered colour outlines for emoji. When the face lacks a bold or italic style,
swash emboldens or skews the outline, as Pango synthesizes those styles today. The nerd-font
constraints in `src/nerd_font.rs` become a swash transform per glyph. The glyph cache keys on
the face, the glyph id, the device size, the style and the constraint.

Alternatives:

- FreeType and HarfBuzz. They match Ghostty's glyphs, but the user chose the pure Rust stack.
- fontdb instead of fontconfig. It reads no fontconfig aliases, so `monospace` and the user's
  fontconfig rules stop working.

### 7. Animation on frame callbacks with a cropped buffer

The tween step, the ease, the clamp, the retarget and the gap rule in `src/anim.rs` stay. The
frame source changes. Each commit during a tween requests a `wl_surface.frame` callback, and
the callback time drives the next eased width. A calloop timer replaces the GLib watchdog,
with the same duration plus 100 ms.

At the tween's start, the painter draws the current grid once into a buffer as wide as the
larger of the start and end widths. The grid sits against the docked edge, and the theme
background fills the rest. Each frame then commits the new layer size, the margins, a
viewporter source crop at the docked edge and the same buffer. No frame draws or uploads
anything. The reserve's exclusive zone moves in the same frames, as today. The tween's end
runs the deferred grid resize and draws the live grid through the plain apply path.

If the compositor lacks viewporter, each frame copies the crop from the cached image into a
fresh buffer. That costs a memory copy and no glyph work.

`Anim::allowed` and its read of `gtk-enable-animations` go away. With the presentation-time
protocol, `PINWIN_FRAMELOG` records the time each frame reached the screen. Without it, the
log records the callback times.

### 8. Input from the seat

The keyboard handler takes the XKB keycode, the keysym and the modifiers from
smithay-client-toolkit and feeds the existing translation in `src/input.rs` and
`src/term/keys.rs`. Key repeat uses the toolkit's calloop repeat, which follows the
compositor's `repeat_info`. Each repeat reaches the terminal as `KeyAction::Repeat`. The
repeat stops on release and on keyboard leave.

The toolkit's `KeyEvent` carries no consumed modifiers, layout group or level-0 keysym, and
`on_key` in `src/input.rs` needs all three. The keyboard handler builds its own xkbcommon
keymap and state from the string in `update_keymap`. It updates that state from the
`RawModifiers` and the `layout` that `update_modifiers` receives. From that state it fills
`consumed_mods`, `is_modifier` and `unshifted_codepoint` as the GDK path does today.

Pointer enter, motion, button and axis events replace the GDK click, motion and scroll
controllers. Axis events carry `value120` for wheels and continuous values for touchpads, and
both map onto the existing scroll units. Keyboard enter and leave replace the GDK focus
controller, so the accent and the focus reports keep their triggers. If the compositor offers
cursor-shape, the panel sets the default shape on each pointer enter.

### 9. Footprint and the link check

The GTK crates leave `Cargo.toml`: gtk4, gdk4, gtk4-layer-shell, pango, pangocairo,
cairo-rs and gdk-pixbuf. smithay-client-toolkit, wayland-client, wayland-protocols,
tiny-skia, swash, a fontconfig binding and png come in. Each new crate passes `cargo audit`.
The demo's `glib::base64_encode` becomes a short encoder inside the demo.

CI drops `gtk4` and `gtk4-layer-shell` from its package list. A new CI step lists the
`pinwin` binary's dynamic dependencies with `ldd`. If a forbidden library appears, the step
fails.
That step is the test for the requirement "No toolkit libraries".

### 10. Tests without a display

The painter draws into an in-memory pixmap, so renderer tests run without a compositor. The
pixel tests are few. One seam test runs at 1.5, one half-block test at 1.25, and one bar
cursor test at 1.5. The text-origin tests run at the scales that their scenarios name. The
snap rules already have their own tests in `src/render/snap.rs`. Font-dependent tests use
the fonts that CI installs (task 2.1). The tests that need a real compositor stay `#[ignore]`d and run
explicitly, as `port-to-rust` D10 set out.

### 11. Module layout

The GTK modules give way to modules split by responsibility. The connection, registry and
event loop live under `src/wayland/`. That directory has one file each for the layer
surfaces, the buffers and scale, the outputs and the seat. `src/panel/gtk_side.rs` becomes
the panel thread. Under `src/render/`, four modules replace `nodes.rs`, `snapshot.rs`,
`node_*.rs`, `texture.rs` and `parity.rs`:

- a canvas module wraps tiny-skia and the channel swap,
- a font module wraps fontconfig,
- a glyph module wraps swash and its cache,
- the grid painter draws the frame.

Other modules hold GTK, cairo, Pango or gdk-pixbuf code too. `src/render.rs`,
`src/render/text.rs`, `metrics.rs`, `sprites.rs` and `images.rs` move into those four modules
or lose that code. `images.rs` loses its gdk-pixbuf decoder. `src/surfaces/area.rs` and
`src/surfaces/hooks.rs` give way to the layer surface module. The tick callback and the GLib
watchdog in `src/anim.rs` give way to the frame callback and the calloop timer.

Every file stays at or under 800 lines.

## Risks / Trade-offs

- [niri does not merge the activation patch] → Hotkey focus stays absent on stock niri, and
  click focus keeps working. The user chose this over a remap. A patched niri build gives
  hotkey focus meanwhile.
- [swash glyphs differ from Ghostty's FreeType glyphs] → The user accepted this trade. Task
  10.2 compares both at scales 1 and 1.5 and records the difference for the user.
- [An emoji font that ships only COLRv1 outlines] → swash renders colour bitmaps and older
  colour outlines. The common Noto Color Emoji build is a bitmap font. A COLRv1-only font
  falls back to its monochrome outline where the font has one.
- [CPU rendering misses frames on the laptop] → Task 10.3 measures a full-grid redraw and a
  tween on the laptop where issue #4 stuttered. Two results open a separate GPU change: a p95
  redraw time above half the refresh interval, or a framelog gap above 1.5 refresh
  intervals.
- [The width follows one compositor round trip behind] → GTK had the same round trip. Each
  frame commits size, crop and buffer together and does not wait for the configure, and task
  1.3 tests this on niri.
- [A shell keeps a stale `XDG_ACTIVATION_TOKEN` in its environment] → niri rejects an
  expired token, so `pinwin --focus` run by hand exits 0 and nothing happens. The README
  says to run the client from a compositor key binding.
- [The rewrite regresses behaviour that GTK gave for free] → The proposal lists each GTK duty,
  and each one has a task and a check. The spec scenarios drive the niri verification in
  group 10.

## Migration Plan

The change lands on one branch. The new modules grow beside the old ones, and then the panel
switches to them. The same change deletes the GTK modules and crates. No feature
flag keeps both paths alive. A revert of the merge restores the GTK version. Until task 8.1
switches `Panel`, no program runs the new path. So every niri verification waits for group 10.

`request_focus` changes its signature, so the crate version moves from 0.1.0 to 0.2.0, and
the README shows the new call. A host that calls `request_focus()` stops compiling, which is
the intended signal.

## Open Questions

- The spec's "Terminal grid during a width animation" says the grid resizes at the start of
  the tween. The code resizes at its end (gsk-render-nodes, C52). This change keeps the code's
  timing and does not modify that requirement. A separate change aligns the text.
