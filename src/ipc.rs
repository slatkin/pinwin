//! The daemon's wire format (replace-terminal-with-app-host D13): one JSON
//! object per line over the daemon's Unix socket.
//!
//! A client sends a [`Request`] — the tagged `cmd` enum with one variant for
//! each client subcommand of `src/cli.rs` and the `create` request only
//! `pinwin run` sends (D2) — and the daemon answers one [`Reply`]:
//! `{"ok":true}`, the same with the new panel's display for a carried-out
//! `create`, the daemon's process ID and its panels for a carried-out
//! `status` ([`StatusReply`], spec "Status"), or
//! `{"ok":false,"error":{"kind","message"}}` with the seven error kinds D13
//! names. A line that is not a request is a
//! [`RequestError::InvalidRequest`], while a known command whose setting
//! value is outside the config file's grammar is a
//! [`RequestError::InvalidValue`], so a bad value gets the kind the spec
//! names instead of the invalid-request kind a failed parse would otherwise
//! give it (spec "Request and reply format", "Invalid value").
//!
//! Every read and write is bounded in size and time: a request line over
//! [`MAX_LINE`] is refused with error kind `invalid_request`, so no client
//! can fill the daemon's memory with one endless line (spec "Clients cannot
//! block the daemon", D12). The client [`call`] connects, writes one line
//! and reads one reply under the [`REPLY_TIMEOUT`] deadline; it uses no
//! async runtime. The client itself lives in `ipc/client.rs` beside the
//! reply types. The parsing, the line limit and the client run without a
//! daemon.

use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::cli::Amount;
use crate::config::{Keyboard, Layer, PanelOptions};
use crate::geometry::{Edge, ReserveMode, Size};
use crate::name::PanelName;

mod client;
mod options;
mod reply;

pub use client::{Answer, CallError, Subscription, call};
pub use reply::{ErrorKind, ErrorReply, PanelEntry, PanelStatus, Reply};

/// The largest request line in bytes, without its newline: a line over it
/// gets error kind `invalid_request` and the daemon closes the connection
/// (spec "Clients cannot block the daemon", D12 "A request line is limited
/// to 64 KiB").
pub const MAX_LINE: usize = 64 * 1024;

/// How long a client waits for the daemon's reply line (spec "Command-line
/// exit codes": "when no reply comes in a bounded time"); the same bound as
/// the daemon start wait of D12.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// The `cmd` of each request, in README order: every client subcommand of
/// `src/cli.rs`, which is `cli::SUBCOMMANDS` less `run` and `daemon` (spec
/// "Command-line client"). The drift test checks the two lists against each
/// other in both directions.
#[cfg(test)]
pub const REQUEST_COMMANDS: [&str; 15] = [
    "toggle",
    "show",
    "hide",
    "focus",
    "set-edge",
    "set-size",
    "resize",
    "set-output",
    "set-layer",
    "set-reserve",
    "set-keyboard",
    "status",
    "subscribe",
    "quit",
    "reload",
];

/// Why a line is not a request the daemon can carry out (spec "Request and
/// reply format", "Panel commands"): the kind is the reply's error kind, so
/// the daemon turns the value into a reply with [`Reply::from`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// The line is not one JSON object of a known command.
    InvalidRequest(String),
    /// The command is known but a setting value is outside the grammar.
    InvalidValue(String),
}

impl RequestError {
    /// The error kind of the reply.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::InvalidRequest(_) => ErrorKind::InvalidRequest,
            Self::InvalidValue(_) => ErrorKind::InvalidValue,
        }
    }

    /// The message of the reply.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidRequest(message) | Self::InvalidValue(message) => message,
        }
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for RequestError {}

impl From<RequestError> for Reply {
    fn from(error: RequestError) -> Self {
        Self::error(error.kind(), error.message())
    }
}

