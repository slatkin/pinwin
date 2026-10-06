//! The `ipc` module's tests, split from `ipc.rs` to keep the module at its
//! line budget (the split is mechanical; every item stays reachable through
//! `use super::*`).

use super::*;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

/// A unique scratch directory under the system temp dir, so the tests
/// never touch the process environment and never collide (nextest runs
/// each test in its own process; a plain `cargo test` shares one).
fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pinwin-ipc-test-{}-{}-{}",
        std::process::id(),
        tag,
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).expect("temp dir");
    dir
}

/// The request line for `token`: `focus <token>\n`
/// (replace-gtk-with-wayland D4).
fn request(token: &[u8]) -> Vec<u8> {
    let mut line = REQUEST_PREFIX.to_vec();
    line.extend_from_slice(token);
    line.push(b'\n');
    line
}

/// A valid name parses, and the boundaries of the character set, the
/// length range and the error message hold.
#[test]
fn the_instance_name_validates() {
    let name = InstanceName::parse("PINWIN_NAME", "notes").expect("valid");
    assert_eq!(&*name, "notes");
    assert_eq!(name.to_string(), "notes");

    // The whole allowed character set, and the 64-character boundary.
    InstanceName::parse("x", "A-z_09-").expect("the full allowed set");
    InstanceName::parse("x", &"a".repeat(64)).expect("64 characters");

    // Empty, too long, and characters outside the set.
    for raw in ["", &"a".repeat(65), "a/b", "a b", "a.b", "ä"] {
        let error = InstanceName::parse("PINWIN_NAME", raw).expect_err(raw);
        assert_eq!(
            error,
            format!(
                "pinwin: PINWIN_NAME: expected a name of 1..=64 characters from \
                 [A-Za-z0-9_-], got '{raw}'"
            ),
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
    let name = InstanceName::parse("x", "notes").expect("valid");
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
    // display are all exit-2 errors: the path must stay inside
    // `$XDG_RUNTIME_DIR/pinwin` and distinct displays must stay distinct.
    for raw in [
        os(""),
        os("way/land"),
        os("/abs/path"),
        OsStr::from_bytes(&[b'w', 0xff]),
    ] {
        let error =
            socket_path(Some(os("/run/user/1000")), Some(raw), &name).expect_err("bad display");
        assert_eq!(
            error,
            format!(
                "pinwin: WAYLAND_DISPLAY: expected a UTF-8 display name without '/', \
                 got '{}'",
                raw.to_string_lossy()
            ),
            "raw = {:?}",
            raw
        );
    }

    // A missing or empty runtime directory is an exit-2 error.
    socket_path(None, Some(os("wayland-0")), &name).expect_err("no runtime directory");
    socket_path(Some(os("")), Some(os("wayland-0")), &name).expect_err("empty runtime directory");
}

/// The request parser accepts exactly `focus <token>\n` with a token the
/// constructor accepts, and rejects every other shape: the old bare
/// `focus\n`, an empty token after the space, a space inside the token, a
/// non-ASCII byte, a 256-byte token, a missing newline and a non-UTF-8
/// byte.
#[test]
fn parse_request_takes_only_a_valid_token_line() {
    assert_eq!(
        parse_request(&request(b"niri-spawn:1"))
            .as_ref()
            .map(ActivationToken::as_str),
        Some("niri-spawn:1")
    );

    // The longest valid token parses.
    let longest = "a".repeat(MAX_TOKEN_BYTES);
    assert_eq!(
        parse_request(&request(longest.as_bytes()))
            .as_ref()
            .map(ActivationToken::as_str),
        Some(longest.as_str())
    );

    for bad in [
        &b"focus\n"[..],
        b"focus \n",
        b"focus a b\n",
        b"focus a\tb\n",
        "focus é\n".as_bytes(),
        &request("a".repeat(MAX_TOKEN_BYTES + 1).as_bytes()),
        b"focus niri-spawn:1",
        b"niri-spawn:1\n",
        b"focus",
    ] {
        assert!(parse_request(bad).is_none(), "bad = {bad:?}");
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
        matches!(bind_instance_socket(&path), Err(BindError::Duplicate)),
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
        Err(BindError::Duplicate)
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

/// The token protocol over a real bound socket: a valid token reaches the
/// focus callback and is answered `ok`, a failing callback `error`, and
/// every malformed line — no token, an empty token, an invalid token, a
/// line one byte over the bound — is answered `error` without the callback
/// running. The longest valid request (a 255-byte token) is accepted. A
/// connection that sends nothing is dropped after the bounded wait without
/// stopping the next request, and the listener stops on the shutdown flag.
/// The retries bound the test, so a wedged loop fails it instead of
/// hanging it.
#[test]
fn the_listener_answers_the_token_protocol_and_stops_on_shutdown() {
    let dir = temp_dir("listener");
    let path = dir.join("wayland-0-default.sock");
    let listener = bind_instance_socket(&path).expect("bind");
    let shutdown = AtomicBool::new(false);
    let fail = AtomicBool::new(false);
    let seen: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let stub = |token: &ActivationToken| {
        seen.lock().expect("seen").push(token.as_str().to_owned());
        if fail.load(Ordering::Relaxed) {
            Err(())
        } else {
            Ok(())
        }
    };
    let longest = "a".repeat(MAX_TOKEN_BYTES);

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
            serve_focus_requests(listener_ref, stub_ref, shutdown_ref);
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

        // A valid token reaches the callback verbatim and is `ok`.
        let mut stream = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(
            ask(&request(b"niri-spawn:1"), &mut stream),
            REPLY_OK.to_vec()
        );
        assert_eq!(
            seen.lock().expect("seen").as_slice(),
            ["niri-spawn:1"],
            "the callback saw the token"
        );

        // The longest valid request (a 255-byte token) is accepted too.
        let mut stream = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(
            ask(&request(longest.as_bytes()), &mut stream),
            REPLY_OK.to_vec()
        );
        assert_eq!(seen.lock().expect("seen").len(), 2);

        // A failing callback — the `request_focus` error case — is `error`.
        fail.store(true, Ordering::Relaxed);
        let mut stream = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(
            ask(&request(b"niri-spawn:2"), &mut stream),
            REPLY_ERROR.to_vec()
        );
        assert_eq!(seen.lock().expect("seen").len(), 3);

        // Malformed lines never reach the callback: the old bare request,
        // an empty token after the space, an unknown command, a token with
        // a space inside, a non-ASCII token, a 256-byte token (its line is
        // one byte over the bound) and an oversized line without a
        // newline.
        fail.store(false, Ordering::Relaxed);
        for bad in [
            &b"focus\n"[..],
            b"focus \n",
            b"hello\n",
            b"focus a b\n",
            "focus é\n".as_bytes(),
            &request("a".repeat(MAX_TOKEN_BYTES + 1).as_bytes()),
            &vec![b'x'; MAX_REQUEST + 1],
        ] {
            let mut stream = net::UnixStream::connect(&path).expect("connect");
            assert_eq!(ask(bad, &mut stream), REPLY_ERROR.to_vec(), "bad = {bad:?}");
        }
        assert_eq!(seen.lock().expect("seen").len(), 3, "no malformed line ran");

        // A connection that sends nothing is dropped after the bounded
        // wait and answered with the error reply — the listener does not
        // hang on it.
        let mut stalled = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(ask(&[], &mut stalled), REPLY_ERROR.to_vec());

        // ...and the next request is still served normally.
        let mut stream = net::UnixStream::connect(&path).expect("connect");
        assert_eq!(
            ask(&request(b"niri-spawn:3"), &mut stream),
            REPLY_OK.to_vec()
        );
        assert_eq!(seen.lock().expect("seen").len(), 4);

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

/// The client's exchange against a scripted host: the request line goes
/// out as `focus <token>\n`; `ok` succeeds, `error` and a garbage reply are
/// reported as the request not being answered, a host that never replies
/// is dropped after the bounded wait, and a stale path (connect
/// refused) too.
#[test]
fn the_focus_client_sends_the_token_and_reports_the_reply() {
    let dir = temp_dir("client");
    let path = dir.join("wayland-0-default.sock");
    let token = ActivationToken::new("niri-spawn:client-test").expect("valid token");
    let expected_line = request(token.as_str().as_bytes());

    // Bind a one-shot host that asserts the request line and sends
    // `reply`. Dropping a listener leaves its socket file behind (a
    // stale file), so each case unlinks it before it binds.
    let scripted = |reply: &[u8]| {
        let _ = fs::remove_file(&path);
        let host = net::UnixListener::bind(&path).expect("host bind");
        let reply = reply.to_vec();
        let expected = expected_line.clone();
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

    // `ok` succeeds.
    let host = scripted(REPLY_OK);
    assert_eq!(focus_client(&path, &token), Ok(()));
    host.join().expect("host thread");

    // `error` reports the request as refused.
    let host = scripted(REPLY_ERROR);
    assert!(
        matches!(
            focus_client(&path, &token),
            Err(FocusError::NotAnswered(message)) if message.contains("could not take focus")
        ),
        "an error reply is not answered"
    );
    host.join().expect("host thread");

    // A garbage reply is no valid answer.
    let host = scripted(b"nope\n");
    assert!(matches!(
        focus_client(&path, &token),
        Err(FocusError::NotAnswered(message)) if message.contains("no valid answer")
    ));
    host.join().expect("host thread");

    // A host that accepts but never replies is dropped after the
    // bounded wait.
    let _ = fs::remove_file(&path);
    let host = net::UnixListener::bind(&path).expect("silent host bind");
    let silent = std::thread::spawn(move || {
        let _ = host.accept().expect("accept");
        std::thread::sleep(HANDSHAKE_TIMEOUT + std::time::Duration::from_millis(200));
    });
    assert!(
        matches!(
            focus_client(&path, &token),
            Err(FocusError::NotAnswered(message)) if message.contains("no valid answer")
        ),
        "a silent host is not answered in time"
    );
    silent.join().expect("silent host thread");

    // No host at all: the file the last listener left behind is stale
    // (nothing answers the connect).
    assert!(
        matches!(
            focus_client(&path, &token),
            Err(FocusError::NotAnswered(message)) if message.contains("no pinwin")
        ),
        "a stale path is not answered"
    );

    fs::remove_dir_all(&dir).expect("cleanup");
}
