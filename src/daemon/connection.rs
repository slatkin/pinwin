//! One IPC connection (replace-terminal-with-app-host D12, D13): read one
//! request line with a bounded wait, answer it, and, for `subscribe`, keep
//! writing the event bus's lines until the client closes the connection
//! (spec "Event stream").
//!
//! Every read and every wait is bounded, so a client that connects and sends
//! nothing, sends a partial line or stops reading holds only its own
//! connection thread (spec "Clients cannot block the daemon", "Silent
//! client"). The daemon runs one of these per accepted connection, so the
//! thread that accepts connections never handles a client's bytes itself.
//!
//! A panel request goes through the registry, while the requests the daemon
//! owns go through its [`Lifetime`]: `create` starts the panel a `pinwin run`
//! asks for and answers with its display (spec "Run an app in a panel"),
//! `quit` writes its success reply and then ends every panel and the daemon
//! (spec "Quit"), and `reload` answers with the reloaded config or the
//! file's error (spec "Reload").

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use super::events::Bus;
use super::lifetime::Lifetime;
use crate::config::PanelOptions;
use crate::guard::{Poisoned, guard};
use crate::ipc::{self, ErrorKind, ErrorReply, MAX_LINE, Reply, Request};
use crate::name::PanelName;
use crate::proxy::{self, StartError};

/// Serve one accepted connection: one request line and its reply, or the
/// event stream of a `subscribe` request until the client closes.
///
/// `timeout` bounds the first read, every reply write and every wait for an
/// event, so no client holds this connection thread longer than it takes to
/// notice a stalled peer. The daemon's own requests go through `lifetime`:
/// a `quit` reply goes out before the panels and the daemon end, and a
/// `reload` reply is the file's success or its `invalid_config` error.
pub fn serve(
    mut stream: UnixStream,
    lifetime: &Arc<Lifetime>,
    timeout: Duration,
) -> Result<(), ipc::LineError> {
    let line = match ipc::read_line(&mut stream, MAX_LINE, timeout) {
        Ok(line) => line,
        Err(error) => {
            // An over-long line, a read that timed out or a failed read:
            // answer with the error's kind and close this connection
            // (spec "Clients cannot block the daemon").
            return answer(&mut stream, &error.reply(), timeout);
        }
    };
    // A line without its newline is a client that closed in the middle of
    // one; there is no request to carry out.
    if !line.ends_with(b"\n") {
        return Ok(());
    }
    let request = match Request::parse(&line) {
        Ok(request) => request,
        Err(error) => return answer(&mut stream, &Reply::from(error), timeout),
    };
    // D5: the request runs under the shared guard, so a panic in `create`,
    // `reload` or `quit` latches this connection's flag and still answers the
    // client with an `internal` error instead of a closed socket.
    let poisoned = Poisoned::new();
    let Ok(result) = guard(&poisoned, || {
        carry_out(&mut stream, lifetime, request, timeout)
    }) else {
        return answer(
            &mut stream,
            &Reply::error(
                ErrorKind::Internal,
                "the daemon hit a panic while carrying out the request",
            ),
            timeout,
        );
    };
    result
}

/// Carry out one parsed request on `stream` (D12): the panel requests go
/// through the registry and the daemon's own requests through `lifetime`.
fn carry_out(
    stream: &mut UnixStream,
    lifetime: &Arc<Lifetime>,
    request: Request,
    timeout: Duration,
) -> Result<(), ipc::LineError> {
    match request {
        Request::Create {
            panel,
            pid,
            app,
            options,
            show_existing,
        } => {
            let reply = create_or_show(
                lifetime,
                &panel,
                pid,
                &app,
                &options,
                show_existing,
                timeout,
            );
            answer(stream, &reply, timeout)
        }
        Request::Subscribe => subscribe(stream, lifetime.bus(), timeout),
        // The reply goes out first, so the client gets its success even
        // though the daemon and its panels end right after (spec "Quit").
        Request::Quit => {
            let written = answer(stream, &Reply::ok(), timeout);
            lifetime.quit();
            written
        }
        Request::Reload => answer(stream, &lifetime.reload(), timeout),
        request => match lifetime.registry().route(&request, timeout) {
            Some(reply) => answer(stream, &reply, timeout),
            // The registry leaves only `subscribe`, `quit` and `reload` to
            // the daemon, and the arms above answered them.
            None => Ok(()),
        },
    }
}

