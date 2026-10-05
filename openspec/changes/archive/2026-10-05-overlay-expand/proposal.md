# Proposal

## Why

Expanding the panel narrow to wide (for example 40 to 120 columns) currently pushes every tiled window aside in niri, then pushes them back on shrink. The expand is meant to be temporary — open wide, work, shrink back — and the double shuffle of all tiles is the jarring part. Same-side expand and shrink should move nothing.

## What Changes

- Each applied panel size chooses one of two behaviors: push windows aside (today's behavior), or cover windows and leave them where they are.
- Default pattern: the small size pushes windows aside; the big size covers them. Both are settings, one per size, not hardcoded.
- Expanding and shrinking on the same side moves no windows while a covering size is active: the gap niri leaves stays at the last pushing width, and only the visible panel grows over tiles. Shrinking back uncovers with no tile movement.
- Moving the panel from one side to the other moves the gap with it, because it has to — that shuffle is expected and belongs to whoever moved it. A side switch is never a covering move, even when the panel is wide.
- The push/cover setting travels with each applied layout, so a host describes its small size as pushing and its big size as covering. `Layout::new` keeps pushing so existing callers do not change meaning; covering is opt-in per layout.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: the reserve-space, apply-layout (plain and animated), animated width transition, exact-final-width, and validation requirements change from "panel width and gap always move together" to "a covering size leaves the gap at the last pushing width". The library Panel API requirement changes to carry the per-size push/cover choice.

## Impact

- `src/layout.rs`: per-size push/cover choice on `Layout` plus validation (covering sizes check the visible panel fits; pushing sizes keep today's gap checks).
- `src/surfaces/mod.rs`: the gap surface holds its last pushing width while a covering size is active; side switches always move the gap.
- `src/panel/mod.rs`: API docs for the new choice.
- `examples/demo.rs`: exercise the choice (the `pinwin` binary stays pushing-only).
- No new dependencies. Behavior change only.
