# Design

## Context

The panel's pixel width is already derived from columns everywhere: `g_cols * g_cell_w` drives the window default size (glue.c `on_activate`), the side geometry (`penguin_side_geometry`) and validation (`penguin_layout_validate` takes `panel_cols`). `glue_layout_metrics()` already hands the current cols and cell metrics to the options code. `glue_publish_layout()` is the single validate-then-apply seam used by the options window. What does not exist today: any runtime path that changes `g_cols` — the width is fixed once at init, and `apply_size()` recomputes only rows.

This change builds on the in-flight `add-penguin-tray-options` change (staged options window, config file, `PenguinLayout` with side + four gutters). See proposal.md for motivation and the spec deltas for behavior.

## Goals / Non-Goals

**Goals:**
- Panel width editable at runtime, expressed in columns.
- Columns persist in the penguin config like the other layout settings.
- Grid and PTY resize on Apply without respawning the command.

**Non-Goals:**
- Pixel-denominated width input.
- Runtime font changes (cell width stays launch-fixed, which is what makes columns exact).
- Any change to `pinwin`, the child command, keyboard mode or launch validation of `COLS`/`GUTTER`.

## Decisions

**D1 — Columns, not pixels, is the input unit.** The panel is already columns-native: pixel width is defined as `cols * cell_w`, and `cell_w` is an integer measured once at startup. Any pixel width that is not a multiple of `cell_w` cannot be drawn as a whole number of columns — it would truncate a column or strand dead pixels at the panel's edge. Columns keep the width exact by construction. No alternative considered further; the user set this direction and the codebase agrees with it.

**D2 — Columns join `PenguinLayout` as a sixth setting.** `PenguinLayout` gains `int32_t cols`; the config file gains `cols=` under `[layout]`; Apply saves it with the rest; a valid saved layout overrides the launch `COLS`. Precedence mirrors the right gutter exactly: saved `cols` > `COLS` env > default 40. The config's whole-file validity rule is unchanged: a malformed `cols` makes the whole saved layout fall back to launch defaults, same as a malformed gutter. Alternative (session-only columns, config stays COLS-free) rejected: reopening the options window would then show a column count that contradicts the live panel, breaking the staged-editing model. This amends the two "SHALL NOT change/override COLS" invariants from add-penguin-tray-options — documented decisions get superseded by changes; the deltas carry the new text.

**D3 — Apply mechanics ride the existing publish seam, with two adjustments.**
- `glue_publish_layout()` must validate against the *staged* `layout->cols`, not the current `g_cols` (today it passes `g_cols`).
- It then sets `g_cols = layout->cols`, resizes the visible panel's width, and re-reserves the strip via the existing `apply_layout_surfaces()`.

Width resizing in GTK4 cannot use `gtk_window_resize` (it does not exist in GTK4) and `gtk_window_set_default_size` does not move an already-mapped window. The natural-size path is the mechanism: set the drawing area's width size request to `cols * cell_w` and let the layer window follow its content. At init this equals today's `gtk_window_set_default_size(g_win, g_cols * g_cell_w, -1)`, so both paths agree on the same number.

**D4 — `apply_size()` learns to react to column changes.** Today it fires `penguin_size` + `glue_pty_resize` only when the row count changes. It must also fire when `g_cols` changed since the last applied grid, so a width-only Apply still resizes the grid and PTY. Columns remain the input (never re-derived from the allocated width the way rows are derived from height): deriving them would fight the reservation and silently drop sub-cell pixels.

**D5 — Parsing and bounds.** A `penguin_parse_cols` companion to `penguin_parse_gutter`: optional nothing, digits only, no sign, value ≥ 1 and ≤ 65535. The 65535 ceiling is the PTY `winsize` column field (unsigned short); values beyond it are rejected at parse time as an invalid field, which is long before the geometry check would fire on any real monitor. Zero is rejected (a zero-column terminal is meaningless and `COLS=0` is already a launch error).

**D6 — The Columns field sits first in the options window.** It is the largest knob and it changes what the gutter validation runs against; placing it above the gutters reads naturally and keeps the existing gutter layout untouched.

## Risks / Trade-offs

- [Layer-shell width follow on a runtime natural-size change] → Verify on a live niri session that the visible panel and the reservation both track the new width (manual checklist item; ask before touching the live session, per project rule).
- [Grid resize while the child runs] → Same path rows already take (`penguin_size` + `glue_pty_resize`); mbv and other TUIs already survive row resizes from output changes.
- [Saved layouts written by the previous build lack `cols`] → Missing key falls back to launch `COLS`; old configs stay valid without migration.
- [Two instances with different column counts write the same config] → Existing last-save-wins semantics, unchanged.

## Migration Plan

Config format is additive: `cols` is optional, absent means launch `COLS`. Rollback is plain revert; a config written by the new build still loads in the old one only if unknown keys are ignored — they are (existing "ignore unknown keys for forward compatibility" rule), so a downgrade reads the file but drops `cols`.

## Open Questions

None.
