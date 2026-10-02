# Proposal

## Why

mbv's `pin-mbv-in-pinwin` change depends on a behaviour the library form never specified:
when the panel's winsize changes, the host's crossterm event loop must see
`Event::Resize`. crossterm learns of resizes by SIGWINCH, and today `pinwin/src/pty.c`
updates the pty winsize (`TIOCSWINSZ`) without ever raising the signal, so the host would
have to poll. This is a required upstream addition for that consumer.

## What Changes

- `pinwin/src/pty.c` raises `SIGWINCH` in the host process after every successful
  `TIOCSWINSZ` on the pty master (both sites: the initial attach and `glue_pty_resize`).
- `pinwin-panel` gains a requirement: winsize updates deliver SIGWINCH to the process;
  a failed ioctl raises nothing.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `pinwin-panel`: ADDED requirement — winsize updates signal the host (SIGWINCH after a
  successful winsize update; none when no update happens).

## Impact

- `pinwin/src/pty.c` (two raise sites, `<signal.h>`).
- `pinwin/src/pinwin_api_test.zig`: one new unit test owning the "resize delivers
  SIGWINCH" contract (no sleeps, no compositor).
- Delivery form: `raise(SIGWINCH)` — signal disposition is process-wide, so the signal
  reaches a handler installed by any thread (crossterm's signal-hook handler included)
  regardless of which pinwin thread raises it; no thread ownership to establish.
