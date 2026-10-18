//! The typed builders of a checked-shape request line (D13): each family of
//! [`Request`] variants parses its own wire fields with the config file's
//! grammar, so a bad value names its field instead of failing the whole
//! line.
//!
//! [`Request::parse`](super::Request::parse) keeps the dispatcher and the
//! raw enum in `src/ipc.rs`; these builders own the parsing of one family
//! each, which keeps the dispatcher and every builder small.

use std::fmt;
use std::str::FromStr;

use serde_json::Number;

use super::options::WireOptions;
use super::{
    RawAmount, RawGutters, RawSize, Request, RequestError, amount, gutter, millis, no_gutter_field,
    optional_setting, panel_name, setting, wire_size,
};
use crate::geometry::GutterChange;
use crate::name::PanelName;

/// The four client commands that name one panel and no value (`toggle`,
/// `show`, `hide` and `focus`), so [`panel_only`] builds each of them from a
/// command value instead of a closure of its own.
#[derive(Clone, Copy)]
pub(super) enum PanelCommand {
    Toggle,
    Show,
    Hide,
    Focus,
}

impl PanelCommand {
    /// The request this command asks for.
    const fn request(self, panel: PanelName) -> Request {
        match self {
            Self::Toggle => Request::Toggle { panel },
            Self::Show => Request::Show { panel },
            Self::Hide => Request::Hide { panel },
            Self::Focus => Request::Focus { panel },
        }
    }
}

/// A request that names one panel and no value (`toggle`, `show`, `hide`
/// and `focus`).
pub(super) fn panel_only(panel: &str, command: PanelCommand) -> Result<Request, RequestError> {
    Ok(command.request(panel_name(panel)?))
}

/// A request whose only value parses with the config file's grammar
/// (`set-edge`, `set-layer`, `set-reserve` and `set-keyboard`).
pub(super) fn setting_value<T>(
    panel: &str,
    raw: &str,
    field: &str,
    build: fn(PanelName, T) -> Request,
) -> Result<Request, RequestError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    Ok(build(panel_name(panel)?, setting(field, raw)?))
}

/// A `set-size` request: the size, the optional reserve mode and the
/// optional duration (spec "Size and reserve in one request", D4).
pub(super) fn set_size(
    panel: &str,
    size: RawSize,
    reserve: Option<&str>,
    ms: Option<Number>,
) -> Result<Request, RequestError> {
    Ok(Request::SetSize {
        panel: panel_name(panel)?,
        size: wire_size(size)?,
        reserve: optional_setting("reserve", reserve)?,
        ms: millis(ms)?,
    })
}

/// A `resize` request: the amount, the optional reserve mode and the
/// optional duration.
pub(super) fn resize(
    panel: &str,
    by: RawAmount,
    reserve: Option<&str>,
    ms: Option<Number>,
) -> Result<Request, RequestError> {
    Ok(Request::Resize {
        panel: panel_name(panel)?,
        by: amount(by)?,
        reserve: optional_setting("reserve", reserve)?,
        ms: millis(ms)?,
    })
}

/// A `set-gutters` request: at least one whole-number gutter, each
/// parsed on its own (spec "Gutter request fields", D7).
pub(super) fn gutters(panel: &str, raw: RawGutters) -> Result<Request, RequestError> {
    let change = GutterChange {
        top: gutter("top", raw.top)?,
        bottom: gutter("bottom", raw.bottom)?,
        left: gutter("left", raw.left)?,
        right: gutter("right", raw.right)?,
    };
    if change.is_empty() {
        return Err(no_gutter_field());
    }
    Ok(Request::SetGutters {
        panel: panel_name(panel)?,
        change,
    })
}

/// A `set-output` request: the panel and the output's name.
pub(super) fn output(panel: &str, output: String) -> Result<Request, RequestError> {
    Ok(Request::SetOutput {
        panel: panel_name(panel)?,
        output,
    })
}

/// A `status` request: every panel, or the one the line names.
pub(super) fn status(panel: Option<&str>) -> Result<Request, RequestError> {
    Ok(Request::Status {
        panel: panel.map(panel_name).transpose()?,
    })
}

/// A `create` request: the caller's process, the app's file name and the
/// run options (spec "Run an app in a panel", D2).
pub(super) fn create(
    panel: &str,
    pid: u32,
    app: String,
    options: WireOptions,
    show_existing: bool,
) -> Result<Request, RequestError> {
    Ok(Request::Create {
        panel: panel_name(panel)?,
        pid,
        app,
        options: super::options::parse(options)?,
        show_existing,
    })
}
