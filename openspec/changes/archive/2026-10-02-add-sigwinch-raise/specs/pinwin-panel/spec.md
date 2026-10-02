# Spec Delta

## ADDED Requirements

### Requirement: Winsize updates signal the host

The library SHALL raise `SIGWINCH` in the host process after every successful
`TIOCSWINSZ` on the pty master — the initial attach and every layout apply that updates
the winsize — so a host that handles SIGWINCH (for example a crossterm event loop) sees
the resize without polling. A failed winsize ioctl raises nothing; a host that installs
no SIGWINCH handler is unaffected (the default disposition is ignore).

#### Scenario: Resize delivers SIGWINCH

- **WHEN** a layout apply updates the pty winsize successfully
- **THEN** the host process receives SIGWINCH after the update

#### Scenario: No update, no signal

- **WHEN** the panel has no attached pty
- **THEN** a resize request raises no SIGWINCH
