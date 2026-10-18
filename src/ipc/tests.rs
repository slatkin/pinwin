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
                reserve: None,
                ms: None,
            },
        ),
        (
            br#"{"cmd":"set-size","panel":"notes","size":640}"#,
            Request::SetSize {
                panel: notes(),
                size: crate::geometry::Size::pixels(640).expect("640 pixels"),
                reserve: None,
                ms: None,
            },
        ),
        (
            br#"{"cmd":"set-size","panel":"notes","size":"30%","ms":0}"#,
            Request::SetSize {
                panel: notes(),
                size: crate::geometry::Size::percent(30).expect("30 percent"),
                reserve: None,
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
                reserve: None,
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
                reserve: None,
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
                reserve: None,
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

/// `set-size` and `resize` take an optional `reserve` (D4, spec "Size and
/// reserve in one request"): an absent field parses as no change, so a
/// 0.4.0 line round-trips unchanged, and a present one parses with the
/// setting grammar.
#[test]
fn the_optional_reserve_parses_with_the_size_commands() {
    let legacy = br#"{"cmd":"set-size","panel":"notes","size":"640px","ms":200}"#;
    let request = Request::parse(legacy).expect("a 0.4.0 set-size line");
    assert_eq!(
        request,
        Request::SetSize {
            panel: notes(),
            size: crate::geometry::Size::pixels(640).expect("640 pixels"),
            reserve: None,
            ms: Some(200),
        }
    );
    assert_eq!(
        request.to_line(),
        "{\"cmd\":\"set-size\",\"panel\":\"notes\",\"size\":\"640px\",\"ms\":200}\n",
        "a 0.4.0 line round-trips without the new field"
    );

    assert_eq!(
        Request::parse(
            br#"{"cmd":"set-size","panel":"notes","size":"1100px","reserve":"cover","ms":200}"#
        )
        .expect("a set-size with a reserve mode"),
        Request::SetSize {
            panel: notes(),
            size: crate::geometry::Size::pixels(1100).expect("1100 pixels"),
            reserve: Some(crate::geometry::ReserveMode::Cover),
            ms: Some(200),
        }
    );
    assert_eq!(
        Request::parse(br#"{"cmd":"resize","panel":"notes","by":"-700px","reserve":"push"}"#)
            .expect("a resize with a reserve mode"),
        Request::Resize {
            panel: notes(),
            by: crate::cli::Amount::shrink(crate::geometry::Size::pixels(700).expect("700 pixels"),),
            reserve: Some(crate::geometry::ReserveMode::Push),
            ms: None,
        }
    );
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
        br#"{"cmd":"set-size","panel":"notes","size":"1100px","reserve":"middle"}"#,
        br#"{"cmd":"resize","panel":"notes","by":"-40px","reserve":"middle"}"#,
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
    assert_eq!(
        reply.as_status().map(super::reply::StatusReply::pid),
        Some(4242)
    );

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
    let answer = super::client::exchange(
        client,
        &Request::Show { panel: notes() },
        Duration::from_secs(1),
    )
    .expect("a reply");
    assert_eq!(
        answer.reply().as_error().map(super::ErrorReply::kind),
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
    let answer = super::call(&path, &Request::Quit).expect("a reply");
    assert_eq!(answer.reply(), &Reply::Ok);
    assert_eq!(answer.line(), b"{\"ok\":true}\n");
    daemon.join().expect("the daemon finishes");
    std::fs::remove_file(&path).expect("remove the scratch socket");
}

/// The client keeps the reply line as the daemon wrote it, so `pinwin
/// status` prints the daemon's JSON unchanged (spec "Command-line exit
/// codes": "`pinwin status` SHALL print the reply's JSON on stdout").
#[test]
fn the_client_keeps_the_status_line_it_prints() {
    let (client, mut server) = UnixStream::pair().expect("a socket pair");
    let daemon = std::thread::spawn(move || {
        let line =
            read_line(&mut server, MAX_LINE, Duration::from_secs(1)).expect("the request line");
        assert_eq!(line, b"{\"cmd\":\"status\"}\n");
        write_line(
            &mut server,
            b"{\"ok\":true,\"pid\":4242,\"panels\":[]}\n",
            Duration::from_secs(1),
        )
        .expect("the reply line");
    });
    let answer = super::client::exchange(
        client,
        &Request::Status { panel: None },
        Duration::from_secs(1),
    )
    .expect("a reply");
    assert!(answer.reply().is_ok());
    assert_eq!(answer.line(), b"{\"ok\":true,\"pid\":4242,\"panels\":[]}\n");
    daemon.join().expect("the daemon finishes");
}

/// A `subscribe` client reads the success reply and then one event line
/// after another until the daemon closes the stream, so `pinwin subscribe`
/// prints one JSON line for the event (spec "Command-line client", "Event
/// stream").
#[test]
fn a_subscription_reads_the_reply_and_the_event_lines() {
    let (client, mut server) = UnixStream::pair().expect("a socket pair");
    let daemon = std::thread::spawn(move || {
        let line =
            read_line(&mut server, MAX_LINE, Duration::from_secs(1)).expect("the request line");
        assert_eq!(line, b"{\"cmd\":\"subscribe\"}\n");
        write_line(&mut server, b"{\"ok\":true}\n", Duration::from_secs(1))
            .expect("the reply line");
        write_line(
            &mut server,
            b"{\"event\":\"shown\",\"panel\":\"notes\"}\n",
            Duration::from_secs(1),
        )
        .expect("the event line");
    });
    let mut subscription =
        super::client::Subscription::over(client).expect("the subscribe request");
    assert_eq!(subscription.reply().expect("the success reply"), Reply::Ok);
    assert_eq!(
        subscription.next_event().expect("an event line"),
        Some(b"{\"event\":\"shown\",\"panel\":\"notes\"}\n".to_vec())
    );
    assert_eq!(
        subscription.next_event().expect("the stream ends"),
        None,
        "a closed stream ends the subscription"
    );
    daemon.join().expect("the daemon finishes");
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
    let error = super::client::exchange(client, &Request::Quit, Duration::from_millis(50))
        .expect_err("no reply");
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
        let Command::Client(request) = cli::parse_args(&args).expect("a client command") else {
            panic!("{subcommand} is not a client subcommand");
        };
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

/// A `create` request carries the panel's name, the caller's process ID and
/// the run options in their setting grammar, and its reply names the display
/// the app connects with (spec "Run an app in a panel", D2, row 7.1).
#[test]
fn a_create_request_carries_the_run_options_and_its_reply_names_the_display() {
    let options = crate::config::PanelOptions {
        edge: Some(crate::geometry::Edge::Right),
        size: crate::geometry::Size::percent(30),
        reserve: Some(crate::geometry::ReserveMode::Cover),
        layer: Some(crate::config::Layer::Top),
        keyboard: Some(crate::config::Keyboard::Exclusive),
        output: Some("eDP-1".to_owned()),
        hidden: Some(true),
        focus_ring: Some(crate::config::FocusRing::On {
            color: crate::config::Rgb::new(0xda, 0xbc, 0x7f),
            width: std::num::NonZeroU32::MIN,
        }),
        ..crate::config::PanelOptions::default()
    };
    let request = Request::Create {
        panel: notes(),
        pid: 4242,
        app: "myeditor".to_owned(),
        options,
    };
    let line = request.to_line();
    let value: serde_json::Value = serde_json::from_str(&line).expect("a request is JSON");
    assert_eq!(value["cmd"], serde_json::json!("create"));
    assert_eq!(value["panel"], serde_json::json!("notes"));
    assert_eq!(value["pid"], serde_json::json!(4242));
    // The app's name is the command's file name, which the daemon's report
    // names (row 7.4, D3).
    assert_eq!(value["app"], serde_json::json!("myeditor"));
    assert_eq!(value["options"]["edge"], serde_json::json!("right"));
    assert_eq!(value["options"]["size"], serde_json::json!("30%"));
    assert_eq!(value["options"]["reserve"], serde_json::json!("cover"));
    assert_eq!(value["options"]["keyboard"], serde_json::json!("exclusive"));
    assert_eq!(
        value["options"]["focus_ring"],
        serde_json::json!("#dabc7f:1")
    );
    assert_eq!(
        Request::parse(line.as_bytes()).expect("the request round-trips"),
        request
    );

    // The reply carries the panel's `WAYLAND_DISPLAY` (D2), and a create
    // reply's bad value is the known value error, not an unknown command.
    let reply = Reply::created("wayland-1");
    assert!(reply.is_ok());
    assert_eq!(reply.display(), Some("wayland-1"));
    let line = reply.to_line();
    assert_eq!(line, "{\"ok\":true,\"display\":\"wayland-1\"}\n");
    assert_eq!(Reply::parse(line.as_bytes()), Ok(reply));
    let error = Request::parse(
        br#"{"cmd":"create","panel":"notes","pid":1,"app":"notes","options":{"edge":"middle"}}"#,
    )
    .expect_err("an invalid option value");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
}
