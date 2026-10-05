//! The `Panel` API's error type (port-to-rust D6): the former `PINWIN_ERR_*`
//! result codes, with the argument classes the ported types make
//! unrepresentable gone.

use std::fmt;

/// A call on the [`Panel`](crate::Panel) API failed.
///
/// `#[non_exhaustive]` so a host cannot exhaustively match: the set of
/// failures is the library's, not the host's (port-to-rust D6). Implements
/// [`std::error::Error`] directly — no derive crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinwinError {
    /// The layout the panel refuses (checked-arithmetic overflow, a
    /// reservation sum below zero, no room for one terminal row, or no output
    /// width left for other windows — including a monitor whose metrics are
    /// degenerate, the former `PINWIN_GEOM_ERR_METRICS`, D6). The applied
    /// layout is unchanged.
    InvalidLayout,
    /// The pty master fd supplied at start is not an open descriptor (D7);
    /// nothing opened.
    InvalidFd,
    /// No GTK display, or the session's compositor lacks wlr-layer-shell
    /// (the spec's "Wayland layer-shell is required" case); nothing opened.
    NoDisplay,
    /// A panel is already running: at most one exists per process.
    AlreadyRunning,
    /// The handle's panel is no longer live (its GTK side ended on its own).
    NotRunning,
    /// An unexpected failure: a caught panic inside the library, a wedged GTK
    /// side, or a terminal grid that could not be allocated.
    Internal,
}

impl fmt::Display for PinwinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLayout => f.write_str("the layout is invalid for this monitor"),
            Self::InvalidFd => f.write_str("the pty fd is not an open descriptor"),
            Self::NoDisplay => f.write_str("no display or no wlr-layer-shell support"),
            Self::AlreadyRunning => f.write_str("a panel is already running"),
            Self::NotRunning => f.write_str("the panel is no longer running"),
            Self::Internal => f.write_str("internal failure"),
        }
    }
}

impl std::error::Error for PinwinError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant displays a non-empty message and the type is a
    /// `std::error::Error`.
    #[test]
    fn every_variant_displays_and_implements_error() {
        let errors = [
            PinwinError::InvalidLayout,
            PinwinError::InvalidFd,
            PinwinError::NoDisplay,
            PinwinError::AlreadyRunning,
            PinwinError::NotRunning,
            PinwinError::Internal,
        ];
        for error in errors {
            assert_ne!(error.to_string(), "");
            // The std::error::Error supertrait is usable through a trait
            // object, which is what `?`/`Box<dyn Error>` consumers rely on.
            let boxed: Box<dyn std::error::Error> = Box::new(error);
            assert_ne!(boxed.to_string(), "");
        }
    }

    /// `#[non_exhaustive]`: a host's match needs a wildcard arm and cannot
    /// rely on the variant list staying complete.
    #[test]
    fn matching_needs_a_wildcard_arm() {
        let error = PinwinError::NoDisplay;
        let described = match error {
            PinwinError::NoDisplay => "display",
            // A host match never lists the whole enum: new failure modes
            // must not break a downstream build.
            _ => "other failure",
        };
        assert_eq!(described, "display");
    }
}
