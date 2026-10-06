//! The xdg-activation token (replace-gtk-with-wayland D4): the one-use
//! permission a compositor gives a program it launches, which
//! [`Panel::request_focus`](crate::Panel::request_focus) passes to the
//! compositor as an xdg-activation request.
//!
//! The type is the spec's validation point: a token is 1..=255 bytes of
//! visible ASCII (`!` to `~`), and the constructor rejects any other value,
//! so no such token reaches the panel (the "Invalid token" scenario). The
//! bytes live in a private field, so an out-of-range token is
//! unrepresentable (`port-to-rust` D6).

use std::fmt;

/// The longest activation token, in bytes (replace-gtk-with-wayland D4).
pub const MAX_TOKEN_BYTES: usize = 255;

/// A valid xdg-activation token: 1..=255 bytes of visible ASCII.
///
/// Built only through [`ActivationToken::new`]; the field is private, so a
/// token that failed validation cannot exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationToken {
    token: String,
}

impl ActivationToken {
    /// Build a token from `token`, or reject it: empty, longer than
    /// [`MAX_TOKEN_BYTES`] bytes, or holding a byte outside the visible
    /// ASCII range `0x21..=0x7E` — a space, a control character, DEL or a
    /// non-ASCII byte.
    ///
    /// # Errors
    /// [`ActivationTokenError`] names the failed rule.
    pub fn new(token: &str) -> Result<Self, ActivationTokenError> {
        if token.is_empty() {
            return Err(ActivationTokenError::Empty);
        }
        if token.len() > MAX_TOKEN_BYTES {
            return Err(ActivationTokenError::TooLong);
        }
        // `is_ascii_graphic` is exactly the visible ASCII range 0x21..=0x7E.
        if !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(ActivationTokenError::NotVisibleAscii);
        }
        Ok(ActivationToken {
            token: token.to_owned(),
        })
    }

    /// The token's bytes, as the xdg-activation request carries them.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.token
    }
}

/// Why [`ActivationToken::new`] rejected a token. `#[non_exhaustive]` so a
/// host cannot exhaustively match: the failure classes are the library's
/// (the style of [`crate::PinwinError`]). Implements
/// [`std::error::Error`] directly — no derive crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActivationTokenError {
    /// The token was empty.
    Empty,
    /// The token was longer than [`MAX_TOKEN_BYTES`] bytes.
    TooLong,
    /// The token held a byte outside the visible ASCII range `0x21..=0x7E`:
    /// a space, a control character, DEL or a non-ASCII byte.
    NotVisibleAscii,
}

impl fmt::Display for ActivationTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the activation token is empty"),
            Self::TooLong => f.write_str("the activation token is longer than 255 bytes"),
            Self::NotVisibleAscii => {
                f.write_str("the activation token holds a non-visible-ASCII byte")
            }
        }
    }
}

impl std::error::Error for ActivationTokenError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid token builds and reads back byte for byte.
    #[test]
    fn a_valid_token_builds() {
        let token = ActivationToken::new("niri-spawn:pinwin-172839").expect("valid token");
        assert_eq!(token.as_str(), "niri-spawn:pinwin-172839");
    }

    /// The length boundaries: exactly [`MAX_TOKEN_BYTES`] visible bytes
    /// build, one more is rejected as too long.
    #[test]
    fn the_length_boundaries_reject_256_bytes() {
        let max = "a".repeat(MAX_TOKEN_BYTES);
        assert!(ActivationToken::new(&max).is_ok(), "255 bytes build");
        let over = "a".repeat(MAX_TOKEN_BYTES + 1);
        assert_eq!(
            ActivationToken::new(&over),
            Err(ActivationTokenError::TooLong)
        );
    }

    /// An empty token is rejected.
    #[test]
    fn an_empty_token_is_rejected() {
        assert_eq!(ActivationToken::new(""), Err(ActivationTokenError::Empty));
    }

    /// A space is not visible ASCII and is rejected.
    #[test]
    fn a_space_is_rejected() {
        assert_eq!(
            ActivationToken::new("niri spawn"),
            Err(ActivationTokenError::NotVisibleAscii)
        );
    }

    /// A newline is not visible ASCII and is rejected.
    #[test]
    fn a_newline_is_rejected() {
        assert_eq!(
            ActivationToken::new("niri-spawn:1\n"),
            Err(ActivationTokenError::NotVisibleAscii)
        );
    }

    /// A non-ASCII byte is rejected.
    #[test]
    fn a_non_ascii_byte_is_rejected() {
        assert_eq!(
            ActivationToken::new("niri-spawn:é"),
            Err(ActivationTokenError::NotVisibleAscii)
        );
    }

    /// The range's edges: DEL (`0x7F`) is excluded, and the lowest and
    /// highest visible characters (`!`, `~`) are accepted.
    #[test]
    fn the_visible_ascii_range_edges_holds() {
        assert!(ActivationToken::new("!").is_ok());
        assert!(ActivationToken::new("~").is_ok());
        // DEL one past the top of the range.
        assert_eq!(
            ActivationToken::new("\u{7f}"),
            Err(ActivationTokenError::NotVisibleAscii)
        );
    }

    /// The error type displays a non-empty message and is a
    /// `std::error::Error`, like the other error types.
    #[test]
    fn every_variant_displays_and_implements_error() {
        for error in [
            ActivationTokenError::Empty,
            ActivationTokenError::TooLong,
            ActivationTokenError::NotVisibleAscii,
        ] {
            assert_ne!(error.to_string(), "");
            let boxed: Box<dyn std::error::Error> = Box::new(error);
            assert_ne!(boxed.to_string(), "");
        }
    }
}
