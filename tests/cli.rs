//! The harness that compiles the command-line tests
//! (replace-terminal-with-app-host row 4.3).
//!
//! `src/main.rs` compiles `src/cli.rs` for the subcommand dispatch (row
//! 5.6), but an integration test cannot name the binary's modules. This file
//! includes the parser and the modules whose types it names, so `cargo
//! nextest run` exercises the parsing rules directly.
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
