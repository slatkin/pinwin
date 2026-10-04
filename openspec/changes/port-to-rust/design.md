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
  posts a teardown to the loop and joins the thread (the port keeps the teardown and the reply
  wait but never joins; see D4). A static mutex holds the running flag.
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

Result (task 1.1 spike; scratch dir `/tmp/ptr-spike`, never in this worktree): **the crate is
rejected, the route is own FFI** — a `ghostty_sys` module (hand-written or bindgen-generated
and checked in) of `extern` declarations against the pinned commit
`3a3047f6b62a791fd8b12d9f07a85b3d2160370b`. The checked crate was `libghostty-vt` v0.2.2
(and `libghostty-vt-sys` v0.2.2, features `kitty-graphics` + `png`); its build script pins
ghostty `a887df42c56f6de86c0fe6da9c4eeca37931e083` (2026-07-11), which `gh` compare shows is
**1404 commits behind** the pin (2026-09-29) and is not an equivalent commit: the C API moved
(`ghostty_terminal_new` changed from `(alloc, &term, GhosttyTerminalOptions)` to
`(alloc, &term, cols, rows)`, `GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y` was added,
`ghostty_render_state_colors_get` and `ghostty_terminal_mode_set` were removed). Under the D2
rule (any "no" means own FFI for all of it) three independent blockers decide it:

| # | Item | Verdict | Evidence |
|---|---|---|---|
| 1 | Terminal new/resize/`vt_write` | **no** (against the pinned commit) | `Terminal::new` passes the crate's transparent `TerminalOptions` struct where the pinned API takes two `u16`s. Probe: `TerminalOptions { cols: 132, rows: 43, max_scrollback: 77 }` produced a terminal whose `rows()` is 77 (the `max_scrollback` word read as rows) and `max_scrollback: 0` returned `InvalidValue`; both are 132x43 / accepted at the crate's own pin. `resize`/`vt_write` themselves match. |
| 2 | Effect callbacks: write-to-pty, size report (`CSI 16 t`), primary device attributes | **yes** | `on_pty_write`/`on_size`/`on_device_attributes` all fired (`pty=3 size=1 da=1`) on `\x1b[16t`, `\x1b[?7$p`, `\x1b[c`. Caveat for D5: the crate's `extern "C"` trampolines do **not** catch panics — a panic in a callback aborted the process ("panic in a function that cannot unwind", exit 134). |
| 3 | Kitty image storage limit and the PNG decode hook | **yes** | `Terminal::set_kitty_image_storage_limit(64 MiB)` and `graphics::set_png_decoder(Some(Box<dyn DecodePng>))` both succeed; pinwin would supply its own gdk-pixbuf decoder (the bundled `RustPngDecoder` has no public constructor, so it cannot be used, but it is not needed). |
| 4 | Render state, row iterator, row cells with styles and colours | **no** | Cells, styles, fg/bg colours and cursor viewport work, but (a) the row's viewport-y position (`GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y`, which `src/cells.zig` reads to place every row) has no accessor and no constant: the crate's bindings stop at `SELECTION`, and at its own pin the datum does not exist; (b) against the pinned commit `Snapshot::colors()` — the only accessor for render-state fg/bg/palette — fails to link: `undefined symbol: ghostty_render_state_colors_get` (removed upstream after the crate pin). |
| 5 | Key encoder (from-terminal options, press/release, mods, utf8), mouse encoder (SGR, cell size), focus encoder | **yes** | Key with `set_options_from_terminal` + kitty `REPORT_EVENTS`: press `\x1b[97;6u`, release `\x1b[97;6:3u`, repeat. Mouse with `set_options_from_terminal` + `Format::Sgr` + `EncoderSize`: `\x1b[<0;2;1M`. `focus::Event::encode` OK. Caveat: `encode_to_vec`'s grow path reserves `required - remaining` instead of enough for `required`, so a `Vec` that already has spare capacity fails with `OutOfSpace`; use `encode` with a caller buffer. |
| 6 | Kitty graphics placement iterator and per-placement render info | **yes** | A PNG command yielded one placement with `image()` 1x1 RGBA (4 bytes) and `placement_render_info()` geometry (viewport position, pixel and grid size). |
| 7 | Builds against the pinned commit; Nerd Font constraints | **no** | The vendored build clones `a887df42` and needs Zig 0.15.2: with the repo's Zig 0.16.0 `zig build` fails ("Your Zig version v0.16.0 does not meet the required build version of v0.15.2"). Setting `GHOSTTY_SOURCE_DIR` to the pinned source does build with Zig 0.16.0, but the wrappers are then ABI-incompatible (item 1) and `Terminal::set_mode`/`Snapshot::colors()` fail to link (item 4). Nerd Font constraints are not exposed by the crate *or* by libghostty-vt's C API (`grep -i nerd include/` is empty; the table is in ghostty `src/font/nerd_font_tables.zig`), so `nerd_font` is a conversion under either route. |
| 8 | `!Send`/`!Sync` compatible with D4 | **yes** | `Terminal` is `!Send` ("`NonNull<TerminalImpl>` cannot be sent between threads safely") and `!Sync` ("`*mut c_void` cannot be shared"), matching D4's thread_local-on-the-GTK-thread plan. |

