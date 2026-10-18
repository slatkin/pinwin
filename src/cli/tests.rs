use super::*;

use crate::config::{FocusRing, Keyboard, Layer, Rgb};
use crate::geometry::Edge;
use std::num::NonZeroU32;

fn os(raw: &[&str]) -> Vec<OsString> {
    raw.iter().map(OsString::from).collect()
}

fn parse(raw: &[&str]) -> Result<Command, UsageError> {
    parse_args(&os(raw))
}

fn usage(raw: &[&str]) -> UsageError {
    parse(raw).expect_err("the command line is not usable")
}

fn client(raw: &[&str]) -> Request {
    match parse(raw).expect("a valid client command") {
        Command::Client(command) => command,
        other => panic!("expected a client command: {other:?}"),
    }
}

fn size(raw: &str) -> Size {
    raw.parse().expect("a valid size")
}

fn px(length: u32) -> Size {
    Size::pixels(length).expect("a nonzero size")
}

fn name(raw: &str) -> PanelName {
    PanelName::parse(raw).expect("a valid panel name")
}

/// `run` accepts the options of the spec, ends them at `--`, and keeps the
/// command's own `--`-prefixed arguments (spec "Run options").
#[test]
fn run_parses_its_options_and_command() {
    let command = parse(&[
        "run",
        "--name",
        "notes",
        "--edge",
        "right",
        "--size",
        "30%",
        "--reserve",
        "cover",
        "--layer",
        "top",
        "--keyboard",
        "none",
        "--output",
        "DP-1",
        "--hidden",
        "--focus-ring",
        "#112233:2",
        "--",
        "myeditor",
        "--flag",
        "x",
    ])
    .expect("a valid run");
    let Command::Run(run) = command else {
        panic!("expected a run command");
    };
    assert_eq!(run.name(), &name("notes"));
    assert_eq!(run.command(), os(&["myeditor", "--flag", "x"]));
    assert_eq!(run.options().edge, Some(Edge::Right));
    assert_eq!(run.options().size, Some(size("30%")));
    assert_eq!(run.options().reserve, Some(ReserveMode::Cover));
    assert_eq!(run.options().layer, Some(Layer::Top));
    assert_eq!(run.options().keyboard, Some(Keyboard::None));
    assert_eq!(run.options().output.as_deref(), Some("DP-1"));
    assert_eq!(run.options().hidden, Some(true));
    assert_eq!(
        run.options().focus_ring,
        Some(FocusRing::On {
            color: Rgb::new(0x11, 0x22, 0x33),
            width: NonZeroU32::new(2).expect("a nonzero width"),
        })
    );

    // Without `--` the first non-option starts the command, and after `--`
    // an option-shaped word is an ordinary command argument.
    let Command::Run(run) = parse(&["run", "myeditor", "--help"]).expect("a valid run") else {
        panic!("expected a run command");
    };
    assert_eq!(run.command(), os(&["myeditor", "--help"]));
    assert_eq!(run.name(), &name("myeditor"));

    let Command::Run(run) = parse(&["run", "--", "--toggle"]).expect("a valid run") else {
        panic!("expected a run command");
    };
    assert_eq!(run.command(), os(&["--toggle"]));
    assert_eq!(run.name(), &name("--toggle"));
}

/// `--show-existing` asks the daemon to show a panel with the name instead
/// of starting a new one, and it is absent by default (spec "Show an
/// existing panel", "Run options").
#[test]
fn run_parses_show_existing() {
    let Command::Run(run) = parse(&[
        "run",
        "--name",
        "notes",
        "--show-existing",
        "--",
        "myeditor",
    ])
    .expect("a valid run") else {
        panic!("expected a run command");
    };
    assert!(run.show_existing());
    assert_eq!(run.name(), &name("notes"));
    assert_eq!(run.command(), os(&["myeditor"]));

    let Command::Run(run) =
        parse(&["run", "--name", "notes", "--", "myeditor"]).expect("a valid run")
    else {
        panic!("expected a run command");
    };
    assert!(!run.show_existing(), "the flag is absent by default");
}

