# Proposal

## Why

pinwin uses GTK as a wrapper around one drawing area on a layer surface. A layer surface is
the Wayland surface kind that panels and bars use. GTK owns the parts that pinwin must
control: the surface lifecycle, the surface size, the pixel grid, the frame timing and the
main loop. Issue #19 shows the cost. The only way to remap a GTK window is hide and present.
That rebuilds the surface, the pty gets an interim size, and the hosted TUI flashes a
different layout. Earlier changes worked around the same ownership: `poc-gsk-texture-grid`
and `gsk-render-nodes` for the width animation, and `snap-grid-edges`, `snap-text-origins`
and `device-pixel-grid` for the pixel grid. GTK also loads 113 shared libraries, about
115 MB, into every process that runs a panel. pinwin uses no GTK widget, no input method and
no clipboard.

## What Changes

- Replace GTK4, gtk4-layer-shell, GDK, GSK, Pango, Cairo and gdk-pixbuf with a direct
  Wayland client. smithay-client-toolkit provides the protocols and calloop provides the
  event loop on the panel thread.
- Render on the CPU into shared-memory buffers sized in device pixels, with the
  fractional-scale and viewporter protocols. A device pixel is one physical pixel. The
  change adds no GPU renderer. The design names the measurement that justifies one.
- Draw text with swash and find fonts with fontconfig. One painter draws every frame, so
  the fallback painter goes away. Text origins sit on whole device pixels, which folds in
  the requirement of `snap-text-origins`.
- **BREAKING**: `Panel::request_focus` takes an activation token. An activation token is a
  one-use permission that the compositor gives a program to take focus. The panel asks for
  focus with xdg-activation and never unmaps. `pinwin --focus` forwards its
  `XDG_ACTIVATION_TOKEN` over the focus socket. Without a token, the client exits 2.
- A focus request never changes the terminal grid or the pty window size and raises no
  `SIGWINCH`. This closes #19.
- Stock niri ignores xdg-activation for layer surfaces, so on stock niri a focus request
  gives no focus. Click focus works on every compositor.
- In `on-demand` mode, the panel maps with no keyboard interactivity and switches to
  `on-demand` after its first frame. niri focuses a newly mapped `on-demand` surface, so
  this keeps the existing "no focus steal on launch" rule true on niri.
- The width animation runs on Wayland frame callbacks. Each frame crops one cached buffer
  through viewporter, so a frame draws nothing new.
- **BREAKING**: the library no longer reads the GTK `gtk-enable-animations` setting. A
  duration of 0 is the only way to apply a layout without animation.
- Key repeat and the pointer cursor become explicit requirements, because GTK did both
  without a requirement.
- Each start runs its own panel thread, and the thread ends after the handle drops. The
  process-lifetime parked GTK thread goes away.
- The library links no GTK, GLib, Pango, Cairo or gdk-pixbuf library, and CI checks it.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `pinwin-panel`: the focus request takes a token and never touches the grid. The command-line
  focus client forwards the token. The animation drops the GTK setting. The pixel-grid
  requirement drops the fallback painter and gains text origins on device pixels. Key repeat,
  the pointer cursor and the toolkit-free link become requirements. Requirements that name
  GTK in their text change their wording only.

## Impact

- Code: `src/surfaces.rs` and `src/surfaces/`, `src/panel.rs` and `src/panel/`,
  `src/render.rs` and `src/render/`, `src/input.rs`, `src/anim.rs`, `src/pty.rs` (its read
  source is a GLib source today), `src/ipc.rs`, `src/main.rs` and `examples/demo.rs`.
  `src/layout.rs`, `src/term/`, `src/surfaces/gap.rs`, `src/render/snap.rs` and
  `src/fontconfig.rs` stay mostly as they are.
- Public API: `Panel::request_focus` gains a token argument, and a new token type joins the
  public API.
- Dependencies: the GTK crates go. smithay-client-toolkit, calloop, wayland-protocols,
  tiny-skia, swash, a fontconfig binding and png come in. The system packages drop to
  libwayland, libxkbcommon and fontconfig.
- Build and docs: the package list in `.github/workflows/build.yml`, `README.md` and
  `AGENTS.md`.
- Other changes: this change supersedes `snap-text-origins`. If this change proceeds, do not
  apply `snap-text-origins`. `device-pixel-grid` stays parked, and this change does not need it.
  `adopt-libghostty-rs` changes `src/ghostty_sys/` and `src/term/`, which this change does
  not restructure, so either change can land first and the second one rebases.
