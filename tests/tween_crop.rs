//! The harness that compiles the moved size-tween crop tests
//! (replace-terminal-with-app-host row 2.1).
//!
//! `src/proxy/crop.rs` is not part of a compiled module tree yet: the crate
//! is a binary-only stub until the proxy module arrives (D15), and a module
//! the binary does not use would be dead code. This file includes the module
//! so `cargo nextest run` still exercises its unit tests, and the geometry
//! core it names the docking edge with (D9). Delete it when the proxy module
//! owns the file.

#[allow(
    dead_code,
    reason = "the crop harness compiles the geometry core for its Edge type; rows 4.3 and 6.4 give the accessors users"
)]
#[path = "../src/geometry.rs"]
mod geometry;

#[path = "../src/proxy/crop.rs"]
mod crop;