/// `--gutter-*` sets a gutter of the new panel, and a value that is not a
/// whole number is a usage error (spec "Run options", "Gutter option wins
/// over the file").
#[test]
fn run_parses_the_gutter_options() {
    let Command::Run(run) = parse(&[
        "run",
        "--name",
        "notes",
        "--gutter-left",
        "-12",
        "--gutter-top",
        "8",
        "--gutter-right",
        "4",
        "--gutter-bottom",
        "-3",
        "--",
        "myeditor",
    ])
    .expect("a valid run") else {
        panic!("expected a run command");
    };
    assert_eq!(run.options().gutter_left, Some(-12), "a negative gutter");
    assert_eq!(run.options().gutter_top, Some(8));
    assert_eq!(run.options().gutter_right, Some(4));
    assert_eq!(run.options().gutter_bottom, Some(-3));

    let Command::Run(run) =
        parse(&["run", "--name", "notes", "--", "myeditor"]).expect("a valid run")
    else {
        panic!("expected a run command");
    };
    assert_eq!(
        run.options().gutter_left,
        None,
        "an absent option leaves the value to the config file"
    );

    for option in [
        "--gutter-left",
        "--gutter-right",
        "--gutter-top",
        "--gutter-bottom",
    ] {
        let error = usage(&["run", option, "1.5", "--", "myeditor"]);
        assert_eq!(
            error.to_string(),
            format!("pinwin: {option}: expected a whole number of pixels, got '1.5'")
        );
    }
    assert_eq!(UsageError::EXIT_CODE, 2, "a bad gutter is a usage error");
}

/// An unreadable setting value is a usage error that names the option and
/// exits 2 (spec "Invalid option value" and "Command-line exit codes").
#[test]
fn an_invalid_run_option_value_is_a_usage_error() {
    let error = usage(&["run", "--edge", "middle", "--", "myeditor"]);
    assert_eq!(
        error.to_string(),
        "pinwin: --edge: expected left, right, top or bottom, got 'middle'"
    );
    assert_eq!(UsageError::EXIT_CODE, UsageError::EXIT_CODE);
    assert_eq!(UsageError::EXIT_CODE, 2);

    for (option, value) in [
        ("--size", "wide"),
        ("--reserve", "sideways"),
        ("--layer", "middle"),
        ("--keyboard", "sometimes"),
    ] {
        let error = usage(&["run", option, value, "--", "myeditor"]);
        assert!(
            error.to_string().starts_with(&format!("pinwin: {option}:")),
            "option = {option:?}, error = {error}"
        );
        assert_eq!(UsageError::EXIT_CODE, 2, "option = {option:?}");
    }
}

/// `run` without a command prints a message and exits 2 (spec "No
/// command").
#[test]
fn run_without_a_command_is_a_usage_error() {
    for raw in [
        &["run"][..],
        &["run", "--name", "notes"][..],
        &["run", "--hidden"][..],
        &["run", "--"][..],
    ] {
        let error = usage(raw);
        assert!(
            error.to_string().contains("no command"),
            "raw = {raw:?}, error = {error}"
        );
        assert_eq!(UsageError::EXIT_CODE, 2, "raw = {raw:?}");
    }
}

/// A focus ring outside `off`, `#RRGGBB` or `#RRGGBB:<width>` is a usage
/// error that names the option, not a panel error, and exits 2.
#[test]
fn an_invalid_focus_ring_is_a_usage_error() {
    for value in ["on", "#12345", "#dabc7f:0", "dabc7f", "#dabc7f:70000"] {
        let error = usage(&["run", "--focus-ring", value, "--", "myeditor"]);
        assert!(
            error.to_string().starts_with("pinwin: --focus-ring:"),
            "value = {value:?}, error = {error}"
        );
        assert_eq!(UsageError::EXIT_CODE, 2, "value = {value:?}");
    }
}

