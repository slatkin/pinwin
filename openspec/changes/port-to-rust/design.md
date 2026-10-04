# Design

## Context

See proposal.md for motivation. Current shape, which the port must preserve:

- Zig core owns libghostty-vt (terminal, render state, key/mouse/focus encoders, kitty
  graphics, PNG decode hook, effect callbacks for pty writes, size and device attributes) and
  exports a C ABI; C glue owns GTK4, layer-shell, Pango, cairo, GdkPixbuf and the pty.
- Thread model (`src/pinwin_api.c`): the host calls from its own thread. `pinwin_start` spawns
  a `pinwin-gtk` thread that runs GTK/layer-shell init and the `GtkApplication` main loop,
  and the host waits on a cond for a start result, so start is synchronous. Every apply is
  posted to the GTK main context with `g_main_context_invoke`; the caller waits up to 5 s
  for the reply (refcounted request so a late callback never touches freed memory). Stop
  posts a teardown to the loop and joins the thread. A static mutex holds the running flag.
- The host owns the pty fd and never has it closed by the library; `raise(SIGWINCH)` follows
  every successful `TIOCSWINSZ`.
- `host/main.c` forks the child with `forkpty`, parses env vars, and starts the panel.
- Archived designs under `openspec/changes/archive/` are referenced from code comments as
  D-numbers. This design's IDs (D1..) are scoped to `port-to-rust`; comments in the new code
  cite them as `port-to-rust D<n>` so they cannot be confused with the archived ones.

## Goals / Non-Goals

**Goals:**
- Delete all C and Zig source; one Cargo crate builds the library, the binary and the example.
- Keep behaviour identical, per the spec delta in `specs/pinwin-panel/spec.md`.
- Make rejected-at-runtime argument classes unrepresentable (spec delta).

**Non-Goals:**
- No C-compatible export, no cdylib/staticlib target.
- No new features, no behaviour changes, no cleanup of the render pipeline beyond what the port
  needs.
- No async runtime and no multi-panel support.

## Decisions

### D1. Single crate, library plus binary

`Cargo.toml` defines package `pinwin` with `src/lib.rs`, `src/main.rs` and `examples/`. The
binary uses only the public library API.

Alternatives: a workspace with `pinwin` (lib) and `pinwin-cli` (bin). Rejected: the proposal
calls for one crate, there is one consumer of the library API in-tree, and the binary being
the library's first real client is enough isolation. Revisit only if a consumer needs the
library without the binary's dependencies (the binary adds none beyond a pty helper).

### D2. libghostty-vt: crate-or-own-FFI decision rule (decided by the spike)

Rule, no mixing: use the existing `libghostty-vt` crate only if it covers everything the
panel uses (every item in the checklist below). If it is partial, use pinwin's own `extern`
declarations (a `ghostty_sys` module, hand-written or bindgen-generated and checked in)
against one pinned ghostty commit for all of it. Never take some features from the crate and
others from raw FFI, since the two would hold separate views of one `GhosttyTerminal`.

Checklist the spike must answer yes/no per item, against the pinned commit
`3a3047f6b62a791fd8b12d9f07a85b3d2160370b` (the one `build.zig.zon` fetches today):

1. Terminal new/resize (cols, rows, cell pixel size)/`vt_write`.
2. Effect callbacks: write-to-pty, size report (`CSI 16 t`), primary device attributes.
3. Kitty image storage limit option, and the PNG decode hook (`ghostty_sys_set` equivalent).
4. Render state, row iterator, row cells with styles and colours.
5. Key encoder (from-terminal options, press/release, mods, utf8), mouse encoder (SGR, cell
   size option), focus encoder.
6. Kitty graphics: placement iterator and per-placement render info.
7. Builds against that commit (or against a commit the spike shows is equivalent). Also record
   whether the crate exposes the Nerd Font glyph constraints; if it does, `nerd_font` is not
   needed, otherwise it is a one-off conversion of ghostty's `src/font/nerd_font_tables.zig` at
   the pinned commit (no generator script exists in this repo).
