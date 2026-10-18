//! The rules of a panel name (spec "Panel names"), moved out of the old
//! instance socket module by replace-terminal-with-app-host row 4.1: a name is
//! 1..=64 characters from `[A-Za-z0-9_-]`, it comes from `--name` or from
//! the file name of the command, and the error of an unusable default
//! names `--name`, the option that replaces it.
//!
//! The rules are pure (`port-to-rust` D10), and the newtype is the only
//! way a name reaches a socket path, a registry key or a reply
//! (`port-to-rust` D6), so every decision here is unit tested with no
//! daemon; `src/cli.rs` is its first user.

use std::ffi::OsStr;
use std::fmt;
use std::path::Path;

/// The name rules as every message states them, so the two errors cannot
/// drift apart: 1..=64 characters from `[A-Za-z0-9_-]` (spec "Panel
/// names").
const RULES: &str = "1..=64 characters from [A-Za-z0-9_-]";

/// A validated panel name (spec "Panel names"): 1..=64 characters from
/// `[A-Za-z0-9_-]`. Fields are private, so only [`PanelName::parse`] and
/// [`PanelName::from_command`] build a value: no caller holds an unchecked
/// name, and the character set keeps a name usable as a registry key, a
/// message and a socket-path component (`port-to-rust` D6).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PanelName(String);

impl PanelName {
    /// Validate `raw` into a [`PanelName`]: the name the user gave, by
    /// `--name` or in a request. The rejection is the typed [`InvalidName`]
    /// without a context label, so the caller adds its own (`pinwin:
    /// --name:` in the binary, `serve-instance-socket` D5).
    pub fn parse(raw: &str) -> Result<PanelName, InvalidName> {
        if valid(raw) {
            Ok(PanelName(raw.to_owned()))
        } else {
            Err(InvalidName(raw.to_owned()))
        }
    }

    /// The default name: the file name of the command (spec "Default
    /// name"), so `pinwin run -- /usr/bin/myeditor` names the panel
    /// `myeditor`. The directories are stripped and the file name follows
    /// the same rules as a given one; a command with no file name, or with
    /// one that is not a valid name, is an [`InvalidDefaultName`], whose
    /// message names `--name` (spec "Invalid default name").
    pub fn from_command(command: &OsStr) -> Result<PanelName, InvalidDefaultName> {
        let Some(file) = Path::new(command).file_name() else {
            return Err(InvalidDefaultName::NoFileName);
        };
        // A file name that is not UTF-8 cannot be inside the character set
        // either, and the error still has to print it.
        match file.to_str() {
            Some(file) if valid(file) => Ok(PanelName(file.to_owned())),
            _ => Err(InvalidDefaultName::Invalid {
                file: file.to_string_lossy().into_owned(),
            }),
        }
    }

    /// The name as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `raw` follows the name rules (spec "Panel names"). The allowed
/// set is ASCII, so counting bytes counts characters, and a name outside it
/// — including a non-ASCII one — is rejected.
fn valid(raw: &str) -> bool {
    (1..=64).contains(&raw.len())
        && raw
            .bytes()
            .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

/// How a [`PanelName`] prints: its own text, so a message can name the
/// panel directly.
impl fmt::Display for PanelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a given name is not a [`PanelName`] (spec "Panel names"): it carries
/// the rejected name, so its [`Display`](fmt::Display) can point at it; the
/// caller adds the option's context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidName(String);

impl fmt::Display for InvalidName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected a name of {RULES}, got '{}'", self.0)
    }
}

impl std::error::Error for InvalidName {}

/// Why the command gave no usable default name (spec "Invalid default
/// name"): the name the command implies cannot be used, and `--name` is the
/// option that replaces it, so every message names `--name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidDefaultName {
    /// The command has no file name to take, as `..` and `/` have none.
    NoFileName,
    /// The file name does not follow the name rules.
    Invalid {
        /// The file name as a message prints it, lossy where it is not
        /// UTF-8.
        file: String,
    },
}

impl fmt::Display for InvalidDefaultName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFileName => f.write_str(
                "the command has no file name to use as the panel name; \
                 pass --name to choose one",
            ),
            Self::Invalid { file } => write!(
                f,
                "the command's file name '{file}' is not a panel name; \
                 expected {RULES}; pass --name to choose one"
            ),
        }
    }
}

impl std::error::Error for InvalidDefaultName {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    /// The name rules hold: the whole allowed set, the 64-character
    /// boundary, and the typed rejection that carries the rejected name
    /// for its message.
    #[test]
    fn the_panel_name_validates() {
        let name = PanelName::parse("notes").expect("valid");
        assert_eq!(name.as_str(), "notes");
        assert_eq!(name.to_string(), "notes");

        // The whole allowed character set, and the 64-character boundary.
        PanelName::parse("A-z_09-").expect("the full allowed set");
        PanelName::parse(&"a".repeat(64)).expect("64 characters");

        // Empty, too long, and characters outside the set: the typed
        // error, whose message carries the rejected name (the caller adds
        // its `pinwin: <option>:` prefix).
        for raw in ["", &"a".repeat(65), "a/b", "a b", "a.b", "ä"] {
            let error = PanelName::parse(raw).expect_err(raw);
            assert_eq!(error, InvalidName(raw.to_owned()), "raw = {raw:?}");
            assert_eq!(
                error.to_string(),
                format!("expected a name of 1..=64 characters from [A-Za-z0-9_-], got '{raw}'"),
                "raw = {raw:?}"
            );
        }
    }

    /// Without `--name` the name is the file name of the command, with the
    /// directories stripped (spec "Default name").
    #[test]
    fn the_default_name_is_the_command_file_name() {
        for (command, expected) in [
            ("/usr/bin/myeditor", "myeditor"),
            ("myeditor", "myeditor"),
            ("/usr/bin/myeditor/", "myeditor"),
            ("./my-editor_2", "my-editor_2"),
            ("a/../b", "b"),
        ] {
            let name = PanelName::from_command(OsStr::new(command)).expect(command);
            assert_eq!(name.as_str(), expected, "command = {command:?}");
        }
    }

    /// A default name that breaks the rules is the typed error, and its
    /// message names `--name`, so the user knows what to do instead (spec
    /// "Invalid default name").
    #[test]
    fn an_invalid_default_name_names_the_name_option() {
        let error = PanelName::from_command(OsStr::new("my.app")).expect_err("a dot");
        assert_eq!(
            error,
            InvalidDefaultName::Invalid {
                file: "my.app".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "the command's file name 'my.app' is not a panel name; expected 1..=64 characters \
             from [A-Za-z0-9_-]; pass --name to choose one"
        );

        // A command without a file name has no default name either.
        for command in ["", "/", "..", "."] {
            let error = PanelName::from_command(OsStr::new(command)).expect_err(command);
            assert_eq!(
                error,
                InvalidDefaultName::NoFileName,
                "command = {command:?}"
            );
            assert!(
                error.to_string().contains("--name"),
                "the message names --name: {error}"
            );
        }

        // A file name that is not UTF-8 cannot be a name; the message still
        // prints it, and still names the option.
        let raw = OsStr::from_bytes(b"m\xff");
        let error = PanelName::from_command(raw).expect_err("not UTF-8");
        assert!(
            matches!(&error, InvalidDefaultName::Invalid { file } if file == "m\u{fffd}"),
            "the file name prints lossily: {error}"
        );
        assert!(
            error.to_string().contains("--name"),
            "the message names --name: {error}"
        );
    }
}
