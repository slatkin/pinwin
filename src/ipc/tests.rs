//! The wire-format tests of replace-terminal-with-app-host row 5.1: one
//! JSON object per line, the error kinds D13 names, the 64 KiB line limit
//! and the client call, all without a daemon.
//!
//! The request list is checked against `cli::SUBCOMMANDS` in both
//! directions, so a client subcommand without a request variant and a
//! request variant without a client subcommand cannot slip through.

use std::ffi::OsString;
use std::io::Write as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

use super::{CallError, ErrorKind, LineError, MAX_LINE, Reply, Request, read_line, write_line};
use crate::cli::{self, Command};
use crate::name::PanelName;

/// The panel name the request tests use.
fn notes() -> PanelName {
    PanelName::parse("notes").expect("notes is a panel name")
}

/// A request line parses into the request, and a success reply is the one
/// line the spec shows (spec "Valid request").
#[test]
fn a_valid_request_parses_and_the_reply_is_one_ok_line() {
    let request =
        Request::parse(br#"{"cmd":"toggle","panel":"notes"}"#).expect("a valid request line");
    assert_eq!(request, Request::Toggle { panel: notes() });
    assert_eq!(
        request.to_line(),
        "{\"cmd\":\"toggle\",\"panel\":\"notes\"}\n"
    );

    let reply = Reply::ok();
    assert!(reply.is_ok());
    assert_eq!(reply.to_line(), "{\"ok\":true}\n");
    assert_eq!(Reply::parse(br#"{"ok":true}"#), Ok(Reply::Ok));
}

/// The size and resize forms the spec names: text, a JSON number of pixels,
/// a percent and the optional `ms` (spec "Size values", "Animated size
/// changes").
#[test]
fn the_wire_size_forms_parse() {
    for (line, expected) in [
        (
            &br#"{"cmd":"set-size","panel":"notes","size":"640px"}"#[..],
            Request::SetSize {
                panel: notes(),
                size: crate::geometry::Size::pixels(640).expect("640 pixels"),
                ms: None,
            },
        ),
        (
            br#"{"cmd":"set-size","panel":"notes","size":640}"#,
            Request::SetSize {
                panel: notes(),
                size: crate::geometry::Size::pixels(640).expect("640 pixels"),
                ms: None,
            },
        ),
        (
            br#"{"cmd":"set-size","panel":"notes","size":"30%","ms":0}"#,
            Request::SetSize {
                panel: notes(),
                size: crate::geometry::Size::percent(30).expect("30 percent"),
                ms: Some(0),
            },
        ),
        (
            br#"{"cmd":"resize","panel":"notes","by":"-40px"}"#,
            Request::Resize {
                panel: notes(),
                by: crate::cli::Amount::shrink(
                    crate::geometry::Size::pixels(40).expect("40 pixels"),
                ),
                ms: None,
            },
        ),
        (
            br#"{"cmd":"resize","panel":"notes","by":-40}"#,
            Request::Resize {
                panel: notes(),
                by: crate::cli::Amount::shrink(
                    crate::geometry::Size::pixels(40).expect("40 pixels"),
                ),
                ms: None,
            },
        ),
        (
            br#"{"cmd":"resize","panel":"notes","by":"+10%","ms":200}"#,
            Request::Resize {
                panel: notes(),
                by: crate::cli::Amount::grow(
                    crate::geometry::Size::percent(10).expect("10 percent"),
                ),
                ms: Some(200),
            },
        ),
    ] {
        let request = Request::parse(line).unwrap_or_else(|error| panic!("{line:?}: {error}"));
        assert_eq!(
            request,
            expected,
            "line = {:?}",
            String::from_utf8_lossy(line)
        );
        assert_eq!(
            Request::parse(request.to_line().as_bytes()).expect("the request round-trips"),
            request
        );
    }
}

/// A line that is not JSON is `invalid_request`, and the reply names it
/// (spec "Not JSON").
#[test]
fn a_line_that_is_not_json_is_invalid_request() {
    let error = Request::parse(b"toggle notes").expect_err("not JSON");
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
    let reply = Reply::from(error);
    let error = reply.as_error().expect("an error reply");
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
    assert_ne!(error.message(), "");
}

/// A known JSON shape with an unknown `cmd` is `invalid_request` (spec
/// "Unknown command").
#[test]
fn an_unknown_command_is_invalid_request() {
    let error = Request::parse(br#"{"cmd":"restart-app","panel":"notes"}"#)
        .expect_err("an unknown command");
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
    assert_eq!(
        Reply::from(error).as_error().map(super::ErrorReply::kind),
        Some(ErrorKind::InvalidRequest)
    );
}

/// A known command whose setting is outside the grammar is `invalid_value`,
/// not `invalid_request` (spec "Invalid value"), because the daemon must
/// name the value and change nothing.
#[test]
fn an_invalid_value_is_invalid_value() {
    for line in [
        &br#"{"cmd":"set-edge","panel":"notes","edge":"middle"}"#[..],
        br#"{"cmd":"set-size","panel":"notes","size":"150%"}"#,
        br#"{"cmd":"set-size","panel":"notes","size":"wide"}"#,
        br#"{"cmd":"resize","panel":"notes","by":"40px"}"#,
        br#"{"cmd":"set-size","panel":"notes","size":"640px","ms":-1}"#,
    ] {
        let error = Request::parse(line).expect_err("an invalid value");
        assert_eq!(
            error.kind(),
            ErrorKind::InvalidValue,
            "line = {:?}",
            String::from_utf8_lossy(line)
        );
    }

    // A command that is not a panel command stays a request error.
    let error =
        Request::parse(br#"{"cmd":"set-resize","panel":"notes"}"#).expect_err("an unknown command");
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
}

/// A JSON number the typed field cannot hold is still a known command with a
/// bad value, so it is `invalid_value`; only a wrong JSON type is
/// `invalid_request` (spec "Panel commands", "Size values").
#[test]
fn a_number_out_of_range_or_fractional_is_invalid_value() {
    for line in [
        &br#"{"cmd":"set-size","panel":"notes","size":-5}"#[..],
        br#"{"cmd":"set-size","panel":"notes","size":4294967296}"#,
        br#"{"cmd":"set-size","panel":"notes","size":1.5}"#,
        br#"{"cmd":"set-size","panel":"notes","size":0}"#,
        br#"{"cmd":"resize","panel":"notes","by":1.5}"#,
        br#"{"cmd":"set-size","panel":"notes","size":"640px","ms":1.5}"#,
        br#"{"cmd":"set-size","panel":"notes","size":"640px","ms":-1}"#,
    ] {
        let error = Request::parse(line).expect_err("an invalid value");
        assert_eq!(
            error.kind(),
            ErrorKind::InvalidValue,
            "line = {:?}",
            String::from_utf8_lossy(line)
        );
    }

    // A wrong JSON type is not a value, so it stays a request error.
    for line in [
        &br#"{"cmd":"set-size","panel":"notes","size":[1]}"#[..],
        br#"{"cmd":"set-size","panel":"notes","size":true}"#,
    ] {
        let error = Request::parse(line).expect_err("a wrong type");
        assert_eq!(
            error.kind(),
            ErrorKind::InvalidRequest,
            "line = {:?}",
            String::from_utf8_lossy(line)
        );
    }
}

/// Every error kind of D13 has its wire name, and a failed reply round-trips
/// through the reply parser.
#[test]
fn every_error_kind_has_its_wire_name() {
    for (kind, name) in [
        (ErrorKind::InvalidRequest, "invalid_request"),
        (ErrorKind::InvalidValue, "invalid_value"),
        (ErrorKind::InvalidLayout, "invalid_layout"),
        (ErrorKind::NoPanel, "no_panel"),
        (ErrorKind::DuplicateName, "duplicate_name"),
        (ErrorKind::InvalidConfig, "invalid_config"),
        (ErrorKind::Internal, "internal"),
    ] {
        let reply = Reply::error(kind, "message");
        let line = reply.to_line();
        let value: serde_json::Value = serde_json::from_str(&line).expect("a reply is JSON");
        assert_eq!(value["ok"], serde_json::json!(false), "kind = {name}");
        assert_eq!(value["error"]["kind"], serde_json::json!(name));
        assert_eq!(value["error"]["message"], serde_json::json!("message"));
        assert_eq!(Reply::parse(line.as_bytes()), Ok(reply));
    }
}

/// A status reply carries the daemon's process ID and one entry per panel,
/// with every field the spec names (spec "Status").
#[test]
fn a_status_reply_carries_the_panel_fields() {
    let status =
        super::PanelStatus::new(crate::config::PanelSettings::built_in(), true, false, 640);
    let reply = super::Reply::status(4242, vec![super::PanelEntry::new(notes(), 77, status)]);
    assert!(reply.is_ok());
    assert_eq!(reply.as_status().map(super::StatusReply::pid), Some(4242));

    let value: serde_json::Value = serde_json::from_str(&reply.to_line()).expect("a reply is JSON");
    assert_eq!(value["ok"], serde_json::json!(true));
    assert_eq!(value["pid"], serde_json::json!(4242));
    assert_eq!(
        value["panels"],
        serde_json::json!([{
            "name": "notes",
            "pid": 77,
            "shown": true,
            "has_window": false,
            "edge": "left",
            "size": "25%",
            "width": 640,
            "reserve": "push",
            "layer": "overlay",
            "keyboard": "on-demand",
            "output": null,
        }])
    );

    // A success without panels keeps the one-line reply the client reads.
    assert_eq!(super::Reply::ok().to_line(), "{\"ok\":true}\n");
}

/// One request line longer than 64 KiB is refused, and the refusal carries
/// the spec's error kind (spec "Clients cannot block the daemon").
#[test]
fn an_over_long_line_is_refused() {
    let (mut client, mut server) = UnixStream::pair().expect("a socket pair");
    let writer = std::thread::spawn(move || {
        client
            .write_all(&vec![b'x'; MAX_LINE + 1])
            .expect("write the over-long line");
        client
            .write_all(b"\n")
            .expect("write the over-long line's newline");
    });
    let error = read_line(&mut server, MAX_LINE, Duration::from_secs(1))
        .expect_err("a line over the limit");
    assert!(matches!(error, LineError::TooLong), "error = {error:?}");
    assert_eq!(
        error.reply().as_error().map(super::ErrorReply::kind),
        Some(ErrorKind::InvalidRequest)
    );
    writer.join().expect("the writer finishes");
}

/// The client writes one request line, reads one reply line and returns the
/// parsed reply, over a socket pair with no daemon.
#[test]
fn the_client_exchanges_one_request_and_reply() {
    let (client, mut server) = UnixStream::pair().expect("a socket pair");
    let daemon = std::thread::spawn(move || {
        let line =
            read_line(&mut server, MAX_LINE, Duration::from_secs(1)).expect("the request line");
        assert_eq!(line, b"{\"cmd\":\"show\",\"panel\":\"notes\"}\n");
        write_line(
            &mut server,
            b"{\"ok\":false,\"error\":{\"kind\":\"no_panel\",\"message\":\"no panel notes\"}}\n",
            Duration::from_secs(1),
        )
        .expect("the reply line");
    });
    let reply = super::exchange(
        client,
        &Request::Show { panel: notes() },
        Duration::from_secs(1),
    )
    .expect("a reply");
    assert_eq!(
        reply.as_error().map(super::ErrorReply::kind),
        Some(ErrorKind::NoPanel)
    );
    daemon.join().expect("the daemon finishes");
}

/// The client call connects to the socket path and returns the reply, so a
/// daemon is only a listener in the test.
#[test]
fn the_client_call_connects_writes_and_reads() {
    let path = std::env::temp_dir().join(format!("pinwin-ipc-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("a scratch socket");
    let daemon = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the client connects");
        let line =
            read_line(&mut stream, MAX_LINE, Duration::from_secs(1)).expect("the request line");
        assert_eq!(line, b"{\"cmd\":\"quit\"}\n");
        write_line(&mut stream, b"{\"ok\":true}\n", Duration::from_secs(1))
            .expect("the reply line");
    });
    let reply = super::call(&path, &Request::Quit).expect("a reply");
    assert_eq!(reply, Reply::Ok);
    daemon.join().expect("the daemon finishes");
    std::fs::remove_file(&path).expect("remove the scratch socket");
}

/// A daemon that takes the request and never answers cannot hold the client
/// past the timeout (spec "Command-line exit codes").
#[test]
fn the_client_gives_up_when_no_reply_comes() {
    let (client, mut server) = UnixStream::pair().expect("a socket pair");
    let daemon = std::thread::spawn(move || {
        let _line =
            read_line(&mut server, MAX_LINE, Duration::from_secs(1)).expect("the request line");
        std::thread::sleep(Duration::from_millis(200));
    });
    let started = Instant::now();
    let error =
        super::exchange(client, &Request::Quit, Duration::from_millis(50)).expect_err("no reply");
    assert!(
        matches!(error, CallError::Receive(LineError::Timeout)),
        "error = {error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the client gave up after {:?}",
        started.elapsed()
    );
    daemon.join().expect("the daemon finishes");
}

/// Every request variant is reachable from the client subcommand of the
/// same name, and the request list and `cli::SUBCOMMANDS` line up in both
/// directions (spec "Command-line client").
#[test]
fn every_client_subcommand_has_one_request_variant() {
    assert_eq!(cli::SUBCOMMANDS[2..], super::REQUEST_COMMANDS[..]);

    let samples: [&[&str]; 15] = [
        &["toggle", "notes"],
        &["show", "notes"],
        &["hide", "notes"],
        &["focus", "notes"],
        &["set-edge", "notes", "right"],
        &["set-size", "notes", "640px"],
        &["resize", "notes", "-40px"],
        &["set-output", "notes", "eDP-1"],
        &["set-layer", "notes", "overlay"],
        &["set-reserve", "notes", "push"],
        &["set-keyboard", "notes", "on-demand"],
        &["status"],
        &["subscribe"],
        &["quit"],
        &["reload"],
    ];
    for (&subcommand, sample) in super::REQUEST_COMMANDS.iter().zip(samples) {
        let args: Vec<OsString> = sample.iter().map(OsString::from).collect();
        let Command::Client(command) = cli::parse_args(&args).expect("a client command") else {
            panic!("{subcommand} is not a client subcommand");
        };
        let request = Request::from(command);
        let line = request.to_line();
        let value: serde_json::Value = serde_json::from_str(&line).expect("a request is JSON");
        assert_eq!(
            value["cmd"],
            serde_json::json!(subcommand),
            "line = {line:?}"
        );
        assert_eq!(
            Request::parse(line.as_bytes()).expect("the request round-trips"),
            request,
            "subcommand = {subcommand}"
        );
    }
}