8. Types being `!Send`/`!Sync` is compatible with D4 (terminal lives on the GTK thread).

Evidence gathered so far (docs.rs, `ketch` on 2026-10-04):

- The crate documents safe bindings for terminal state, `key`, `mouse`, `focus`, `render`
  (`RenderState`) and `kitty` modules, and effect-callback traits including `PtyWriteFn`,
  `SizeFn` and `DeviceAttributesFn` (`docs.rs/libghostty-vt/latest/libghostty_vt/terminal`).
- Its `kitty::graphics` module is behind a `kitty-graphics` cargo feature
  (`docs.rs/libghostty-vt/latest/libghostty_vt/kitty`).
- It states the API is unstable with breaking changes expected, and that all types are
  `!Send`/`!Sync` (`docs.rs/libghostty-vt`).
- A low-level `libghostty-vt-sys` crate is re-exported as `ffi`.

Unverified, so the spike's job: items 3, 6 (render-info detail), 7, which ghostty commit the
crate pins and whether it equals ours, whether its build needs Zig and a network fetch, and
whether callback trampolines catch panics (D5). The docs do not say; a missing item is a
spike result of "partial", not an assumption.

Result: **TO BE RECORDED by task 1.1** (chosen route, crate version or ghostty commit, the
per-item yes/no table, and the rationale). Tasks 2 onward do not start until it is filled in.

Alternatives: always own FFI (more code, but no dependency on an unstable third-party API);
always the crate (breaks the moment an item is missing). The spike picks between them instead
of guessing.

### D3. Module layout

Ported one-to-one by responsibility so each file stays small:

| Rust module | Replaces |
|---|---|
| `layout` (pure, no GTK) | `options.c`, layout types, geometry/validation, pty yield decision |
| `term` | `main.zig`, `cells.zig`, `keys.zig`, `input.zig`: terminal wrapper, encoders, cell/glyph logic |
| `nerd_font` | `nerd_font_tables.h`: const table, regenerated from the pinned ghostty source |
| `pty` | `pty.c`: fd attach, read source, winsize, `SIGWINCH` raise |
| `fontconfig` | `fontconfig.c`: Ghostty config and theme |
| `render`, `images` | `render.c`, `images.c` |
| `input` | `input.c`: GTK controllers |
| `surfaces`, `anim` | `glue.c`, `glue_anim.c` (the tween easing lives in `anim`, ported line by line and untested like the C) |
| `panel` (+ `lib.rs` re-exports) | `pinwin_api.c`: thread, handshake, `Panel`, `PinwinError` |

The module map is the plan, not a contract: merge or split if a file exceeds about 400 lines.
Only `main.rs` reads pinwin environment variables (`COLS`, `GUTTER`, `PINWIN_*`); no library module
may, per the main spec's "No pinwin-owned configuration".

### D4. Thread and main-loop model: keep the existing one

`Panel::start` spawns one `pinwin-gtk` thread (`std::thread`). That thread calls `gtk::init`
and layer-shell setup, creates the application and runs its main loop; all GTK objects,
terminal state and the pty source live in a `thread_local` on it, so none need to be `Send`
(which also satisfies the crate's `!Send` terminal types). The host thread waits on an
`mpsc` channel for the start result.

Apply: the host thread posts a closure with `glib::MainContext::invoke`, carrying the layout
(a `Copy` value) and a `mpsc::sync_channel(1)` reply sender, then `recv_timeout(5 s)`. A
timeout is `PinwinError::Internal`. Dropping the receiver replaces the C refcount, since a
late reply fails harmlessly on send. Drop: post teardown (cancel animation, close surfaces,
quit the loop), then join the thread. A process-wide `Mutex<bool>`/atomic enforces
single-instance (`AlreadyRunning`).

Alternatives: run GTK on the host's main thread (rejected: the host owns its own loop, e.g.
crossterm, and the existing contract is a library that brings its own thread); `async`
channels (rejected, no runtime needed for a few calls); share state behind `Arc<Mutex<_>>`
with the host reading it directly (rejected: GTK objects are thread-bound).

