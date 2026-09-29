# pinwin

Pins a Ghostty window running [`mbv`](https://github.com/slatkin/mbv) (or any command) to the left edge of [niri](https://github.com/niri-wm/niri) as a fixed sidebar. Tiled windows are laid out to its right. The pin moves with you across workspaces, and when the command exits the sidebar goes away and the tiles reclaim the space.

```
|<- pin ->|gutter|<-------- scrolling tiles -------->|
|  mbv    |      | [col] [col] [col] ...              |
```

Stock niri and stock Ghostty from the package manager are enough. pinwin needs no patches or plugins.

## Usage

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

## Install

Requirements:
- niri
- Ghostty
- `jq`
- [`nirius`](https://sr.ht/~tsdh/nirius/), with `niriusd` running

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

## How it works

On start, pinwin:

1. Writes a window rule into `pin.kdl`. The rule matches app-id `dev.pinwin` and opens the window floating at the left edge, full height. pinwin then runs `niri msg action load-config-file`.
2. Starts `ghostty --gtk-single-instance=false --class=dev.pinwin --window-width=$COLS`. The `--gtk-single-instance=false` makes the command run in its own process instead of being handed to an already-running Ghostty.
3. Waits until niri reports the same window size twice in a row. It then shrinks the height to respect the top and bottom struts, and writes `left = width + GUTTER` into the struts.
4. Turns on `nirius` follow mode (`if-invisible`), so the window moves to whichever workspace is active on its output.

On exit, a trap empties `pin.kdl` and reloads niri's config.

## Caveats

- **Strut values:** `LEFT/RIGHT/TOP/BOTTOM` at the top of the script must match the `struts` block in your niri layout. niri replaces the whole `struts` block rather than merging it.
- **Scope:** struts are global, so the left gap also appears on other outputs.
- **Crashes:** a `SIGKILL` skips the cleanup trap. Clear the gap with `: > ~/.config/niri/woims/pin.kdl`.
- **App-id:** window rules match app-ids as unanchored regexes. Make sure none of your existing rules catches `dev.pinwin`. For example, a rule for `mbv`'s mpv window must not match it.
- **Focus key:** the focus key toggles between tiles and the most recently focused floating window. With several floating windows open, that may not be the pin.
