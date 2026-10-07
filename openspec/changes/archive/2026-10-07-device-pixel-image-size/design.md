# Design

## Context

See proposal.md — Why. device-pixel-cell-reports D2 kept drawing logical; the terminal
already holds the resolved scale as its shared scale note (1/120 units, read by
`size_report`). The image pass builds each placement's destination in `image_next`:
`virtual_rect` uses the image's pixel size, `viewport_rect` uses ghostty's resolved
`pixel_width`/`pixel_height`, which ghostty computes from the logical cell when the
placement gives a column or row count and from the image size otherwise.

## Goals / Non-Goals

**Goals:** image-sized destinations are divided by the scale; cell-sized ones are not.

**Non-Goals:**
- Changing ghostty-vt's own cell size or the grid ghostty reserves for an image-sized
  viewport placement (see Risks).
- Fitting a placeholder image into its placeholder cell box (kitty's aspect fit); the panel
  keeps drawing it at its own size.

## Decisions

**D1 — The scale comes from the terminal's scale note.** `Terminal::image_next` reads
`ctx.scale_120` and passes it down, the same note `size_report` answers from, so the drawn
size and the reported size can never use different scales. Alternative: pass the scale from
the image pass caller — rejected, a second source of the same fact.

**D2 — One conversion, the inverse of the report's.** Logical = device × 120 / scale_120,
rounded half up in integer arithmetic, as a small function next to `virtual_rect` with a
unit test. At scale 1 it is the identity.

**D3 — Divide a viewport placement only when it gives neither columns nor rows.** The
placement's `COLUMNS`/`ROWS` data (0 = not given) decide it. When only one is given, ghostty
derives the other from the logical cell and the aspect ratio, so the result is already
logical. Alternative: compare `pixel_width` with the image width — rejected, a cell-sized
placement can coincidentally equal it.

**D4 — Source rectangles stay in image pixels.** Only `w`/`h` of the destination change.

## Risks / Trade-offs

- [ghostty reserves rows for an image-sized viewport placement from the logical cell, so it
  moves the cursor scale× too many rows after placing one] → out of scope here; the drawn
  size is correct, and the cursor movement only matters for hosts that place without
  placeholders and without a size. Recorded for a follow-up if one appears.
- [The viewport half was not exercised live] → unit tests cover the decision and the
  conversion; the placeholder half carries the live proof.
