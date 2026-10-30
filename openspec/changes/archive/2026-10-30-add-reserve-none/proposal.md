# Proposal

## Why

Once a panel has pushed, no request releases its held strip while the panel runs. `cover`
keeps the strip on purpose, so a panel can expand over tiles and collapse back without moving
any tiled window. A consumer that turns off space reservation on a running panel therefore
keeps the reserved strip until the panel restarts (issue #59).

## What Changes

- Add a third reserve mode, `none`, next to `push` and `cover`. A `none` panel draws over
  tiled windows and holds a strip of zero. Every apply that lands in `none` zeroes the held
  strip, as a pushing apply resets it to its own strip.
- Accept `none` wherever a reserve mode is accepted: `run --reserve`, the `reserve` setting of
  a panel table, `set-reserve`, the `reserve` field of `set-size` and `resize`, and the tray
  reserve choice. `status` and events report `none`.
- A change that lands in `none` with an animation holds the strip during the animation and
  drops it to zero when the animation ends, so tiled windows reflow once.
- `cover` keeps its hold semantics. This is not a change to `cover`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: the reserve requirement gains the `none` mode; the animation requirements
  say where the strip lands when the target is `none`.
- `panel-control`: `--reserve`, the `reserve` request field and `set-reserve` accept `none`.
- `panel-tray`: the reserve choice in each panel submenu gains `none`.

## Impact

- Code: `src/geometry.rs` (`ReserveMode`, `Geometry::validate`, `HeldGap`),
  `src/config.rs` (mode parsing), `src/tray.rs` (reserve choices), `src/proxy/tween.rs`
  (end gap of an animated change), `src/cli.rs` (usage text).
- Docs: `README.md` option tables and `AGENTS.md` where it describes the held strip.
- No new dependency and no change to the wire format beyond one more accepted value.
