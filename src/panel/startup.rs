//! The startup arguments one panel runs with (port-to-rust D6, D7): the
//! plain data a start carries across threads, split from the GTK side's
//! glue (`gtk_side`) so the host-facing type does not depend on the
//! lifecycle internals that consume it.

use std::os::fd::RawFd;

/// The startup arguments one panel runs with (D6, D7): the host-owned pty
/// master fd, the full layout, the keyboard mode and the optional focus
/// accent. `Copy`, so a start command can carry it across threads.
#[derive(Clone, Copy, Debug)]
pub struct Startup {
    /// The host-owned pty master fd (D7). The library never closes it.
    pub fd: RawFd,
    /// The full startup layout.
    pub layout: crate::layout::Layout,
    /// The keyboard interactivity mode, fixed at start time.
    pub keyboard: crate::layout::Keyboard,
    /// The focus accent; `None` disables it.
    pub accent: Option<crate::layout::Accent>,
}