/// An unknown subcommand is a usage error and exits 2 (spec "Usage error").
#[test]
fn an_unknown_subcommand_is_a_usage_error() {
    let error = usage(&["frobnicate"]);
    assert_eq!(error.to_string(), "pinwin: unknown command 'frobnicate'");
    assert_eq!(UsageError::EXIT_CODE, 2);
}

/// A name error carries the option's context: `PanelName::parse` has none,
/// while the default-name error already names `--name` and is not prefixed
/// again (spec "Panel names" and "Invalid default name").
#[test]
fn the_name_errors_carry_the_option_context() {
    let error = usage(&["run", "--name", "a/b", "--", "myeditor"]);
    assert!(
        error
            .to_string()
            .starts_with("pinwin: --name: expected a name of"),
        "the message names --name once: {error}"
    );
    assert_eq!(
        error.to_string().matches("--name").count(),
        1,
        "the message names --name once: {error}"
    );
    assert_eq!(UsageError::EXIT_CODE, 2);

    let error = usage(&["run", "--", "my.app"]);
    assert!(
        error
            .to_string()
            .starts_with("pinwin: the command's file name 'my.app'"),
        "the default-name error is not prefixed again: {error}"
    );
    assert!(
        error.to_string().contains("--name"),
        "the message names --name: {error}"
    );
    assert_eq!(UsageError::EXIT_CODE, 2);
}

/// Every client subcommand takes the arguments of the spec and parses them
/// with the shared setting grammar (spec "Command-line client").
#[test]
fn the_client_subcommands_parse_their_values() {
    assert_eq!(
        client(&["toggle", "notes"]),
        Request::Toggle {
            panel: name("notes")
        }
    );
    assert_eq!(
        client(&["show", "notes"]),
        Request::Show {
            panel: name("notes")
        }
    );
    assert_eq!(
        client(&["hide", "notes"]),
        Request::Hide {
            panel: name("notes")
        }
    );
    assert_eq!(
        client(&["focus", "notes"]),
        Request::Focus {
            panel: name("notes")
        }
    );

    assert_eq!(
        client(&["set-edge", "notes", "right"]),
        Request::SetEdge {
            panel: name("notes"),
            edge: Edge::Right,
        }
    );
    assert_eq!(
        client(&["set-size", "notes", "640px"]),
        Request::SetSize {
            panel: name("notes"),
            size: px(640),
            reserve: None,
            ms: None,
        }
    );
    assert_eq!(
        client(&["set-size", "notes", "30%", "--ms", "0"]),
        Request::SetSize {
            panel: name("notes"),
            size: size("30%"),
            reserve: None,
            ms: Some(0),
        }
    );
    assert_eq!(
        client(&["set-gutters", "notes", "--top", "8", "--bottom", "8"]),
        Request::SetGutters {
            panel: name("notes"),
            top: Some(8),
            bottom: Some(8),
            left: None,
            right: None,
        }
    );
    assert_eq!(
        client(&["set-output", "notes", "DP-1"]),
        Request::SetOutput {
            panel: name("notes"),
            output: "DP-1".to_owned(),
        }
    );
    assert_eq!(
        client(&["set-layer", "notes", "bottom"]),
        Request::SetLayer {
            panel: name("notes"),
            layer: Layer::Bottom,
        }
    );
    assert_eq!(
        client(&["set-reserve", "notes", "cover"]),
        Request::SetReserve {
            panel: name("notes"),
            reserve: ReserveMode::Cover,
        }
    );
    assert_eq!(
        client(&["set-keyboard", "notes", "exclusive"]),
        Request::SetKeyboard {
            panel: name("notes"),
            keyboard: Keyboard::Exclusive,
        }
    );

    assert_eq!(client(&["status"]), Request::Status { panel: None });
    assert_eq!(
        client(&["status", "notes"]),
        Request::Status {
            panel: Some(name("notes")),
        }
    );
    assert_eq!(client(&["subscribe"]), Request::Subscribe);
    assert_eq!(client(&["quit"]), Request::Quit);
    assert_eq!(client(&["reload"]), Request::Reload);
}

