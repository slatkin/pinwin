# pinwin

Two ways to pin a terminal running [`mbv`](https://github.com/slatkin/mbv) (or any command) to the left edge of [niri](https://github.com/niri-wm/niri) as a fixed sidebar, with tiled windows laid out to its right:

- **`penguin`** — a small Zig program that *is* the terminal. It opens a layer-shell surface, reserves the space through the compositor, emulates the terminal itself, and needs no niri configuration. Recommended.
- **`pinwin`** — the original Bash script. It moves a floating Ghostty window and rewrites niri's global struts. Kept because it works with stock Ghostty, but the reserved space is global and a `SIGKILL` leaves it behind.

Both are additive and independent: `pinwin` is unchanged and keeps working exactly as before.

## penguin

```
|<- panel ->|gutter|<-------- scrolling tiles -------->|
|   mbv     |      | [col] [col] [col] ...             |
```

A layer-shell panel on the `overlay` layer, docked to the left edge of the focused monitor (the default) or the right edge, covering any bar behind it. An invisible `penguin-reserve` surface reserves the panel width plus its gutters, so the compositor tiles windows beside it on that monitor only. The docking side, width and gutters can be changed while it runs from a tray options window and are saved to a small config file; nothing else is written to disk and no niri configuration is involved. The compositor releases the space when the panel goes away, including after a `SIGKILL`.

```sh
make install                       # from repo root: build and install ~/.local/bin/penguin
cd penguin && zig build            # build without installing
./zig-out/bin/penguin              # runs mbv
./zig-out/bin/penguin htop         # any command, arguments passed through
COLS=60 GUTTER=8 ./zig-out/bin/penguin   # width in columns (default 40), extra gap in px (default 0)
```

To launch it from the mbv desktop entry, set `Exec=/home/you/.local/bin/penguin` in `~/.local/share/applications/mbv.desktop` (`Terminal=false`).

Requirements: Zig 0.16 (`zig version` must print `0.16.`), GTK 4, gtk4-layer-shell, Pango and a compositor implementing wlr-layer-shell. The tray options need `dbusmenu-glib-0.4` (build and runtime) and the tray icon needs a GdkPixbuf SVG loader (`librsvg`); both are only used for the tray, and a missing tray never stops the terminal. `zig build` fetches and statically links libghostty-vt from a pinned commit of [ghostty](https://github.com/ghostty-org/ghostty) the first time.

**Font:** the panel uses the font from your Ghostty config (`~/.config/ghostty/config`): the first `font-family` and the `font-size`, so the glyphs and cell size follow your terminal. `PENGUIN_FONT` and `PENGUIN_FONT_SIZE` override them, and `monospace 11` is the fallback when there is no config. Default foreground and background colours follow the Ghostty config/theme.

**Focus:** by default the panel takes the keyboard only after you click inside it, and gives it back when you click a compositor window. `PENGUIN_KEYBOARD=exclusive` makes it take the keyboard as soon as it opens; `PENGUIN_KEYBOARD=none` makes it never take it.

**Debugging:** `PENGUIN_DEBUG=1` writes libghostty-vt's own log plus every key, mouse, scroll and focus event to stderr.

**Checks:** the layout core (parsing, validation, side formulas, config round-trip, icon byte conversion) has a small assertion-based check. From the repository root:

```sh
cc -std=gnu11 -Wall -Wextra penguin/src/options.c penguin/src/tray.c \
  penguin/tools/check_options.c \
  $(pkg-config --cflags --libs gtk4 dbusmenu-glib-0.4) -o /tmp/penguin-check-options
/tmp/penguin-check-options
```

It prints `check_options: all passed` and exits 0. Config tests run in an isolated temporary directory (`XDG_CONFIG_HOME`); they never touch your real config.

**Gaps:** the visible gap between the panel and the first tile is the `Right` gutter (see below) plus whatever left strut niri itself adds (24 in the example layout, or 372 while `pinwin` is running). Other layer surfaces do not push the visible panel down; it covers a bar behind it on the same monitor. By default `GUTTER=0`, so the gap is the strut alone; set `GUTTER` for extra space.

**Tray and options:** every running penguin puts an entry in the system tray (StatusNotifierItem) named `penguin`, with its icon loaded from `$HOME/penguin.svg`. Right-click gives `Options...`, which opens that instance's options window. A tray host (a bar with StatusNotifier support) must be running; without one penguin keeps running without an entry and registers when a host appears. A missing or unreadable icon falls back to the terminal theme icon with a diagnostic on stderr.

**Options window:** a `Columns` field (the panel width in terminal columns, shown first), four gutter fields in pixels, a `Dock to: Left/Right` selector, `Apply` and `Close`.

```text
Left dock:   |L|panel|R| | tile | tile | ...        Right dock:  ... | tile | |L|panel|R|
             ^left    ^right gap before tiles                    ^gap after tiles ^right
```

Gutters are literal screen directions, independent of the docking side: `Left` insets the panel from the left output edge, `Right` insets it from the right edge, `Top`/`Bottom` inset it vertically. The panel's pixel width is exactly `Columns` times the font's cell width, so it is always a whole number of columns. The reserved strip is always `Left + panel width + Right` wide, always full height, on the panel's side. `Top`/`Bottom` do not shrink the reservation; they only shrink the visible terminal (rows follow automatically, like any terminal resize).

Gutters may be negative: the panel's edge moves past the screen (part of the panel off-screen), or the reservation ends before the panel's far side so tiles overlap the panel. The sum `Left + panel width + Right` may not go negative, and the panel must never cover the whole output.

Editing is staged: nothing changes until `Apply`, and `Apply` also saves. A failed Apply (bad value, or a layout that leaves no room for one terminal row or for other windows) shows an inline error and changes nothing. `Close` discards unapplied edits; reopening while the window is open brings back the same window with its pending edits. `Apply` resizes the running panel in place: the reserved strip, the terminal grid and the PTY follow the new column count without restarting the command. Font, keyboard and the command are never touched by an Apply.

**Layout settings:** the six values live in `$XDG_CONFIG_HOME/penguin/config` (`~/.config/penguin/config` by default):

```ini
[layout]
side=left
cols=40
top=0
bottom=0
left=0
right=0
```

The file is written atomically, mode 0600, only by a successful Apply. To reset the layout, delete the file. A valid saved layout overrides the launch-derived defaults on the next launch (left docking, zero top/bottom/left, `Right` = `GUTTER`, and the saved column count instead of `COLS`). Precedence mirrors the right gutter: saved `cols` > `COLS` environment variable > default 40, and the saved layout > the `GUTTER`-derived default. An unreadable, malformed or geometrically unusable one produces a diagnostic, falls back to the defaults and leaves the file untouched. Launching, opening and closing the options window never write it. The font, `PENGUIN_KEYBOARD` and the command stay launch-time only: the config file never carries them. This file is penguin's own; `pinwin` and niri's configuration are untouched by any of this.

**Differences from `pinwin`:**

| | `penguin` | `pinwin` |
| --- | --- | --- |
| space reserved by | wlr-layer-shell exclusive zone | rewritten niri struts |
| scope | the panel's monitor | every output |
| niri config | none | `include "woims/pin.kdl"` + `LEFT/RIGHT/TOP/BOTTOM` kept in sync |
| terminal | libghostty-vt + Pango in-process, font from the Ghostty config | Ghostty |
| survives `SIGKILL` | yes, the compositor frees the space | no, the strut stays |
| layout settings | tray options + own config file | environment only |
| needs Zig to build | yes | no |

## pinwin

Pins a Ghostty window running `mbv` (or any command) to the left edge of niri as a fixed sidebar. Tiled windows are laid out to its right. The pin moves with you across workspaces, and when the command exits the sidebar goes away and the tiles reclaim the space.

```
|<- pin ->|gutter|<-------- scrolling tiles -------->|
|  mbv    |      | [col] [col] [col] ...              |
```

Stock niri and stock Ghostty from the package manager are enough. pinwin needs no patches or plugins.

### Usage

```sh
pinwin            # runs mbv
pinwin htop       # any command
COLS=60 pinwin    # width in terminal columns (default 40)
GUTTER=8 pinwin   # px between the pin and the first tile (default 12)
```

**Focus:** `switch-focus-between-floating-and-tiling` moves focus between the pin and your tiles:

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
   ln -s ~/Dev/pinwin/pinwin ~/.local/bin/pinwin
   ```

2. Add this line at the end of `~/.config/niri/config.kdl`. It has to come after the file that defines your `layout`:

   ```kdl
   include optional=true "woims/pin.kdl"
   ```

   pinwin writes that file only while it's running, and empties it on exit.

3. Optional: to have app launchers start mbv pinned, override the package's desktop entry in `~/.local/share/applications/mbv.desktop`:

   ```ini
   Exec=/home/you/.local/bin/pinwin
   Terminal=false
   ```

### How it works

On start, pinwin:

1. Writes a window rule into `pin.kdl`. The rule matches app-id `dev.pinwin` and opens the window floating at the left edge, full height. pinwin then runs `niri msg action load-config-file`.
2. Starts `ghostty --gtk-single-instance=false --class=dev.pinwin --window-width=$COLS`. The `--gtk-single-instance=false` makes the command run in its own process instead of being handed to an already-running Ghostty.
3. Waits until niri reports the same window size twice in a row. It then shrinks the height to respect the top and bottom struts, and writes `left = width + GUTTER` into the struts.
4. Locks the height with `min-height`/`max-height` in the rule. Width stays resizable.
5. Watches `niri msg --json event-stream` in the background:
   - When a workspace on the pin's output is activated, it moves the pin there without taking focus.
   - About 0.3 s after the pin stops changing, it moves the pin back to the left edge, which undoes an accidental drag. If the width changed, it rewrites the strut to match.

On exit, a trap empties `pin.kdl` and reloads niri's config.

### Caveats

- **Strut values:** `LEFT/RIGHT/TOP/BOTTOM` at the top of the script must match the `struts` block in your niri layout. niri replaces the whole `struts` block rather than merging it.
- **Scope:** struts are global, so the left gap also appears on other outputs.
- **Crashes:** a `SIGKILL` skips the cleanup trap. Clear the gap with `: > ~/.config/niri/woims/pin.kdl`.
- **App-id:** window rules match app-ids as unanchored regexes. Make sure none of your existing rules catches `dev.pinwin`. For example, a rule for `mbv`'s mpv window must not match it.
- **Focus key:** the focus key toggles between tiles and the most recently focused floating window. With several floating windows open, that may not be the pin.
