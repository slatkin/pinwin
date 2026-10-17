//! The daemon's replies (replace-terminal-with-app-host D13): the `ok`
//! flag, the seven error kinds, and the `error` object a client prints.
//!
//! A reply is one JSON object on one line: `{"ok":true}`, or
//! `{"ok":false,"error":{"kind","message"}}` (spec "Request and reply
//! format"). The kind is a closed set from D13, so a client can match it and
//! print the message it carries.

use serde::{Deserialize, Serialize, Serializer};

/// The error kind of a failed reply (D13, spec "Request and reply format"):
/// the closed set the daemon uses and the `kind` a client prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The line is not a request: not JSON, an unknown `cmd`, a missing or
    /// invalid panel, a field of the wrong type, or an over-long line.
    InvalidRequest,
    /// A known command carries a setting value outside the grammar.
    InvalidValue,
    /// The output cannot hold the geometry the command asks for.
    InvalidLayout,
    /// No panel has the requested name.
    NoPanel,
    /// A panel of that name already exists.
    DuplicateName,
    /// The config file is invalid (spec "Invalid config file").
    InvalidConfig,
    /// The daemon could not carry the command out.
    Internal,
}

/// The `error` object of a failed reply: its kind and the message a client
/// prints (D13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReply {
    kind: ErrorKind,
    message: String,
}

impl ErrorReply {
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// The kind the client prints.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The message the client prints.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// The daemon's answer to one request (D13, spec "Request and reply
/// format"): `{"ok":true}` or
/// `{"ok":false,"error":{"kind","message"}}` on one line. A success reply
/// may carry more fields; the client keeps only the ok flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The command succeeded.
    Ok,
    /// The command failed with a kind and a message.
    Error(ErrorReply),
}

impl Reply {
    /// The success reply.
    #[must_use]
    pub const fn ok() -> Self {
        Self::Ok
    }

    /// A failed reply with `kind` and `message`.
    #[must_use]
    pub fn error(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::Error(ErrorReply::new(kind, message))
    }

    /// Whether the command succeeded.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }

    /// The error object of a failed reply, or `None` for a success.
    #[must_use]
    pub const fn as_error(&self) -> Option<&ErrorReply> {
        match self {
            Self::Error(error) => Some(error),
            Self::Ok => None,
        }
    }

    /// The line the daemon writes: the JSON object plus its newline.
    ///
    /// # Panics
    ///
    /// Only if a reply's `Serialize` fails, which no wire type here can do.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a reply is serializable");
        line.push('\n');
        line
    }

    /// Parse one reply line, as the client reads it.
    pub(super) fn parse(line: &[u8]) -> Result<Self, String> {
        let reply: ReplyLine =
            serde_json::from_slice(line).map_err(|error| format!("not a reply: {error}"))?;
        if reply.ok {
            return Ok(Self::Ok);
        }
        reply
            .error
            .map(Self::Error)
            .ok_or_else(|| "the failed reply has no error object".to_owned())
    }
}

/// One reply on the wire: the `ok` flag plus, for a failure, the error
/// object. The success fields a later reply carries (spec "Status") are
/// ignored here.
#[derive(Serialize)]
struct ReplyJson<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a ErrorReply>,
}

impl Serialize for Reply {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let json = match self {
            Self::Ok => ReplyJson {
                ok: true,
                error: None,
            },
            Self::Error(error) => ReplyJson {
                ok: false,
                error: Some(error),
            },
        };
        json.serialize(serializer)
    }
}

/// A reply as it arrives: the `ok` flag and, for a failure, the error
/// object.
#[derive(Deserialize)]
struct ReplyLine {
    ok: bool,
    #[serde(default)]
    error: Option<ErrorReply>,
}
