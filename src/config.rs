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
//! built-in default. `src/cli.rs` fills the options from the command line
//! and `src/main.rs` reads the file.

use std::collections::BTreeMap;
use std::ffi::OsStr;
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

/// The directory and file name of the config file under the config home
/// (spec "Config file": `$XDG_CONFIG_HOME/pinwin/config.toml`).
const CONFIG_DIR: &str = "pinwin";
const CONFIG_FILE: &str = "config.toml";

/// The keys a `[panel.defaults]` or `[panel.<name>]` table accepts, in
/// README order (spec "Panel settings and precedence"). Kept beside
/// [`PanelOptions`], whose serde field names it mirrors: the README test
/// compares the two so a documented key cannot be missing from the file and
/// a parsed key cannot be undocumented.
#[cfg(test)]
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
#[cfg(test)]
pub const DAEMON_KEYS: [&str; 1] = ["exit_when_idle"];

/// The layer-shell layer a panel draws in (spec "Run options").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

impl Layer {
    /// The layer's text in the setting grammar and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Bottom => "bottom",
            Self::Top => "top",
            Self::Overlay => "overlay",
        }
    }
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

impl Keyboard {
    /// The mode's text in the setting grammar and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::OnDemand => "on-demand",
            Self::Exclusive => "exclusive",
        }
    }
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

/// The widest focus ring the spec allows, in pixels (spec "Keyboard focus by
/// clicking": a width of `1..=65535`).
const FOCUS_RING_MAX_WIDTH: u32 = 65535;

/// A focus-ring width from its text: `1..=65535` pixels, or `None` for a
/// width outside that range or not a number at all (spec "Keyboard focus by
/// clicking").
fn parse_focus_ring_width(raw: &str) -> Option<NonZeroU32> {
    let width = parse_digits(raw)?;
    if width > FOCUS_RING_MAX_WIDTH {
        return None;
    }
    NonZeroU32::new(width)
}

