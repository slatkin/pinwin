# Design

## Context

`HeldGap` (`src/geometry.rs`) is the strip the reserve surface holds: an edge and a zone.
`start` and `after_publish` move it: push resets the zone to its own strip, cover keeps it.
Nothing resets it to zero while the panel runs. See proposal.md for why that is a problem.

The animated change path (`src/proxy/tween.rs`) tweens the gap with the panel only when both
the old and the new mode push. Otherwise it holds the gap and sets the end gap with
`final_gap`, which today leaves a covering target at the resting gap.

## Goals / Non-Goals

**Goals:**
- One request, `set-reserve none` (or `reserve: none` on a size change), zeroes the held strip
  of a running, visible panel.
- `none` is a stable mode: later applies in `none` keep the strip at zero.

**Non-Goals:**
- Changing `cover`'s hold semantics.
- A one-shot `release` command.
- Tweening the strip down to zero.

## Decisions

**D1: A third `ReserveMode::None`, not a one-shot release.** The mode is visible in `status`
and events and survives later applies. A release command would leave the mode at `cover`,
where the next push re-reserves and `status` cannot tell a released panel from a never-pushed
one. `none` mirrors `push`: each mode resets the zone to a known value.

**D2: `HeldGap::start` and `after_publish` set the zone to zero for `none`.** The edge follows
the geometry as for the other modes.

**D3: Validation treats `none` like `cover`.** `none` refuses only a panel wider than the
output. The strip is zero, so `NoReserve` and the push-only `NoWidth` strip check do not apply.

**D4: Animation lands the strip at the end.** For a `none` target the gap holds during the
animation and `final_gap` returns a zero zone on the last frame. This reuses the existing
cover to push rule, so no new tween path exists. `tweens_with_panel` stays push-to-push only.
Alternative: tween the strip to zero with the panel. Rejected: it adds a second tween path
for a rarely used transition.

**D5: Cover to none follows D4 too.** One rule for every `none` target: hold during the
animation, zero at the end. With `ms` 0 the zero applies in the same step.

**D6: `none` is accepted wherever `push` and `cover` are.** The shared `FromStr` impl in
`src/config.rs` serves `run --reserve`, config tables, `set-reserve` and the `reserve` field
of size changes. The tray list `RESERVES` gains one entry.

## Risks / Trade-offs

- [A `none` end gap is easy to miss in `final_gap`] -> A tween test covers push to none and
  cover to none, and checks the last frame's zone.
- [Existing exhaustive `match` on `ReserveMode` fails to compile] -> The compiler lists every
  site; that is the intended way to find them.
- [Tiled windows reflow once at the end of a push to none animation] -> Accepted by the user.
