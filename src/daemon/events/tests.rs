//! The event-stream tests of replace-terminal-with-app-host row 5.4: a
//! subscriber receives the panels' events, a subscriber that stops reading is
//! closed, and a client that sends nothing blocks nobody (spec "Toggle
//! events", "Slow subscriber", "Silent client").
//!
//! The tests drive `connection::serve` over a socket pair with a test thread
//! in the panel's place, so no Wayland connection and no daemon are needed.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{Bus, Event};
use crate::daemon::connection;
use crate::daemon::panel::{Command, EventFd, PanelHandle, PanelReply, PanelRequest};
use crate::daemon::registry::Registry;
use crate::ipc::{LineReader, MAX_LINE, read_line, write_line};
use crate::name::PanelName;

/// How long a test connection waits for one step. A test that has to time
/// out a client pays this, not the production wait.
const CONNECTION_WAIT: Duration = Duration::from_millis(200);

/// How long a client waits for its reply or event.
const WAIT: Duration = Duration::from_millis(500);

/// The panel name the tests use.
fn notes() -> PanelName {
    PanelName::parse("notes").expect("notes is a panel name")
}

/// A stand-in for a panel thread: it takes each command, clears the eventfd
/// that delivered it and answers what the test says (D12).
fn panel_thread(
    answer: impl Fn(&PanelRequest) -> PanelReply + Send + 'static,
) -> (PanelName, PanelHandle, JoinHandle<()>) {
    let (commands, incoming) = mpsc::channel::<Command>();
    let wake = Arc::new(EventFd::new().expect("an eventfd"));
    let watch = Arc::clone(&wake);
    let thread = thread::spawn(move || {
        while let Ok(command) = incoming.recv() {
            watch.drain().expect("the panel clears its wake");
            let reply = answer(command.request());
            command.answer(reply);
        }
    });
    (notes(), PanelHandle::new(7, commands, wake), thread)
}

/// Serve one daemon side of a socket pair on its own thread, as the daemon
/// does for each accepted connection (D12), so the test drives the other side
/// as a client.
fn serve_connection(stream: UnixStream, registry: Arc<Registry>, bus: Bus) -> JoinHandle<()> {
    thread::spawn(move || {
        let _ = connection::serve(stream, &registry, &bus, CONNECTION_WAIT);
    })
}

/// Write one request line and read its reply, as a client does.
fn ask(client: &mut UnixStream, request: &str) -> String {
    write_line(client, request.as_bytes(), WAIT).expect("the request is sent");
    let line = read_line(client, MAX_LINE, WAIT).expect("the reply line arrives");
    String::from_utf8(line).expect("the reply is UTF-8")
}

/// A client connection served by its own thread, which answers panel
/// requests through the registry.
fn connect_client(registry: &Arc<Registry>, bus: &Bus) -> UnixStream {
    let (client, server) = UnixStream::pair().expect("a socket pair");
    serve_connection(server, Arc::clone(registry), bus.clone());
    client
}

/// A client that has sent `subscribe` and read its success reply, plus the
/// thread that serves its connection.
fn subscribed_client(bus: &Bus, registry: &Arc<Registry>) -> (UnixStream, JoinHandle<()>) {
    let (mut client, server) = UnixStream::pair().expect("a socket pair");
    let task = serve_connection(server, Arc::clone(registry), bus.clone());
    assert_eq!(
        ask(&mut client, "{\"cmd\":\"subscribe\"}\n"),
        "{\"ok\":true}\n"
    );
    (client, task)
}

/// Read the next complete event line.
fn read_event(lines: &mut LineReader, client: &mut UnixStream) -> String {
    let line = lines
        .next_line(client, MAX_LINE, WAIT)
        .expect("an event line arrives");
    assert!(
        line.ends_with(b"\n"),
        "the daemon sent no complete event line: {line:?}"
    );
    String::from_utf8(line).expect("the event line is UTF-8")
}

/// Read until the daemon closes the connection, so buffered event lines do
/// not hide the close; false when the connection is still open after 2 s.
fn read_to_eof(client: &mut UnixStream) -> bool {
    let mut lines = LineReader::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match lines.next_line(client, MAX_LINE, CONNECTION_WAIT) {
            // A complete line is an event still in the socket's buffer.
            Ok(line) if line.ends_with(b"\n") => {}
            // An empty or partial line is the end of the stream.
            Ok(_) => return true,
            Err(_) => return false,
        }
    }
    false
}

/// Make both ends of a socket pair hold only a few event lines, so a client
/// that stops reading stalls its writer after a few writes.
fn shrink_buffers(one: &UnixStream, two: &UnixStream) {
    let size: libc::c_int = 4096;
    let length: libc::socklen_t = size_of::<libc::c_int>().try_into().expect("an int fits");
    for stream in [one, two] {
        for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
            // SAFETY: `setsockopt` reads one `c_int` of the length given and
            // writes nothing; the descriptor belongs to `stream`.
            let result = unsafe {
                libc::setsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    option,
                    std::ptr::addr_of!(size).cast(),
                    length,
                )
            };
            assert_eq!(result, 0, "the socket buffer is shrunk");
        }
    }
}

