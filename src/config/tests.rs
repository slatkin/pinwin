use super::*;
use crate::geometry::OutputSize;

const PATH: &str = "/home/user/.config/pinwin/config.toml";

fn parse(text: &str) -> Result<Config, ConfigError> {
    Config::parse(Path::new(PATH), text)
}

fn name(raw: &str) -> PanelName {
    PanelName::parse(raw).expect("a valid panel name")
}

/// The config file is `$XDG_CONFIG_HOME/pinwin/config.toml`, or
/// `$HOME/.config/pinwin/config.toml` when `XDG_CONFIG_HOME` is unset or
/// empty (spec "Config file").
#[test]
fn the_config_file_is_named_for_the_config_home() {
    let home = OsStr::new("/home/user");
    let config_home = OsStr::new("/run/user/1000/config");
    assert_eq!(
        config_path(Some(config_home), Some(home)),
        Some(PathBuf::from("/run/user/1000/config/pinwin/config.toml"))
    );
    assert_eq!(
        config_path(None, Some(home)),
        Some(PathBuf::from("/home/user/.config/pinwin/config.toml"))
    );
    assert_eq!(
        config_path(Some(OsStr::new("")), Some(home)),
        Some(PathBuf::from("/home/user/.config/pinwin/config.toml")),
        "an empty XDG_CONFIG_HOME falls back to HOME"
    );
    assert_eq!(
        config_path(None, None),
        None,
        "no home at all names no file"
    );
}

/// A missing file means the built-in defaults (spec "No config file").
#[test]
fn a_missing_config_file_means_the_built_in_defaults() {
    let path = std::env::temp_dir().join(format!("pinwin-config-missing-{}", std::process::id()));
    assert!(
        !path.exists(),
        "the test path must not exist: {}",
        path.display()
    );

    let config = Config::load(&path).expect("a missing file is not an error");
    let settings = config.panel(&name("myeditor"), &PanelOptions::default());
    assert_eq!(settings, PanelSettings::built_in());
    assert!(config.daemon().exit_when_idle());
}

/// An invalid file names the file and the line of the bad value or key
/// (spec "Invalid config file").
#[test]
fn an_invalid_config_file_error_names_the_line() {
    let text = "[panel.defaults]\nedge = \"left\"\n\n[panel.chat]\nedge = \"middle\"\n";
    let error = parse(text).expect_err("middle is not an edge");
    assert_eq!(
        error.to_string(),
        "/home/user/.config/pinwin/config.toml:5: expected left, right, top or bottom, got \
             'middle'"
    );

    // `deny_unknown_fields` (D13) reports the key's own line.
    let error = parse("[panel.defaults]\nreserve = \"push\"\ntween = 200\n")
        .expect_err("tween is not a key");
    assert!(
        error
            .to_string()
            .starts_with("/home/user/.config/pinwin/config.toml:3:"),
        "the message names the file and line: {error}"
    );
    assert!(error.to_string().contains("tween"), "{error}");

    // A syntax error names its line too.
    let error = parse("[panel.defaults]\nedge = \"left\"\n=\n").expect_err("bad TOML");
    assert!(
        error.to_string().contains(":3:"),
        "the message names the line: {error}"
    );
}

/// A `[panel.<name>]` key must be a panel name (spec "Config file"): a key
/// outside the name rules is refused with the file, the line and the key,
/// because the key has no setting value to point at.
#[test]
fn a_panel_table_key_must_be_a_panel_name() {
    let text = "[daemon]\nexit_when_idle = true\n\n[panel.\"my.app\"]\nedge = \"left\"\n";
    let error = parse(text).expect_err("a dot is not in the name rules");
    assert_eq!(
        error.to_string(),
        "/home/user/.config/pinwin/config.toml:4: the [panel.<name>] key is not a panel \
         name: expected a name of 1..=64 characters from [A-Za-z0-9_-], got 'my.app'"
    );

    // An empty quoted key is not a name either, and the error names line 1.
    let error = parse("[panel.\"\"]\nedge = \"left\"\n").expect_err("an empty name");
    assert!(
        error
            .to_string()
            .starts_with("/home/user/.config/pinwin/config.toml:1:"),
        "the message names the file and line: {error}"
    );
    assert!(error.to_string().contains("got ''"), "{error}");
}

