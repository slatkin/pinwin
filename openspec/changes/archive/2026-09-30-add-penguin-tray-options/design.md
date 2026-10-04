# Design

## Context

See proposal.md for motivation and `specs/penguin-tray-options/spec.md` for behavior.

Observed integration points:
- `penguin/src/glue.c` owns GTK, Pango, PTY lifecycle and both layer-shell surfaces. It is already about 1,470 lines; keep new tray/options responsibilities outside the renderer rather than refactoring unrelated rendering code.
- `on_activate` creates an overlay terminal anchored LEFT/TOP/BOTTOM with exclusive zone -1, plus a transparent bottom-layer full-height reservation anchored to the same edges. The reservation is currently a local variable; live updates need a retained handle.
- `on_area_resize` calls `apply_size`; that updates the existing ghostty terminal through `penguin_size` and the existing PTY through `glue_pty_resize`. `spawn_pty` is protected by `g_spawned`. Reuse this path rather than recreating anything.
- `main.zig` validates COLS/GUTTER/keyboard before GTK and passes values through a GTK-free C header. Keep these launch checks and this boundary.
- The durable spec inventory is empty. The previous unarchived change's left-only/no-file-write restrictions conflict with the new requirements; this proposal explicitly supersedes them, but does not rewrite old artifacts.
- The machine has GTK4, gtk4-layer-shell 1.3.0, GIO, SVG support via librsvg, and `dbusmenu-glib-0.4` 16.04.0. Indicator libraries installed here depend on GTK3 and are inappropriate in this GTK4 process. A live `org.kde.StatusNotifierWatcher` reports a registered tray host and supports `RegisterStatusNotifierItem`.

## Goals / Non-Goals

**Goals:**
- Keep all changes on the existing GTK main loop; retain one command, PTY and ghostty terminal per instance.
- Host-rendered tray menu over session D-Bus, not an X11 tray or a subprocess.
- A single applied layout and a window-local draft, with failure-before-mutation.

**Non-Goals:**
- Renderer cleanup, a general settings framework, new daemon, monitor following, width/font/keyboard controls, autostart, hide/show or quit menu actions.
- Hot-reloading external config edits or synchronizing layouts among running instances.
- Modifying the user's SVG, desktop entries or niri config.

## Decisions

### D1. Separate new responsibilities, retain existing runtime ownership

Add `src/options.c` / `src/options.h` for layout values, validation, config I/O and the options editor, and `src/tray.c` / `src/tray.h` for tray publication and its menu. Layout values stay next to their validation and editing, not in a generic utility file. `glue.c` retains both window handles, original output and the actual surface update operation; options passes a validated layout through a narrow C integration boundary. Tray only needs an open-options callback and application lifetime integration. Keep GTK types out of `penguin.h`; main.zig can retain the existing glue_init inputs.

Alternative: append everything to glue.c. Rejected because the independent tray protocol and layout editor would substantially enlarge a renderer/PTY file. No extraction of existing drawing code is needed.

### D2. StatusNotifierItem through GIO; reuse libdbusmenu for the menu

Explicitly link `gio-2.0` and `dbusmenu-glib-0.4` in build.zig and compile the new C sources. Export `/StatusNotifierItem` using the conventional `org.kde.StatusNotifierItem` interface, with Category ApplicationStatus, stable Id/Title penguin, Status Active, WindowId zero, ItemIsMenu true and Menu pointing to the exported dbusmenu object. Give each process its own bus identity so parallel instances do not collide. Provide the standard icon/tooltip properties and methods expected by hosts. Register with `org.kde.StatusNotifierWatcher`; watch its owner and re-register on return. Bus/registration errors are nonfatal diagnostics, not command failures. Unregister and release menu and bus objects on shutdown.

Use `DbusmenuServer` and a single `Options...` child item; its activation invokes the options window. Hosts render the menu at the tray pointer, avoiding Wayland global-position restrictions. Support ContextMenu/Activate as the host's request to present this exported menu, using the server's activation request mechanism where needed; do not bypass the agreed menu by opening options directly on right-click. Verify this interaction against the actual host as well as D-Bus introspection.

Alternatives: GTK3 libappindicator is rejected because mixing GTK major versions is not supported. Hand-writing the entire dbusmenu protocol is rejected because its GTK-independent system library is already installed. There is no requirement to add a general tray abstraction.

