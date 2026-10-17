//! The harness that compiles the config-file tests
//! (replace-terminal-with-app-host row 4.2).
//!
//! `src/config.rs` is not part of a compiled module tree yet: the option
//! parser that reads the file arrives with row 4.3 and the daemon with row
//! 5.2 (D15), and a module the binary does not use would be dead code. This
//! file includes the module and the two modules whose types it names —
//! `geometry` for the edge, size, reserve mode and gutters, and `name` for
//! the panel name a table is looked up by — so `cargo nextest run` still
//! exercises the config rules. Delete it when row 4.3 owns the module.
//!
//! The modules are `pub` so the dead-code lint does not report the parts of
//! the included modules these tests do not reach.

#[path = "../src/geometry.rs"]
pub mod geometry;

#[path = "../src/name.rs"]
pub mod name;

#[path = "../src/config.rs"]
pub mod config;
