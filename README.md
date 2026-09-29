# winpin

Pins a Ghostty window running `mbv` (or any command) to the left edge of niri. Tiled windows are laid out to its right. The pin follows you across workspaces and releases its space when the command exits.

```sh
winpin            # runs mbv
winpin htop       # any command
COLS=60 winpin    # width in terminal columns (default 40)
GUTTER=8 winpin   # px between the pin and the first tile (default 12)
```

## How it works

- The script writes a window rule and a left strut sized to the window into `~/.config/niri/woims/pin.kdl`, and empties that file on exit.
- `nirius` follow mode (`if-invisible`) moves the window along with the active workspace.

## Requirements

- niri, Ghostty, `jq`
- `nirius`, with `niriusd` running
- this line at the end of `~/.config/niri/config.kdl`:

  ```kdl
  include optional=true "woims/pin.kdl"
  ```

## Caveats

- **Strut values:** the script repeats the struts from `woims/layout.kdl` (`LEFT/RIGHT/TOP/BOTTOM`), because niri replaces the whole `struts` block rather than merging it. Keep the two in sync.
- **Scope:** struts are global, so the left gap also appears on other outputs.
- **Crashes:** if the script is killed with `SIGKILL`, the gap stays. Run `: > ~/.config/niri/woims/pin.kdl` to clear it.
- **App-id:** the window's app-id is `dev.winpin`. Don't use a class containing `mbv`, because the mpv rule in `rules.kdl` would catch it.
