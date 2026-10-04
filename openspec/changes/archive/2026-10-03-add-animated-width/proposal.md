# Proposal

## Why

The client toggles the panel between a collapsed and an expanded width with one
`pinwin_apply_layout`. niri does not animate layer-surface resizes, so the panel and the
tiling reflow snap in a single frame, which reads as jarring. Only pinwin owns the surface and
its GTK4 frame clock, so the smoothing has to live here.

## What Changes

- **New additive ABI symbol** `pinwin_apply_layout_animated(const PinwinLayout*, uint32_t duration_ms)`
  in `src/pinwin_api.h`. `PinwinLayout`, `PinwinStartup`, every existing symbol and every
  constant stay byte-for-byte unchanged; `pinwin_apply_layout` stays the snap path.
- **Tick-driven width tween** (new `src/glue_anim.c`): one `gtk_widget_add_tick_callback`
  drives the visible panel's width (`g_area` size request plus window default size) and the
  reservation surface's exclusive zone together, so niri's tiled windows reflow alongside.
- **Target-first terminal behavior**: the grid and PTY winsize are resized once, to the target
  columns, when the animation starts; the grid is drawn anchored to the docked edge.
- **Interruption**: a new apply mid-animation retargets from the current animated width;
  teardown cancels the tween; a stalled frame clock is snapped by a watchdog.
- **Reduced motion**: `gtk-enable-animations = FALSE` forces a snap.
- **Defaults**: `PINWIN_ANIM_DEFAULT_MS` (200) documented in the header as a hint; ease-out
  cubic; durations clamp to 1000 ms.
- **Version**: `build.zig.zon` `.version` 0.1.0 → 0.2.0 and a new release tag.

## Capabilities

### Modified Capabilities
- `pinwin-panel`: ADDED animated-width requirements; MODIFIED "Apply layout without restarting
  the terminal" and "C ABI lifecycle" to admit the animated entry point.

## Impact

- `src/glue_anim.c` (new), `src/glue.c`, `src/glue_internal.h`, `src/pinwin_api.c`,
  `src/pinwin_api.h`, `src/render.c`, `src/input.c`, `build.zig`, `build.zig.zon`.
- No new dependencies.
- **Rollout order: pinwin first, then the client.** Any pinwin `src/` change alters the Zig package
  content hash, so the client's `the consumer's ffi crate/build.zig.zon` URL+hash must be bumped
  (`zig fetch --save`) to consume it. The client then adds the `ffi.rs` declaration and
  `Panel::apply_layout_animated(Layout, Duration)`, then switches its toggle path. The client cannot
  call the symbol before its pin includes it. Those the client edits are a separate change in the the client
  repo and are out of scope here.
- Non-goals: animating side or gutter changes (they snap); spring physics; a runtime config
  or env override; any consumer-side change.
