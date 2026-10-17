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

use super::create;
use super::events::Bus;
use super::lifetime::Lifetime;
use crate::guard::{Poisoned, guard};
use crate::ipc::{self, ErrorKind, MAX_LINE, Reply, Request};

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
        } => {
            let reply = create::panel(lifetime, panel, app, &options, pid);
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
            Ok(event) => ipc::write_line(stream, event.to_line().as_bytes(), timeout)?,
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
    // SAFETY: `polled` is one valid descriptor entry for the duration of the
    // call, and poll writes only its `revents` field.
    let ready = unsafe { libc::poll(std::ptr::addr_of_mut!(polled), 1, 0) };
    ready > 0 && polled.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
}
