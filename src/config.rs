//! The config file: its `[daemon]`, `[panel.defaults]` and
//! `[panel.<name>]` tables, the built-in defaults and the precedence
//! between the file and the `pinwin run` options (spec "Config file" and
//! "Panel settings and precedence", replace-terminal-with-app-host D13).
//!
//! The file is TOML read with `serde` and `deny_unknown_fields`, so a typo
//! or an invalid value is an error that names the file and the line (D13).
//! A `[panel.<name>]` key is checked against the name rules too, because a
//! key that is not a name would never match a panel; `defaults` is the
//! defaults table, so no panel table can be named `defaults`.
//! The setting grammar here is also the grammar of the run options: row 4.3
//! parses a command-line value with the same `FromStr` implementations, so
//! the file and the options cannot drift apart.
//!
//! The merge is pure: [`Config::panel`] resolves the options a `pinwin run`
//! call carries, the panel's own table, `[panel.defaults]` and, last, the
//! built-in default. `src/cli.rs` fills the options from the command line;
//! `tests/config.rs` includes this file so its unit tests run until the
//! daemon owns it.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;
use toml::Spanned;

use crate::geometry::{Edge, Gutters, ReserveMode, Size};
use crate::name::PanelName;

/// The `[panel.defaults]` key: the table that applies to every panel, not to
/// a panel named `defaults`. The two cannot coexist in one file.
const DEFAULTS: &str = "defaults";

/// The keys a `[panel.defaults]` or `[panel.<name>]` table accepts, in
/// README order (spec "Panel settings and precedence"). Kept beside
/// [`PanelTable`], whose serde field names it mirrors: the README test
/// compares the two so a documented key cannot be missing from the file and
/// a parsed key cannot be undocumented.
pub const PANEL_KEYS: [&str; 13] = [
    "edge",
    "size",
    "reserve",
    "layer",
    "keyboard",
    "output",
    "hidden",
    "focus_ring",
    "tween_ms",
    "gutter_left",
    "gutter_right",
    "gutter_top",
    "gutter_bottom",
];

/// The keys the `[daemon]` table accepts (spec "Daemon lifetime
/// preference"), in README order.
pub const DAEMON_KEYS: [&str; 1] = ["exit_when_idle"];

/// The layer-shell layer a panel draws in (spec "Run options").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

impl FromStr for Layer {
    type Err = InvalidValue;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "background" => Ok(Self::Background),
            "bottom" => Ok(Self::Bottom),
            "top" => Ok(Self::Top),
            "overlay" => Ok(Self::Overlay),
            _ => Err(InvalidValue::new(raw, "background, bottom, top or overlay")),
        }
    }
}

/// The keyboard interactivity of the panel's layer surface (spec "Run
/// options").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyboard {
    None,
    OnDemand,
    Exclusive,
}

impl FromStr for Keyboard {
    type Err = InvalidValue;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "none" => Ok(Self::None),
            "on-demand" => Ok(Self::OnDemand),
            "exclusive" => Ok(Self::Exclusive),
            _ => Err(InvalidValue::new(raw, "none, on-demand or exclusive")),
        }
    }
}

/// An RGB colour from a `#RRGGBB` focus-ring setting, one byte per channel.
/// Fields are private, so a value is always three bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl Rgb {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// The three channels, red first, as the proxy writes them into the
    /// ring's `wl_shm` buffer.
    #[must_use]
    pub const fn channels(self) -> [u8; 3] {
        [self.red, self.green, self.blue]
    }
}

/// The panel's focus ring (spec "Built-in defaults"): off, or a colour and a
/// width in pixels. The width lives in a [`NonZeroU32`], so a ring of width
/// zero is unrepresentable; `off` is its own variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusRing {
    Off,
    On { color: Rgb, width: NonZeroU32 },
}

impl FromStr for FocusRing {
    type Err = InvalidValue;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw == "off" {
            return Ok(Self::Off);
        }
        let (color, width) = match raw.split_once(':') {
            Some((color, width)) => (color, parse_digits(width).and_then(NonZeroU32::new)),
            None => (raw, Some(NonZeroU32::MIN)),
        };
        match (parse_rgb(color), width) {
            (Some(color), Some(width)) => Ok(Self::On { color, width }),
            _ => Err(InvalidValue::new(raw, "off, #RRGGBB or #RRGGBB:width")),
        }
    }
}

