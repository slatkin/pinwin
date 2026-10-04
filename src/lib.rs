//! pinwin library (port-to-rust).
//!
//! the `Panel` API (D4, D6) and the remaining ported modules arrive in tasks
//! 3 and 4; everything below is scaffolding. `guard` is the shared D5 panic
//! guard; `ghostty_sys` holds the hand-written FFI against the pinned
//! libghostty-vt (D2).

pub mod anim;
pub mod fontconfig;
pub mod ghostty_sys;
pub mod guard;
pub mod input;
pub mod layout;
pub mod nerd_font;
pub mod pty;
pub mod render;
pub mod surfaces;
pub mod term;
