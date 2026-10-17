//! The daemon's client (replace-terminal-with-app-host D13, spec
//! "Command-line exit codes"): connect to the daemon socket, send one
//! request line, read one reply line and keep both the parsed reply and the
//! line itself, so `pinwin status` prints the daemon's JSON unchanged.
//!
//! [`Subscription`] is the `subscribe` client: it reads the success reply
//! and then one event line after another until the daemon closes the stream
//! (spec "Command-line client", "Event stream"). The first reply is bounded
//! by [`REPLY_TIMEOUT`] like any other; waiting for an event is not, because
//! an idle event bus is normal.

use std::fmt;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    LineError, LineReader, MAX_LINE, REPLY_TIMEOUT, Reply, Request, read_line, write_line,
};

/// Why a client could not get a reply (spec "Command-line exit codes").
#[derive(Debug)]
pub enum CallError {
    /// The connect failed: no daemon answers on the path.
    Connect {
        /// The socket path nothing answered on.
        path: PathBuf,
        /// The connect's own error.
        source: io::Error,
    },
    /// The request line could not be sent.
    Send(LineError),
    /// No reply line came back.
    Receive(LineError),
    /// The reply line is not one reply object.
    InvalidReply(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect { path, source } => {
                write!(f, "no daemon answered on {} ({source})", path.display())
            }
            Self::Send(error) => write!(f, "could not send the request: {error}"),
            Self::Receive(error) => write!(f, "no reply came from the daemon: {error}"),
            Self::InvalidReply(message) => {
                write!(f, "the daemon's reply is not valid: {message}")
            }
        }
    }
}

impl std::error::Error for CallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect { source, .. } => Some(source),
            Self::Send(error) | Self::Receive(error) => Some(error),
            Self::InvalidReply(_) => None,
        }
    }
}

/// Connect to the daemon socket at `path`, send `request` as one line and
/// read one reply line, every wait bounded by [`REPLY_TIMEOUT`] (spec
/// "Command-line exit codes", D13). The [`Answer`] keeps the reply line as
/// well as the parsed reply, so `pinwin status` prints the daemon's JSON
/// unchanged (spec "Command-line exit codes").
pub fn call(path: &Path, request: &Request) -> Result<Answer, CallError> {
    let stream = UnixStream::connect(path).map_err(|source| CallError::Connect {
        path: path.to_owned(),
        source,
    })?;
    exchange(stream, request, REPLY_TIMEOUT)
}

/// One reply as the daemon sent it: the parsed reply plus the line itself,
/// which the client prints for `pinwin status` (spec "Command-line exit
/// codes": "`pinwin status` SHALL print the reply's JSON on stdout").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    reply: Reply,
    line: Vec<u8>,
}

impl Answer {
    /// The parsed reply: its ok flag and, for a failure, the error object
    /// whose message the client prints.
    #[must_use]
    pub const fn reply(&self) -> &Reply {
        &self.reply
    }

    /// The reply line as the daemon wrote it, with its newline.
    #[must_use]
    pub fn line(&self) -> &[u8] {
        &self.line
    }
}

/// [`call`] over an already-connected stream, so a test drives the exchange
/// with a socket pair and no daemon.
pub(super) fn exchange(
    mut stream: UnixStream,
    request: &Request,
    timeout: Duration,
) -> Result<Answer, CallError> {
    write_line(&mut stream, request.to_line().as_bytes(), timeout).map_err(CallError::Send)?;
    let line = read_line(&mut stream, MAX_LINE, timeout).map_err(CallError::Receive)?;
    if !line.ends_with(b"\n") {
        return Err(CallError::InvalidReply(
            "the connection closed before the reply line ended".to_owned(),
        ));
    }
    let reply = Reply::parse(&line).map_err(CallError::InvalidReply)?;
    Ok(Answer { reply, line })
}

/// A `subscribe` client (spec "Command-line client": "`pinwin subscribe`
/// prints one JSON line for the event"): the daemon's success reply and then
/// one event line after another until the daemon closes the stream (spec
/// "Event stream"). The success reply is waited for within
/// [`REPLY_TIMEOUT`] like any other reply; an event line is not, because an
/// idle event bus is normal.
#[derive(Debug)]
pub struct Subscription {
    stream: UnixStream,
    reader: LineReader,
}

impl Subscription {
    /// Connect to the daemon at `path` and send the `subscribe` request.
    pub fn connect(path: &Path) -> Result<Self, CallError> {
        let stream = UnixStream::connect(path).map_err(|source| CallError::Connect {
            path: path.to_owned(),
            source,
        })?;
        Self::over(stream)
    }

    /// [`Subscription::connect`] over an already-connected stream, so a test
    /// drives the stream with a socket pair and no daemon.
    pub(super) fn over(mut stream: UnixStream) -> Result<Self, CallError> {
        write_line(
            &mut stream,
            Request::Subscribe.to_line().as_bytes(),
            REPLY_TIMEOUT,
        )
        .map_err(CallError::Send)?;
        Ok(Self {
            stream,
            reader: LineReader::new(),
        })
    }

    /// The daemon's reply to the `subscribe` request, waited for within
    /// [`REPLY_TIMEOUT`] (spec "Command-line exit codes").
    pub fn reply(&mut self) -> Result<Reply, CallError> {
        let line = self
            .reader
            .next_line(&mut self.stream, MAX_LINE, REPLY_TIMEOUT)
            .map_err(CallError::Receive)?;
        if !line.ends_with(b"\n") {
            return Err(CallError::InvalidReply(
                "the connection closed before the reply line ended".to_owned(),
            ));
        }
        Reply::parse(&line).map_err(CallError::InvalidReply)
    }

    /// The next event line, or `None` when the daemon closed the stream.
    /// The wait repeats over [`REPLY_TIMEOUT`], so an idle bus keeps the
    /// client open (spec "Event stream").
    pub fn next_event(&mut self) -> Result<Option<Vec<u8>>, CallError> {
        loop {
            match self
                .reader
                .next_line(&mut self.stream, MAX_LINE, REPLY_TIMEOUT)
            {
                Ok(line) if line.is_empty() => return Ok(None),
                Ok(line) => return Ok(Some(line)),
                Err(LineError::Timeout) => {}
                Err(error) => return Err(CallError::Receive(error)),
            }
        }
    }
}
