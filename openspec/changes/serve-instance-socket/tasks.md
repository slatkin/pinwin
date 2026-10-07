# Tasks

## 1. Move the instance socket into the library

- [x] 1.1 Move `src/ipc.rs` and `src/ipc/tests.rs` to `src/instance.rs` and `src/instance/tests.rs`, add `pub mod instance` to `src/lib.rs`, and make `InstanceName`, the bind and `socket_path` public. Verify: `cargo nextest run` passes with the moved tests unchanged, and the binary builds against `pinwin::instance`.
- [x] 1.2 Replace the `String` errors with typed errors (`InvalidName`, `PathError`, `InstanceError { Duplicate, Path, Io }`), each with `Display` and `std::error::Error`. The binary adds its `pinwin: <context>:` prefixes. Verify: the existing name and path tests assert the variants, the `cli.rs` tests still match the `pinwin: --toggle:` prefix, and the main.rs tests pin the exit codes (2 for a name or path error, 1 for no instance).
- [x] 1.3 Add `InstanceSocket::bind(&InstanceName)`, which reads the environment and owns the listener and the socket file, as a thin wrapper over a crate-private `bind_at(&Path)`. Dropping an unused socket removes its file. Verify: `temp_dir` tests through `bind_at` bind, drop the socket and bind the same path again, and a second live bind returns `InstanceError::Duplicate`. Exactly one test goes through `bind` and the environment, and it does not set `XDG_RUNTIME_DIR`: it asserts only the `Path` error when the variable is absent, or it is marked `#[ignore]` if it needs the variable.

## 2. Requests and the client call

- [x] 2.1 Add `pub enum Request { Toggle, Show }` and make the accept loop parse `toggle\n` and `show\n` into a callback that takes the `Request`. Other lines get `error\n`, as before. Verify: the protocol test covers both lines, `hide\n` and an oversized line.
- [x] 2.2 Add `instance::send(&InstanceName, Request) -> Result<(), SendError>` with `#[non_exhaustive]` `SendError { Environment, NotListening, Refused, NoAnswer }`, built on the existing bounded client, as a thin wrapper over a crate-private `send_to(&Path, Request)`. Verify: `temp_dir` tests through `send_to` for no socket file (`NotListening`), a refused connect on a stale file (`NotListening`), an `error\n` reply (`Refused`) and a silent listener (`NoAnswer`).

## 3. Panel show and the listener thread

- [ ] 3.1 Add `PanelCommand::Show` and `Panel::show()`. It shows a hidden panel and answers at once on a shown one. Share the guard and the poisoned → live → bounded-wait path with the toggle. Verify: display-free tests, written like the toggle's in `commands.rs`, show that `Show` on `Shown` leaves `Visibility::Shown` with no commit and that `Show` on `Hidden` shows. A dead panel returns `NotRunning`.
- [ ] 3.2 Make `Startup` own an optional `InstanceSocket` through `Startup::with_instance` and drop `Copy`. `Panel::start` takes the socket out before it builds the `StartCommand`. Verify: `cargo build --all-targets`, and a start that fails `InvalidFd` with a bound socket leaves no socket file.
- [ ] 3.3 After a successful handshake, spawn the detached `pinwin-instance` listener thread over clones of `Arc<Inner>` and the command sender. `Panel` holds the shutdown flag and the `SocketFile`. `Drop` sets the flag and removes the file before the teardown, and never joins. A failed spawn drops the panel and returns `Internal`. Verify: an `#[ignore]`d display test starts with a socket, calls `send(Toggle)` and `send(Show)`, drops the handle, and asserts that the file is gone and the drop takes less than the teardown bound. A second variant first opens a connection and writes half a request line, so the listener is blocked in its bounded read. The half request is written just before the drop, so a joining drop would wait out the listener's 500 ms read bound. The test asserts that the drop returns in under 250 ms.

## 4. The pinwin binary

- [ ] 4.1 Make `host_panel` bind through `InstanceSocket::bind` before `forkpty` and pass the socket through `Startup::with_instance`. Delete `serve_toggle_until_exit` and the binary's own listener thread, and wait for the child directly. Verify: `cargo nextest run`, plus a display-free main.rs test of the bind-error mapping: every `InstanceError` (`Duplicate`, `Path`, `Io`) maps to exit 2 with a `pinwin:` message, which keeps today's behavior. The bind's own duplicate detection is covered by task 1.3, and the live run is task 5.2.
- [ ] 4.2 Parse `--show [name]` like `--toggle`, with its own name-error label, and route both client modes through `instance::send`: `Environment` exits 2, every other error exits 1. Verify: `cli.rs` tests for `--show`, `--show notes`, `--show a/b` (exit 2), `-- --show`, and the main.rs test that `--show` with no listener exits 1.

## 5. Release

- [ ] 5.1 Document in the README's library section the bind-then-start opt-in, `Panel::show` and `instance::send`, and `--show` in "Toggle request". In "Migrating from `request_focus`", replace the "must track the shown state itself" text with a pointer to `Panel::show`. Add a "Migrating to 0.3.0" section: `Startup` is no longer `Copy`, and the opt-in socket is new. Add the README's socket snippet as `examples/instance_host.rs`, identical apart from a `main`. Verify: `cargo build --example instance_host` compiles it, and `diff` of the snippet's body against the README's fenced block shows no difference.
- [ ] 5.2 Set the version in `Cargo.toml` to 0.3.0 and run the full check. Verify: `cargo clippy --all-targets -- -D warnings` and `cargo nextest run` both pass, and on niri `pinwin --show` and `pinwin --toggle` behave as the "Show from the command line" scenario says.

## Workflow follow-up

- Tag 0.3.0 after merge.
