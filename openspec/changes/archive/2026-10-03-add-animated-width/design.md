# Design

## Context

Verified against the sources:

- The apply path: `pinwin_apply_layout` (`src/pinwin_api.c`) copies the layout into an
  `ApplyRequest` and `g_main_context_invoke`s `apply_on_gtk_thread`, which calls
  `glue_publish_layout` (`src/glue.c`): validate against the live monitor and cell metrics, set
  `g_layout`/`g_cols`, then `apply_panel_width`, `apply_layout_surfaces`, `apply_size`.
- pinwin never calls layer-shell `set_size`. Width is `gtk_widget_set_size_request(g_area,
  g_cols * g_cell_w, -1)` plus `gtk_window_set_default_size` (`apply_panel_width`); GTK drives
  configure/ack/commit.
- Tiled windows reflow because of a second surface, `g_reserve` (transparent, bottom layer),
  whose exclusive zone is `left + panel + right` (`apply_layout_surfaces`).
- cols→pixels is `g_cols * g_cell_w`, an integer product (`cell_w` is an integer).
- `apply_size` (`src/pty.c`) calls `pinwin_size`, `TIOCSWINSZ` and `raise(SIGWINCH)` only when
  rows or `g_cols != g_grid_cols` change, so `on_area_resize` per frame is a no-op once
  `g_cols` is the target.
- The Rust wrapper documents `Panel::apply_layout` as stateless and repeatable
  (`the consumer's ffi crate/src/panel.rs`). Only that doc comment was checked, not the client's call sites.
- `PinwinLayout` is six `int32`s (24 bytes), passed by pointer, with no size or version field.
- The "hash" the client pins is the Zig package content hash over `build.zig`, `build.zig.zon`, `src`,
  `host` and `demo`; there is no separate ABI hash. The only tag is `library-abi` (on
  `6c2201f`); the client pins commit `835d39c`.

## Goals / Non-Goals

**Goals:** smooth, continuous width change of the panel and its reservation on request; final
frame exactly the requested width; non-animated path unchanged in latency and correctness;
one frame callback, no per-frame allocation.

**Non-Goals:** side/gutter animation; spring physics; env/config overrides; the client changes.

## Decisions

**D1. Additive entry point, not a default and not a struct change.**
`int pinwin_apply_layout_animated(const PinwinLayout*, uint32_t duration_ms)`.
`duration_ms == 0` snaps; above 1000 clamps to 1000. Return codes are those of
`pinwin_apply_layout`; `PINWIN_OK` means validated and accepted, not finished, so the caller's
5 s wait never spans the animation. `ApplyRequest` gains an internal `uint32_t duration_ms`
(not ABI). `PinwinLayout` is untouched.
*Rejected:* (a) animate by default: silently changes every host, no opt-out or duration
control, risks the snap path. (b) a flag/field on `PinwinLayout` or a size-prefixed struct:
changes the 24-byte layout, the Rust `repr(C)` mirror would misread it, and it forces a
lockstep consumer bump.

**D2. Tween both surfaces from one tick callback.**
Per frame: `gtk_widget_set_size_request` and `gtk_window_set_default_size` on the visible
panel, and `gtk_layer_set_exclusive_zone` on `g_reserve` with the sum from the checked
`pinwin_side_geometry`. No manual `set_size`/commit; GTK coalesces if frames outrun
configures. A `panel_px()` helper returns the animated width while a tween is active, else
`g_cols * g_cell_w`; `apply_panel_width` and `apply_layout_surfaces` use it.
*Rejected:* tweening only the overlay and snapping the reservation: windows jump while the
panel fills in. Kept as a fallback mode if reflow benchmarking is bad.

**D3. Target-first terminal behavior.**
At t0, set `g_cols` to the target and call `apply_size()` once: one `pinwin_size`, one
`TIOCSWINSZ`, one SIGWINCH. The grid is drawn anchored to the docked (outer) edge, with the
rest filled with theme background: left dock at x=0, right dock translated by
`width − cols*cell_w`. Expand reveals a pre-sized grid from the inner edge (right-hand columns
briefly clipped); collapse shows a shrinking background strip; content never slides. Mouse
coordinates in `input.c` take the same right-dock offset.
*Rejected:* per-frame grid/pty resize (about a dozen SIGWINCH and full TUI relayouts);
keeping the old grid until the end (truncated content, then a snap).

**D4. Animation state machine** (GTK thread, one static struct, no allocation):
`{active, from_px, to_px, cur_px, t0_us, dur_us, tick_id, watchdog_id}`.
- Idle → Animating when `duration_ms > 0`, side unchanged, left/right gutters unchanged, cols
  differ, and `gtk-enable-animations` is TRUE. Otherwise the existing snap path runs.
- Start: validate the target, set `g_layout`/`g_cols`, `apply_size()` once, `cur_px` = old
  width, add one tick callback and a `g_timeout` watchdog at `dur + 100 ms`.
- Tick: `t = (frame_time − t0)/dur` from `gdk_frame_clock_get_frame_time`; `cur_px =
  from + (to − from) * ease(t)`, rounded; apply D2; queue a draw.