Rationale: an ABI-broken constructor, two data paths the panel uses today that the crate cannot
reach (row viewport-y, render-state colours), and a pin 1404 commits behind the commit the
panel was written and verified against. Own FFI also keeps D5 under pinwin's control: the
crate's `extern "C"` effect trampolines do not catch panics, so with the crate every callback
body would need its own guard and a missed one aborts the host; with own declarations the
trampoline is pinwin's and can wrap `catch_unwind` itself. Tasks 2 onward may start from this
result.

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

`Panel::start` lazily spawns the one process-lifetime `pinwin-gtk` thread (`std::thread`).
That thread calls `gtk::init` and layer-shell setup once; each `start` then creates and runs
its own `GtkApplication` (with its own distinct application id) and main loop on that shared
thread, and the thread is parked between panels. All GTK objects, terminal state and the pty
source live in a `thread_local` on it, so none need to be `Send`
(ghostty's terminal types are `!Send`/`!Sync`, D2 item 8). The host thread waits on an
`mpsc` channel for the start result.

Apply: the host thread posts a closure with `glib::MainContext::invoke`, carrying the layout
(a `Copy` value) and a `mpsc::sync_channel(1)` reply sender, then `recv_timeout(5 s)`. A
timeout is `PinwinError::Internal`. Dropping the receiver replaces the C refcount, since a
late reply fails harmlessly on send. Drop: post teardown (cancel animation, close surfaces,
quit the application loop) and wait for the reply; it does not join the parked thread (restart
caveat below). A process-wide `Mutex<bool>`/atomic enforces single-instance (`AlreadyRunning`).

Alternatives: run GTK on the host's main thread (rejected: the host owns its own loop, e.g.
crossterm, and the existing contract is a library that brings its own thread); `async`
channels (rejected, no runtime needed for a few calls); share state behind `Arc<Mutex<_>>`
with the host reading it directly (rejected: GTK objects are thread-bound).

Restart caveat (task 1.2 spike, `/tmp/ptr-spike/gtk-restart`; gtk4 crate 0.11.5, GTK
4.22.5): **confirmed, and the parked-thread fallback is adopted**. `gtk::init()` records the
first initialising thread and *panics* (`Attempted to initialize GTK from two different
threads`, `gtk4-0.11.5/src/rt.rs:138`) when a second thread calls it, so a thread per `start`
that is joined on `Drop` cannot restart the panel: the spike's first panel (its own thread)
ran and quit, the second (a new thread) panicked inside `gtk::init`. Decision: one
process-lifetime GTK thread, created lazily on the first `start` and parked between panels;
`start` and `Drop` post work to it with
`MainContext::invoke` exactly as above, and `Drop` never joins it. The spike verified two
consecutive panel lifecycles (`GtkApplication::run` → `quit`, distinct application ids) on the
same parked thread with a single `gtk::init`, both succeeding.

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
(which does not use `unwrap`) still closes — it posts teardown and waits for the reply — and
never joins the GTK thread. A panic while the host is blocked in
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
  resolved: the task 1.1 spike rejected the crate and the port uses own FFI for everything
  (D2), so the cost was known before any module was written.
- gtk-rs refuses GTK init on a second thread after restart → resolved: one process-lifetime
  GTK thread parked between panels (task 1.2 spike); the spike verified two consecutive panel
  lifecycles on the same parked thread.
- Behaviour drift in rendering (Pango/cairo glyph metrics, Nerd Font constraints, tween
  frame cache) is not caught by unit tests → one manual acceptance task at the end (no
  per-phase live testing, per the review decision); render code is ported line by line to keep
  the diff reviewable against the C.
- Panic inside a C callback unwinds through C frames → D5 guard on every trampoline.
- Build now needs Zig at build time for ghostty (and network for the ghostty fetch unless
  vendored) → documented in README and AGENTS.md; pinwin has no Zig source.
- Pinned ghostty commit moves → own FFI (D2) targets the single pinned commit
  `3a3047f6b62a791fd8b12d9f07a85b3d2160370b`; bumping it is a deliberate, reviewable change to
  `ghostty_sys`.
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