/// One client request (spec "Command-line client", D13): the tagged `cmd`
/// enum the CLI builds for a client subcommand, the daemon parses from the
/// wire and the panel answers. A test checks the `cmd` names against
/// `cli::SUBCOMMANDS`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    /// Show or hide the panel.
    Toggle { panel: PanelName },
    /// Show the panel.
    Show { panel: PanelName },
    /// Hide the panel.
    Hide { panel: PanelName },
    /// Show the panel and give it the keyboard.
    Focus { panel: PanelName },
    /// Dock the panel at another edge.
    SetEdge { panel: PanelName, edge: Edge },
    /// Set the panel's size and, when given, its reserve mode, over `ms`
    /// milliseconds or its `tween_ms` (spec "Size and reserve in one
    /// request").
    SetSize {
        panel: PanelName,
        size: Size,
        #[serde(skip_serializing_if = "Option::is_none")]
        reserve: Option<ReserveMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ms: Option<u32>,
    },
    /// Change the panel's size by `by` and, when given, its reserve mode,
    /// over `ms` milliseconds or its `tween_ms`.
    Resize {
        panel: PanelName,
        by: Amount,
        #[serde(skip_serializing_if = "Option::is_none")]
        reserve: Option<ReserveMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ms: Option<u32>,
    },
    /// Open the panel on another output.
    SetOutput { panel: PanelName, output: String },
    /// Draw the panel in another layer.
    SetLayer { panel: PanelName, layer: Layer },
    /// Push or cover the strip the panel occupies.
    SetReserve {
        panel: PanelName,
        reserve: ReserveMode,
    },
    /// Set the panel's keyboard interactivity.
    SetKeyboard {
        panel: PanelName,
        keyboard: Keyboard,
    },
    /// Report the daemon's process ID and its panels, all of them or one.
    Status {
        #[serde(skip_serializing_if = "Option::is_none")]
        panel: Option<PanelName>,
    },
    /// Start a new panel and report the display its app connects with
    /// (spec "Run an app in a panel", D2). Only `pinwin run` sends this
    /// request; scripts use the client subcommands.
    Create {
        panel: PanelName,
        /// The process ID `pinwin run` reports; after `exec` it is the
        /// app's process, which the daemon's pidfd watches (D2, D3).
        pid: u32,
        /// The command's file name, which the daemon's report of an app
        /// that exits without a window names (row 7.4, D3, spec "Hand-off
        /// app is reported").
        app: String,
        #[serde(serialize_with = "options::serialize")]
        options: PanelOptions,
        /// Show a panel with this name instead of starting a new one (spec
        /// "Show an existing panel", D6). Absent on the wire means false,
        /// so a 0.4.0 request line still parses.
        #[serde(skip_serializing_if = "Absent::absent")]
        show_existing: bool,
    },
    /// Receive one line for each event.
    Subscribe,
    /// End the daemon and its panels.
    Quit,
    /// Reload the config file.
    Reload,
}

impl Request {
    /// Parse one request line (spec "Request and reply format"). A line that
    /// is not one JSON object of a known command is
    /// [`RequestError::InvalidRequest`]; a known command whose setting value
    /// is outside the grammar is [`RequestError::InvalidValue`], so the
    /// daemon can name the value it refused.
    pub fn parse(line: &[u8]) -> Result<Self, RequestError> {
        let raw: RawRequest = serde_json::from_slice(line)
            .map_err(|error| RequestError::InvalidRequest(format!("not a request: {error}")))?;
        Self::from_raw(raw)
    }