/// `defaults` names the defaults table, so it cannot also name a panel's own
/// table: its key is lifted out and no `[panel.defaults]` table remains.
#[test]
fn defaults_is_the_defaults_table_not_a_panel_table() {
    let config = parse("[panel.defaults]\nedge = \"right\"\n").expect("a valid file");
    assert!(!config.panels.contains_key(DEFAULTS));
    let defaults = config.panel(&name("defaults"), &PanelOptions::default());
    assert_eq!(defaults.edge(), Edge::Right);
}

/// A panel's own table wins over `[panel.defaults]` (spec "Per-name
/// override").
#[test]
fn a_panel_table_wins_over_the_defaults_table() {
    let config = parse(
        "[panel.defaults]\nedge = \"left\"\nsize = \"20%\"\n\n[panel.chat]\nedge = \"right\"\n",
    )
    .expect("a valid file");

    let chat = config.panel(&name("chat"), &PanelOptions::default());
    assert_eq!(chat.edge(), Edge::Right);
    assert_eq!(chat.size(), Size::percent(20).expect("a valid size"));

    // A panel without its own table keeps the defaults table's values.
    let notes = config.panel(&name("notes"), &PanelOptions::default());
    assert_eq!(notes.edge(), Edge::Left);
    assert_eq!(notes.size(), Size::percent(20).expect("a valid size"));
}

/// A run option wins over the panel's own table (spec "Option wins").
#[test]
fn a_run_option_wins_over_the_panel_table() {
    let config = parse("[panel.chat]\nsize = \"30%\"\nedge = \"right\"\n").expect("a valid file");
    let options = PanelOptions {
        size: Size::pixels(500),
        ..PanelOptions::default()
    };

    let chat = config.panel(&name("chat"), &options);
    assert_eq!(chat.size(), Size::pixels(500).expect("a valid size"));
    assert_eq!(
        chat.edge(),
        Edge::Right,
        "a setting the option does not carry keeps the file's value"
    );
}

/// A gutter run option wins over the panel's own table and leaves the
/// gutters it does not name (spec "Gutter option wins over the file",
/// "Option wins", D7).
#[test]
fn a_gutter_run_option_wins_over_the_panel_table() {
    let config = parse("[panel.notes]\ngutter_left = 4\ngutter_right = 6\n").expect("a valid file");
    let options = PanelOptions {
        gutter_left: Some(12),
        ..PanelOptions::default()
    };

    let notes = config.panel(&name("notes"), &options);
    assert_eq!(notes.gutters(), Gutters::new(12, 6, 0, 0));
}

/// The built-in defaults are the spec's (spec "Defaults").
#[test]
fn the_built_in_defaults_match_the_spec() {
    let settings = PanelSettings::built_in();
    assert_eq!(settings.edge(), Edge::Left);
    assert_eq!(settings.size(), Size::percent(25).expect("a valid size"));
    assert_eq!(settings.reserve(), ReserveMode::Push);
    assert_eq!(settings.layer(), Layer::Overlay);
    assert_eq!(settings.keyboard(), Keyboard::OnDemand);
    assert_eq!(settings.output(), None);
    assert!(!settings.hidden());
    assert_eq!(settings.gutters(), Gutters::new(0, 0, 0, 0));
    assert_eq!(settings.tween_ms(), 200);
    assert_eq!(
        settings.focus_ring(),
        FocusRing::On {
            color: Rgb::new(0xda, 0xbc, 0x7f),
            width: NonZeroU32::MIN,
        }
    );

    // On a 2560 pixel output the 25% default resolves to 640 pixels.
    let output = OutputSize::new(2560, 1440).expect("an output");
    assert_eq!(settings.size().resolve(Edge::Left, output), 640);
}

