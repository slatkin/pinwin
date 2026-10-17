//! The harness that compiles the panel-name tests
//! (replace-terminal-with-app-host row 4.1).
//!
//! `src/name.rs` is not part of a compiled module tree yet: the crate is a
//! binary-only stub until the parser, the IPC and the daemon arrive (D15),
//! and a module the binary does not use would be dead code. This file
//! includes the module so `cargo nextest run` still exercises the name
//! rules; `src/cli.rs` uses them and `tests/cli.rs` also compiles them.

#[path = "../src/name.rs"]
mod name;