    /// The line a client sends: the JSON object plus its newline.
    ///
    /// # Panics
    ///
    /// Only if a request's `Serialize` fails, which no wire type here can
    /// do.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a request is serializable");
        line.push('\n');
        line
    }

    /// Turn the checked-shape line into the typed request: the panel name
    /// and every setting value are parsed with the config file's grammar.
    fn from_raw(raw: RawRequest) -> Result<Self, RequestError> {
        Ok(match raw {
            RawRequest::Toggle { panel } => Self::Toggle {
                panel: panel_name(&panel)?,
            },
            RawRequest::Show { panel } => Self::Show {
                panel: panel_name(&panel)?,
            },
            RawRequest::Hide { panel } => Self::Hide {
                panel: panel_name(&panel)?,
            },
            RawRequest::Focus { panel } => Self::Focus {
                panel: panel_name(&panel)?,
            },
            RawRequest::SetEdge { panel, edge } => Self::SetEdge {
                panel: panel_name(&panel)?,
                edge: setting("edge", &edge)?,
            },
            RawRequest::SetSize {
                panel,
                size,
                reserve,
                ms,
            } => Self::SetSize {
                panel: panel_name(&panel)?,
                size: wire_size(size)?,
                reserve: optional_setting("reserve", reserve.as_deref())?,
                ms: millis(ms)?,
            },
            RawRequest::Resize {
                panel,
                by,
                reserve,
                ms,
            } => Self::Resize {
                panel: panel_name(&panel)?,
                by: amount(by)?,
                reserve: optional_setting("reserve", reserve.as_deref())?,
                ms: millis(ms)?,
            },
            RawRequest::SetOutput { panel, output } => Self::SetOutput {
                panel: panel_name(&panel)?,
                output,
            },
            RawRequest::SetLayer { panel, layer } => Self::SetLayer {
                panel: panel_name(&panel)?,
                layer: setting("layer", &layer)?,
            },
            RawRequest::SetReserve { panel, reserve } => Self::SetReserve {
                panel: panel_name(&panel)?,
                reserve: setting("reserve", &reserve)?,
            },
            RawRequest::SetKeyboard { panel, keyboard } => Self::SetKeyboard {
                panel: panel_name(&panel)?,
                keyboard: setting("keyboard", &keyboard)?,
            },
            RawRequest::Status { panel } => Self::Status {
                panel: panel.map(|name| panel_name(&name)).transpose()?,
            },
            RawRequest::Create {
                panel,
                pid,
                app,
                options,
                show_existing,
            } => Self::Create {
                panel: panel_name(&panel)?,
                pid,
                app,
                options: options::parse(*options)?,
                show_existing,
            },
            RawRequest::Subscribe => Self::Subscribe,
            RawRequest::Quit => Self::Quit,
            RawRequest::Reload => Self::Reload,
        })
    }
}

/// A request line before its values are checked: every setting keeps the
/// text the config file's grammar parses, so a bad value does not become a
/// parse failure of the whole line.
#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
enum RawRequest {
    Toggle {
        panel: String,
    },
    Show {
        panel: String,
    },
    Hide {
        panel: String,
    },
    Focus {
        panel: String,
    },
    SetEdge {
        panel: String,
        edge: String,
    },
    SetSize {
        panel: String,
        size: RawSize,
        #[serde(default)]
        reserve: Option<String>,
        #[serde(default)]
        ms: Option<Number>,
    },
    Resize {
        panel: String,
        by: RawAmount,
        #[serde(default)]
        reserve: Option<String>,
        #[serde(default)]
        ms: Option<Number>,
    },
    SetOutput {
        panel: String,
        output: String,
    },
    SetLayer {
        panel: String,
        layer: String,
    },
    SetReserve {
        panel: String,
        reserve: String,
    },
    SetKeyboard {
        panel: String,
        keyboard: String,
    },
    Status {
        #[serde(default)]
        panel: Option<String>,
    },
    Create {
        panel: String,
        pid: u32,
        app: String,
        // Boxed, so the raw enum's largest variant does not dwarf the rest.
        #[serde(default)]
        options: Box<options::WireOptions>,
        #[serde(default)]
        show_existing: bool,
    },
    Subscribe,
    Quit,
    Reload,
}

/// A wire size: `640px` or `30%` as text, or a JSON number of pixels (spec
/// "Size values"). Every JSON number is kept, so a negative, fractional or
/// over-`u32` one is a bad value of a known command rather than a serde
/// failure of the whole line (spec "Panel commands").
#[derive(Deserialize)]
#[serde(untagged)]
enum RawSize {
    Text(String),
    Pixels(Number),
}

/// A wire resize amount: a signed size as text (`-40px`, `+10%`), or a JSON
/// number of pixels whose sign sets the direction (spec "Size values").
#[derive(Deserialize)]
#[serde(untagged)]
enum RawAmount {
    Text(String),
    Steps(Number),
}