- Finish at `t >= 1`: `cur_px = g_cols * g_cell_w` exactly, then the ordinary
  `apply_panel_width()` + `apply_layout_surfaces()`, remove the tick and watchdog. The final
  frame runs the same code as the snap path, so there is no drift.
- Watchdog: if the frame clock stalls (output off/occluded), snap to the final layout.
- New animated apply mid-animation: retarget `from = cur_px`, `t0 = now`, fresh duration, same
  tick; last write wins, a few field writes. Plain apply or `duration_ms == 0` mid-animation:
  cancel and snap. Same target cols with only top/bottom gutters changed: apply the gutters
  now and keep the tween.
- Collapse then expand is a retarget from the current width (one more SIGWINCH).
- Teardown: `glue_close_surfaces` removes the tick and watchdog; GTK also drops tick callbacks
  on widget destroy; nothing commits after destroy.
- Failure: validation failure changes nothing (`PINWIN_ERR_INVALID`). If `apply_size()` fails
  at t0 (`GLUE_ERR_TERMINAL`), snap to the final layout and return `PINWIN_ERR_INTERNAL` as
  today. The surface is never left mid-state.

**D5. Ease-out cubic, 200 ms default, per-call duration.**
200 ms is about 12 frames at 60 Hz: clearly motion, but short because every tiled client
re-renders each frame. From memory (unverified here) niri's default window-resize spring is
critically damped with stiffness 800, settling in roughly 200 ms; ease-out cubic approximates
it with no solver. A retarget restarts easing from the current width with no velocity carry.
`PINWIN_ANIM_DEFAULT_MS` is a documented hint in the header; the default itself lives in the
host. *Rejected:* spring physics (solver and tuning for no visible gain).

**D6. Reduced motion via `gtk-enable-animations`.**
Read once per apply with `g_object_get` on `GtkSettings`; FALSE forces `duration = 0`. No
env var or flag.

**D7. Versioning and rollout.**
Bump `.version` to 0.2.0 and no new tag (OQ-1). pinwin lands first; the client
then bumps its pin and adds the call (separate change).

## Risks / Trade-offs

- [Every tiled client re-renders each frame] → short default; overlay-only fallback mode;
  benchmark on real clients.
- [Right-dock offset errors in draw or mouse mapping] → tasks 4.x, manual both-sides check.
- [Stalled frame clock leaves a mid-width panel] → watchdog snap.
- [Full redraw per frame (Pango per glyph over all cells)] → name for later benchmarking; the
  tick makes no allocations.
- [Repeated SIGWINCH on rapid toggles] → accepted; debounce only if benchmarking shows need.

## Open Questions

- **OQ-1 (resolved).** No new tag: the client pins a commit hash, not `library-abi`.
- **OQ-2.** Does `gtk_layer_set_exclusive_zone` on `g_reserve`, an unrendered opacity-0
  window, commit every frame without that window redrawing? Needs gtk4-layer-shell source or
  an experiment (task 1.1). Biggest risk.
  Spike attempt (one run, throwaway edit reverted): a 16 ms `g_timeout` raising `g_reserve`'s
  exclusive zone by 4 px per step under `zig build demo`, sampled with
  `niri msg -j windows` (first tiled window `tile_size` width). Observed 714 → 677 → 669 → 661
  → 654 → 646 → 638 → 634 → 633 over successive ~120 ms samples, i.e. intermediate widths
  rather than a single jump, which suggests per-step reservation commits and incremental
  reflow. Not conclusive (coarse sampling, no pixel capture, single run): task 1.1 stays
  unchecked pending the user's visual confirmation.
  **Resolved** by running the real tween: tile widths step through ~8 intermediate values
  per 200 ms animation (634 → 548 on expand), so the reservation commits and niri reflows on
  each step without the reservation window redrawing.
- **OQ-3 (resolved).** niri does not animate layer-surface resizes. Per the niri wiki
  (Configuration: Animations,
  https://github.com/niri-wm/niri/wiki/Configuration:-Animations), the window-resize animation
  covers only manual resizes (e.g. `switch-preset-column-width`, `maximize-column`) and skips
  changes up to 10 px; a layer-shell surface resize or exclusive-zone change is not such an
  action, so niri reflows tiles immediately on each change, i.e. per frame during the tween.
  Source: the client orchestrator review. Whether the reservation commits each step is still tested by task 1.1
  (OQ-2).
- **OQ-4 (resolved).** the client holds no one-shot state, verified against its call sites:
  `src/pin.rs` builds a fresh `Layout` per call (`layout_from_config`, `apply_layout`);
  `src/app/dispatch/settings.rs` `toggle_pinned_width` calls `pin::apply_layout` at the
  toggled width and stores the width only on `Ok`; `the consumer's ffi crate` `Panel::apply_layout`
  is stateless and repeatable. Retargeting from the client is safe.
- **OQ-5.** Does `input.c` need more than a coordinate offset for right docking? Not read.

## Benchmarks to run later

`on_draw` frame time during the tween; client reflow cost under niri with heavy windows;
dropped frames at 60 and 120 Hz.
