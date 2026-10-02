# pinwin

`pinwin` was formerly called penguin.

A fixed terminal sidebar for [niri](https://github.com/niri-wm/niri): a panel pinned to one
edge of a monitor, with tiled windows laid out beside it. The repository holds two independent
ways to get one:

- **`pinwin.sh`** — the original Bash launcher. It moves a floating Ghostty window running
  [`mbv`](https://github.com/slatkin/mbv) (or any command) to the edge and rewrites niri's
  global struts. Stock niri, stock Ghostty and `jq` are enough.
- **`libpinwin`** — a Zig static library with a small C ABI. Its terminal is
  libghostty-vt + Pango rendered in-process and shown as a Wayland layer-shell panel, so it
  needs no auxiliary window and writes nothing into niri's configuration. A host program
  supplies a pty master fd and a full layout at every start; the library opens, moves and
  closes the panel and never exits the host process.

Both are additive and independent. `pinwin.sh` is unchanged and keeps working exactly as
before.

```text
|<- panel ->|gutter|<-------- scrolling tiles -------->|
|   mbv     |      | [col] [col] [col] ...             |
```

## The launcher: `pinwin.sh`

Pins a Ghostty window running `mbv` (or any command) to the left edge of niri as a fixed
sidebar. Tiled windows are laid out to its right. The pin moves with you across workspaces,
and when the command exits the sidebar goes away and the tiles reclaim the space.

Stock niri and stock Ghostty from the package manager are enough. pinwin.sh needs no patches
or plugins.

### Usage

```sh
pinwin.sh            # runs mbv
pinwin.sh htop       # any command
COLS=60 pinwin.sh    # width in terminal columns (default 40)
GUTTER=8 pinwin.sh   # px between the pin and the first tile (default 12)
```

**Focus:** `switch-focus-between-floating-and-tiling` moves focus between the pin and your
tiles:

```kdl
binds {
    Mod+Shift+V { switch-focus-between-floating-and-tiling; }
}
```

### Install

Requirements:
- niri
- Ghostty
- `jq`

Steps:

1. Put the script on your `PATH`:

   ```sh
   ln -s ~/Dev/pinwin/pinwin.sh ~/.local/bin/pinwin.sh
   ```

2. Add this line at the end of `~/.config/niri/config.kdl`. It has to come after the file
   that defines your `layout`:

   ```kdl
   include optional=true "woims/pin.kdl"
   ```

   pinwin.sh writes that file only while it's running, and empties it on exit.

3. Optional: to have app launchers start mbv pinned, override the package's desktop entry in
   `~/.local/share/applications/mbv.desktop`:

   ```ini
   Exec=/home/you/.local/bin/pinwin.sh
   Terminal=false
   ```

### How it works

On start, pinwin.sh:

1. Writes a window rule into `pin.kdl`. The rule matches app-id `dev.pinwin` and opens the
   window floating at the left edge, full height. pinwin.sh then runs
   `niri msg action load-config-file`.
2. Starts `ghostty --gtk-single-instance=false --class=dev.pinwin --window-width=$COLS`. The
   `--gtk-single-instance=false` makes the command run in its own process instead of being
   handed to an already-running Ghostty.
3. Waits until niri reports the same window size twice in a row. It then shrinks the height
   to respect the top and bottom struts, and writes `left = width + GUTTER` into the struts.
4. Locks the height with `min-height`/`max-height` in the rule. Width stays resizable.
5. Watches `niri msg --json event-stream` in the background:
   - When a workspace on the pin's output is activated, it moves the pin there without taking
     focus.
   - About 0.3 s after the pin stops changing, it moves the pin back to the left edge, which
     undoes an accidental drag. If the width changed, it rewrites the strut to match.

On exit, a trap empties `pin.kdl` and reloads niri's config.

### Caveats

- **Strut values:** `LEFT/RIGHT/TOP/BOTTOM` at the top of the script must match the `struts`
  block in your niri layout. niri replaces the whole `struts` block rather than merging it.
- **Scope:** struts are global, so the left gap also appears on other outputs.
- **Crashes:** a `SIGKILL` skips the cleanup trap. Clear the gap with
  `: > ~/.config/niri/woims/pin.kdl`.
- **App-id:** window rules match app-ids as unanchored regexes. Make sure none of your
  existing rules catches `dev.pinwin`. For example, a rule for `mbv`'s mpv window must not
  match it.
- **Focus key:** the focus key toggles between tiles and the most recently focused floating
  window. With several floating windows open, that may not be the pin.

## libpinwin

A static library a host program links and drives from its own thread. The host creates a pty,
forks its child on the slave side and owns that child; the library attaches to the master fd
and renders the child's terminal as a layer-shell panel on the `overlay` layer, docked to the
left or right edge of the focused monitor, spanning its full height. An invisible reservation
surface reserves the panel width plus its gutters, so the compositor tiles windows beside it
on that monitor only, and releases the space when the panel closes — including after a
`SIGKILL`. The panel stays on its original monitor and is visible on every workspace there.

The terminal supports alternate screen; 24-bit and 256-color text with bold, italic and
inverse; the kitty keyboard protocol including disambiguate-escape-codes with press/release
and Shift/Ctrl/Alt/Super; SGR mouse reporting in cells; focus in/out reports; and the kitty
graphics protocol. It follows the Ghostty config for font and theme (first `font-family` and
`font-size`, fallback `monospace 11`), with no environment overrides. The library has no
environment configuration, no config file and no control socket; the `pinwin` program below
adds a command line and a few environment settings on top of it.

### The `pinwin` program

`zig build` also installs `zig-out/bin/pinwin`, a thin host over the C ABI: it forks the
command on a pty, docks the panel to the left edge and exits with the command's status when it
ends.

```sh
./zig-out/bin/pinwin [--] [command...]   # default: $SHELL (/bin/sh if unset)
./zig-out/bin/pinwin mbv
COLS=60 GUTTER=8 ./zig-out/bin/pinwin htop
PINWIN_KEYBOARD=exclusive ./zig-out/bin/pinwin   # or on-demand (default), none
```

`COLS` is the width in columns (default 40) and `GUTTER` the extra gap in px (default 0); a bad
value exits 2 before anything opens. The former tray, options window, control socket and
layout config file are not part of this program.

### Build

Requirements: Zig 0.16 (`zig version` must print `0.16.`), GTK 4, gtk4-layer-shell, Pango and
a compositor implementing wlr-layer-shell. **The panel is Wayland-only:** without
wlr-layer-shell (an X11 session, or a Wayland compositor without it such as GNOME),
`pinwin_start` returns `PINWIN_ERR_NO_DISPLAY` and opens nothing; there is no fallback to an
ordinary window.

From the `pinwin` directory:

```sh
zig build        # libraries into zig-out/lib/, the pinwin program into zig-out/bin/
```

`zig build` fetches and statically links libghostty-vt from a pinned commit of
[ghostty](https://github.com/ghostty-org/ghostty) on the first build, and installs four
archives:

- `zig-out/lib/libpinwin.a`
- `zig-out/lib/libghostty-vt.a`
- `zig-out/lib/libsimdutf.a`
- `zig-out/lib/libhighway.a`

Zig static-library artifacts do not merge linked static archives, so `libpinwin.a` does
**not** bundle libghostty-vt, and ghostty's vendored SIMD libraries (simdutf, highway) are
separate archives as well. Consumers link **all four**.

### Link a consumer

`pinwin/src/pinwin_api.h` is the whole public surface; it is plain C with no GTK types, so a
consumer includes it without the GTK headers. Add `pinwin/src` to the include path and link
both archives plus GTK4, gtk4-layer-shell and pango/cairo:

```sh
cc consumer.c \
    -I path/to/pinwin/src \
    zig-out/lib/libpinwin.a zig-out/lib/libghostty-vt.a \
    zig-out/lib/libsimdutf.a zig-out/lib/libhighway.a \
    $(pkg-config --cflags --libs gtk4 gtk4-layer-shell-0 pangocairo) \
    -pthread
```

### ABI

```c
#include "pinwin_api.h"

typedef struct {
    int32_t master_fd;    /* host-owned pty master, set non-blocking by the library */
    PinwinLayout layout;  /* side, cols, and the four gutters */
    int32_t keyboard_mode; /* one of PINWIN_KEYBOARD_* */
} PinwinStartup;

typedef struct {
    int32_t side;                          /* PINWIN_SIDE_LEFT or PINWIN_SIDE_RIGHT */
    int32_t cols;                          /* panel width in terminal columns, 1..=65535 */
    int32_t top, bottom, left, right;      /* gutters in pixels, may be negative */
} PinwinLayout;

int  pinwin_start(const PinwinStartup* startup);
int  pinwin_apply_layout(const PinwinLayout* layout);
void pinwin_stop(void);
```

`PINWIN_KEYBOARD_NONE`, `PINWIN_KEYBOARD_EXCLUSIVE` and `PINWIN_KEYBOARD_ON_DEMAND` select
whether the panel never takes the keyboard, takes it on open, or takes it only after a click.
The mode is fixed at `pinwin_start` time; there is no runtime override.

Result codes, all negative-free so `if (pinwin_start(...))` reads as failure:

| Code | Meaning |
| --- | --- |
| `PINWIN_OK` (0) | success |
| `PINWIN_ERR_INVALID` (1) | bad arguments or a layout the panel refuses; the applied layout is unchanged |
| `PINWIN_ERR_ALREADY_RUNNING` (2) | a panel is already running |
| `PINWIN_ERR_NOT_RUNNING` (3) | no panel to act on |
| `PINWIN_ERR_NO_DISPLAY` (4) | no GTK display / no wlr-layer-shell; nothing opens |
| `PINWIN_ERR_INTERNAL` (5) | unexpected failure, such as a terminal grid that could not be allocated (the previous grid stays) |

`pinwin_start` validates the arguments purely, stores the startup, spawns the GTK thread that
owns the `GtkApplication` main loop, and blocks until the panel is live (GTK/layer-shell init
plus the first draw that resolves the monitor) or that thread reports a startup failure.
Returning only once the panel is live is what lets a valid layout applied immediately after
`pinwin_start` succeed. A second start while running returns `PINWIN_ERR_ALREADY_RUNNING`; a
prior successful `pinwin_stop` leaves the library restartable.

`pinwin_apply_layout` applies a full layout — docking side, column count and all four gutters
— as one operation, resizing the existing terminal grid and PTY winsize without recreating
the terminal or touching the host's child. It validates first (unknown side, columns outside
1..=65535, checked-arithmetic overflow, a reservation sum below zero, less than one complete
terminal row, or a reservation that leaves no output width) and returns a synchronous result;
`PINWIN_ERR_INVALID` leaves the previous applied layout untouched. Structural rejection needs
no GTK surface. Negative gutters are allowed: they push the panel's edge past the output edge
or end the reservation before the panel's far edge, so tiles may overlap the panel.

`pinwin_stop` closes the visible and reservation surfaces, quits the GTK main loop and joins
the GTK thread. It is a no-op when not running.

**Threading contract.** Call the ABI from the host's thread, not from the GTK thread.
`pinwin_apply_layout` posts validation-and-publish to the GTK thread and waits for it, so
calling it from that thread would deadlock; `pinwin_stop` joins the GTK thread and would join
the calling thread. The header states both.

**No-exit contract.** No library call terminates the host process for any reason — bad
arguments, bad layout, missing display, missing Ghostty config, terminal allocation failure
or GTK errors are result codes or degraded states. The library writes no diagnostics.

### Layout ownership

The host supplies the full layout with every start and every apply. The library reads no
pinwin-specific environment variable, has no config file and persists nothing; the host owns
persistence and passes the values back explicitly. Ghostty font/theme following is appearance
only and never influences layout.

### Pinned dependency

`pinwin/build.zig.zon` pins ghostty at commit
`3a3047f6b62a791fd8b12d9f07a85b3d2160370b` (`ghostty-1.3.2-dev`), and the minimum supported
Zig is 0.16.0 (`.minimum_zig_version`, and the `0.16.` compiler check above). Consumers
pinning by revision should use the annotated **`library-abi`** tag — the first tag of the
library form — rather than a bare commit; it names the tree whose README you are reading.

## Development

From the `pinwin` directory:

```sh
zig build          # the library archives only
zig build demo     # dev-only demo binary, never installed
zig build check    # layout-core and ABI-contract Zig tests
```

`zig build demo` builds a small dev-only driver of the C ABI (into `zig-out/bin/`) that
creates its own pty pair and canned child, then applies a live layout change and an invalid
one so the panel's manual checks are self-contained. It is never installed.

`zig build check` runs the Zig tests: strict cols/gutter parsing, checked side geometry, every
layout validation code, and the ABI contract (`pinwin_apply_layout` with an invalid layout
returns `PINWIN_ERR_INVALID`, distinct from `PINWIN_ERR_NOT_RUNNING`, with no GTK thread
started). The `pinwin` directory also ships `compile_flags.txt` for editor/`clangd` use.

## Differences between the two forms

| | `libpinwin` | `pinwin.sh` |
| --- | --- | --- |
| space reserved by | wlr-layer-shell exclusive zone | rewritten niri struts |
| scope | the panel's monitor | every output |
| niri config | none | `include "woims/pin.kdl"` + `LEFT/RIGHT/TOP/BOTTOM` kept in sync |
| terminal | libghostty-vt + Pango in-process, font from the Ghostty config | Ghostty |
| survives `SIGKILL` | yes, the compositor frees the space | no, the strut stays |
| layout settings | supplied by the host over the ABI | environment only |
| needs Zig to build | yes | no |
