# Spec Delta

## Purpose

Lets the program running inside a pinwin panel, or anything it passes the address to, drive that
panel instance from outside pinwin's own tray, through a per-instance local socket.

## ADDED Requirements

### Requirement: Per-instance control socket
Each running pinwin instance SHALL listen on its own Unix stream socket at
`$XDG_RUNTIME_DIR/pinwin/<pid>.sock`, where `<pid>` is pinwin's process ID. The `pinwin` directory
SHALL be created with mode 0700 when missing, and the socket SHALL be accessible only to the
owning user. pinwin SHALL start listening before it spawns the command, and SHALL remove its socket
file when it exits normally. When `XDG_RUNTIME_DIR` is unset or the socket cannot be created,
pinwin SHALL print a diagnostic to stderr and run normally without a control socket.

#### Scenario: Socket exists while running
- **WHEN** pinwin with PID 4242 is running and `XDG_RUNTIME_DIR=/run/user/1000`
- **THEN** a socket exists at `/run/user/1000/pinwin/4242.sock`, accessible only to the user

#### Scenario: Removed on exit
- **WHEN** the command exits and pinwin exits normally
- **THEN** pinwin's socket file no longer exists

#### Scenario: No runtime directory
- **WHEN** pinwin starts with `XDG_RUNTIME_DIR` unset
- **THEN** pinwin prints a diagnostic, runs the command normally, and has no control socket

### Requirement: Socket address in the child environment
When its control socket is listening, pinwin SHALL set `PINWIN_SOCKET` to the socket's absolute
path in the command's environment. When there is no control socket, pinwin SHALL remove
`PINWIN_SOCKET` from the command's environment, so an inherited value never points at another
instance.

#### Scenario: Child learns the address
- **WHEN** pinwin runs `mbv` with a listening control socket
- **THEN** `mbv`'s environment contains `PINWIN_SOCKET` equal to that socket's path

#### Scenario: Nested launch does not inherit
- **WHEN** pinwin has no control socket and was itself started with `PINWIN_SOCKET` set by another instance
- **THEN** the command's environment has no `PINWIN_SOCKET`

### Requirement: Line request protocol
A client SHALL connect, send one request line terminated by `\n`, and read one reply line
terminated by `\n`; pinwin SHALL then close the connection. The request `options` SHALL open that
instance's options window, with the same behavior as the tray's `Options...` (including presenting
an already open window without resetting pending edits), and reply `ok`. Any other request SHALL
change nothing and reply `error unknown request`. A connection that closes, or sends more than 256
bytes, before a complete line SHALL be dropped without effect. Handling a request SHALL NOT block or
interrupt the terminal or the command.

#### Scenario: Open options
- **WHEN** a client sends `options\n` to the instance's socket
- **THEN** that instance's options window opens and the client receives `ok\n`

#### Scenario: Unknown request
- **WHEN** a client sends `resize 60\n`
- **THEN** nothing changes and the client receives `error unknown request\n`

#### Scenario: Garbage input
- **WHEN** a client sends 1000 bytes with no newline
- **THEN** pinwin drops the connection, changes nothing, and the panel keeps running

#### Scenario: Works with and without a tray
- **WHEN** an `options` request reaches a pinwin started with or without `--no-tray`
- **THEN** the options window opens in both cases