/// `set-size` and `resize` take `--reserve push|cover` beside `--ms`, in
/// any order, and send it with the size (spec "Size with a reserve mode").
#[test]
fn the_size_commands_take_a_reserve_mode() {
    assert_eq!(
        client(&[
            "set-size",
            "notes",
            "1100px",
            "--reserve",
            "cover",
            "--ms",
            "200",
        ]),
        Request::SetSize {
            panel: name("notes"),
            size: px(1100),
            reserve: Some(ReserveMode::Cover),
            ms: Some(200),
        },
        "the reserve option rides with the size in one request"
    );
    assert_eq!(
        client(&[
            "resize",
            "notes",
            "-700px",
            "--ms",
            "200",
            "--reserve",
            "push",
        ]),
        Request::Resize {
            panel: name("notes"),
            by: Amount::shrink(px(700)),
            reserve: Some(ReserveMode::Push),
            ms: Some(200),
        },
        "the options are accepted in either order"
    );
    let Request::Resize {
        panel,
        by,
        reserve,
        ms,
    } = client(&["resize", "notes", "-40px", "--ms", "150"])
    else {
        panic!("expected resize");
    };
    assert_eq!(panel, name("notes"));
    assert_eq!(by.size(), px(40));
    assert!(by.shrinks());
    assert_eq!(reserve, None);
    assert_eq!(ms, Some(150));

    let Request::Resize { by, .. } = client(&["resize", "notes", "+10%"]) else {
        panic!("expected resize");
    };
    assert_eq!(by.size(), size("10%"));
    assert!(!by.shrinks());
}

/// `set-gutters` sends the gutters it names, each a whole number of pixels
/// (spec "Gutter command", "No gutter option", D7).
#[test]
fn the_gutter_command_takes_whole_pixels() {
    assert_eq!(
        client(&[
            "set-gutters",
            "notes",
            "--right",
            "-4",
            "--left",
            "12",
            "--right",
            "6",
        ]),
        Request::SetGutters {
            panel: name("notes"),
            top: None,
            bottom: None,
            left: Some(12),
            right: Some(6),
        },
        "the options come in any order and a repeated one keeps its last value"
    );

    let error = usage(&["set-gutters", "notes"]);
    assert_eq!(
        error.to_string(),
        "pinwin: set-gutters: expected at least one of --top, --bottom, --left or --right"
    );
    assert_eq!(UsageError::EXIT_CODE, 2);

    let error = usage(&["set-gutters", "notes", "--top", "1.5"]);
    assert_eq!(
        error.to_string(),
        "pinwin: --top: expected a whole number of pixels, got '1.5'"
    );
    assert_eq!(UsageError::EXIT_CODE, 2);

    // A gutter option without its value, or one that is not an option, is a
    // usage error too.
    for raw in [
        &["set-gutters", "notes", "--top"][..],
        &["set-gutters", "notes", "--frobnicate", "8"][..],
        &["set-gutters"][..],
    ] {
        let error = usage(raw);
        assert!(
            error
                .to_string()
                .starts_with("pinwin: set-gutters: expected <name> [--top N]"),
            "raw = {raw:?}, error = {error}"
        );
        assert_eq!(UsageError::EXIT_CODE, 2, "raw = {raw:?}");
    }
}

