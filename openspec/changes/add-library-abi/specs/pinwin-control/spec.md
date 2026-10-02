# Spec Delta

## Purpose

The per-instance control socket is gone with the program form: host and panel are one process
and call across the C ABI directly. This capability is retired in full.

## REMOVED Requirements

### Requirement: Per-instance control socket
The socket in `glue.c` (`control_start`, `ControlConnection`, `g_control_socket_path`) is
deleted with design D1. No replacement: in-process callers use `pinwin_apply_layout`.

### Requirement: Socket address in the child environment
`PINWIN_SOCKET` is deleted with the socket and the fork-child setenv in `pty.c`. The host's
child never needs a socket address; nothing folds back.

### Requirement: Line request protocol
The `options` request and its replies are deleted with `control.c`/`control.h`. The only
request the socket ever served (open the options window) is doubly gone: the window is gone
too. Nothing folds back.
