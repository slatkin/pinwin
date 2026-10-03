# Design

## Context

Verified against the sources at 984ffb4:

- `glue_publish_layout` (`src/glue.c`) validates the staged target, then immediately sets
  `g_layout = *layout; g_cols = layout->cols` and calls `apply_size()` — so `src/pty.c`
  does `TIOCSWINSZ` + `raise(SIGWINCH)` at tween start (the archived design's D3
  "target-first" and its D4 start step).
- The child answers the t0 resize with a full redraw and image re-transmission; the pty read
  source (`on_pty_readable`, `src/pty.c`) drains unbounded on the GTK main loop and starves
  the frame-clock tick (`src/glue_anim.c`), so the watchdog snaps — the observed jerk.
- `apply_size` (`src/pty.c`) resizes only when `rows != g_rows || g_cols != g_grid_cols`, so
  a publish that leaves `g_cols` alone is a column no-op.
- `glue_anim_draw_offset` (`src/glue_anim.c`) computes the grid shift as
  `gtk_widget_get_width(g_area) - g_cols * g_cell_w`; render and input already anchor the
  grid by it. With `g_cols` held at the applied count this offset is exactly right for the
  whole tween (expand: surface wider than grid, theme-background strip on the inner edge;
  collapse: inner columns clipped) — the intermediate look the archived design intended.

## Goals / Non-Goals

**Goals:** no winsize change or SIGWINCH reaches the child during a tween; exactly one resize
to the final columns when the tween ends, on every path that ends or abandons it; the end
path is the same geometry path as a non-animated apply; rejected applies change nothing;
non-animated applies unchanged.

**Non-Goals:** bounding the pty drain; animating gutters/sides; mbv changes; new tests that
would fake GTK coverage.

## Decisions

**D1. The tween carries the pending target; `g_layout`/`g_cols` stay applied during the tween.**
`Anim` gains a `PinwinLayout pending`. At tween start `g_layout` and `g_cols` are untouched;
the tween eases `panel_px()` from the current width to `pending.cols * g_cell_w`. Everything
the child can observe (grid columns, PTY winsize, SIGWINCH) therefore stays put for the whole
tween, which removes the t0 redraw/re-transmit burst that starved the frame clock. The
draw-offset anchoring needs no change: it already anchors a `g_cols` grid against the docked
edge while the surface is another width.
*Rejected:* keeping target-first and only deferring the pty resize — the grid would still be
the target grid mid-tween (clipped the wrong way), and the grid reallocation itself is part
of the burst.

**D2. One commit path, used by every end.**
`anim_commit` sets `g_layout = a.pending`, `g_cols = pending.cols`, then runs
`glue_apply_geometry()` + `apply_size()` — the same tail a non-animated publish runs — so the
final width is exact and the child sees exactly one resize to the final columns.
- `anim_finish` (tick `t >= 1` and the watchdog) stops the sources and commits.
- `glue_anim_end()` (new): no-op without a tween; otherwise stops the sources and commits.
  `glue_close_surfaces` calls it instead of `glue_anim_cancel`, so `pinwin_stop` teardown
  delivers the final columns before the surfaces go (while `g_area` is still alive).
- `glue_anim_cancel()` keeps its meaning: drop the sources without committing, used where the
  caller applies a new target immediately (a snap publish mid-tween, and the
  `GLUE_ERR_TERMINAL` snap-back, which then runs the normal publish tail — one resize attempt
  at the requested columns, matching the archived "panel at the requested final width"
  failure contract).

**D3. Retarget is last-write-wins with a single trailing resize.**
A mid-tween animated apply updates the tween's `pending` (newest wins) and, when the column
count changed, restarts the tween from the current animated width with a fresh duration via
`glue_anim_begin` (which already stops and re-adds the tick and watchdog). No resize happens
mid-tween; the single trailing resize at the end uses the newest pending. A mid-tween apply
with the same columns but different gutters only updates `pending` and keeps the tween's
timing, matching the archived behavior for repeated applies. A mid-tween non-animated apply
cancels and snaps as today.

**D4. Publish structure unchanged otherwise.**
Validation still validates the staged target against live metrics before any mutation, and a
rejected apply still leaves everything untouched. The animate-eligibility test (duration > 0,
animations enabled, same side, same left/right gutters) compares against the applied
`g_layout`, which during a tween always has the same side and gutters as the pending target.
The no-column-change publish still snaps the gutters exactly as today.

## Risks / Trade-offs

- [During the tween the drawn grid is the old width, so an expand reveals a theme-background
  strip instead of content until the end] → that is the intended intermediate look (contract
  5); content lands once, without the jerk.
- [Teardown commits a resize the user will never see] → cheap (one ioctl + SIGWINCH) and it
  keeps the child's state equal to the last requested layout, per the exactly-once contract.
- [A row resize failure mid-tween (`on_area_resize` path) is only retried at the end] →
  pre-existing shape; the end commit retries through `apply_size`.

## Open Questions

- None.
