# Design

## Context

`src/term/` is the only module that calls `ghostty_sys`. The rest of the crate uses only
`term::Terminal` and `term::cells`. So the port stays inside `src/term/`. The one exception is `src/lib.rs`, which declares `pub mod ghostty_sys`.

The port-to-rust D2 spike rejected `libghostty-vt` v0.2.2. Its blockers and their state at the
head of Uzaaft/libghostty-rs#85 (`5c5a454`, which sits on top of #83, #98, #84 and #99) are:

| Spike blocker | State at `5c5a454` |
|---|---|
| Pin 1,404 commits behind ours | Pin `0081d45`, 44 commits ahead of `3a3047f6` |
| No row viewport Y | `RowIteration::viewport_y()` (`render.rs:916`) |
| Constructor ABI mismatch at our pin | Gone: we use the crate's own pin |
| Needs Zig 0.15.2 | Needs Zig 0.16 |
| Trampolines do not catch panics | Still true (`terminal.rs:2112` documents the abort) |
| `encode_to_vec` reserves too little | Still true (`key.rs:75`, `mouse.rs:81`) |

A read-only gap analysis of master (`scratchpad/ghostty-gap-67486b54.report.txt`, not checked
in) maps every `ghostty_sys` item that `src/term/` uses to a safe crate item. The row viewport Y
was the only missing item, and #85 adds it.

## Goals / Non-Goals

**Goals:**

- Delete `src/ghostty_sys/` and `build.rs`, and use the crate for all libghostty-vt access.
- Keep the output of `term::Terminal` and `term::cells` identical for the same input bytes.
- Keep D5: no panic in a terminal callback reaches C code or the host.

**Non-Goals:**

- Using new crate features (selection, search, dirty-row skipping, render hold).
- Changing `term`'s public surface beyond what removing raw handles requires.
- Upstreaming `catch_unwind` or the `encode_to_vec` fix to libghostty-rs. That is welcome but
  is not this change.

## Decisions

### A1. Dependency source and gate

`libghostty-vt = { git = "https://github.com/Uzaaft/libghostty-rs", rev = "<sha>" }`, where
`<sha>` is the first master commit that contains both #84 and #85. The default features stay
(`kitty-graphics`). The `png` feature stays off because pinwin decodes PNG with gdk-pixbuf.
When a crates.io release contains both PRs, switch to the version. Task 1.1 is a gate: if
either PR is not merged, stop and report.

Alternative: depend on the PR branch `stack/ghostty-render`. Rejected because stacked branches
are force-pushed and can disappear after merge.

### A2. Build configuration through `.cargo/config.toml`

The crate's build script reads `LIBGHOSTTY_VT_SYS_OPTIMIZE` and `LIBGHOSTTY_VT_SYS_CPU` and
declares `rerun-if-env-changed` for both. A checked-in `.cargo/config.toml` sets them in
`[env]` to `ReleaseSafe` and `x86_64_v2`, the current `build.rs` values. Without the override,
the crate picks `Debug` for the dev profile, which the spec forbids.

Limit: `cargo install --git` run outside the repository does not read this file. Such a build
then gets the crate's defaults: `ReleaseFast` in release and CPU `baseline`. Neither is Debug,
and `baseline` is a lower CPU floor, so the build is still correct.

### A3. Panic guards inside every closure

The crate's trampolines are `extern "C"` without `catch_unwind`, so a panic aborts the process.
Every closure pinwin hands to the crate runs its whole body through `guard::guard_default`
with the terminal's poison latch: `on_pty_write`, `on_size`, `on_device_attributes`, and the
`DecodePng` forwarder. On a panic, the closure returns the safe default (no write, `None`,
decode failure) and latches poison, as the current trampolines do.

Alternative: patch the crate. Rejected because of the non-goal above.

### A4. PNG decoder routing

`kitty::graphics::set_png_decoder` installs one process-global decoder, which matches the
current `install_sys_hooks` design. pinwin installs one forwarder inside a `Once`. The forwarder
keeps the per-thread `DECODE_CONTEXTS` registry to find the live terminal's `PngDecoder`. It
keeps the check that the RGBA length equals `width * height * 4`, and it allocates the output
with `Bytes::new_with_alloc` from the allocator the crate passes in.

### A5. Ownership in `term::Terminal`

`term::Terminal` owns `libghostty_vt::Terminal<'static, 'static>` (default allocator) and the
`RenderState`, `RowIterator`, `CellIterator` and `PlacementIterator`. Each cell or image pass
calls `update()` once and refreshes its iterator from that snapshot, so the borrow checker sees
one snapshot per pass. Callback closures share state through `Rc<RefCell<_>>` captures instead
of the userdata pointer. The lazy init path and the `GHOSTTY_REJECTED` sentinel go away: a
failed `Terminal::new` maps to `PinwinError::Internal` and keeps the previous grid, as the spec
requires. Early pty bytes are still buffered until the terminal exists.

The raw-handle accessors on `term::Terminal` (`terminal()`, `key_encoder()`, `key_event()`,
`mouse_encoder()`, `mouse_event()`) are only used inside `src/term/`. They become private or
are removed.

### A6. Encoders

Use `encode(&mut [u8])` with a stack buffer. On `Error::OutOfSpace { required }`, retry once
into a `Vec` of exactly `required` bytes. Do not use `encode_to_vec`, because its grow path
reserves too little. An unshifted codepoint of `0` means no codepoint, so pinwin skips
`set_unshifted_codepoint` in that case. UTF-8 text goes through `set_utf8(Some(..))`, which
allocates a `String` for each key event. That cost is accepted.

### A7. Golden output check

Before any port code, task 2 records a golden fixture from the current FFI. It feeds a fixed
VT byte stream through `term::Terminal` and records the `term::cells` output. The output holds
every `Cell` (`x`, `y`, text, width, style, colors), the cursor, the colors, and the kitty
image placements. The stream covers wide characters, styles, 24-bit color, a scrolled viewport, and
one kitty PNG placement. After the port, the same test must produce the same output. This
replaces the `ghostty_sys` layout tests, which check headers that no longer exist here.

### A8. `pub mod ghostty_sys` removal

`src/lib.rs` exports `ghostty_sys` publicly. Removing it shrinks the library surface. The
`Panel` API in the spec is unchanged. No `libghostty_vt` type is re-exported.

## Risks / Trade-offs

- [Unstable upstream API] → The `rev` pin freezes it. A pin bump is a separate change that runs
  the golden test.
- [Ghostty behavior drift across 66 commits] → The golden test catches drift in cell output.
  Input encoding drift is caught by the existing `term/input.rs` tests.
- [A missed closure guard aborts the host] → Task 4.2 adds one test per closure that panics
  inside it and asserts poison without an abort.
- [Upstream changes the API before merge] → Task 1.1 re-checks the gap list against the merged
  commit before porting.
- [We lose control of the ghostty pin] → The user accepts this.
