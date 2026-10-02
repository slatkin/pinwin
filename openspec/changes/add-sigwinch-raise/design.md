# Design

## Context

`pinwin/src/pty.c` sets the pty winsize in two places: `attach_pty` (the initial
`TIOCSWINSZ` for the host-supplied master fd, design D6 of add-library-abi) and
`glue_pty_resize` (every layout apply that changes the grid). Neither raises any signal
today. The mbv consumer (`pin-mbv-in-pinwin`, design D2 "required upstream addition" 1 and
D3) renders with crossterm, which reports `Event::Resize` only when the process receives
SIGWINCH.

## Decisions

**D1. `raise(SIGWINCH)` from the calling thread — not `pthread_kill` of a specific
thread.** Signal disposition is process-wide: `raise()` delivers SIGWINCH to the process
and the kernel runs the handler on any thread that does not block the signal. crossterm
uses signal-hook, whose handler is process-wide and thread-agnostic (it writes to a
self-pipe that the event loop reads). So a raise from the GTK thread reaches the host's
handler; pinning a thread would add state (`pthread_t` bookkeeping) for nothing. If the
host installs no handler, the default disposition for SIGWINCH is ignore — the raise is a
no-op, so libraries that are not crossterm hosts are unaffected.

**D2. Raise only after a successful winsize update.** The contract is "the host sees
SIGWINCH after the winsize is updated". An `ioctl` failure leaves the winsize untouched;
raising then would report a resize that did not happen. Both sites gate the raise on
`ioctl(...) >= 0`. `glue_pty_resize` still returns early with no raise when no pty is
attached (`g_pty_fd < 0`).

## Risks / Trade-offs

- The initial attach raises SIGWINCH before the host may expect it (the panel opens and
  the host learns its own pty size immediately). This is uniform with every later resize
  and is what "after every TIOCSWINSZ" asks for; a host that does not want it can ignore
  the first event.