impl FromStr for FocusRing {
    type Err = InvalidValue;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw == "off" {
            return Ok(Self::Off);
        }
        let (color, width) = match raw.split_once(':') {
            Some((color, width)) => (color, parse_focus_ring_width(width)),
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
            "none" => Ok(Self::None),
            _ => Err(InvalidValue::new(raw, "push, cover or none")),
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
pub(crate) fn parse_digits(raw: &str) -> Option<u32> {
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

/// The whole file: the two known tables and the `[panel.*]` map, which keeps
/// `defaults` until [`Config::parse`] lifts it out. The map keys keep their
/// byte span, so a key outside the name rules can name its line, which the
/// setting values get from serde's own error span.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    daemon: DaemonTable,
    panel: BTreeMap<Spanned<String>, PanelOptions>,
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

impl std::error::Error for ConfigError {}

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

/// One source of a panel's settings: the `pinwin run` options that override
/// the file, a `[panel.defaults]` table or a `[panel.<name>]` table (spec
/// "Run options", "Panel settings and precedence"). Every setting is
/// optional: an absent field falls through to the next level of the
/// precedence. `src/cli.rs` fills it from the command line, the config file
/// deserializes it with the same `FromStr` grammar (D13), and the merge
/// chains the levels highest first.
///
/// The fields are public because this is a plain set of overrides: the
/// invariant lives in each field's type, not in a constructor (`port-to-rust`
/// D6).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct PanelOptions {
    #[serde(deserialize_with = "deserialize_option_from_str")]
    pub edge: Option<Edge>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    pub size: Option<Size>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    pub reserve: Option<ReserveMode>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    pub layer: Option<Layer>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    pub keyboard: Option<Keyboard>,
    pub output: Option<String>,
    pub hidden: Option<bool>,
    #[serde(deserialize_with = "deserialize_option_from_str")]
    pub focus_ring: Option<FocusRing>,
    pub tween_ms: Option<u32>,
    pub gutter_left: Option<i32>,
    pub gutter_right: Option<i32>,
    pub gutter_top: Option<i32>,
    pub gutter_bottom: Option<i32>,
}

impl PanelOptions {
    /// The merge of two sources: each setting from `self` when it has it,
    /// else from `other`. The levels of the precedence are chained highest
    /// first (spec "Panel settings and precedence").
    #[must_use]
    fn or(self, other: Self) -> Self {
        Self {
            edge: self.edge.or(other.edge),
            size: self.size.or(other.size),
            reserve: self.reserve.or(other.reserve),
            layer: self.layer.or(other.layer),
            keyboard: self.keyboard.or(other.keyboard),
            output: self.output.or(other.output),
            hidden: self.hidden.or(other.hidden),
            focus_ring: self.focus_ring.or(other.focus_ring),
            tween_ms: self.tween_ms.or(other.tween_ms),
            gutter_left: self.gutter_left.or(other.gutter_left),
            gutter_right: self.gutter_right.or(other.gutter_right),
            gutter_top: self.gutter_top.or(other.gutter_top),
            gutter_bottom: self.gutter_bottom.or(other.gutter_bottom),
        }
    }
}

/// A panel's resolved settings: the file and the options merged, every
/// setting present. The fields are crate-visible so a live apply sets one
/// of them with struct update syntax and leaves the rest alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelSettings {
    pub(crate) edge: Edge,
    pub(crate) size: Size,
    pub(crate) reserve: ReserveMode,
    pub(crate) layer: Layer,
    pub(crate) keyboard: Keyboard,
    pub(crate) output: Option<String>,
    pub(crate) hidden: bool,
    pub(crate) focus_ring: FocusRing,
    pub(crate) tween_ms: u32,
    pub(crate) gutters: Gutters,
}

impl PanelSettings {
    /// The built-in defaults (spec "Built-in defaults"): they are the merge
    /// of two empty tables and no options, so there is one definition.
    #[cfg(test)]
    #[must_use]
    pub fn built_in() -> Self {
        resolve(&PanelOptions::default(), None, &PanelOptions::default())
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
    defaults: PanelOptions,
    panels: BTreeMap<String, PanelOptions>,
}

/// The config file's path (spec "Config file"):
/// `$XDG_CONFIG_HOME/pinwin/config.toml`, or `$HOME/.config/pinwin/config.toml`
/// when `XDG_CONFIG_HOME` is unset or empty. `None` means neither variable
/// names a config home, so no config file can be named.
#[must_use]
pub fn config_path(config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let dir = match config_home.filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(home.filter(|home| !home.is_empty())?).join(".config"),
    };
    Some(dir.join(CONFIG_DIR).join(CONFIG_FILE))
}

/// [`config_path`] from the process environment: `XDG_CONFIG_HOME` and
/// `HOME`, so the daemon reads the file the spec names.
#[must_use]
pub fn config_path_from_env() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME");
    let home = std::env::var_os("HOME");
    config_path(config_home.as_deref(), home.as_deref())
}

impl Config {
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
        let mut defaults = PanelOptions::default();
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

/// The merge behind [`Config::panel`] and [`PanelSettings::built_in`]
/// (spec "Panel settings and precedence"): the levels chained highest
/// first, then the built-in value of each setting.
fn resolve(
    defaults: &PanelOptions,
    named: Option<&PanelOptions>,
    options: &PanelOptions,
) -> PanelSettings {
    let merged = options
        .clone()
        .or(named.cloned().unwrap_or_default())
        .or(defaults.clone());
    PanelSettings {
        edge: merged.edge.unwrap_or(Edge::Left),
        size: merged.size.unwrap_or_else(default_size),
        reserve: merged.reserve.unwrap_or(ReserveMode::Push),
        layer: merged.layer.unwrap_or(Layer::Overlay),
        keyboard: merged.keyboard.unwrap_or(Keyboard::OnDemand),
        output: merged.output,
        hidden: merged.hidden.unwrap_or(false),
        focus_ring: merged.focus_ring.unwrap_or_else(default_focus_ring),
        tween_ms: merged.tween_ms.unwrap_or(200),
        gutters: Gutters::new(
            merged.gutter_left.unwrap_or(0),
            merged.gutter_right.unwrap_or(0),
            merged.gutter_top.unwrap_or(0),
            merged.gutter_bottom.unwrap_or(0),
        ),
    }
}

#[cfg(test)]
mod tests;
