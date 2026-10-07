//! The `instance` module's tests, split from `instance.rs` to keep the
//! module at its line budget (the split is mechanical; every item stays
//! reachable through `use super::*`).

use super::*;
use std::sync::atomic::AtomicUsize;

/// A unique scratch directory under the system temp dir, so the tests
/// never touch the process environment and never collide (nextest runs
/// each test in its own process; a plain `cargo test` shares one).
fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pinwin-instance-test-{}-{}-{}",
        std::process::id(),
        tag,
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).expect("temp dir");
    dir
}

/// The request lines the protocol defines: exactly `toggle\n` and `show\n`
/// (replace-gtk-with-wayland D4, `serve-instance-socket` D3).
const TOGGLE_LINE: &[u8] = b"toggle\n";
const SHOW_LINE: &[u8] = b"show\n";

/// A valid name parses, and the boundaries of the character set, the
/// length range and the [`InvalidName`] message hold.
#[test]
fn the_instance_name_validates() {
    let name = InstanceName::parse("notes").expect("valid");
    assert_eq!(&*name, "notes");
    assert_eq!(name.to_string(), "notes");

    // The whole allowed character set, and the 64-character boundary.
    InstanceName::parse("A-z_09-").expect("the full allowed set");
    InstanceName::parse(&"a".repeat(64)).expect("64 characters");

    // Empty, too long, and characters outside the set: the typed error,
    // whose message carries the rejected name (the binary adds the
    // `pinwin: <context>:` prefix).
    for raw in ["", &"a".repeat(65), "a/b", "a b", "a.b", "ä"] {
        let error = InstanceName::parse(raw).expect_err(raw);
        assert!(matches!(error, InvalidName(_)), "raw = {raw:?}");
        assert_eq!(
            error.to_string(),
            format!("expected a name of 1..=64 characters from [A-Za-z0-9_-], got '{raw}'"),
            "raw = {raw:?}"
        );
    }
}

/// The name used when none is given is `default`.
#[test]
fn the_default_instance_name_is_default() {
    assert_eq!(&*InstanceName::default_instance(), "default");
}

/// The socket path is `$XDG_RUNTIME_DIR/pinwin/$WAYLAND_DISPLAY-<name>.sock`,
/// with the `wayland-0` fallback for an unset display, and a set display
/// that is valid UTF-8 and holds no `/`. The runtime directory is passed
/// through as an `OsStr`, with no lossy conversion.
#[test]
fn socket_path_composes_the_runtime_path() {
    let name = InstanceName::parse("notes").expect("valid");
    let os = OsStr::new;
    assert_eq!(
        socket_path(Some(os("/run/user/1000")), Some(os("wayland-1")), &name),
        Ok(PathBuf::from("/run/user/1000/pinwin/wayland-1-notes.sock"))
    );
    assert_eq!(
        socket_path(Some(os("/run/user/1000")), None, &name),
        Ok(PathBuf::from("/run/user/1000/pinwin/wayland-0-notes.sock"))
    );

    // An empty display, a display with a separator, and a non-UTF-8
    // display are all [`PathError`]s: the path must stay inside
    // `$XDG_RUNTIME_DIR/pinwin` and distinct displays must stay distinct.
    for raw in [
        os(""),
        os("way/land"),
        os("/abs/path"),
        OsStr::from_bytes(&[b'w', 0xff]),
    ] {
        let error =
            socket_path(Some(os("/run/user/1000")), Some(raw), &name).expect_err("bad display");
        assert!(matches!(error, PathError::BadDisplay(_)), "raw = {raw:?}");
        assert_eq!(
            error.to_string(),
            format!(
                "WAYLAND_DISPLAY: expected a UTF-8 display name without '/', \
                 got '{}'",
                raw.to_string_lossy()
            ),
            "raw = {:?}",
            raw
        );
    }

    // A missing or empty runtime directory is the `NoRuntimeDir` variant.
    for runtime_dir in [None, Some(os(""))] {
        let error = socket_path(runtime_dir, Some(os("wayland-0")), &name)
            .expect_err("no runtime directory");
        assert!(matches!(error, PathError::NoRuntimeDir));
        assert_eq!(
            error.to_string(),
            "XDG_RUNTIME_DIR: expected the runtime directory of the Wayland session"
        );
    }
}