/// Answer a `create` request: show the panel when it exists and the request
/// allows it, otherwise start it (D6, spec "Show an existing panel", "One
/// decision for a duplicate launch").
///
/// The daemon decides, so two `pinwin run --show-existing` calls cannot both
/// create a panel: the first registration wins and the other's `show` finds
/// it. The loop is D6's step 3: a spawn refused because another launch
/// registered the name first goes back to the show, and a name that is free
/// again by then starts a panel as without the flag. A refusal other than a
/// duplicate name, and every IO failure, is the request's error.
fn create_or_show(
    lifetime: &Arc<Lifetime>,
    panel: &PanelName,
    pid: u32,
    app: &str,
    options: &PanelOptions,
    show_existing: bool,
    timeout: Duration,
) -> Reply {
    loop {
        if show_existing {
            let show = Request::Show {
                panel: panel.clone(),
            };
            match lifetime.registry().route(&show, timeout) {
                Some(reply) if reply.is_ok() => return Reply::existing(),
                Some(reply)
                    if reply.as_error().map(ErrorReply::kind) == Some(ErrorKind::NoPanel) => {}
                Some(reply) => return reply,
                // The registry answers every panel command, `show` included.
                None => {
                    return Reply::error(
                        ErrorKind::Internal,
                        format!("{panel}: the daemon could not route the show request"),
                    );
                }
            }
        }
        // The options are merged with the config file's tables; a name another
        // panel uses is refused and that panel stays (spec "Duplicate name").
        // A panel whose thread or socket cannot start is error kind `internal`,
        // so the client reports it as the daemon's failure rather than its own.
        let settings = lifetime.panel_settings(panel, options);
        match proxy::spawn(panel.clone(), settings, app.to_owned(), pid, lifetime) {
            Ok(started) => return Reply::created(started.display().to_owned()),
            Err(StartError::Refused(error))
                if show_existing && error.kind() == ErrorKind::DuplicateName => {}
            Err(StartError::Refused(error)) => return Reply::Error(error),
            Err(StartError::Io(error)) => {
                return Reply::error(
                    ErrorKind::Internal,
                    format!("the panel did not start: {error}"),
                );
            }
        }
    }
}

/// Write one reply line.
fn answer(stream: &mut UnixStream, reply: &Reply, timeout: Duration) -> Result<(), ipc::LineError> {
    ipc::write_line(stream, reply.to_line().as_bytes(), timeout)
}

/// Answer a `subscribe` request with success and then one JSON line per bus
/// event (spec "Event stream").
///
/// The subscription is registered before the success line is written, so an
/// event published while the line goes out is queued rather than lost. A
/// client that closes its connection is noticed on the next wait; a client
/// that stops reading fills its queue, and the bus closes it (spec "Slow
/// subscriber").
fn subscribe(stream: &mut UnixStream, bus: &Bus, timeout: Duration) -> Result<(), ipc::LineError> {
    let subscription = bus.subscribe();
    answer(stream, &Reply::ok(), timeout)?;
    loop {
        match subscription.next(timeout) {
            Ok(line) => ipc::write_line(stream, line.as_bytes(), timeout)?,
            Err(RecvTimeoutError::Timeout) => {
                if peer_closed(stream) {
                    return Ok(());
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Whether the client closed its connection. A subscriber sends nothing
/// after its request, so a socket that poll reports readable is at its end
/// (spec "Event stream": the stream lasts "until the client closes it").
fn peer_closed(stream: &UnixStream) -> bool {
    let mut polled = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let Ok(ready) = crate::fd::poll(std::slice::from_mut(&mut polled), Some(Duration::ZERO)) else {
        // A failed poll leaves the connection open until the next pass.
        return false;
    };
    ready > 0 && polled.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
}
