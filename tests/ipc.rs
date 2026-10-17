//! The harness that compiles the IPC wire-format tests
//! (replace-terminal-with-app-host row 5.1).
//!
//! `src/ipc.rs` is not part of a compiled module tree yet: `src/main.rs`
//! still prints the usage and the daemon and the client dispatch arrive with
//! the rows after 5.1 (D15), and a module the binary does not use would be
//! dead code. This file includes the wire module, the parser whose
//! subcommands it mirrors and the modules their value types name, so
//! `cargo nextest run` still exercises the request, reply, line-limit and
//! client rules without a daemon.
//!
//! The modules are `pub` so the dead-code lint does not report the parts of
//! the included modules these tests do not reach.

#[path = "../src/geometry.rs"]
pub mod geometry;

#[path = "../src/name.rs"]
pub mod name;

#[path = "../src/config.rs"]
pub mod config;

#[path = "../src/cli.rs"]
pub mod cli;

#[path = "../src/ipc.rs"]
pub mod ipc;