impl FromStr for Edge {
    type Err = InvalidValue;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "top" => Ok(Self::Top),
            "bottom" => Ok(Self::Bottom),
            _ => Err(InvalidValue::new(raw, "left, right, top or bottom")),
        }
    }
}

impl FromStr for ReserveMode {
    type Err = InvalidValue;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "push" => Ok(Self::Push),
            "cover" => Ok(Self::Cover),
            _ => Err(InvalidValue::new(raw, "push or cover")),
        }
    }
}

impl FromStr for Size {
    type Err = InvalidValue;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let size = if let Some(px) = raw.strip_suffix("px") {
            parse_digits(px).and_then(Size::pixels)
        } else if let Some(percent) = raw.strip_suffix('%') {
            parse_digits(percent).and_then(Size::percent)
        } else {
            None
        };
        size.ok_or_else(|| InvalidValue::new(raw, "640px or 30%"))
    }
}

/// Why a setting value is not one of its accepted forms. The caller adds the
/// context (the option or the file), as [`PanelName`](crate::name::PanelName)
/// does for its own errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidValue {
    value: String,
    expected: &'static str,
}

impl InvalidValue {
    pub(crate) fn new(value: &str, expected: &'static str) -> Self {
        Self {
            value: value.to_owned(),
            expected,
        }
    }
}

impl fmt::Display for InvalidValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected {}, got '{}'", self.expected, self.value)
    }
}

impl std::error::Error for InvalidValue {}

/// A whole number of digits. `u32::from_str` also accepts a sign, so the
/// grammar checks the bytes first: `+640px` is not a size.
fn parse_digits(raw: &str) -> Option<u32> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

/// A `#RRGGBB` colour; every other form is `None`, because the spec names
/// only that form.
fn parse_rgb(raw: &str) -> Option<Rgb> {
    let digits = raw.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(Rgb::new(
        u8::from_str_radix(&digits[0..2], 16).ok()?,
        u8::from_str_radix(&digits[2..4], 16).ok()?,
        u8::from_str_radix(&digits[4..6], 16).ok()?,
    ))
}

/// The `[daemon]` table (spec "Daemon lifetime preference"). Missing keys
/// are `None` here and take their built-in default in the merge.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DaemonTable {
    exit_when_idle: Option<bool>,
}

/// One `[panel.defaults]` or `[panel.<name>]` table (spec "Panel settings
/// and precedence"). Every key is optional: an absent key falls through to
/// the next level of the precedence. The string values are parsed with the
/// same `FromStr` grammar as the run options.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PanelTable {
    #[serde(deserialize_with = "deserialize_option_from_str")]
    edge: Option<Edge>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    size: Option<Size>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    reserve: Option<ReserveMode>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    layer: Option<Layer>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    keyboard: Option<Keyboard>,
    output: Option<String>,
    hidden: Option<bool>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    focus_ring: Option<FocusRing>,
    tween_ms: Option<u32>,
    gutter_left: Option<i32>,
    gutter_right: Option<i32>,
    gutter_top: Option<i32>,
    gutter_bottom: Option<i32>,
}

/// The whole file: the two known tables and the `[panel.*]` map, which keeps
/// `defaults` until [`Config::parse`] lifts it out. The map keys keep their
/// byte span, so a key outside the name rules can name its line, which the
/// setting values get from serde's own error span.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    daemon: DaemonTable,
    panel: BTreeMap<Spanned<String>, PanelTable>,
}

/// A string value parsed with the setting grammar, including the line that
/// the TOML deserializer attaches to the serde error.
fn deserialize_option_from_str<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    Option::<String>::deserialize(deserializer)?
        .map(|raw| raw.parse::<T>().map_err(serde::de::Error::custom))
        .transpose()
}

/// Why the config file could not be read (spec "Config file").
#[derive(Debug)]
pub enum ConfigError {
    /// The file exists but cannot be read.
    Read { path: PathBuf, source: io::Error },
    /// The file is not valid TOML, or it names an unknown key or an invalid
    /// value; the line is one-based.
    Invalid {
        path: PathBuf,
        line: usize,
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Invalid {
                path,
                line,
                message,
            } => write!(f, "{}:{line}: {message}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Invalid { .. } => None,
        }
    }
}

/// The daemon preferences (spec "Daemon lifetime preference").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaemonSettings {
    exit_when_idle: bool,
}

