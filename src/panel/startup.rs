//! The startup arguments one panel runs with (port-to-rust D6, D7): the
//! plain data a start carries across threads, split from the panel-thread
//! glue that consumes it so the host-facing type does not depend on those
//! internals.

use std::os::fd::RawFd;

use crate::instance::InstanceSocket;

/// The startup arguments one panel runs with (D6, D7): the host-owned pty
/// master fd, the full layout, the keyboard mode, the optional focus
/// accent and the optional bound instance socket. Neither `Copy` nor
/// `Clone`, because the instance socket is an owned fd with one owner
/// (serve-instance-socket D1); a start command carries it by move.
#[derive(Debug)]
pub struct Startup {
    fd: RawFd,
    layout: crate::layout::Layout,
    keyboard: crate::layout::Keyboard,
    accent: Option<crate::layout::Accent>,
    /// The bound instance socket, if the host opted in (D1). A start
    /// without one listens on nothing. `Panel::start` takes it out before
    /// it builds the start command — the panel thread never sees it.
    instance: Option<InstanceSocket>,
}

impl Startup {
    /// Bundle the start arguments: the host-owned pty master fd (D7, never
    /// closed by the library), the full layout, the keyboard interactivity
    /// mode (fixed at start time) and the focus accent (`None` disables it).
    /// No instance socket: the panel listens on nothing.
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
            instance: None,
        }
    }

    /// Hand the bound instance socket to the start
    /// (`serve-instance-socket` D1): a panel started with a socket serves
    /// `toggle` and `show` requests until the handle drops; a start
    /// without a socket listens on nothing. Consumes and returns the
    /// startup, builder-style.
    #[must_use]
    pub fn with_instance(mut self, socket: InstanceSocket) -> Self {
        self.instance = Some(socket);
        self
    }

    /// Take the optional instance socket out, for
    /// [`Panel::start`](crate::panel::Panel::start) (D1: the socket never
    /// reaches the panel thread; the host side of the start owns it and
    /// serves it through the detached listener).
    pub(crate) fn take_instance(&mut self) -> Option<InstanceSocket> {
        self.instance.take()
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
