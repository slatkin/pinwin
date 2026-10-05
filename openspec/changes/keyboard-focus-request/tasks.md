# Tasks

## 1. Remap check on niri

- [x] 1.1 If `self.reserve` is already `Some`, make `Surfaces::on_map` return early, before it sets `latch`. Then a second map creates no second reserve window and no second scale handler, and `resolve_monitor` and the start handshake hook do not run again. Verify: the live check in 1.2 observes one reserve window and an unchanged gap.
- [x] 1.2 Add a GTK-thread remap of the panel window (hide, then present) for `on-demand` mode only. Verify on niri with a temporary debug trigger. After the remap, typed keys reach the pty and the accent appears. The tiled windows keep their position and size. A click on a tiled window releases focus. Record in this task whether a one-frame blink is visible. If it is visible, stop and ask the user before group 2. Result: a one-frame blink is visible (1-2 frames, about 8-16 ms, in 121 fps recordings); no blink-free path exists on niri, so the user accepted it.

## 2. Library focus request

- [x] 2.1 Add `Panel::request_focus(&self) -> Result<(), PinwinError>`. It posts the remap to the GTK thread. It uses the same poisoned test, `NotRunning` test and bounded wait as `apply_via_inner`. In `none` and `exclusive` mode it returns `Ok(())` and changes nothing. Remove the temporary debug trigger from 1.2. Verify: a test for `NotRunning` on a dead panel, and a test for `Ok` with no remap in `none` mode. Also repeat the niri test from 1.2 through the new method.
- [x] 2.2 Update the rustdoc on `Panel`, on `Keyboard` ("fixed at start time"), and on the new method to describe focus on request. Verify: `cargo doc` builds with no warnings.

## 3. Binary command-line IPC

- [x] 3.1 Parse `PINWIN_NAME` in `read_settings` (default `default`, 1..=64 characters from `[A-Za-z0-9_-]`, exit 2 otherwise). Verify: unit tests for the default, a valid name, an empty name, `a/b` and a 65-character name.
- [x] 3.2 Parse `--focus [name]` as a client mode before `--`, with the same name check. Verify: unit tests for `--focus`, `--focus notes`, `--focus a/b`, and `-- --focus` running a command.
- [x] 3.3 Add the socket path from `$XDG_RUNTIME_DIR/pinwin/$WAYLAND_DISPLAY-<name>.sock`, with the directory at mode 0700. Add the duplicate check (a successful connect exits 2), stale-file removal, and bind, all before any surface opens. Verify: tests that a stale file is replaced and that a live listener on the path makes the start fail.
- [x] 3.4 Run the listener on a scoped thread that answers `focus\n` with `ok\n` or `error\n` from `request_focus`. Remove the socket file after the child exits. Verify: a test drives the protocol over a socket pair against a stub, and the socket file is gone after `pinwin true` exits.
- [x] 3.5 Implement the client: connect, write `focus\n`, read the reply with a bounded wait, exit 0 on `ok`, and exit 1 with a message otherwise. Verify: `pinwin --focus notes` with no host prints a message and exits 1.
- [x] 3.6 Add `PINWIN_NAME` and `--focus [name]` to the usage block at the top of `src/main.rs` and to the README. Add the niri bind example `Mod+P { spawn "pinwin" "--focus"; }`. Verify: the docs name both forms.
- [x] 3.7 Test the spec scenarios on niri: focus the default instance, focus one of two named instances, the duplicate name, and a SIGKILL-ed instance followed by a new start. Verify: each scenario in `specs/pinwin-panel/spec.md` under "Focus request from the command line" behaves as written.

## 4. Follow-up for downstream hosts

- [ ] 4.1 Update issue #15 in the pinwin repo: replace the `pkill -USR1` example with the `pinwin --focus` bind, and link this change. Verify: `gh issue view 15 --json body` shows the new text.
- [ ] 4.2 File an issue in the `slatkin/mbv` repo with the implementation details for mbv as a host: mbv calls `Panel::request_focus()` and adds its own command-line IPC. For example, it can use a `--focus` client over a Unix socket, as the `pinwin` binary does. The user binds that client to a compositor hotkey. Include the remap behavior, the `none` and `exclusive` no-op, the error cases, and a link to this change. Verify: `gh issue view <n> --repo slatkin/mbv --json title,body` shows the issue.
