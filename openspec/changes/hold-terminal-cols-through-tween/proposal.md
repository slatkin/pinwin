# Proposal

## Why

The animated width tween shipped in `2026-10-03-add-animated-width` resizes the terminal grid
and PTY winsize to the target columns at tween start (its design D3 "target-first"). The
child (mbv) then redraws fully and re-transmits its image payloads for the new width, and that
pty burst is drained in an unbounded loop on the GTK main loop, starving the frame-clock tick
until the watchdog snaps — the user sees "a janky jerk on resize" and "images flash on this
janky jerk". The tween's whole point was to remove that jerk.

## What Changes

- **Previous-hold terminal behavior**: during an animated column change the child's terminal
  (grid + PTY winsize) stays at the previously applied column count for the whole tween; the
  target columns are applied exactly once, at the end, through the same code path as a
  non-animated apply. The intermediate look is the existing `glue_anim_draw_offset` anchoring:
  during an expand the surface is wider than the grid (theme-background strip on the inner
  edge), during a collapse the grid's inner columns are clipped.
- **Every end path delivers the final columns exactly once**: natural finish, the
  stalled-frame watchdog, teardown (`pinwin_stop`), a non-animated apply mid-tween, a failed
  apply's snap-back, and a mid-tween retarget (last-write-wins: newest target, single trailing
  resize).
- Layout validation still validates the staged target layout against live metrics; a rejected
  apply leaves state untouched, as today. Non-animated applies behave exactly as today.
- The tween carries the pending target layout instead of publishing it at t0, so nothing about
  the child's world changes until the tween ends.

## Capabilities

### Modified Capabilities
- `pinwin-panel`: MODIFIED "Terminal grid during a width animation" (previous-hold instead of
  target-first) and "Interrupting an animation" (exactly-once trailing resize on every end
  path).

## Impact

- `src/glue.c`, `src/glue_anim.c`, `src/glue_internal.h`. `src/pty.c` and `src/render.c` are
  untouched: with `g_cols` held at the applied count, `apply_size` is a column no-op during
  the tween and the existing draw-offset anchoring is already correct for the previous-count
  grid.
- No new dependencies, no ABI change, no version bump (the ABI surface and its structs are
  unchanged; mbv consumes a commit SHA).
- **Rollout order: pinwin first, then mbv.** mbv's row 4.3 of `panel-expand-toggle` fails
  against the current t0 resize; this change is the upstream half of that correction. mbv
  pins a commit SHA, so it only needs to re-fetch.
- Non-goals: bounding the pty drain (tried in 2ac05e3/f5ba91e and reverted; with no t0 resize
  there is no mid-tween burst to drain), animating gutters or sides, mbv-side changes.
