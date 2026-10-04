#!/bin/bash
# Pin a command to the left edge of niri. Rule + reservation live in pin.kdl only while running.
{ # parse the whole script up front, so editing it doesn't break running pins
PIN=~/.config/niri/woims/pin.kdl
COLS=${COLS:-40}
GUTTER=${GUTTER:-12} # px between the pin and the first tile
# Must match the struts in woims/layout.kdl (a struts block replaces, not merges).
LEFT=24 RIGHT=24 TOP=12 BOTTOM=12

write_pin() { # $1 $2 = pin width, height in px (0 = not measured yet)
    local lock=
    (($2)) && lock="min-height $2; max-height $2;" # width stays resizable
    cat > "$PIN" <<KDL
layout {
    struts {
        left $(($1 ? $1 + GUTTER : LEFT))
        right $RIGHT
        top $TOP
        bottom $BOTTOM
    }
}
window-rule {
    match app-id="^dev\\\\.pinwin\$"
    open-floating true
    default-floating-position x=0 y=$TOP relative-to="top-left"
    default-window-height { proportion 1.0; }
    $lock
}
KDL
    niri msg action load-config-file
}
cleanup() { pkill -P $$; : > "$PIN"; niri msg action load-config-file; }
trap cleanup EXIT

write_pin 0 0
ghostty --gtk-single-instance=false --class=dev.pinwin --window-width="$COLS" --window-height=1000 -e "${@:-$SHELL}" &
gpid=$!
# Wait for the window and for its size to settle (first reports are pre-configure).
prev=
for _ in $(seq 50); do
    sleep 0.2
    cur=$(niri msg --json windows | jq -r '.[] | select(.app_id=="dev.pinwin") | "\(.id) \(.layout.window_size[0]) \(.layout.window_size[1])"')
    [ -n "$cur" ] && [ "$cur" = "$prev" ] && break
    prev=$cur
done
read -r id w h <<< "$cur"
# proportion 1.0 = full working area; shrink so top/bottom struts show.
h=$((h - TOP - BOTTOM))
niri msg action set-window-height --id "$id" "$h"
write_pin "$w" "$h"
niri msg action focus-window --id "$id"

follow() { # $1 = activated workspace id; bring the pin along if it's on the pin's output
    local pin idx
    pin=$(niri msg --json windows | jq --argjson id "$id" '.[] | select(.id == $id) | .workspace_id')
    idx=$(niri msg --json workspaces | jq --argjson ws "$1" --argjson pin "$pin" '
        (.[] | select(.id == $pin) | .output) as $o
        | .[] | select(.id == $ws and .id != $pin and .output == $o) | .idx')
    [ -n "$idx" ] && niri msg action move-window-to-workspace --window-id "$id" --focus false "$idx"
}
# Follow workspace switches at once. Once changes to the pin settle, snap it home (a no-op if
# already there) and follow its width with the strut.
niri msg --json event-stream | jq --unbuffered -r --argjson id "$id" '
    (.WorkspaceActivated? | select(.) | "ws \(.id)"),
    ((.WindowLayoutsChanged?.changes[]? | select(.[0] == $id) | .[1]),
     (.WindowOpenedOrChanged?.window | select(.id == $id) | .layout)
     | "w \(.window_size[0])")' |
while :; do
    read -r -t 0.3 kind val; rc=$?
    if ((rc == 0)); then
        case $kind in ws) follow "$val" ;; w) last=$val ;; esac
        continue
    fi
    ((rc > 128)) || break # timeout loops on, EOF ends
    [ -n "$last" ] || continue
    niri msg action move-floating-window --id "$id" -x 0 -y "$TOP"
    ((last != w)) && w=$last && write_pin "$w" "$h"
    last=
done &
wait $gpid
exit
}