/// A panel name from the wire (spec "Panel names"). A name outside the rules
/// names no panel, so it is no request for one.
fn panel_name(raw: &str) -> Result<PanelName, RequestError> {
    PanelName::parse(raw).map_err(|error| RequestError::InvalidRequest(format!("panel: {error}")))
}

/// A setting parsed with the config file's grammar (D13), so the option, the
/// file and the wire cannot drift apart.
fn setting<T>(field: &str, raw: &str) -> Result<T, RequestError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    raw.parse()
        .map_err(|error| RequestError::InvalidValue(format!("{field}: {error}")))
}

/// The `create` request's wire rule for a false `show_existing` flag: it is
/// absent, so a 0.4.0 request line keeps its shape (D6). The predicate takes
/// `&self` as serde's `skip_serializing_if` requires.
trait Absent {
    fn absent(&self) -> bool;
}

impl Absent for bool {
    fn absent(&self) -> bool {
        !*self
    }
}

/// An optional setting: an absent field keeps the panel's value, a present
/// one goes through the same grammar (D4).
fn optional_setting<T>(field: &str, raw: Option<&str>) -> Result<Option<T>, RequestError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    raw.map(|raw| setting(field, raw)).transpose()
}

/// A wire number as a whole number of its field's unit: a fractional or an
/// over-`i64` value is none, so the field turns it into `invalid_value`.
fn whole(number: &Number) -> Option<i64> {
    number.to_string().parse().ok()
}

/// The value error of a `by` number the grammar cannot hold.
fn invalid_amount(number: &Number) -> RequestError {
    RequestError::InvalidValue(format!(
        "by: expected a non-zero whole number of pixels, got {number}"
    ))
}

/// A wire size: text through the grammar, a number as pixels.
fn wire_size(raw: RawSize) -> Result<Size, RequestError> {
    match raw {
        RawSize::Text(text) => setting("size", &text),
        RawSize::Pixels(number) => whole(&number)
            .and_then(|px| u32::try_from(px).ok())
            .and_then(Size::pixels)
            .ok_or_else(|| {
                RequestError::InvalidValue(format!(
                    "size: expected a whole number of pixels from 1, got {number}"
                ))
            }),
    }
}

/// A wire resize amount: text through the grammar, a number's sign as the
/// direction and its magnitude as pixels.
fn amount(raw: RawAmount) -> Result<Amount, RequestError> {
    match raw {
        RawAmount::Text(text) => setting("by", &text),
        RawAmount::Steps(number) => {
            let steps = whole(&number).ok_or_else(|| invalid_amount(&number))?;
            let magnitude = u32::try_from(steps.unsigned_abs())
                .ok()
                .and_then(Size::pixels)
                .ok_or_else(|| invalid_amount(&number))?;
            Ok(if steps < 0 {
                Amount::shrink(magnitude)
            } else {
                Amount::grow(magnitude)
            })
        }
    }
}

/// The `ms` field: an absent one means the panel's `tween_ms`, and `0` the
/// snap animation (spec "Animated size changes").
fn millis(raw: Option<Number>) -> Result<Option<u32>, RequestError> {
    let Some(number) = raw else {
        return Ok(None);
    };
    whole(&number)
        .and_then(|ms| u32::try_from(ms).ok())
        .map(Some)
        .ok_or_else(|| {
            RequestError::InvalidValue(format!(
                "ms: expected a whole number of milliseconds from 0, got {number}"
            ))
        })
}

/// Why one line could not be read from a stream.
#[derive(Debug)]
pub enum LineError {
    /// The line passed [`MAX_LINE`] bytes before its newline.
    TooLong,
    /// No complete line arrived within the timeout.
    Timeout,
    /// The stream failed.
    Io(io::Error),
}

