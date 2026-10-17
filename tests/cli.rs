//! The harness that compiles the command-line tests
//! (replace-terminal-with-app-host row 4.3).
//!
//! `src/cli.rs` is not part of a compiled module tree yet: `src/main.rs`
//! still prints the usage and the subcommand dispatch arrives with the
//! daemon rows (D15), and a module the binary does not use would be dead
//! code. This file includes the parser and the modules whose types it
//! names, so `cargo nextest run` still exercises the parsing rules.
//!
//! The modules are `pub` so the dead-code lint does not report the parts
//! of the included modules these tests do not reach.

#[path = "../src/geometry.rs"]
pub mod geometry;

#[path = "../src/name.rs"]
pub mod name;

#[path = "../src/config.rs"]
pub mod config;

#[path = "../src/cli.rs"]
pub mod cli;