Restart caveat: gtk-rs records the thread that first initialised GTK. If a second `start`
after a drop fails because GTK cannot be initialised on a new thread, the fallback is one
process-lifetime GTK thread that is parked between panels. The spike (task 1.2) checks this
before the panel module is written.

### D5. Panics never cross the API: `catch_unwind` at every boundary

`panic = "unwind"` is set explicitly in the release and dev profiles; `panic = "abort"` would
defeat the requirement and is forbidden by a comment in `Cargo.toml`. A small
`guard(f) -> Result<T, PinwinError>` helper wraps `catch_unwind(AssertUnwindSafe(f))`, and is
applied at:

1. Every public `Panel` entry point (`start`, `apply_layout`, `apply_layout_animated`) on the
   host thread.
2. The GTK thread's entry function.
3. Every closure handed to GTK/glib (draw function, event controllers, frame-clock tick,
   timeouts, the fd source, the invoked apply/teardown closures) and every `extern "C"`
   terminal callback trampoline. Unwinding out of those frames would cross C code.

On a caught panic the GTK side logs to stderr, sets a shared `poisoned` flag, stops
drawing and quits the loop. Later calls on the handle return `Err(Internal)`: the `poisoned`
check comes before the "GTK side ended" check, so a panic reports `Internal`, never
`NotRunning`. `Drop`
(which does not use `unwrap`) still closes and joins. A panic while the host is blocked in
apply is released by the reply channel closing, which reports `Internal`.

Alternatives: `panic = "abort"` (rejected: kills the host, violating the spec); catching only
at the API surface (rejected: panics occur in GTK callbacks, not on the host's call stack, and
unwinding through C is undefined).

### D6. Layout types that cannot express the former rejections

`Layout { side: Side, cols: NonZeroU16, top: i32, bottom: i32, left: i32, right: i32 }`,
`Keyboard { None, OnDemand, Exclusive }`, `Accent { rgb: [u8; 3], width: NonZeroU16 }` passed as
`Option<Accent>`, and `Side { Left, Right }` are `Copy` types with public fields (no builder:
the invariants live in the field types). `PinwinError` is a `#[non_exhaustive]` enum with
`InvalidLayout`, `InvalidFd`, `NoDisplay`, `AlreadyRunning`, `NotRunning`, `Internal`,
implementing `std::error::Error` without a derive crate. Monitor validation stays a runtime
check inside the GTK thread, reusing the existing checked-arithmetic geometry in `layout`.

Alternative: keep integer fields and validate at runtime as before (rejected, the proposal
and spec delta make these type guarantees).

### D7. The pty fd is a `RawFd`

`Panel::start` takes `RawFd`, validated with `fcntl(F_GETFL)` before any GTK work, set
non-blocking by the library as today, and never closed by it. `OwnedFd` is rejected because
the host owns the lifetime; `BorrowedFd<'_>` is rejected because the GTK thread must outlive
any borrow the caller could hand it, and the lifetime would infect `Panel`.

### D8. Binary: pty via `rustix`/`libc`, no CLI crate

`src/main.rs` ports `host/main.c` literally: `forkpty` (through `libc` or `rustix`/`nix`, the
smallest that gives `forkpty`), `std::env`, a hand-rolled `--`/unknown-option check, SIGINT and
SIGTERM handlers that `kill(child, SIGHUP)`. No `clap`: the program has no options beyond `--`,
and the env-var contract is fixed by the spec.

### D9. The demo becomes a Cargo example

