//! pinwin library (port-to-rust).
//!
//! The public API is the [`Panel`] handle and its [`PinwinError`] results
//! (D4, D6): start takes a host-owned pty fd and a full layout, applies run
//! through the handle, and dropping it closes the panel. The `panel` module
//! owns the panel-thread lifecycle; the ported modules below are its parts —
//! `guard` is the shared D5 panic guard, and `term` wraps the `libghostty-vt`
//! crate (adopt-libghostty-rs).

pub mod anim;
pub mod fontconfig;
pub mod guard;
pub mod instance;
pub mod layout;
pub mod nerd_font;
pub mod panel;
pub mod pty;
pub mod render;
pub mod surfaces;
pub mod term;

pub use panel::{Panel, PinwinError};
