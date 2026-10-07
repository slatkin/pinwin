#!/bin/bash
# Probe the panel's host-visible sizes: the pty window size's pixel fields
# (TIOCGWINSZ) plus the CSI 16 t cell-size report. Run this inside the panel
# (on its slave pty), e.g. `pinwin scripts/probe-winsize.sh`.
#
# Output (one line each): `winsize: rows=<r> cols=<c> xpixel=<x> ypixel=<y>`
# and `csi16: cell_width=<w> cell_height=<h>`. With an argument, the lines
# go to that file instead of stdout — the panel renders the pty, so a file
# is the only way the numbers escape a `pinwin <probe> <file>` run.
set -u

if (($# > 0)); then
    # Keep a handle on the slave pty: the query must go to the terminal,
    # not into the output file.
    exec 3>&1
    exec >"$1"
else
    exec 3>&1
fi

# The panel thread starts after the child is forked; give it a moment to
# come up so the query below is not sent before the terminal exists.
sleep 2

# Stdout may be the output file by now, so ask stdin (the slave pty).
python3 -c "
import fcntl, termios, struct, sys
buf = fcntl.ioctl(sys.stdin.fileno(), termios.TIOCGWINSZ, b'\0' * 8)
rows, cols, xp, yp = struct.unpack('HHHH', buf)
print(f'winsize: rows={rows} cols={cols} xpixel={xp} ypixel={yp}', flush=True)
"

saved_stty=$(stty -g)
restore() { stty "$saved_stty"; }
trap restore EXIT
# Raw, non-canonical reads with a 5 s overall timeout so an unanswered query
# fails closed instead of hanging the panel child.
stty -icanon -echo min 0 time 50

printf '\033[16t' >&3
reply=""
deadline=$((SECONDS + 5))
while ((SECONDS < deadline)); do
    if IFS= read -r -n1 -t 1 ch; then
        reply+="$ch"
        [[ $ch == "t" ]] && break
    fi
done

# The reply is ESC [ 6 ; <height> ; <width> t; anything else is noise.
if [[ $reply =~ \[6\;([0-9]+)\;([0-9]+)t ]]; then
    echo "csi16: cell_width=${BASH_REMATCH[2]} cell_height=${BASH_REMATCH[1]}"
else
    echo "csi16: no reply (got: $(printf '%q' "$reply"))"
    exit 1
fi