`examples/demo.rs` replaces `demo/main.c` (the same stdin commands, `DEMO_DENSE=1` stand-in
child, own pty pair). An example is built only by `cargo build --examples` / `cargo run
--example demo`, never installed, like `zig build demo`. Alternative: a second `[[bin]]`
(rejected: it would be installed with `cargo install`).

### D10. Tests: only those that still catch real failures

Ported to `cargo test` (unit tests in `layout` and `panel`, no display needed; `Panel` wraps an
inner display-free handle type (state, reply channel, poisoned flag) so the dead-handle and
panic tests can build one without GTK, and `pty` / `term` take their fd and terminal as
parameters rather than process-globals for the same reason):
- Monitor validation: one table-driven test with a case per reachable `InvalidLayout` cause
  (overflow, negative reservation, no row, no output width) from `options_test.zig`.
- Layout math: side geometry margins and reservations, the pty yield decision. The tween
  easing has no test today and gets none.
- `NotRunning` and `Internal` from the dead-handle path, and the panic-containment test
  (a deliberate panic inside a guarded closure yields `Err(Internal)`, not an unwind).
- `SIGWINCH` after a successful winsize ioctl and replay of pty data that arrives before the
  terminal exists (both from `pinwin_api_test.zig`, they catch real regressions).

Not ported: invalid side, keyboard mode, zero or oversized columns, accent enabled/width
checks, null-layout checks. These are type guarantees now; there is no test per former
rejection (optionally one `compile_fail` doctest is NOT added either, as it only tests the
compiler).

### D11. Generated and legacy files

- Build artifacts `zig-out/`, `zig-pkg/` and `.zig-cache/` are untracked (checked with
  `git ls-files` and `.gitignore`), so there is nothing to `git rm`; their `.gitignore` lines
  are replaced by `target/`. Leftover local directories may be deleted by the user.
- `build.zig`, `build.zig.zon` and `compile_flags.txt` are tracked and are removed (proposal
  Impact).
- `pinwin.sh` is tracked, a standalone bash script that predates the library and shares no
  code with it. It is not a Zig build artifact and the proposal does not list it. Decision:
  keep it, and keep the AGENTS.md note that it is legacy reference. Confirm with the user
  during review if deletion is wanted.

## Risks / Trade-offs

- Crate covers only part of libghostty-vt (kitty graphics feature, callbacks, PNG hook) →
  D2 rule falls back to own FFI for everything; spike runs first so the cost is known before
  any module is written.
- gtk-rs may refuse GTK init on a second thread after restart → spike task 1.2; fallback in D4.
- Behaviour drift in rendering (Pango/cairo glyph metrics, Nerd Font constraints, tween
  frame cache) is not caught by unit tests → one manual acceptance task at the end (no
  per-phase live testing, per the review decision); render code is ported line by line to keep
  the diff reviewable against the C.
- Panic inside a C callback unwinds through C frames → D5 guard on every trampoline.
- Build now needs Zig at build time for ghostty (and network for the ghostty fetch unless
  vendored) → documented in README and AGENTS.md; pinwin has no Zig source.
- Pinned ghostty commit moves under the crate (if D2 picks the crate) → pin the crate version
  exactly in `Cargo.toml` and record the commit in the D2 result.
- A large single change is hard to review → tasks are ordered so the crate compiles after
  every group, and the C/Zig sources are deleted only after the Rust code replaces them.

## Migration Plan

1. Land the Rust crate beside the C/Zig sources until acceptance passes; the Rust build and
   the Zig build do not share files (`Cargo.toml` vs `build.zig`).
2. Delete C/Zig sources, `host/`, `demo/`, `build.zig*`, `compile_flags.txt`.
3. Rewrite AGENTS.md and README.md for Cargo.
4. Sync the delta into `openspec/specs/pinwin-panel/spec.md` and archive the change.

Rollback: before step 2 the old build is intact; after it, `git revert` of the deletion
commit restores it. Consumers of `libpinwin.a` lose that interface by design (proposal).