impl DaemonSettings {
    /// Whether the daemon exits after its last panel ends; the built-in
    /// default is true.
    #[must_use]
    pub const fn exit_when_idle(self) -> bool {
        self.exit_when_idle
    }
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            exit_when_idle: true,
        }
    }
}

/// The `pinwin run` options that override the file, one `Option` for each
/// setting (spec "Run options"). `src/cli.rs` fills it from the command
/// line; an absent field leaves the file's value in place. The fields are
/// public because this is a plain set of overrides: the invariant lives in
/// each field's type, not in a constructor (`port-to-rust` D6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelOptions {
    pub edge: Option<Edge>,
    pub size: Option<Size>,
    pub reserve: Option<ReserveMode>,
    pub layer: Option<Layer>,
    pub keyboard: Option<Keyboard>,
    pub output: Option<String>,
    pub hidden: Option<bool>,
    pub focus_ring: Option<FocusRing>,
    pub tween_ms: Option<u32>,
    pub gutter_left: Option<i32>,
    pub gutter_right: Option<i32>,
    pub gutter_top: Option<i32>,
    pub gutter_bottom: Option<i32>,
}

/// A panel's resolved settings: the file and the options merged, every
/// setting present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelSettings {
    edge: Edge,
    size: Size,
    reserve: ReserveMode,
    layer: Layer,
    keyboard: Keyboard,
    output: Option<String>,
    hidden: bool,
    focus_ring: FocusRing,
    tween_ms: u32,
    gutters: Gutters,
}

impl PanelSettings {
    /// The built-in defaults (spec "Built-in defaults"): they are the merge
    /// of two empty tables and no options, so there is one definition.
    #[must_use]
    pub fn built_in() -> Self {
        resolve(&PanelTable::default(), None, &PanelOptions::default())
    }

    #[must_use]
    pub const fn edge(&self) -> Edge {
        self.edge
    }

    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    #[must_use]
    pub const fn reserve(&self) -> ReserveMode {
        self.reserve
    }

    #[must_use]
    pub const fn layer(&self) -> Layer {
        self.layer
    }

    #[must_use]
    pub const fn keyboard(&self) -> Keyboard {
        self.keyboard
    }

    /// The output the panel opens on; `None` lets the compositor choose.
    #[must_use]
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    #[must_use]
    pub const fn hidden(&self) -> bool {
        self.hidden
    }

    #[must_use]
    pub const fn focus_ring(&self) -> FocusRing {
        self.focus_ring
    }

    #[must_use]
    pub const fn tween_ms(&self) -> u32 {
        self.tween_ms
    }

    #[must_use]
    pub const fn gutters(&self) -> Gutters {
        self.gutters
    }
}

/// The built-in size (spec "Built-in defaults"): 25% of the output.
fn default_size() -> Size {
    // 25 is inside the percent type's 1..=100, so this cannot fail.
    Size::percent(25).expect("25 is a valid percent")
}

/// The built-in focus ring (spec "Built-in defaults"): `#dabc7f`, one pixel.
fn default_focus_ring() -> FocusRing {
    FocusRing::On {
        color: Rgb::new(0xda, 0xbc, 0x7f),
        width: NonZeroU32::MIN,
    }
}

/// The config file: the parsed tables after the daemon preferences are
/// resolved. A missing file is [`Config::default`], the built-in defaults.
#[derive(Debug, Default)]
pub struct Config {
    daemon: DaemonSettings,
    defaults: PanelTable,
    panels: BTreeMap<String, PanelTable>,
}

impl Config {
    /// Read `path` (spec "Config file"): a missing file means the built-in
    /// defaults, an unreadable or invalid one is an error that names the
    /// file and, for an invalid file, the line.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(path, &text),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Parse the text of the file the error messages name as `path`.
    pub fn parse(path: &Path, text: &str) -> Result<Self, ConfigError> {
        let file: ConfigFile = toml::from_str(text).map_err(|error| ConfigError::Invalid {
            path: path.to_path_buf(),
            line: line_of(&error, text),
            message: error.message().to_owned(),
        })?;
        let mut defaults = PanelTable::default();
        let mut panels = BTreeMap::new();
        for (key, table) in file.panel {
            let line = line_at(text, key.span().start);
            let key = key.into_inner();
            if key == DEFAULTS {
                defaults = table;
                continue;
            }
            PanelName::parse(&key).map_err(|error| ConfigError::Invalid {
                path: path.to_path_buf(),
                line,
                message: format!("the [panel.<name>] key is not a panel name: {error}"),
            })?;
            panels.insert(key, table);
        }
        Ok(Self {
            daemon: DaemonSettings {
                exit_when_idle: file.daemon.exit_when_idle.unwrap_or(true),
            },
            defaults,
            panels,
        })
    }

