# Proposal

## Why

At a fractional output scale (the user's laptop runs 1.8), kitty images shown by a host
such as mbv render visibly pixelated in the panel while the same host in ghostty renders
them sharp. The cause: pinwin reports the terminal cell size in **logical** pixels (pty
winsize `ws_xpixel`/`ws_ypixel` and the `CSI 14/16/18 t` size reports), while real
terminals — ghostty measured directly: ~18 px/cell at scale 1.8 versus pinwin's constant
9 px — report **device** pixels. Hosts size their transmitted bitmaps from that report,
so in the panel every image arrives at ~56% of device resolution and the image pass must
upscale it; the resolution is lost at transmit time and no renderer setting recovers it.
Text is unaffected because the panel rasterises glyphs itself at device scale.

## What Changes

- The pty window size's pixel fields (`ws_xpixel`/`ws_ypixel`) and the `CSI 14 t` /
  `CSI 16 t` size reports carry **device pixels**: the reported cell is the logical cell
  size times the panel's resolved output scale, and the text-area pixels and winsize
  fields are the column/row counts times that reported cell — recomputed when the scale
  changes (with the usual SIGWINCH). Column/row counts are unchanged.
- The `CSI 16 t` cell size a host reads therefore grows with the output scale, matching
  ghostty and kitty; the spec scenarios that mandated scale-independent replies
  ("Sizes stay logical", "Text sizes stay logical") are replaced.
- Everything internal stays logical: the layout, the drawn panel width, the grid
  derivation, ghostty-vt's own cell size (which drives kitty placement geometry), and
  the mouse encoder's units (Wayland pointer positions are surface-local logical).

**BREAKING**: hosts that read cell pixels and assume they are logical-scale-invariant
see different numbers at fractional scales. This is the intended convergence with
every real terminal; hosts that size images from the report (mbv) gain device
resolution automatically.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: the "Correct size reports" requirement changes from logical-pixel
  reporting to device-pixel reporting (winsize pixel fields and `CSI 14/16 t`), with a
  scale-change update obligation; the "Seamless cell grid at any output scale" and
  "Cell text on the device pixel lattice" requirements have their "stay logical"
  scenarios reworded, which contradicted the "match the cell size actually drawn" rule at
  fractional scales.

## Impact

- `src/panel/wayland_side/glue.rs` / `sizing.rs` / `state.rs` (`apply_pty_size`): the
  winsize push gains the resolved scale and pushes device pixel fields; the scale-note
  handlers (`note_integer_scale`, `note_preferred_scale`) re-push the winsize when the
  device pixel size changed.
- `src/term/callbacks.rs` (`size_report`) plus the shared callback context: the report
  answers device pixels from a panel-thread-supplied scale note.
- `src/term.rs` (`push_size`, mouse encoder) and ghostty-vt's cell size: unchanged —
  logical, for placement geometry and pointer-cell mapping.
- Tests: display-free units for the device conversion, the report arithmetic and the
  scale-change re-push; a live niri check at a fractional scale against ghostty.

## Experiment result

Filled by task 1.1 before the implementation tasks run: whether the device-pixel cell
report makes mbv's panel images sharp at scale 1.8 (the picker's font size from the
`startup.image_picker.initialized` log line, the transmitted bitmap size, and a
side-by-side against ghostty). If the images are not sharp, the premise is wrong and the
rest of the change stops for re-planning.

- [ ] recorded
