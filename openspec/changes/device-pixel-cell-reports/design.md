# Design

## Context

See proposal.md — Why. Current state: the panel thread measures the cell once from the
font in logical pixels (`render::cell_metrics::measure`, points × 96/72, no output scale)
and that one number serves everything: `Terminal::push_size` feeds it to ghostty-vt and
to the mouse encoder, the pty winsize multiplies it by the grid, and the
`GHOSTTY_TERMINAL_OPT_SIZE` callback (`term/callbacks.rs::size_report`) answers
`CSI 14/16/18 t` with it. The output scale lives on the panel thread's session
(`Scale::resolved()`, 1/120 units) and already drives the renderer and the buffer device
sizes; nothing host-visible reads it.

Measured on niri at scale 1.8, same font: ghostty reports ~18 device px per cell,
pinwin a constant 9. Hosts size kitty transmissions from `CSI 16 t`
(`ratatui_image::Picker::from_query_stdio`), so panel images arrive under-resolution.

## Goals / Non-Goals

**Goals:**
- `CSI 16 t` and the winsize pixel fields answer device pixels at the resolved scale,
  updated on scale change.
- Hosts (mbv) transmit device-resolution bitmaps in the panel with no host change.
- Zero change to layout, drawing, placement geometry or pointer→cell mapping.

**Non-Goals:**
- No re-measure of the font at the device size: the logical cell stays the single
  source, scaled arithmetically.
- No host re-query protocol beyond what terminals already do (SIGWINCH on the scale
  change); a host that caches the cell size across a mid-run scale change behaves as it
  would on any other terminal.
- No changes to the instance socket, toggle/show, layout validation or animation paths.

## Decisions

### D1: One device cell; the pixel-area reports derive from it
`CSI 16 t` reports `round(cell_logical × scale)` per dimension — an integer cell, the
xterm convention (ghostty and kitty also report integer cell values). The pinned vt
computes `CSI 14 t` itself as `columns × cell_width` from the callback's values
(`size_report.zig`'s `widthPixels`), and `apply_winsize` likewise derives `ws_xpixel` as
`cols × cell_w` (`src/pty.rs`), so text-area pixel sizes are always the column/row counts
times the reported cell — the callback cannot report an independently rounded grid width.
The design therefore uses one device-cell value everywhere: 16 device px for a 9-px cell
at 1.8, 640 for a 40-column grid. This can differ from the drawn grid's true device width
(`round(cols × cell × scale)` = 648) by the fractional remainder, because the drawn
cell pitch is fractional on average; that mismatch is what real terminals show too
(ghostty's own cell and text-area reports do not multiply out: 1482 ≠ 82 × 18), and
hosts consume the cell for bitmap sizing and the text area as an upper bound. Alternative
— an independently rounded "drawn width" report — rejected: unreachable through the vt's
own derivation without forking the encoder, for a number no host consumes.

### D2: Only the reports go device; the mouse encoder and ghostty-vt stay logical
Wayland pointer motion arrives in surface-local logical coordinates, and the mouse
encoder maps them to cells using `GhosttyMouseEncoderSize`'s logical cell — the pair is
self-consistent and stays untouched. ghostty-vt's own cell size
(`ghostty_terminal_resize`) also stays logical: its kitty placement geometry
(`cells/images.rs`) is computed in that space and the painter reads `img.x/y/w/h` as
logical (`PainterMetrics::logical_rect`), so feeding it device cells would double-scale
every image. Alternative — convert the whole terminal to device cells and scale pointer
coordinates — rejected: wider churn across the seat and painter for no observable gain.

### D3: The size report reads a shared scale note
`size_report` runs on the terminal's callback context, which also feeds the mouse
encoder, so `ctx.cell_w` must stay logical (D2). Instead the panel thread publishes its
resolved scale into a shared note the context holds (an `Rc<Cell<u32>>` of 1/120 units,
the same units `FractionalScale` carries); `size_report` multiplies and rounds at reply
time. The thread updates the note wherever it already updates the renderer's scale
(`sync_renderer_scale`), so the two can never drift. Alternative — push device values
into the context and unscale for the encoder — rejected: two meanings for one field.

### D4: The winsize push gains the scale, and scale changes re-push
The single grid-push sink (`PanelState::grid_sink`) lives on the state that holds the
session, so the pty push (`apply_pty_size`) gains the resolved scale and passes the
device cell per D1 — `apply_winsize`'s own `cols × cell_w` product then yields the
device pixel fields with no signature change; column/row counts and the terminal's
logical cell are
unchanged, so no grid resize, no extra configure work. The scale-note handlers
(`note_integer_scale`, `note_preferred_scale`) re-push the winsize after the renderer
sync whenever the device pixel size actually changed; they get the fd from the state's
`startup` and re-derive the current grid from `Sizing` (its live columns and the last
configure height), pushing through the same sink the configures use — `grid_sink` never
repeats a grid on its own, so the re-push derives it explicitly. `SIGWINCH` follows from
the existing rule (every successful `TIOCSWINSZ`). A first push that lands before the
preferred scale arrives reports scale-1 pixels briefly; the re-push self-heals it, the
same way the first configure precedes the preferred scale today. A tween's deferred
push and the hidden apply funnel through the same sink, so they pick the current scale
for free.

### D5: The reply-time multiply uses the same rational rounding as the buffers
`scale_dimension` (1/120 exact arithmetic, round half up) is the one rounding rule for
the device-cell conversion; no `f64` product joins the decision, matching the crop/buffer
conventions (`replace-gtk-with-wayland` D5) so a reported size can never disagree with
a drawn one by rounding-path alone. The pixel-area reports need no rounding of their
own: they are integer multiples of the reported cell (D1).

## Risks / Trade-offs

- [Hosts that assume scale-invariant cell pixels] A host hardcoding "cell px are
  logical" sees different numbers at fractional scales. → Mitigation: this is the
  convergence every real terminal already has; mbv and ratatui_image consumers read the
  report, so they gain resolution automatically. Documented as the proposal's breaking
  change.
- [Mid-run scale change and cached pickers] A host that queried `CSI 16 t` once and
  caches the size keeps its stale value until it re-queries. → Mitigation: the scale
  change raises `SIGWINCH`, the standard signal to re-read the size; the same contract
  as a window resize on any terminal.
- [Fractional-scale rounding visible in the reports] A 16.2-px average cell reported as
  16 understates the pitch by 1.2%, and a bitmap sized to the reported cell can come out
  a few device px narrower than the drawn grid. → Mitigation: accepted (D1); hosts fit
  and centre their placements, and the alternative — a "drawn width" report the vt
derives away — does not exist.
- [Double SIGWINCH on scale change plus configure] A scale change that also changes the
  height pushes twice. → Mitigation: the winsize ioctl is cheap and idempotent; the
  spec's signal rule already covers repeated updates.

## Migration Plan

Library-internal change; hosts update by re-querying as they already do on SIGWINCH.
No data migration. Rollback is reverting the commits; the spec deltas revert with the
archive.
