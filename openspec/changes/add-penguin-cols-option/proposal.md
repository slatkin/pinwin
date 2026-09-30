# Proposal

## Why

The penguin panel's width is fixed at launch by the `COLS` environment variable. A user who wants a wider or narrower sidebar must quit penguin and relaunch with a different `COLS` value, losing the running terminal's state. Every other layout property (docking side, gutters) is already editable and persistent through the tray options window; width is the one remaining launch-only knob.

## What Changes

- Add a `Columns` numeric field to the penguin options window, placed before the gutter fields. It edits the panel width in terminal columns — the unit the panel is already built on (pixel width is exactly `cols * cell_w`, no rounding).
- On Apply, resize the running panel to the new column count: re-reserve the strip, resize the existing terminal grid and PTY through the normal resize path, and redraw. The command is never respawned.
- Extend `PenguinLayout` and the config file (`[layout] cols=`) with the column count. Precedence mirrors the right gutter: saved `cols` > `COLS` env > default 40.
- Amend the two tray-options invariants that said Apply/config never touch `COLS`: the applied column count is now a sixth layout setting, saved and restored like the gutters. Fonts, keyboard mode and the command remain launch-only.
- Validation: columns must be a positive integer (digits only, no minus sign) that fits the PTY's 16-bit column field; the existing geometry validation already rejects a width that leaves no room for other windows.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `penguin-panel`: the "Width and gutter settings" requirement's width definition changes from "COLS times the cell width" to "applied columns times the cell width", where applied columns come from a saved layout or `COLS` at launch.
- `penguin-tray-options`: the "Staged options editing", "Apply without restarting the command", "Validate before saving or applying" and "Persist and restore layout settings" requirements change — the options window gains the Columns field, Apply may change columns and must resize the grid accordingly, and the config file gains `cols`.

## Impact

- `penguin/src/options.h` / `options.c`: `PenguinLayout` gains `cols`; strict parser for the new field; config load/save; options window field; validation callers pass the staged cols.
- `penguin/src/glue.c`: `glue_publish_layout` applies the column count (grid resize + PTY + surface width + reservation); `glue_layout_metrics` unchanged (already exposes cols).
- `penguin/tools/check_options.c` and the check it drives: new parser and validation cases.
- `README.md`: the "COLS is never touched by an Apply" statements and the options-window description.
- No changes to `pinwin` (its pin is drag-resizable and Ghostty owns the pixel-to-cell mapping there).
- Builds on the in-flight `add-penguin-tray-options` change (not yet archived); its delta is the baseline this change amends.