References: [StatusNotifierItem properties and methods](https://www.freedesktop.org/wiki/Specifications/StatusNotifierItem/StatusNotifierItem/); locally inspected `libdbusmenu-glib/server.h` documents root-tree export and activation-request signals. The live host uses the KDE interface names, so use those on the wire rather than blindly copying the freedesktop naming from prose.

### D3. SVG rasterized into tray pixmaps

Load `$HOME/penguin.svg` with the existing GdkPixbuf stack, rasterizing at standard 16/22/32/48 sizes while preserving aspect ratio and transparent padding. Supply IconPixmap as `a(iiay)` with network-order ARGB bytes (A,R,G,B), not native cairo premultiplied ARGB. Leave IconName empty for the custom image so hosts prefer the supplied pixmaps. No icon installation, temporary PNG or hard-coded personal home path. Missing SVG/loader uses a generic theme icon with a diagnostic; if that cannot be resolved, provide a simple in-memory fallback pixmap.

Alternative: setting IconName to an absolute SVG path is host-dependent, while pixmaps are part of the protocol. Reference: [StatusNotifier icon serialization](https://www.freedesktop.org/wiki/Specifications/StatusNotifierItem/Icons/).

### D4. Persist only the five layout values

Use GLib GKeyFile INI syntax in the per-user config directory:

```ini
[layout]
side=left
top=0
bottom=0
left=0
right=0
```

The file is `g_get_user_config_dir()/penguin/config`. Missing files use the launch baseline: side left, top/bottom/left zero, right equal to the already-validated GUTTER. A valid saved layout overrides this baseline as a whole; COLS, font, keyboard and command remain launch-controlled. Reject malformed/incomplete recognized layout fields as a whole with a diagnostic and use the baseline; ignore unknown keys for forward compatibility. Geometry validity is checked when the original monitor and font metrics are known. An unusable saved layout falls back without repairing the file. No startup write or migration is performed.

Apply serializes all five fields, creates the directory when needed, and atomically replaces the file using GLib's consistent file-write facility (`g_file_set_contents_full` with consistency semantics and mode 0600). Do not truncate the destination before replacement. Check all errors. Successful replacement precedes applying live state so save failure leaves both previous states intact. No file watcher or locking: multiple processes have independent live layouts, last successful save wins.

Alternative: new env variables for each gutter would not satisfy persistent tray settings; a custom parser buys nothing over GKeyFile. Precedence and last-writer-wins are planning defaults, not new user-selected controls.

### D5. Explicit geometry for both surfaces

Use GTK/layer-shell logical pixels, consistent with existing GUTTER and font metrics. Set the visible panel's horizontal anchor to the chosen edge only, retain TOP/BOTTOM anchors, and use the corresponding outer horizontal margin plus both vertical margins. Width stays COLS * cell_w. Margins may be negative (layer-shell margins are signed), which moves the panel edge past the output edge; the reservation sum left + panel_width + right is validated to stay non-negative. Keep exclusive zone -1 so top/bottom insets are measured against the output itself, not against another bar's work area.

Keep the reservation transparent, on BOTTOM, full-height and with zero margins. Its selected horizontal edge plus TOP/BOTTOM anchors yield an unambiguous side reservation. Set its exclusive zone explicitly to left + panel_width + right, independent of visible panel margins. Disable the obsolete horizontal anchor when changing sides; never leave both side anchors set. Retain and update both window handles in the same main-loop turn.

```text
Left:  edge | left | panel | right | tiles
Right: tiles | left | panel | right | edge
```

Capture the visible surface's resolved original GdkMonitor once mapped and pin the reservation to that monitor; retain that monitor for later layout validation and side changes. Do not reselect the focused output from an options-window activation. Set the reservation's initial geometry only once the original output is resolved. For returning from options, keep the configured keyboard policy; no exclusive keyboard grab is introduced.

Reference: [gtk4-layer-shell setters](https://wmww.github.io/gtk4-layer-shell/gtk4-layer-shell-GTK4-Layer-Shell.html). Use existing anchor/margin/exclusive-zone/monitor setters; do not rely on the 1.4-only exclusive-edge API on installed 1.3.0. Automatic exclusive zones would account for margins differently, so keep a manual zero-margin reservation.

### D6. Apply is validate, save, then publish

The window owns a draft initialized from the instance's applied layout. Numeric fields use GTK spin controls with integer semantics and explicit commit/validation of typed text; do not silently clamp negative, fractional or malformed input on Apply. Labels associate with controls, tab navigation works, and errors are visible inline. Keep one normal GTK application options window (not another layer-shell terminal); Close/window close destroys its draft. Present an already-open window without resetting it.

Apply performs checked arithmetic against signed layer-shell integer limits and original-output geometry: nonnegative gutters, valid side, reservation < output width, and output height - top - bottom >= cell_h. Validate panel width multiplication too. Only after validation and atomic save succeed, replace applied values and request surface updates. Allow the compositor's subsequent resize callback to update rows, TIOCSWINSZ, ghostty and input encoder sizes through the existing path. Force redraw after layout/size changes. Apply remains open and resets the draft's baseline to applied values; failed Apply preserves the draft.

Do not promise an atomic compositor transaction or rollback after display disconnection: the logical operation is on one GTK main-loop turn, and persistence is committed first. A compositor disconnect already terminates the window session; the saved requested layout remains available for next launch. Normal live changes must not unmap/restart the command or initialize ghostty again.

Alternative: applying before saving requires reverting surface state when disk writes fail. Save-first avoids that extra failure path.

## Risks / Trade-offs

- [Tray-host compatibility differs] --> Export a real dbusmenu tree and test right-click/Options on the current host, including host restart. D-Bus introspection alone is not acceptance.
- [Transparent reservation accidentally targets a different output] --> Resolve the visible monitor before presenting the reservation and use that same monitor for every update.
- [Fractional scaling or resize changes rows differently than expected] --> Validate/render in the existing logical coordinate system; accept configured widget size as authoritative and test with the real client and size queries.
- [Output unplug/geometry changes invalidate saved values] --> Validate at startup and each Apply. Dynamic monitor relocation after unplug remains compositor behavior, not a new monitor-following feature.
- [Config persistence supersedes an old no-write guarantee] --> Write only the penguin config on successful Apply; never touch niri. Call out the unarchived spec conflict for reconciliation at promotion.
- [Existing launch defaults themselves do not fit a tiny output] --> Preserve existing launch behavior; interactive Apply must satisfy the stricter validation. Do not silently rewrite COLS or saved gutters.
- [New library availability] --> Document dbusmenu-glib as a build/runtime dependency and fail clearly at build time; no GTK3 linkage.

## Migration Plan

Build and install penguin normally. No file is created until the first successful Apply, so existing launch behavior is retained without a saved config. To reset layout, remove only the penguin config and restart. To roll back the binary, reinstall the earlier build; the config can remain because it is not read by that build. Leave the icon at `$HOME/penguin.svg`; it is not copied or changed.

At later spec promotion, reconcile the old penguin-panel left-only/full-height/environment-only/no-write requirements with this new layout capability rather than publishing contradictory contracts. That promotion is not an implementation task or a write authorized by this planning workflow.