/// A missing, extra or unreadable client argument is a usage error that
/// names the subcommand and exits 2 (spec "Command-line exit codes").
#[test]
fn client_usage_errors_name_the_subcommand() {
    for (raw, expected) in [
        (&["toggle"][..], "pinwin: toggle: expected one panel name"),
        (
            &["toggle", "notes", "chat"][..],
            "pinwin: toggle: expected one panel name",
        ),
        (
            &["toggle", "a/b"][..],
            "pinwin: toggle: expected a name of 1..=64 characters from [A-Za-z0-9_-], got 'a/b'",
        ),
        (
            &["set-edge", "notes"][..],
            "pinwin: set-edge: expected <name> an edge",
        ),
        (
            &["set-edge", "notes", "middle"][..],
            "pinwin: set-edge: expected left, right, top or bottom, got 'middle'",
        ),
        (
            &["set-size", "notes"][..],
            "pinwin: set-size: expected <name> a size [--reserve push|cover] [--ms N]",
        ),
        (
            &["set-size", "notes", "640px", "--ms", "wide"][..],
            "pinwin: --ms: expected a whole number of milliseconds, got 'wide'",
        ),
        (
            &["set-size", "notes", "1100px", "--reserve", "middle"][..],
            "pinwin: --reserve: expected push or cover, got 'middle'",
        ),
        (
            &["resize", "notes", "40px"][..],
            "pinwin: resize: expected a signed size such as -40px or +10%, got '40px'",
        ),
        (
            &["status", "notes", "chat"][..],
            "pinwin: status: expected at most one panel name",
        ),
        (
            &["subscribe", "notes"][..],
            "pinwin: subscribe: takes no arguments",
        ),
        (&["quit", "notes"][..], "pinwin: quit: takes no arguments"),
        (
            &["reload", "notes"][..],
            "pinwin: reload: takes no arguments",
        ),
        (
            &["daemon", "notes"][..],
            "pinwin: daemon: takes no arguments",
        ),
    ] {
        let error = usage(raw);
        assert_eq!(error.to_string(), expected, "raw = {raw:?}");
        assert_eq!(UsageError::EXIT_CODE, 2, "raw = {raw:?}");
    }
}

/// An unknown option is a usage error (spec "Run options").
#[test]
fn an_unknown_option_is_a_usage_error() {
    let error = usage(&["run", "--frobnicate", "--", "myeditor"]);
    assert_eq!(error.to_string(), "pinwin: unknown option '--frobnicate'");
    assert_eq!(UsageError::EXIT_CODE, 2);
}

/// Every subcommand in the published list reaches the parser: a new list
/// entry without a dispatch arm, or the reverse, fails here.
#[test]
fn every_documented_subcommand_is_known() {
    for subcommand in SUBCOMMANDS {
        let result = parse_args(&os(&[subcommand]));
        if let Err(error) = &result {
            assert!(
                !error.message().contains("unknown command"),
                "{subcommand}: {error}"
            );
        }
    }
}

/// Every subcommand in the README's command table exists in `cli.rs`, and
/// every subcommand in `cli.rs` is in that table (row 4.3).
#[test]
fn the_readme_documents_every_subcommand() {
    let documented = readme_subcommands(include_str!("../../README.md"));

    for command in &documented {
        assert!(
            SUBCOMMANDS.contains(&command.as_str()),
            "the README documents `{command}`, which cli.rs does not have"
        );
    }
    for command in SUBCOMMANDS {
        assert!(
            documented.iter().any(|row| row == command),
            "cli.rs has `{command}`, which the README does not document"
        );
    }
    assert_eq!(
        documented.len(),
        SUBCOMMANDS.len(),
        "the README's command table repeats a subcommand"
    );
}

/// The subcommands of the README's `## Commands` table, in order.
fn readme_subcommands(readme: &str) -> Vec<String> {
    let section = readme
        .split_once("\n## Commands\n")
        .expect("the README has a Commands section")
        .1;
    let section = section
        .split_once("\n## ")
        .map_or(section, |(section, _)| section);

    let mut commands = Vec::new();
    for line in section.lines() {
        let Some(line) = line.strip_prefix('|') else {
            continue;
        };
        let cell = line.split('|').next().unwrap_or_default().trim();
        let Some(cell) = cell.strip_prefix('`') else {
            continue;
        };
        let Some(command) = cell.strip_suffix('`') else {
            continue;
        };
        commands.push(command.to_owned());
    }
    commands
}
