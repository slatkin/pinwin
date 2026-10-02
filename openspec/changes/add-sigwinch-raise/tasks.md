# Tasks

- [x] 1.1 Raise `SIGWINCH` after each successful `TIOCSWINSZ` in `pinwin/src/pty.c` (`attach_pty` and `glue_pty_resize`); a failed ioctl raises nothing. Verify: `rg -n "SIGWINCH" pinwin/src` finds both raises, and `zig build check` passes.
- [x] 1.2 Contract test in `pinwin/src/pinwin_api_test.zig` ("winsize update delivers SIGWINCH to the process"): a real pty master on `g_pty_fd`, a SIGWINCH handler counting raises, `glue_pty_resize` sets the flag; with `g_pty_fd = -1` it does not. No sleeps, no compositor. Verify: `zig build check` passes.
