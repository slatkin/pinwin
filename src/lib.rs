//! pinwin library (port-to-rust).
//!
//! This is the crate skeleton from task 2.1/2.2. The `Panel` API (D4, D6) and
//! the remaining ported modules arrive in tasks 3 and 4; everything below is
//! scaffolding. `ghostty_sys` holds the hand-written FFI against the pinned
//! libghostty-vt (D2).

pub mod fontconfig;
pub mod ghostty_sys;
pub mod layout;
pub mod nerd_font;
pub mod pty;
pub mod term;
