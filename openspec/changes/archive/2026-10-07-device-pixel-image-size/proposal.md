# Proposal

## Why

Since device-pixel-cell-reports (PR #33), the panel reports its cell size in device pixels,
so a host that sizes kitty images from that report transmits device-resolution bitmaps. The
image pass still draws an image whose size comes from its own pixels at one logical pixel
per image pixel, so at an output scale above 1 the image is drawn scale× too large: at 1.8 a
thumbnail runs past the panel's right edge and covers the rows below it. Scale 1 is
unaffected, which is why the scale-1 live check of PR #33 missed it.

## What Changes

- A kitty image whose drawn size comes from its own pixel size is drawn at that size divided
  by the panel's resolved output scale, so one image pixel lands on one device pixel:
  - a unicode-placeholder (virtual) placement, always;
  - a viewport placement that gives neither a column nor a row count.
- A viewport placement that gives a column or row count keeps its current size: it is sized
  from the logical cell already, and dividing it would shrink it.
- The source rectangle (which image pixels are drawn) is unchanged.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: a new requirement fixes the drawn size of kitty images whose size comes
  from their own pixels: device pixels, matching the device-pixel cell size reports.

## Impact

- `src/term/cells/images.rs` (`image_next`, `virtual_rect`, `viewport_rect`) and
  `src/term/cells.rs` (`Terminal::image_next`): the image pass takes the resolved scale
  from the terminal's existing scale note.
- `src/ghostty_sys/kitty.rs`: the placement `COLUMNS`/`ROWS` data selectors (10, 11) are
  declared.
- No change to the reports, the layout, the grid or the pointer mapping.

## Experiment result

- [x] recorded

Ad hoc experiment on 2026-10-07, in the worktree `~/Dev/worktrees/device-pixel-image-size`
(uncommitted `EXPERIMENT` patch dividing the image-sized destination by the scale note),
run by the user on the laptop at output scale 1.8 with a host showing a unicode-placeholder
kitty image. The log reported `virtual img=576x324 scale120=216 dest=320x180` on every
frame, and the user confirmed the image is now the correct size, fits the panel and no
longer covers the rows below. Before the patch the same image was drawn at 576×324 logical
pixels. The viewport path ran no placement in that session, so its half of the fix is proven
by unit tests only.
