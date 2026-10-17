//! The daemon's replies (replace-terminal-with-app-host D13): the `ok`
//! flag, the seven error kinds, and the `error` object a client prints.
//!
//! A reply is one JSON object on one line: `{"ok":true}`, or
//! `{"ok":false,"error":{"kind","message"}}` (spec "Request and reply
//! format"). The kind is a closed set from D13, so a client can match it and
//! print the message it carries. A `status` success also carries the
//! daemon's process ID and its panels ([`StatusReply`], spec "Status").
//!
//! [`PanelStatus`] is the part of a status entry a panel reports; the daemon
//! adds the panel's name and the app's process ID from its registry, so the
//! panel thread and the wire type share one shape.

use serde::{Deserialize, Serialize, Serializer};

use crate::config::{Keyboard, Layer, PanelSettings};
use crate::geometry::{Edge, ReserveMode, Size};
use crate::name::PanelName;

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
/// format"): `{"ok":true}`, `{"ok":true,"pid":...,"panels":[...]}` for a
/// carried-out status, or `{"ok":false,"error":{"kind","message"}}` on one
/// line. The parsed value is what the client reads; the line it came in is
/// kept beside it in [`Answer`](super::Answer), because `pinwin status`
/// prints the daemon's JSON unchanged (spec "Command-line exit codes").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The command succeeded.
    Ok,
    /// The command succeeded and reports the daemon's panels (spec
    /// "Status").
    Status(StatusReply),
    /// The command failed with a kind and a message.
    Error(ErrorReply),
}

impl Reply {
    /// The success reply.
    #[must_use]
    pub const fn ok() -> Self {
        Self::Ok
    }

    /// The success reply of a `status` request: the daemon's process ID and
    /// its panels (spec "Status").
    #[must_use]
    pub const fn status(pid: u32, panels: Vec<PanelEntry>) -> Self {
        Self::Status(StatusReply { pid, panels })
    }

    /// A failed reply with `kind` and `message`.
    #[must_use]
    pub fn error(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::Error(ErrorReply::new(kind, message))
    }

    /// Whether the command succeeded.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok | Self::Status(_))
    }

    /// The status payload, or `None` for another reply.
    #[must_use]
    pub const fn as_status(&self) -> Option<&StatusReply> {
        match self {
            Self::Status(status) => Some(status),
            Self::Ok | Self::Error(_) => None,
        }
    }

    /// The error object of a failed reply, or `None` for a success.
    #[must_use]
    pub const fn as_error(&self) -> Option<&ErrorReply> {
        match self {
            Self::Error(error) => Some(error),
            Self::Ok | Self::Status(_) => None,
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
/// object, and for a status, the daemon's process ID and its panels. The
/// reader keeps the parsed reply only; the line itself is kept beside it for
/// a client that prints it.
#[derive(Serialize)]
struct ReplyJson<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a ErrorReply>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    panels: Option<&'a [PanelEntry]>,
}

impl Serialize for Reply {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let json = match self {
            Self::Ok => ReplyJson {
                ok: true,
                error: None,
                pid: None,
                panels: None,
            },
            Self::Status(status) => ReplyJson {
                ok: true,
                error: None,
                pid: Some(status.pid),
                panels: Some(&status.panels),
            },
            Self::Error(error) => ReplyJson {
                ok: false,
                error: Some(error),
                pid: None,
                panels: None,
            },
        };
        json.serialize(serializer)
    }
}

/// A `status` reply (spec "Status"): the daemon's process ID and its
/// panels, every panel or the one the request names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusReply {
    pid: u32,
    panels: Vec<PanelEntry>,
}

impl StatusReply {
    /// The daemon's process ID.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// The panels, in the order they started.
    #[must_use]
    pub fn panels(&self) -> &[PanelEntry] {
        &self.panels
    }
}

/// One panel of a status reply (spec "Status"): its name, the app's
/// process ID, whether it is shown, whether it holds a window, its edge,
/// size, resolved width in pixels, reserve mode, layer, keyboard mode and
/// output name. The output name is null while the panel lets the compositor
/// choose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PanelEntry {
    name: PanelName,
    pid: u32,
    shown: bool,
    has_window: bool,
    edge: Edge,
    size: Size,
    width: u32,
    reserve: ReserveMode,
    layer: Layer,
    keyboard: Keyboard,
    output: Option<String>,
}

impl PanelEntry {
    /// The entry of the panel `name`, whose app has process ID `pid`, from
    /// what the panel reports about itself.
    #[must_use]
    pub fn new(name: PanelName, pid: u32, status: PanelStatus) -> Self {
        let settings = status.settings;
        Self {
            name,
            pid,
            shown: status.shown,
            has_window: status.has_window,
            edge: settings.edge(),
            size: settings.size(),
            width: status.width,
            reserve: settings.reserve(),
            layer: settings.layer(),
            keyboard: settings.keyboard(),
            output: settings.output().map(str::to_owned),
        }
    }

    #[must_use]
    pub const fn name(&self) -> &PanelName {
        &self.name
    }

    /// The app's process ID.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    #[must_use]
    pub const fn shown(&self) -> bool {
        self.shown
    }

    /// Whether the panel's window is in it.
    #[must_use]
    pub const fn has_window(&self) -> bool {
        self.has_window
    }

    #[must_use]
    pub const fn edge(&self) -> Edge {
        self.edge
    }

    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    /// The size across the edge, resolved to pixels on the panel's output.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn reserve(&self) -> ReserveMode {
        self.reserve
    }

    #[must_use]
    pub const fn layer(&self) -> Layer {
        self.layer
    }

    #[must_use]
    pub const fn keyboard(&self) -> Keyboard {
        self.keyboard
    }

    /// The output the panel is pinned to; `None` lets the compositor choose.
    #[must_use]
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }
}

/// What a panel reports about itself for a status reply (spec "Status"):
/// its settings and its view state. The daemon's registry adds the name and
/// the app's process ID, so a panel thread never needs either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelStatus {
    settings: PanelSettings,
    shown: bool,
    has_window: bool,
    width: u32,
}

impl PanelStatus {
    /// `settings` as they are now, whether the panel is shown, whether it
    /// holds a window and its resolved width in pixels.
    #[must_use]
    pub const fn new(settings: PanelSettings, shown: bool, has_window: bool, width: u32) -> Self {
        Self {
            settings,
            shown,
            has_window,
            width,
        }
    }

    /// The panel's current settings.
    #[must_use]
    pub const fn settings(&self) -> &PanelSettings {
        &self.settings
    }

    #[must_use]
    pub const fn shown(&self) -> bool {
        self.shown
    }

    #[must_use]
    pub const fn has_window(&self) -> bool {
        self.has_window
    }

    /// The size across the edge, resolved to pixels on the panel's output.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
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