/// Every setting's string form parses, and a wrong one is rejected with
/// the accepted forms (spec "Size values" and "Run options").
#[test]
fn the_setting_forms_parse() {
    assert_eq!("left".parse::<Edge>(), Ok(Edge::Left));
    assert_eq!("bottom".parse::<Edge>(), Ok(Edge::Bottom));
    assert_eq!("push".parse::<ReserveMode>(), Ok(ReserveMode::Push));
    assert_eq!("cover".parse::<ReserveMode>(), Ok(ReserveMode::Cover));
    assert_eq!("none".parse::<ReserveMode>(), Ok(ReserveMode::None));
    for mode in [ReserveMode::Push, ReserveMode::Cover, ReserveMode::None] {
        assert_eq!(
            mode.as_str().parse::<ReserveMode>(),
            Ok(mode),
            "the text form round-trips"
        );
    }
    assert_eq!("top".parse::<Layer>(), Ok(Layer::Top));
    assert_eq!("overlay".parse::<Layer>(), Ok(Layer::Overlay));
    assert_eq!("none".parse::<Keyboard>(), Ok(Keyboard::None));
    assert_eq!("on-demand".parse::<Keyboard>(), Ok(Keyboard::OnDemand));
    assert_eq!(
        "640px".parse::<Size>(),
        Ok(Size::pixels(640).expect("a valid size"))
    );
    assert_eq!(
        "30%".parse::<Size>(),
        Ok(Size::percent(30).expect("a valid size"))
    );
    assert_eq!("off".parse::<FocusRing>(), Ok(FocusRing::Off));
    assert_eq!(
        "#dabc7f".parse::<FocusRing>(),
        Ok(FocusRing::On {
            color: Rgb::new(0xda, 0xbc, 0x7f),
            width: NonZeroU32::MIN,
        })
    );
    assert_eq!(
        "#000000:2".parse::<FocusRing>(),
        Ok(FocusRing::On {
            color: Rgb::new(0, 0, 0),
            width: NonZeroU32::new(2).expect("a nonzero width"),
        })
    );
    assert_eq!(
        "#000000:65535".parse::<FocusRing>(),
        Ok(FocusRing::On {
            color: Rgb::new(0, 0, 0),
            width: NonZeroU32::new(65535).expect("a nonzero width"),
        })
    );
    assert_eq!(Rgb::new(1, 2, 3).channels(), [1, 2, 3]);
}

/// A value outside a setting's grammar is refused, with the accepted
/// forms in the message (spec "Size values" and "Run options").
#[test]
fn the_setting_forms_reject_other_values() {
    for raw in ["", "middle", "Left"] {
        assert!(raw.parse::<Edge>().is_err(), "edge {raw:?}");
    }
    for raw in ["0px", "150%", "wide", "640", "-5px", "+5px", "0%"] {
        assert!(raw.parse::<Size>().is_err(), "size {raw:?}");
    }
    for raw in ["", "on_demand", "Off"] {
        assert!(raw.parse::<Keyboard>().is_err(), "keyboard {raw:?}");
    }
    for raw in ["", "pushing", "Cover"] {
        assert!(raw.parse::<ReserveMode>().is_err(), "reserve {raw:?}");
    }
    for raw in ["", "Underlay", "Top "] {
        assert!(raw.parse::<Layer>().is_err(), "layer {raw:?}");
    }
    for raw in [
        "",
        "#dabc7",
        "#dabc7f:0",
        "dabc7f",
        "#dabc7f:",
        "#dabc7f:65536",
        "#dabc7f:70000",
    ] {
        assert!(raw.parse::<FocusRing>().is_err(), "focus ring {raw:?}");
    }
    let error = "middle".parse::<Edge>().expect_err("not an edge");
    assert_eq!(
        error.to_string(),
        "expected left, right, top or bottom, got 'middle'"
    );
}