impl LineError {
    /// The reply the daemon sends for a line it could not read (spec
    /// "Clients cannot block the daemon"): an over-long line is
    /// `invalid_request`, and a failed read is `internal`.
    #[must_use]
    pub fn reply(&self) -> Reply {
        match self {
            Self::TooLong => Reply::error(
                ErrorKind::InvalidRequest,
                format!("the request line is longer than {MAX_LINE} bytes"),
            ),
            Self::Timeout => Reply::error(
                ErrorKind::InvalidRequest,
                "no complete request line arrived in time",
            ),
            Self::Io(error) => Reply::error(ErrorKind::Internal, format!("read failed: {error}")),
        }
    }
}

impl fmt::Display for LineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => write!(f, "a line is longer than {MAX_LINE} bytes"),
            Self::Timeout => f.write_str("no line arrived in time"),
            Self::Io(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for LineError {}

/// Read one line from `stream`, bounded in size and time: the read stops
/// with [`LineError::TooLong`] as soon as the line passes `limit` bytes
/// before its newline, so an endless line cannot fill memory. A stream
/// switch to non-blocking is part of the bound: every wait polls the
/// descriptor for at most `timeout`.
pub fn read_line(
    stream: &mut UnixStream,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, LineError> {
    LineReader::new().next_line(stream, limit, timeout)
}

/// A reader for a stream that carries more than one line, such as a
/// subscriber's event stream (spec "Event stream"): the bytes of a read
/// that followed the line it returned stay for the next call, so no event
/// line is lost behind the line before it. [`read_line`] is one line from a
/// fresh reader, which is all a request and its reply need.
#[derive(Debug, Default)]
pub struct LineReader {
    /// Bytes read from the stream that no call has returned as a line yet.
    pending: Vec<u8>,
}

impl LineReader {
    /// A reader with no buffered bytes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the next line, bounded like [`read_line`]. A line that ends at
    /// the end of the stream without its newline is returned as it is, so
    /// the caller can report the truncated stream.
    pub fn next_line(
        &mut self,
        stream: &mut UnixStream,
        limit: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, LineError> {
        stream.set_nonblocking(true).map_err(LineError::Io)?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                if end > limit {
                    return Err(LineError::TooLong);
                }
                return Ok(self.pending.drain(..=end).collect());
            }
            if self.pending.len() > limit {
                return Err(LineError::TooLong);
            }
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk) {
                // The peer closed; a partial line without its newline is the
                // caller's to judge.
                Ok(0) => return Ok(std::mem::take(&mut self.pending)),
                Ok(read) => self.pending.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(LineError::Timeout);
                    }
                    if !poll_ready(stream.as_raw_fd(), libc::POLLIN, remaining)
                        .map_err(LineError::Io)?
                    {
                        return Err(LineError::Timeout);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(LineError::Io(error)),
            }
        }
    }
}

/// Write `line` to `stream`, bounded like [`read_line`]: a peer that never
/// reads gives up its wait instead of blocking the writer.
pub fn write_line(
    stream: &mut UnixStream,
    line: &[u8],
    timeout: Duration,
) -> Result<(), LineError> {
    stream.set_nonblocking(true).map_err(LineError::Io)?;
    let deadline = Instant::now() + timeout;
    let mut written = 0;
    while written < line.len() {
        match stream.write(&line[written..]) {
            Ok(0) => {
                return Err(LineError::Io(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "the stream accepted no bytes",
                )));
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(LineError::Timeout);
                }
                if !poll_ready(stream.as_raw_fd(), libc::POLLOUT, remaining)
                    .map_err(LineError::Io)?
                {
                    return Err(LineError::Timeout);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(LineError::Io(error)),
        }
    }
    Ok(())
}

/// Wait until `fd` reports `events` (`POLLIN`/`POLLOUT`), at most `timeout`.
/// False when the wait expired, and an error when poll itself failed, so a
/// real failure never reports as a timeout (spec "Command-line exit codes").
fn poll_ready(
    fd: std::os::fd::RawFd,
    events: libc::c_short,
    timeout: Duration,
) -> io::Result<bool> {
    let mut fds = [libc::pollfd {
        fd,
        events,
        revents: 0,
    }];
    let ready = crate::fd::poll(&mut fds, Some(timeout))?;
    Ok(ready > 0 && fds[0].revents & events != 0)
}

#[cfg(test)]
mod tests;
