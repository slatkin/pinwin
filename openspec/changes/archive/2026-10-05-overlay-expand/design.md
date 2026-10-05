# Design

## Context

See proposal.md for why. Current state: `Surfaces` derives the gap from the applied
`Layout` every frame (`apply_layout_surfaces` sets the reserve surface's exclusive zone
to `left + panel_px + right`), and the width tween moves panel and gap together, so niri
reflows tiles continuously. `Layout` (`src/layout.rs`) has no push/cover notion;
`publish` (`src/surfaces/mod.rs`) validates the staged layout's gap and mutates live
state only on success.

## Goals / Non-Goals

**Goals:**
- Same-side covering expand/shrink moves no tiles; the gap stays at the last pushing
  strip for the whole excursion.
- Side switches always move the gap, even when wide.
- Pushing callers see zero behavior change.

**Non-Goals:**
- No per-frame compositor protocol changes; the gap surface and its layer stay as-is.
- No changes to focus, accent, input mapping, grid caching, or the pty drain beyond
  reading the held gap.
- No `pinwin` binary option; the binary stays pushing-only.

## Decisions

### D1: The choice lives on `Layout`, pushing by default
Add a push/cover flag to `Layout` with a builder-style opt-in (`Layout::new` keeps
pushing). Rationale: the user requirement is "each size has its own setting", and the
setting must travel with startup layouts as well as applies — a separate apply-time
parameter would split one logical layout across two channels. Alternative (global host
flag) rejected: it cannot express "small pushes, big covers" in one handle.

### D2: `Surfaces` holds the gap separately from the applied layout
Store the held gap as a (side, zone px) pair updated only when a pushing layout
publishes, or on any side switch — where the side flips and the width stays at
the last pushing width (an empty held strip stays empty); covering publishes on
the same side leave it untouched.
`apply_layout_surfaces` draws the visible panel from the current layout plus the
tweened `panel_px`, and the reserve surface from the held gap. Alternatives considered:
deriving the gap from the layout each frame (today's behavior — the bug being fixed);
hiding/destroying the reserve surface while covering (churns surfaces per toggle and
risks compositor flicker; setting the zone from held state is a single commit).

### D3: Animation moves the panel always, the gap only when pushing
`should_animate` admits layouts differing only in columns and/or the push/cover flag
(same side, same left/right gutters). Per frame the panel width tweens; the gap tweens
only when the target pushes and its strip differs from the held one. Snap rules are
unchanged: side/gutter change, zero duration, or reduced motion snaps. A push/cover-flag
difference alone never forces a snap — the existing side/gutter comparison already
admits it. Rationale: this
keeps one tween path with a conditional gap, rather than two animation modes.

### D4: Validation splits by choice
Pushing keeps today's checks (overflow, non-negative gap, gap leaves output width,
one full row). Covering checks overflow, visible panel fits within the output width,
and the vertical row — never the untouched gap. `publish` validates before mutating
either the applied layout or the held gap, so a rejected covering apply changes
nothing. Rationale: a covering panel wider than the output is nonsense even though it
reserves nothing; everything else about covering is intentionally unchecked.

### D5: Covering start holds an empty gap
A panel started pushing holds its startup strip from the start. A panel started
covering holds a zero strip on its side until the first pushing apply
establishes it. Rationale: there is no earlier pushing width to hold; "reserve
nothing" is the only non-jarring initial state.

## Risks / Trade-offs

- [Covered tiles are unreachable] Covered windows cannot be clicked while covered; the
  host must shrink to reach them. → Mitigation: documented behavior; expand is
  explicitly temporary, and focus/cover never traps the keyboard (click-away still
  works on uncovered tiles).
- [Negative gutters plus covering] A covering layout with a negative edge gutter still
  insets the visible panel past the output edge while the gap holds. → Mitigation: no
  special-casing; the panel follows its own margins, the gap follows the held strip,
  and validation still rejects structural overflow.
- [Side switch while covering] Must move tiles by definition. → Mitigation: spec
  makes it explicit and expected, not a regression.
- [Held-gap staleness] The held strip can outlive the layout that set it across many
  covering toggles. → Mitigation: single stored pair, updated on every pushing
  publish; unit tests pin push→cover→cover→push sequences.
- [Covering retarget mid-tween] A covering apply that interrupts a pushing tween holds
  the gap at the held strip from that pushing publish while the panel retargets from
  its live width, so one frame may shift tiles a single step. → Mitigation: specified
  behavior — every scenario starts from a settled gap; the window is one tween
  duration and the next push re-syncs.
