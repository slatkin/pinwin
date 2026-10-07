//! The startup arguments one panel runs with (port-to-rust D6, D7): the
//! plain data a start carries across threads, split from the former GTK
//! side's lifecycle glue so the host-facing type does not depend on the
//! internals that consume it.

use std::os::fd::RawFd;

/// The startup arguments one panel runs with (D6, D7): the host-owned pty
/// master fd, the full layout, the keyboard mode and the optional focus
/// accent. `Copy`, so a start command can carry it across threads.
#[derive(Clone, Copy, Debug)]
pub struct Startup {
    fd: RawFd,
    layout: crate::layout::Layout,
    keyboard: crate::layout::Keyboard,
    accent: Option<crate::layout::Accent>,
}

impl Startup {
    /// Bundle the start arguments: the host-owned pty master fd (D7, never
    /// closed by the library), the full layout, the keyboard interactivity
    /// mode (fixed at start time) and the focus accent (`None` disables it).
    #[must_use]
    pub fn new(
        fd: RawFd,
        layout: crate::layout::Layout,
        keyboard: crate::layout::Keyboard,
        accent: Option<crate::layout::Accent>,
    ) -> Self {
        Startup {
            fd,
            layout,
            keyboard,
            accent,
        }
    }

    /// The host-owned pty master fd.
    #[must_use]
    pub fn fd(&self) -> RawFd {
        self.fd
    }

    /// The full startup layout.
    #[must_use]
    pub fn layout(&self) -> crate::layout::Layout {
        self.layout
    }

    /// The keyboard interactivity mode.
    #[must_use]
    pub fn keyboard(&self) -> crate::layout::Keyboard {
        self.keyboard
    }

    /// The focus accent, if any.
    #[must_use]
    pub fn accent(&self) -> Option<crate::layout::Accent> {
        self.accent
    }
}
