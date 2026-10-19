# Proposal

## Why

A panel that pushes its strip moves a bar that another layer reserves. On umbriel, the
noctalia bar sits on the `top` layer and reserves 30 pixels there; a left panel with a 200
pixel strip on the default `overlay` layer moved that bar from x=500 to x=700 and narrowed it
by 200 pixels. The strip exists to keep tiled windows beside the panel; it must not take
space away from a surface that reserves its own edge on another layer.

## What Changes

- The reserve surface, which carries the panel's exclusive zone, is always created on the
  `background` layer, whatever layer the panel window draws on. A compositor that applies
  exclusive zones layer by layer (overlay first, background last) then gives every other
  layer's reservation priority: the noctalia bar keeps its place, and tiled windows start
  beside the strip and below the bar.
- `set-layer` still changes the panel window's layer in place; it no longer moves the strip
  into that layer. The strip is invisible, so its layer never shows.
- The panel window keeps `exclusive_zone` -1 and the output's full area, as it does today;
  only the strip's layer, and with it the order in which the zone applies, changes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: "Reserve space so tiles start beside the panel" gains the rule that the
  strip's surface never outranks another layer's reservation, with scenarios for a reserved
  top bar and for `set-layer`.

## Impact

- Code: `src/proxy/reserve.rs` (the strip's fixed layer), `src/proxy/dock.rs` (creation,
  re-dock and `set-layer` no longer take a layer for the strip), and the `start_reserve` call
  site in `src/proxy/routing/xdg_surface.rs`.
- Docs: `AGENTS.md` entries for `src/proxy/reserve.rs` and `src/proxy/dock.rs`, and the
  README behaviour bullets.
- No wire format, command-line, configuration or daemon behaviour changes.
