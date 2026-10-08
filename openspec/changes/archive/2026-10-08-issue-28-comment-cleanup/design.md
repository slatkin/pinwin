# Design

## Context

See `proposal.md` for motivation. Current state (measured at `ba95cde`):

- 408 `row X.Y` / `dispatch DN` comment references across 30 files, heavily
  concentrated in `src/panel/wayland_side/` (apply, tween, frame log, seat,
  surfaces, glue). Many sit alongside sanctioned `replace-gtk-with-wayland
  Dn` / `port-to-rust Dn` citations that already meet the AGENTS.md comment
  standard — the cleanup must preserve those.
- ~171 prose-style GTK/GDK/glib references (`GTK side's dispatch_apply`,
  `GDK path's on_mouse`, `glib twin`, `focus socket`) vs 103 sanctioned
  `replace-gtk-with-wayland` citations and 29 `GTK-free` uses. The prose
  describes removed code; the citations describe live design decisions.
- `src/panel.rs:28` declares `pub mod wayland_side`; inside
  `src/panel/wayland_side.rs` eight children (`buffers`, `commands`, `crop`,
  `present`, `renderer`, `seat`, `sizing`, `tween`) are `pub`, the rest
  already `pub(crate)`. All `use` references resolve inside `src/panel*`
  (plus doc-only mentions in `src/pty.rs`, `src/term/input.rs`); nothing in
  `examples/`, tests outside the tree, or the public `Panel`/`Startup`/
  `PinwinError` API names these paths.
- `BytePath` (`src/panel/wayland_side/glue.rs:96`) is still an anonymous
  `pub(crate)` 4-tuple with its meaning split across an orphaned paragraph
  above the alias and a second paragraph below. The issue's `gridsink`
  half is stale: no gridsink item exists.

## Goals / Non-Goals

**Goals:**

- Every remaining comment meets the AGENTS.md standard (niri, Wayland or
  timing behaviour not obvious from the code; D-numbers point at designs).
- `wayland_side` is crate-internal so `dead_code` fires again; any newly
  surfaced dead item is either used, deleted, or kept with a stated reason.
- `BytePath` is a named shape, not an anonymous tuple.

**Non-Goals:**

- No behaviour, spec, dependency, config, or niri-configuration change.
- No work on the ~1000-item clippy backlog beyond items this change itself
  surfaces (and no new `allow`s: each needs per-instance user approval).
- No re-architecture of the panel thread, render passes, or pty handling.

## Decisions

1. **Per-site comment rewrites, not bulk deletion.** Many row refs guard
   real timing/ordering knowledge (tween staging `row 6.2`, first-callback
   Wayland order `row 9.8`, hidden-apply rule `row 9.1`). Bulk-deleting
   would lose it; bulk-regex would produce nonsense. Each site is rewritten
   to state the current behaviour and cite the design
   (`replace-gtk-with-wayland Dn`), or deleted if the code is now obvious.
   Alternative (delete all row refs): rejected — loses timing knowledge the
   standard explicitly wants kept.
2. **GTK prose rewritten to present-tense Wayland behaviour; citations
   kept.** Rule of thumb: `the GTK side's X` becomes a sentence about what
   the current code does and why (e.g. `should_animate` rule, reject-before-
   mutate, closed-surface verdict). The `frame_log` wire-compat fact (field
   names kept so parsing scripts still work) stays but is reframed as log-
   format stability, not GTK history. The 103 `replace-gtk-with-wayland`
   citations and `GTK-free` uses stay where they already meet the standard.
3. **`BytePath` becomes a struct with named private fields**, constructed by
   the existing `byte_path()` function; use sites destructure by name.
   Alternative (keep the alias, merge the two doc paragraphs): rejected —
   the issue explicitly calls out the anonymous tuple, and named fields are
   the D6-style fix (invariant lives in the field type). The struct stays
   `pub(crate)`; fields stay private with the constructor as the only
   builder, matching the project's public-type discipline.
4. **Visibility narrowed outside-in.** First `pub mod wayland_side` →
   `pub(crate)`, then inner `pub` children → `pub(crate)` wherever the
   compiler still passes. Newly flagged dead items are deleted only if
   genuinely unused; anything kept gets a reason in a comment. No `allow`
   attributes are added.
5. **Work order: visibility → BytePath → comments.** Narrowing first
   surfaces dead items before comment work, so comments are written once
   against the final shapes. Comment rewrites go module by module
   (`apply`, `tween`/`tween_draw`, `frame_log`/`watchdog`, `seat`, `glue`,
   then the long tail) to keep diffs reviewable.

## Risks / Trade-offs

- [Risk] Rewrites lose timing knowledge embedded in row refs → Mitigation:
  per-site rewrites preserving D-numbers; reviewer diff-checks for deleted
  behavioural claims; `cargo test` must stay green throughout.
- [Risk] `pub(crate)` breaks an external user of the documented
  `pinwin::panel::wayland_side` path → Mitigation: accepted and declared
  **BREAKING** in the proposal; the supported API is `Panel`/`Startup`/
  `PinwinError`, which is unchanged.
- [Risk] Struct-ifying `BytePath` touches all glue use sites → Mitigation:
  it is `pub(crate)` with few construction/use sites in one module; the
  compiler enumerates them.
- [Risk] 800-line file budget (`make check-code-file-lines`) on touched
  files → Mitigation: rewrites shorten comments on balance; check the gate
  before finishing.

## Migration Plan

None: no runtime, config, or protocol change. Verification is the standard
gate set: `cargo build`, `cargo test` (incl. `#[ignore]`d display tests
only if a niri session is available), `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`,
`make check-code-file-lines`, plus the demo exercise per AGENTS.md
(`cargo run --example demo`; width animation both ways, focus accent).
Rollback is a plain revert; commits stay behaviour-preserving so bisection
is unaffected.