/// The `[daemon]` table sets the daemon preference, and a non-boolean is
/// an error on its line (spec "Daemon lifetime preference").
#[test]
fn the_daemon_table_sets_exit_when_idle() {
    assert!(Config::default().daemon().exit_when_idle());
    let config = parse("[daemon]\nexit_when_idle = false\n").expect("a valid file");
    assert!(!config.daemon().exit_when_idle());
    let error = parse("[daemon]\nexit_when_idle = \"no\"\n").expect_err("not a boolean");
    assert!(
        error
            .to_string()
            .starts_with("/home/user/.config/pinwin/config.toml:2:"),
        "the message names the line: {error}"
    );
}

/// Every key in the README's configuration table exists here, and every
/// key here is in that table (row 4.2).
#[test]
fn the_readme_documents_every_config_key() {
    let rows = readme_rows(include_str!("../../README.md"));

    for (table, key) in &rows {
        let keys = match table.as_str() {
            "[daemon]" => DAEMON_KEYS.as_slice(),
            "[panel.*]" => PANEL_KEYS.as_slice(),
            other => panic!("the README's configuration table names an unknown table {other}"),
        };
        assert!(
            keys.contains(&key.as_str()),
            "the README documents `{key}` in `{table}`, which config.rs does not have"
        );
    }

    for key in DAEMON_KEYS {
        assert!(
            rows.iter()
                .any(|(table, row)| table == "[daemon]" && row == key),
            "config.rs has `{key}` in `[daemon]`, which the README does not document"
        );
    }
    for key in PANEL_KEYS {
        assert!(
            rows.iter()
                .any(|(table, row)| table == "[panel.*]" && row == key),
            "config.rs has `{key}` in `[panel.*]`, which the README does not document"
        );
    }
    assert_eq!(
        rows.len(),
        DAEMON_KEYS.len() + PANEL_KEYS.len(),
        "the README's configuration table repeats a key"
    );
}

/// The table fields are the documented keys (row 4.2): a `PanelOptions` or
/// `DaemonTable` field renamed without updating its key list changes the
/// field list `deny_unknown_fields` reports, and this test fails.
#[test]
fn the_table_fields_are_the_documented_keys() {
    let error = parse("[panel.defaults]\nunknown = 1\n").expect_err("unknown is not a key");
    assert_eq!(expected_keys(&error.to_string()), documented(PANEL_KEYS));

    let error = parse("[daemon]\nunknown = true\n").expect_err("unknown is not a key");
    assert_eq!(expected_keys(&error.to_string()), documented(DAEMON_KEYS));
}

/// The key list serde's `deny_unknown_fields` error names, with the
/// quoting removed: several names after `expected one of`, or one name
/// after `expected`.
fn expected_keys(message: &str) -> Vec<String> {
    let (_, tail) = message
        .split_once("expected ")
        .expect("the unknown-field message names the expected keys");
    let names = tail.strip_prefix("one of ").unwrap_or(tail);
    names
        .split(", ")
        .map(|name| name.trim_matches('`').to_owned())
        .collect()
}

/// The documented keys in declaration order.
fn documented(keys: impl IntoIterator<Item = &'static str>) -> Vec<String> {
    keys.into_iter().map(str::to_owned).collect()
}

/// The `(table, key)` pairs of the README's `## Configuration` table.
fn readme_rows(readme: &str) -> Vec<(String, String)> {
    let section = readme
        .split_once("\n## Configuration\n")
        .expect("the README has a Configuration section")
        .1;
    let section = section
        .split_once("\n## ")
        .map_or(section, |(section, _)| section);

    let mut rows = Vec::new();
    for line in section.lines() {
        let Some(line) = line.strip_prefix('|') else {
            continue;
        };
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let (Some(table), Some(key)) = (
            cells.first().and_then(|cell| cell.strip_prefix('`')),
            cells.get(1).and_then(|cell| cell.strip_prefix('`')),
        ) else {
            continue;
        };
        let Some(table) = table.strip_suffix('`') else {
            continue;
        };
        let Some(key) = key.strip_suffix('`') else {
            continue;
        };
        if table.starts_with('[') {
            rows.push((table.to_owned(), key.to_owned()));
        }
    }
    rows
}
