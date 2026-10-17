//! The harness that compiles the moved width-tween crop tests
//! (replace-terminal-with-app-host row 2.1).
//!
//! `src/proxy/crop.rs` is not part of a compiled module tree yet: the crate
//! is a binary-only stub until the proxy module arrives (D15), and a module
//! the binary does not use would be dead code. This file includes the module
//! so `cargo nextest run` still exercises its unit tests. Delete it when the
//! proxy module owns the file.

#[path = "../src/proxy/crop.rs"]
mod crop;