/// A subscriber receives a `hidden` event and then a `shown` event when
/// another client toggles the panel twice (spec "Toggle events").
#[test]
fn a_subscriber_receives_toggle_events() {
    let bus = Bus::new();
    let registry = Arc::new(Registry::new());
    let toggles = Arc::new(AtomicUsize::new(0));
    let publisher = {
        let bus = bus.clone();
        let toggles = Arc::clone(&toggles);
        move |request: &PanelRequest| {
            if matches!(request, PanelRequest::Toggle) {
                let event = if toggles.fetch_add(1, Ordering::Relaxed) == 0 {
                    Event::Hidden { panel: notes() }
                } else {
                    Event::Shown { panel: notes() }
                };
                bus.publish(&event);
            }
            PanelReply::Done
        }
    };
    let (name, handle, panel) = panel_thread(publisher);
    registry.add(name, handle).expect("notes is free");

    let (mut subscriber, task) = subscribed_client(&bus, &registry);
    let mut events = LineReader::new();
    // Each `pinwin toggle` is its own connection and its own request.
    for _ in 0..2 {
        let mut client = connect_client(&registry, &bus);
        assert_eq!(
            ask(&mut client, "{\"cmd\":\"toggle\",\"panel\":\"notes\"}\n"),
            "{\"ok\":true}\n"
        );
    }
    assert_eq!(
        read_event(&mut events, &mut subscriber),
        "{\"event\":\"hidden\",\"panel\":\"notes\"}\n"
    );
    assert_eq!(
        read_event(&mut events, &mut subscriber),
        "{\"event\":\"shown\",\"panel\":\"notes\"}\n"
    );

    // A client that closes its subscription frees its queue (spec "Event
    // stream": the stream lasts until the client closes it).
    drop(subscriber);
    task.join().expect("the subscriber's connection ends");
    assert_eq!(bus.subscriber_count(), 0, "the closed subscriber is gone");
    drop(registry);
    panel.join().expect("the panel thread ends");
}

/// A subscriber that stops reading is closed once its bounded queue fills,
/// and other requests are still answered (spec "Slow subscriber", D12).
#[test]
fn a_slow_subscriber_is_closed_and_other_requests_are_answered() {
    let bus = Bus::with_queue_depth(2);
    let registry = Arc::new(Registry::new());
    let (name, handle, panel) = panel_thread(|_| PanelReply::Done);
    registry.add(name, handle).expect("notes is free");

    let (mut subscriber, server) = UnixStream::pair().expect("a socket pair");
    shrink_buffers(&subscriber, &server);
    let task = serve_connection(server, Arc::clone(&registry), bus.clone());
    assert_eq!(
        ask(&mut subscriber, "{\"cmd\":\"subscribe\"}\n"),
        "{\"ok\":true}\n"
    );

    // The client stops reading while many events happen.
    for _ in 0..100 {
        bus.publish(&Event::GeometryChanged { panel: notes() });
    }
    assert_eq!(
        bus.subscriber_count(),
        0,
        "the stalled subscriber was dropped"
    );

    // Another client's request is still answered.
    let mut client = connect_client(&registry, &bus);
    assert_eq!(
        ask(&mut client, "{\"cmd\":\"toggle\",\"panel\":\"notes\"}\n"),
        "{\"ok\":true}\n"
    );
    assert!(
        read_to_eof(&mut subscriber),
        "the daemon closed the slow subscriber's connection"
    );
    task.join().expect("the slow subscriber's connection ends");

    drop(registry);
    panel.join().expect("the panel thread ends");
}

/// A client that connects and sends nothing does not block another client's
/// request, and the bounded read closes its connection (spec "Silent
/// client").
#[test]
fn a_silent_client_does_not_block_another_request() {
    let bus = Bus::new();
    let registry = Arc::new(Registry::new());
    let (name, handle, panel) = panel_thread(|_| PanelReply::Done);
    registry.add(name, handle).expect("notes is free");

    let (mut silent, server) = UnixStream::pair().expect("a socket pair");
    let silent_task = serve_connection(server, Arc::clone(&registry), bus.clone());

    let mut client = connect_client(&registry, &bus);
    let started = Instant::now();
    assert_eq!(
        ask(&mut client, "{\"cmd\":\"toggle\",\"panel\":\"notes\"}\n"),
        "{\"ok\":true}\n"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the silent client delayed the reply by {:?}",
        started.elapsed()
    );

    // The silent client's own bounded read ends with an error reply.
    let line = read_line(&mut silent, MAX_LINE, Duration::from_secs(2)).expect("a reply line");
    let text = String::from_utf8(line).expect("the reply is UTF-8");
    assert!(
        text.contains("invalid_request"),
        "the silent client got {text}"
    );
    silent_task.join().expect("the silent connection ends");

    drop(registry);
    panel.join().expect("the panel thread ends");
}
