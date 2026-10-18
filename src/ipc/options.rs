//! The wire text of a setting value (replace-terminal-with-app-host D13,
//! spec "Run options"): the `create` request's run options in the same
//! grammar as the config file and the command line, and the `Serialize` of
//! every type a request carries.
//!
//! The options keep their text on the wire, so the daemon parses each value
//! with the grammar the config file uses: a bad value is the known command's
//! `invalid_value`, not a request the parser cannot read (D13). The
//! serialization here is the one place a value gets its wire text, so the
//! options and the panel commands cannot drift apart.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize, Serializer};
use serde_json::Number;

use super::{RequestError, setting};
use crate::cli::Amount;
use crate::config::{FocusRing, Keyboard, Layer, PanelOptions};
use crate::geometry::{Edge, ReserveMode, Size};
use crate::name::PanelName;

/// The run options on the wire (spec "Run options", D2, D13): the same text
/// grammar as the config file and the command line, so a value cannot change
/// its meaning between them. An absent field leaves the config file's value
/// in place (spec "Panel settings and precedence"), and the fields the run
/// options do not carry stay at their default.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct WireOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    edge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reserve: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keyboard: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    focus_ring: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gutter_left: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gutter_right: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gutter_top: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gutter_bottom: Option<Number>,
}

/// The run options as the wire carries them, each value in its setting
/// grammar (spec "Run options").
fn wire_options(options: &PanelOptions) -> WireOptions {
    WireOptions {
        edge: options.edge.map(|value| value.as_str().to_owned()),
        size: options.size.map(size_text),
        reserve: options.reserve.map(|value| value.as_str().to_owned()),
        layer: options.layer.map(|value| value.as_str().to_owned()),
        keyboard: options.keyboard.map(|value| value.as_str().to_owned()),
        output: options.output.clone(),
        hidden: options.hidden,
        focus_ring: options.focus_ring.map(focus_ring_text),
        gutter_left: options.gutter_left.map(Number::from),
        gutter_right: options.gutter_right.map(Number::from),
        gutter_top: options.gutter_top.map(Number::from),
        gutter_bottom: options.gutter_bottom.map(Number::from),
    }
}

/// [`wire_options`] for the `Serialize` of [`Request::Create`].
pub(super) fn serialize<S: Serializer>(
    options: &PanelOptions,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    wire_options(options).serialize(serializer)
}

/// The run options of a `create` request, each value parsed with the config
/// file's grammar (D13), so a bad value is `invalid_value` of the known
/// command.
pub(super) fn parse(raw: WireOptions) -> Result<PanelOptions, RequestError> {
    Ok(PanelOptions {
        edge: option_setting("edge", raw.edge.as_deref())?,
        size: option_setting("size", raw.size.as_deref())?,
        reserve: option_setting("reserve", raw.reserve.as_deref())?,
        layer: option_setting("layer", raw.layer.as_deref())?,
        keyboard: option_setting("keyboard", raw.keyboard.as_deref())?,
        output: raw.output,
        hidden: raw.hidden,
        focus_ring: option_setting("focus_ring", raw.focus_ring.as_deref())?,
        gutter_left: option_gutter("gutter_left", raw.gutter_left)?,
        gutter_right: option_gutter("gutter_right", raw.gutter_right)?,
        gutter_top: option_gutter("gutter_top", raw.gutter_top)?,
        gutter_bottom: option_gutter("gutter_bottom", raw.gutter_bottom)?,
        ..PanelOptions::default()
    })
}

/// An optional setting, absent when its field is absent.
fn option_setting<T>(field: &str, raw: Option<&str>) -> Result<Option<T>, RequestError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    raw.map(|text| setting(field, text)).transpose()
}

/// An optional gutter of the run options (D7): an absent field keeps the
/// config file's value, while a fractional or an over-`i32` value is the
/// known command's `invalid_value`, not a request line the parser cannot
/// read (spec "Run options", "Gutter request fields").
fn option_gutter(field: &str, raw: Option<Number>) -> Result<Option<i32>, RequestError> {
    raw.map(|number| {
        super::whole(&number)
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| {
                RequestError::InvalidValue(format!(
                    "{field}: expected a whole number of pixels, got {number}"
                ))
            })
    })
    .transpose()
}

/// The wire text of a size: `640px` or `30%`.
fn size_text(size: Size) -> String {
    match size {
        Size::Pixels(px) => format!("{}px", px.get()),
        Size::Percent(percent) => format!("{}%", percent.get()),
    }
}

impl Serialize for PanelName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for Size {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&size_text(*self))
    }
}

impl Serialize for Amount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let sign = if self.shrinks() { '-' } else { '+' };
        let size = size_text(self.size());
        serializer.serialize_str(&format!("{sign}{size}"))
    }
}

/// The wire text of a focus ring (spec "Run options"): `off` or
/// `#RRGGBB:width`, the forms the setting grammar parses.
fn focus_ring_text(ring: FocusRing) -> String {
    match ring {
        FocusRing::Off => "off".to_owned(),
        FocusRing::On { color, width } => {
            let [red, green, blue] = color.channels();
            format!("#{red:02x}{green:02x}{blue:02x}:{}", width.get())
        }
    }
}

impl Serialize for Edge {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for ReserveMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for Layer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for Keyboard {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