    /// The daemon preferences (spec "Daemon lifetime preference").
    #[must_use]
    pub const fn daemon(&self) -> DaemonSettings {
        self.daemon
    }

    /// `name`'s settings: the options win over `[panel.<name>]`, which wins
    /// over `[panel.defaults]`, which wins over the built-in default (spec
    /// "Panel settings and precedence").
    #[must_use]
    pub fn panel(&self, name: &PanelName, options: &PanelOptions) -> PanelSettings {
        resolve(&self.defaults, self.panels.get(name.as_str()), options)
    }
}

/// The line of `text` a TOML error points at, one-based (spec "Invalid
/// config file"). A syntax error always has a span; the fallback keeps an
/// error without one at the first line rather than hiding it.
fn line_of(error: &toml::de::Error, text: &str) -> usize {
    let Some(span) = error.span() else {
        return 1;
    };
    line_at(text, span.start)
}

/// The one-based line of the byte `offset` in `text`: a span before the file
/// names the first line rather than hiding the error.
fn line_at(text: &str, offset: usize) -> usize {
    text.get(..offset)
        .map_or(1, |before| before.split('\n').count())
}

/// A setting from the highest level that has it: the option, else the
/// panel's own table, else `[panel.defaults]`, else the built-in value.
fn pick<T: Copy>(
    option: Option<T>,
    named: Option<&PanelTable>,
    defaults: &PanelTable,
    field: fn(&PanelTable) -> Option<T>,
    built_in: T,
) -> T {
    option
        .or_else(|| named.and_then(field))
        .or(field(defaults))
        .unwrap_or(built_in)
}

/// The merge behind [`Config::panel`] and [`PanelSettings::built_in`]
/// (spec "Panel settings and precedence").
fn resolve(
    defaults: &PanelTable,
    named: Option<&PanelTable>,
    options: &PanelOptions,
) -> PanelSettings {
    PanelSettings {
        edge: pick(
            options.edge,
            named,
            defaults,
            |table| table.edge,
            Edge::Left,
        ),
        size: pick(
            options.size,
            named,
            defaults,
            |table| table.size,
            default_size(),
        ),
        reserve: pick(
            options.reserve,
            named,
            defaults,
            |table| table.reserve,
            ReserveMode::Push,
        ),
        layer: pick(
            options.layer,
            named,
            defaults,
            |table| table.layer,
            Layer::Overlay,
        ),
        keyboard: pick(
            options.keyboard,
            named,
            defaults,
            |table| table.keyboard,
            Keyboard::OnDemand,
        ),
        output: options
            .output
            .clone()
            .or_else(|| named.and_then(|table| table.output.clone()))
            .or_else(|| defaults.output.clone()),
        hidden: pick(options.hidden, named, defaults, |table| table.hidden, false),
        focus_ring: pick(
            options.focus_ring,
            named,
            defaults,
            |table| table.focus_ring,
            default_focus_ring(),
        ),
        tween_ms: pick(
            options.tween_ms,
            named,
            defaults,
            |table| table.tween_ms,
            200,
        ),
        gutters: Gutters::new(
            pick(
                options.gutter_left,
                named,
                defaults,
                |table| table.gutter_left,
                0,
            ),
            pick(
                options.gutter_right,
                named,
                defaults,
                |table| table.gutter_right,
                0,
            ),
            pick(
                options.gutter_top,
                named,
                defaults,
                |table| table.gutter_top,
                0,
            ),
            pick(
                options.gutter_bottom,
                named,
                defaults,
                |table| table.gutter_bottom,
                0,
            ),
        ),
    }
}

// The path is explicit because `tests/config.rs` includes this file with
// `#[path]`, and a module loaded that way searches its own children in the
// included file's directory rather than beside it (row 4.2, as in
// `src/geometry.rs`).
#[cfg(test)]
#[path = "config/tests.rs"]
mod tests;