/// A stale file on the socket path is removed and bound again, and the
/// new listener answers connects. On Linux both a SIGKILL-ed host's
/// socket file and a plain regular file refuse the connect
/// (`ECONNREFUSED`), so both count as a dead owner and are replaced.
#[test]
fn a_stale_file_on_the_socket_path_is_replaced() {
    let dir = temp_dir("stale");
    let path = dir.join("wayland-0-notes.sock");

    // The realistic stale case: a bound socket whose listener is gone.
    drop(net::UnixListener::bind(&path).expect("stale socket file"));
    net::UnixStream::connect(&path).expect_err("nothing listens on the stale file");

    let listener = bind_instance_socket(&path).expect("stale socket replaced");
    assert!(path.exists(), "the socket file is back");
    net::UnixStream::connect(&path).expect("the bind is live");
    drop(listener);

    // A plain regular file refuses the connect the same way.
    fs::remove_file(&path).expect("drop the leftover socket file");
    fs::write(&path, b"not a socket").expect("regular file");
    net::UnixStream::connect(&path).expect_err("a regular file is not a socket");
    let listener = bind_instance_socket(&path).expect("regular file replaced");
    drop(listener);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The parent directory is created with mode 0700, also when it already
/// existed.
#[test]
fn the_socket_directory_is_private() {
    let dir = temp_dir("perm");
    let path = dir.join("pinwin").join("wayland-0-default.sock");

    // First bind creates `pinwin` inside the scratch dir.
    let listener = bind_instance_socket(&path).expect("first bind");
    drop(listener);
    let mode = fs::metadata(path.parent().expect("parent"))
        .expect("stat")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "the pinwin dir is 0700");

    // A second bind keeps the directory usable and private.
    let listener = bind_instance_socket(&path).expect("second bind");
    drop(listener);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A live listener on the path makes the bind fail as a duplicate: the
/// second host of the same name must exit 2 before it opens a surface.
#[test]
fn a_live_listener_makes_the_bind_fail() {
    let dir = temp_dir("duplicate");
    let path = dir.join("wayland-0-default.sock");

    let first = bind_instance_socket(&path).expect("first bind");
    assert!(
        matches!(bind_instance_socket(&path), Err(InstanceError::Duplicate)),
        "the live listener is a duplicate"
    );
    drop(first);
    // After the first listener is gone the name is free again (its file
    // is now stale): a fresh host binds instead of failing.
    bind_instance_socket(&path).expect("the name is free again");
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The bind-first order: a second host must not unlink the first host's
/// live socket, even though both of their binds race on the same path.
#[test]
fn a_racing_second_host_leaves_the_live_socket_in_place() {
    let dir = temp_dir("race");
    let path = dir.join("wayland-0-default.sock");

    let first = bind_instance_socket(&path).expect("first bind");
    // The second host reports the duplicate without touching the file...
    assert!(matches!(
        bind_instance_socket(&path),
        Err(InstanceError::Duplicate)
    ));
    // ...and the first owner's socket still answers.
    net::UnixStream::connect(&path).expect("the live socket survived");
    drop(first);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The exec'd command never inherits the listener: the bound descriptor
/// carries the close-on-exec flag (`F_GETFD` shows it).
#[test]
fn the_bound_listener_is_close_on_exec() {
    let dir = temp_dir("cloexec");
    let path = dir.join("wayland-0-default.sock");
    let listener = bind_instance_socket(&path).expect("bind");
    // SAFETY: `F_GETFD` takes no argument and the fd is this test's own
    // listener.
    let flags = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0, "fcntl");
    assert_ne!(flags & libc::FD_CLOEXEC, 0, "the listener is CLOEXEC");
    drop(listener);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// An [`InstanceSocket`] owns its socket file: dropping an unused socket
/// removes the file, and the name binds again at once
/// (`serve-instance-socket` task 1.3).
#[test]
fn a_dropped_unused_socket_removes_its_file() {
    let dir = temp_dir("instance-drop");
    let path = dir.join("wayland-0-default.sock");

    let socket = bind_at(&path).expect("bind");
    assert!(path.exists(), "the bind created the socket file");
    net::UnixStream::connect(&path).expect("the bind is live");
    drop(socket);
    assert!(!path.exists(), "the dropped socket's file is gone");

    // The name is free again, and the second socket cleans up the same
    // way.
    drop(bind_at(&path).expect("the name is free again"));
    assert!(!path.exists(), "the second socket's file is gone too");
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A second live [`bind_at`] on the same path is the typed duplicate
/// error, and the first socket stays untouched.
#[test]
fn a_second_live_bind_is_a_duplicate() {
    let dir = temp_dir("instance-duplicate");
    let path = dir.join("wayland-0-default.sock");

    let first = bind_at(&path).expect("first bind");
    assert!(
        matches!(bind_at(&path), Err(InstanceError::Duplicate)),
        "the live listener is a duplicate"
    );
    net::UnixStream::connect(&path).expect("the first socket still answers");
    drop(first);
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The environment-reading [`InstanceSocket::bind`], the one test that
/// goes through the process environment: with no `XDG_RUNTIME_DIR` there
/// is no socket path, and the bind fails with the typed `Path` error
/// before anything is created. The test removes the variable instead of
/// setting it — it must not depend on a real runtime directory — and no
/// other test in this binary reads it (nextest runs each test in its own
/// process besides).
#[test]
fn bind_without_a_runtime_directory_is_a_path_error() {
    // SAFETY: single-threaded with respect to this variable — no other
    // test in this process reads or writes `XDG_RUNTIME_DIR`.
    unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    let name = InstanceName::parse("notes").expect("valid");
    assert!(
        matches!(
            InstanceSocket::bind(&name),
            Err(InstanceError::Path(PathError::NoRuntimeDir))
        ),
        "a missing runtime directory is a typed path error"
    );
}

/// The listener answers exactly the `toggle\n` and `show\n` requests: each
/// runs the callback with its parsed [`Request`] and is answered `ok`, a
/// failing callback `error`, and every malformed line — any other command
/// (`hide\n` among them), an argument, an empty line, a line one byte over
/// the bound — is answered `error` without the callback running. A
/// connection that sends nothing is dropped after the bounded wait without
/// stopping the next request, and the listener stops on the shutdown flag.
/// The retries bound the test, so a wedged loop fails it instead of hanging
/// it.
#[test]
fn the_listener_answers_the_protocol_and_stops_on_shutdown() {
    let dir = temp_dir("listener");
    let path = dir.join("wayland-0-default.sock");
    let listener = bind_instance_socket(&path).expect("bind");
    let shutdown = AtomicBool::new(false);
    let fail = AtomicBool::new(false);
    let toggles = AtomicUsize::new(0);
    let shows = AtomicUsize::new(0);
    let stub = |request: Request| {
        match request {
            Request::Toggle => toggles.fetch_add(1, Ordering::Relaxed),
            Request::Show => shows.fetch_add(1, Ordering::Relaxed),
        };
        if fail.load(Ordering::Relaxed) {
            Err(())
        } else {
            Ok(())
        }
    };

    std::thread::scope(|scope| {
        // The thread reports its own exit through a flag the test polls
        // with a deadline, so a wedged loop fails the test instead of
        // hanging it.
        let stopped = std::sync::Arc::new(AtomicBool::new(false));
        let thread_stopped = std::sync::Arc::clone(&stopped);
        let listener_ref = &listener;
        let stub_ref = &stub;
        let shutdown_ref = &shutdown;
        scope.spawn(move || {
            serve_requests(listener_ref, stub_ref, shutdown_ref);
            thread_stopped.store(true, Ordering::Relaxed);
        });

        // One full exchange; retried reads absorb the bounded waits the
        // stalled connection below puts into the accept loop.
        let ask = |request: &[u8], stream: &mut net::UnixStream| -> Vec<u8> {
            write_bounded(stream, request);
            let reply =
                (0..20).find_map(|_| read_bounded(stream).filter(|reply| !reply.is_empty()));
            let Some(reply) = reply else {
                panic!("no reply within the bounded retries");
            };
            reply
        };

        // The two valid requests reach the callback with their parsed
        // [`Request`] and are `ok`.
        let mut stream = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(ask(TOGGLE_LINE, &mut stream), REPLY_OK.to_vec());
        assert_eq!(toggles.load(Ordering::Relaxed), 1, "the toggle ran");
        let mut stream = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(ask(SHOW_LINE, &mut stream), REPLY_OK.to_vec());
        assert_eq!(shows.load(Ordering::Relaxed), 1, "the show ran");

        // A failing callback — the panel thread's error case — is `error`.
        fail.store(true, Ordering::Relaxed);
        let mut stream = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(ask(SHOW_LINE, &mut stream), REPLY_ERROR.to_vec());
        assert_eq!(shows.load(Ordering::Relaxed), 2);

        // Malformed lines never reach the callback: the old focus request,
        // an unknown command, a request with an argument, a bare word
        // without a newline, an empty line and an oversized line without
        // a newline.
        fail.store(false, Ordering::Relaxed);
        for bad in [
            &b"focus niri-spawn:1\n"[..],
            b"focus\n",
            b"toggle niri-spawn:1\n",
            b"toggle \n",
            b"hide\n",
            b"hello\n",
            b"toggle",
            b"\n",
            &[b'x'; MAX_REQUEST + 1],
        ] {
            let mut stream = net::UnixStream::connect(&path).expect("connect");
            assert_eq!(ask(bad, &mut stream), REPLY_ERROR.to_vec(), "bad = {bad:?}");
        }
        assert_eq!(
            toggles.load(Ordering::Relaxed),
            1,
            "no malformed line ran the toggle"
        );
        assert_eq!(
            shows.load(Ordering::Relaxed),
            2,
            "no malformed line ran the show"
        );

        // A connection that sends nothing is dropped after the bounded
        // wait and answered with the error reply — the listener does not
        // hang on it.
        let mut stalled = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(ask(&[], &mut stalled), REPLY_ERROR.to_vec());

        // ...and the next request is still served normally.
        let mut stream = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(ask(TOGGLE_LINE, &mut stream), REPLY_OK.to_vec());
        assert_eq!(toggles.load(Ordering::Relaxed), 2, "the toggle ran");

        // The shutdown flag ends the listener within a few accept polls.
        shutdown.store(true, Ordering::Relaxed);
        for _ in 0..100 {
            if stopped.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            stopped.load(Ordering::Relaxed),
            "the listener stopped on the shutdown flag"
        );
    });
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A client that closes before the callback would run does not run it: a
/// toggle is a non-idempotent flip, so a client whose 500 ms wait ran out
/// while the listener was busy in an earlier request — the queued request
/// the loop reaches later — must be dropped, not flipped.
#[test]
fn a_peer_that_left_before_the_callback_does_not_run_it() {
    let dir = temp_dir("gone-peer");
    let path = dir.join("wayland-0-default.sock");
    let listener = bind_instance_socket(&path).expect("bind");
    let shutdown = AtomicBool::new(false);
    let seen = AtomicUsize::new(0);
    let stub = |_request: Request| {
        seen.fetch_add(1, Ordering::Relaxed);
        Ok::<(), ()>(())
    };

    std::thread::scope(|scope| {
        scope.spawn(|| serve_requests(&listener, &stub, &shutdown));

        // The client sends the request line and hangs up without waiting
        // for the reply, the shape of a client whose bounded wait ran out
        // while the listener was busy.
        let mut gone = net::UnixStream::connect(&path).expect("connect");
        write_bounded(&mut gone, TOGGLE_LINE);
        drop(gone);

        // The accept queue is FIFO, so a second client's completed exchange
        // proves the first connection was already handled by the loop.
        let mut probe = net::UnixStream::connect(&path).expect("connect");
        write_bounded(&mut probe, b"hello");
        let reply =
            (0..20).find_map(|_| read_bounded(&mut probe).filter(|reply| !reply.is_empty()));
        assert_eq!(reply, Some(REPLY_ERROR.to_vec()), "the probe exchange");
        assert_eq!(
            seen.load(Ordering::Relaxed),
            0,
            "the gone peer's request did not run the callback"
        );

        // ...and a peer that stays connected still reaches the callback.
        let mut live = net::UnixStream::connect(&path).expect("connect");
        write_bounded(&mut live, TOGGLE_LINE);
        let reply = (0..20).find_map(|_| read_bounded(&mut live).filter(|reply| !reply.is_empty()));
        assert_eq!(reply, Some(REPLY_OK.to_vec()), "the live exchange");
        assert_eq!(
            seen.load(Ordering::Relaxed),
            1,
            "the live peer's request ran the callback"
        );

        shutdown.store(true, Ordering::Relaxed);
    });
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A peer that half-closes — sends the request, shuts down its write side
/// and keeps reading — still gets its request answered: an end-of-file on
/// the read side is only a write-side shutdown, not the peer leaving, so
/// the piped-in client shape (a `socat` whose stdin closed) keeps working.
/// The `peer_gone` check must tell a full close (the test above, no
/// callback) from this half-close (the callback runs, the reply goes
/// out).
#[test]
fn a_half_closed_peer_still_gets_the_toggle() {
    let dir = temp_dir("half-close");
    let path = dir.join("wayland-0-default.sock");
    let listener = bind_instance_socket(&path).expect("bind");
    let shutdown = AtomicBool::new(false);
    let seen = AtomicUsize::new(0);
    let stub = |_request: Request| {
        seen.fetch_add(1, Ordering::Relaxed);
        Ok::<(), ()>(())
    };

    std::thread::scope(|scope| {
        scope.spawn(|| serve_requests(&listener, &stub, &shutdown));

        // The client sends the request line, half-closes its write side
        // and stays connected to read the reply.
        let mut half = net::UnixStream::connect(&path).expect("connect");
        write_bounded(&mut half, TOGGLE_LINE);
        half.shutdown(std::net::Shutdown::Write)
            .expect("half close");

        // The ok reply itself proves the loop handled this connection and
        // ran the callback for it.
        let reply = (0..20).find_map(|_| read_bounded(&mut half).filter(|reply| !reply.is_empty()));
        assert_eq!(reply, Some(REPLY_OK.to_vec()), "the half-closed exchange");
        assert_eq!(
            seen.load(Ordering::Relaxed),
            1,
            "the half-closed peer's request ran the callback"
        );

        shutdown.store(true, Ordering::Relaxed);
    });
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// The client call against a scripted host (`serve-instance-socket` task
/// 2.2): the request line goes out as the request's word plus a newline
/// (`toggle` and `show` both exercised); `ok` succeeds, `error` is the
/// typed refusal, a garbage reply and a host that never replies are no
/// valid answer in time, and a connect that fails with `ENOENT` (no socket
/// file) or `ECONNREFUSED` (a stale file left by an instance that no
/// longer runs) is not-listening. The message texts are pinned, since the
/// binary prints them after its `pinwin: ` prefix.
#[test]
fn send_to_reports_the_reply_and_the_connect_failures() {
    let dir = temp_dir("client");
    let path = dir.join("wayland-0-default.sock");

    // Bind a one-shot host that asserts the request line and sends
    // `reply`. Dropping a listener leaves its socket file behind (a
    // stale file), so each case unlinks it before it binds.
    let scripted = |request: Request, reply: &[u8]| {
        let _ = fs::remove_file(&path);
        let host = net::UnixListener::bind(&path).expect("host bind");
        let reply = reply.to_vec();
        let expected = request.line().to_vec();
        std::thread::spawn(move || {
            let (mut stream, _) = host.accept().expect("accept");
            let mut buffer = [0u8; MAX_REQUEST];
            let mut read = 0;
            while read < buffer.len() && !buffer[..read].ends_with(b"\n") {
                match stream.read(&mut buffer[read..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => read += n,
                }
            }
            assert_eq!(&buffer[..read], expected.as_slice(), "the request line");
            let _ = stream.write_all(&reply);
        })
    };

    // `ok` succeeds, for both requests.
    for request in [Request::Toggle, Request::Show] {
        let host = scripted(request, REPLY_OK);
        assert!(
            send_to(&path, request).is_ok(),
            "the request is answered ok"
        );
        host.join().expect("host thread");
    }

    // `error` is the typed refusal carrying the request; the message text
    // says what did not happen.
    for (request, did_not) in [
        (Request::Toggle, "did not toggle"),
        (Request::Show, "did not show"),
    ] {
        let host = scripted(request, REPLY_ERROR);
        let error = send_to(&path, request).expect_err("the host refused");
        host.join().expect("host thread");
        assert!(
            matches!(error, SendError::Refused { request: refused } if refused == request),
            "an error reply is a refusal of the request"
        );
        assert!(
            error.to_string().contains(did_not),
            "the refusal names the request: {error}"
        );
    }

    // A garbage reply is no valid answer in time, and the message names
    // the path.
    let host = scripted(Request::Show, b"nope\n");
    let error = send_to(&path, Request::Show).expect_err("a garbage reply");
    host.join().expect("host thread");
    assert!(
        matches!(error, SendError::NoAnswer { .. }),
        "the error class"
    );
    assert_eq!(
        error.to_string(),
        format!("no valid answer came from {} in time", path.display())
    );

    // A host that accepts but never replies is dropped after the bounded
    // wait.
    let _ = fs::remove_file(&path);
    let host = net::UnixListener::bind(&path).expect("silent host bind");
    let silent = std::thread::spawn(move || {
        let _ = host.accept().expect("accept");
        std::thread::sleep(HANDSHAKE_TIMEOUT + std::time::Duration::from_millis(200));
    });
    let error = send_to(&path, Request::Toggle).expect_err("a silent host");
    silent.join().expect("silent host thread");
    assert!(
        matches!(error, SendError::NoAnswer { .. }),
        "the error class"
    );
    assert_eq!(
        error.to_string(),
        format!("no valid answer came from {} in time", path.display())
    );

    // No socket file at all: the connect fails with `ENOENT`, the
    // not-listening class.
    let _ = fs::remove_file(&path);
    let error = send_to(&path, Request::Toggle).expect_err("no socket file");
    assert!(
        matches!(error, SendError::NotListening { .. }),
        "a missing socket file is not listening"
    );
    assert!(
        error.to_string().contains("no pinwin is listening on")
            && error.to_string().contains(&*path.display().to_string()),
        "the message names the path: {error}"
    );

    // A stale file — the socket of an instance that no longer runs —
    // refuses the connect (`ECONNREFUSED`) and is the same class, not
    // no-answer.
    drop(net::UnixListener::bind(&path).expect("stale bind"));
    let error = send_to(&path, Request::Show).expect_err("a stale file");
    assert!(
        matches!(error, SendError::NotListening { .. }),
        "a stale socket file is not listening"
    );

    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A connect failure that is neither `ENOENT` nor `ECONNREFUSED` keeps the
/// no-answer class (`serve-instance-socket` D4) without losing the cause:
/// the message names the failed connect and the errno, and `source()`
/// carries the connect error. A path through a regular file fails with
/// `ENOTDIR`.
#[test]
fn send_to_names_a_connect_error_that_is_not_missing_or_stale() {
    let dir = temp_dir("noanswer-cause");
    let file = dir.join("a-regular-file");
    fs::write(&file, b"not a directory").expect("regular file");
    let path = file.join("wayland-0-default.sock");

    let error = send_to(&path, Request::Toggle).expect_err("ENOTDIR");
    assert!(
        matches!(error, SendError::NoAnswer { error: Some(_), .. }),
        "the no-answer class keeps the connect error"
    );
    assert_eq!(
        error.to_string(),
        format!(
            "could not connect to {} ({})",
            path.display(),
            io::Error::from_raw_os_error(libc::ENOTDIR)
        )
    );
    let source = std::error::Error::source(&error)
        .and_then(|error| error.downcast_ref::<io::Error>())
        .expect("the connect error as the source");
    assert_eq!(source.raw_os_error(), Some(libc::ENOTDIR));

    fs::remove_dir_all(&dir).expect("cleanup");
}
