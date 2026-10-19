# Design

## Context

See `proposal.md` for the reason and the specs delta for the behaviour.

Current code this design changes:

- `src/proxy/reserve.rs`: `Reserve::create` takes the panel's layer and passes it to
  `zwlr_layer_shell_v1.get_layer_surface`; `Reserve::set_layer` sends `set_layer` on the
  strip. The strip is a transparent 1x1-buffer surface that carries the exclusive zone.
- `src/proxy/dock.rs`: `start_reserve`, `recreate_reserve` and `dock_reserve` take the layer;
  `set_layer` sends the new layer to the panel surface through the commit coordinator and
  then calls `Reserve::set_layer` with the same value.
- `src/proxy/routing/xdg_surface.rs`: `take_slot` passes `layer_of(self.routing.layer())` to
  `start_reserve`.
- The panel window's own surface stays `exclusive_zone` -1; a compositor gives a -1 surface
  the output's full area, so the panel's geometry never depends on the reservation order.

## Measured behaviour

umbriel's `Output::arrangeLayers` configures exclusive surfaces layer by layer, overlay first
and background last, and `wlr_scene_layer_surface_v1_configure` shrinks the usable area as it
configures each one. The first layer processed therefore keeps the area it asked for, and
later ones are placed in the area the earlier zones leave. A strip on `overlay` claims first
and a `top` bar is placed in what remains; a strip on `background` claims last, so the bar
keeps the full output width and the strip is placed below it.

The panel defaults to `overlay` and the noctalia bar is on `top`. With a 200 pixel test panel
the live layer list showed `noctalia-bar-Main` moving from x=500, w=2440 to x=700, w=2240,
then back after the panel ended. Sway's `arrange_layers` uses the same exclusive-pass order
(overlay, top, bottom, background). Niri delegates to smithay's `LayerMap::arrange`, which
walks its map in insertion order; a bar mapped at session start already precedes a strip
mapped later, so it keeps its area there too.

## D1: the strip's surface is always on the background layer

The strip is created with `ZwlrLayerShellV1Layer::BACKGROUND` and nothing moves it afterwards.
The panel window keeps the layer its settings name. The strip is invisible, so its own layer
has no visual consequence; only the precedence of its exclusive zone changes, and it becomes
the last zone applied on a compositor with layer-ordered arrangement.

Consequences:

- A bar or dock that reserves its own edge keeps its position and width; the strip takes what
  is left, and tiled windows start beside the strip and below the bar.
- `set-layer` changes the panel window's layer in place and leaves the strip where it is. The
  strip is on the background layer in every panel, so the request has nothing to send it.
- The strip no longer needs the panel's layer, so the layer parameter leaves
  `Reserve::create`, `start_reserve`, `recreate_reserve` and `dock_reserve`, and
  `Reserve::set_layer` is deleted.

## Alternatives rejected for D1

- **Default the panel to `top` instead of `overlay`.** This only works while the bar maps
  before the strip: a bar that reloads or re-creates its surface is inserted after the strip
  in the same layer tree and moves again, and an explicit `--layer overlay` brings the bug
  back.
- **Set the strip's exclusive edge explicitly.** The edge selects which output edge shrinks,
  not when the zone applies; the shift comes from the order alone.
- **Keep the zone but reserve with margins and zone 0.** A zero zone moves the strip itself
  out of other reservations and reserves nothing for the tiled windows.
- **Look for another client's reservation before pushing.** Layer-shell exposes no query for
  another surface's zone, and the reservation must not depend on map order.
