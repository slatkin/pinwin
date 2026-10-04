## 1. Spike (blocks everything after it)

- [x] 1.1 Answer the D2 checklist (items 1–8) against the pinned ghostty commit for the `libghostty-vt` crate; record the route, crate version or commit, the per-item yes/no table and the rationale in design.md D2 "Result". If any item is "no", the route is own FFI for all of it.
- [x] 1.2 Check D4's restart caveat: start a gtk-rs panel thread, drop it, start a second one. Record the result in design.md D4; if it fails, adopt the parked-GTK-thread fallback.

## 2. Crate skeleton

- [x] 2.1 Add `Cargo.toml` (package `pinwin`, lib + bin, `panic = "unwind"` in all profiles with a comment, gtk4, gtk4-layer-shell, pango, pangocairo, cairo-rs, gdk-pixbuf, the D2 ghostty route) and `.gitignore` `target/` (D1, D2, D5, D11).
- [x] 2.2 Add `build.rs` or crate config so ghostty builds from the pinned commit; document the Zig-at-build-time requirement (D2).

## 3. Port modules (crate compiles after each)

- [x] 3.1 `layout`: types from D6, checked-arithmetic geometry, pty yield decision, from `options.c` (D3, D6).
- [x] 3.2 `nerd_font`: unless the D2 spike shows the crate exposes the constraints, convert the table from ghostty's `src/font/nerd_font_tables.zig` at the pinned commit (D2, D3).
- [x] 3.3 `term`: terminal wrapper, encoders, cell and glyph logic, from `main.zig`, `cells.zig`, `keys.zig`, `input.zig` (D3).
- [x] 3.4 `pty`: fd attach, non-blocking read source, `TIOCSWINSZ`, `raise(SIGWINCH)`, hangup handling (D3, D7).
- [x] 3.5 `fontconfig`: Ghostty font and theme from the config, `monospace 11` fallback (D3).
- [x] 3.6 `render`, `images`: drawing, tween frame cache, focus accent, kitty image surfaces (D3).
- [x] 3.7 `input`, `surfaces`, `anim`: GTK controllers, layer-shell surfaces and reservation, width tween with its easing from `glue_anim.c` (D3).

## 4. Library API

- [x] 4.1 `panel` + `lib.rs`: `Panel`, `PinwinError`, GTK thread and start handshake, apply via `MainContext::invoke` with the 5 s bounded reply, `Drop` teardown, single-instance guard (D4, D6, D7).
- [x] 4.2 `guard` helper and `catch_unwind` at every boundary listed in D5; poisoned flag.

## 5. Binary and example

- [x] 5.1 `src/main.rs`: port `host/main.c` (forkpty, env contract, `--` handling, SIGINT/SIGTERM to SIGHUP, exit statuses) (D8).
- [x] 5.2 `examples/demo.rs`: port `demo/main.c`, including `DEMO_DENSE=1` (D9).

## 6. Tests

- [x] 6.1 Port the `layout` unit tests from `options_test.zig`: one table-driven test over the reachable `InvalidLayout` causes, side geometry and reservation, yield decision (D10).
- [x] 6.2 Port from `pinwin_api_test.zig` only: `NotRunning`/`Internal` on a dead handle, `SIGWINCH` after a successful winsize ioctl, replay of pty data arriving before the terminal exists. Add one panic-containment test (a panic in a guarded closure yields `Err(Internal)`). Build these on the display-free inner handle and fd/terminal parameters described in D10 (D10).
- [x] 6.3 `cargo test` passes; `cargo build --examples` succeeds.

## 7. Remove old code (only after 3–6 are done)

- [x] 7.1 `git rm` `src/*.c`, `src/*.h`, `src/*.zig`, `host/`, `demo/`, `build.zig`, `build.zig.zon`, `compile_flags.txt`. Keep `pinwin.sh` (D11). Remove the `zig-pkg/`, `zig-out/`, `.zig-cache/` lines from `.gitignore`.

## 8. Docs

- [x] 8.1 Rewrite `AGENTS.md` (structure, Cargo commands, Rust style, tests) and `README.md` (install, Cargo dependency use, the `Panel` API, build needs Zig for ghostty).

## 9. Acceptance and close-out

- [x] 9.1 One manual acceptance pass in a niri session using the `pinwin` binary and the demo example: launch, width animation both ways, focus accent, cleanup when the command exits, kitty images (`DEMO_DENSE=1`). Fix any drift found.
- [ ] 9.2 Run `openspec validate port-to-rust`, sync the delta into `openspec/specs/pinwin-panel/spec.md` and archive the change.
