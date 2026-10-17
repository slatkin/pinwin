//! The hand-written command line (replace-terminal-with-app-host D13, D15):
//! `pinwin run` with its options, `pinwin daemon`, and one client subcommand
//! for each daemon request (spec "Run options" and "Command-line client").
//!
//! The parser is pure and display-free (`port-to-rust` D10): it turns the
//! arguments into a [`Command`], which carries the resolved panel name, the
//! [`PanelOptions`] overrides of the config file and the command to run, or
//! one [`ClientCommand`]. It opens no socket and starts nothing; rows 5.x
//! and 6.x own the daemon, the IPC and the panel. Every setting value is
//! parsed with the same `FromStr` grammar as the config file, so the option
//! and the file cannot drift apart.
//!
//! A command line that names no usable command is a [`UsageError`]: the
//! binary prints its message and exits 2 (spec "Command-line exit codes").

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::str::FromStr;

use crate::config::{FocusRing, InvalidValue, Keyboard, Layer, PanelOptions};
use crate::geometry::{Edge, ReserveMode, Size};
use crate::name::PanelName;

/// Every subcommand the parser accepts, in README order (spec
/// "Command-line client"). The README's command table is checked against
/// this list in both directions, as the configuration keys are.
#[cfg(test)]
pub const SUBCOMMANDS: [&str; 17] = [
    "run",
    "daemon",
    "toggle",
    "show",
    "hide",
    "focus",
    "set-edge",
    "set-size",
    "resize",
    "set-output",
    "set-layer",
    "set-reserve",
    "set-keyboard",
    "status",
    "subscribe",
    "quit",
    "reload",
];

/// The forms a `resize` amount accepts (spec "Size values": "A `resize`
/// amount SHALL use the same forms with a sign").
const SIGNED_SIZE: &str = "a signed size such as -40px or +10%";

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Start an app in a new panel (spec "Run an app in a panel").
    Run(Run),
    /// Start the daemon in the foreground (spec "One daemon for each
    /// Wayland display").
    Daemon,
    /// Send one request to the daemon (spec "Command-line client").
    Client(ClientCommand),
}

/// A `pinwin run` invocation: the resolved panel name, the setting
/// overrides and the command to replace this process with. Fields are
/// private, so the name always comes from [`PanelName`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    name: PanelName,
    options: PanelOptions,
    command: Vec<OsString>,
}

impl Run {
    /// The panel name: `--name`, else the command's file name (spec
    /// "Panel names").
    #[must_use]
    pub const fn name(&self) -> &PanelName {
        &self.name
    }

    /// The setting overrides the options carry; an absent field leaves the
    /// config file's value in place (spec "Panel settings and precedence").
    #[must_use]
    pub const fn options(&self) -> &PanelOptions {
        &self.options
    }

    /// The command and its arguments, never empty (spec "No command").
    #[must_use]
    pub fn command(&self) -> &[OsString] {
        &self.command
    }
}

/// The one request a client subcommand sends (spec "Command-line client").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientCommand {
    Toggle(PanelName),
    Show(PanelName),
    Hide(PanelName),
    Focus(PanelName),
    SetEdge {
        panel: PanelName,
        edge: Edge,
    },
    SetSize {
        panel: PanelName,
        size: Size,
        ms: Option<u32>,
    },
    Resize {
        panel: PanelName,
        amount: Amount,
        ms: Option<u32>,
    },
    SetOutput {
        panel: PanelName,
        output: String,
    },
    SetLayer {
        panel: PanelName,
        layer: Layer,
    },
    SetReserve {
        panel: PanelName,
        reserve: ReserveMode,
    },
    SetKeyboard {
        panel: PanelName,
        keyboard: Keyboard,
    },
    Status {
        panel: Option<PanelName>,
    },
    Subscribe,
    Quit,
    Reload,
}

/// A relative size change for `pinwin resize`: the magnitude uses the size
/// grammar and the sign sets the direction, so `-40px` shrinks by 40 pixels
/// and `+10%` grows by ten percent (spec "Size values").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Amount {
    sign: Sign,
    size: Size,
}

/// The direction of a [`Amount`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sign {
    Grow,
    Shrink,
}

impl Amount {
    /// A growing change of `size`: the wire's positive form and the `+` of
    /// the command line (spec "Size values").
    #[must_use]
    pub const fn grow(size: Size) -> Self {
        Self {
            sign: Sign::Grow,
            size,
        }
    }

    /// A shrinking change of `size`: the wire's negative form and the `-` of
    /// the command line (spec "Size values").
    #[must_use]
    pub const fn shrink(size: Size) -> Self {
        Self {
            sign: Sign::Shrink,
            size,
        }
    }

    /// The magnitude, in the same forms as a size.
    #[must_use]
    pub const fn size(self) -> Size {
        self.size
    }

    /// Whether the change removes length; without it the change adds.
    #[must_use]
    pub const fn shrinks(self) -> bool {
        matches!(self.sign, Sign::Shrink)
    }
}

impl FromStr for Amount {
    type Err = InvalidValue;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (sign, magnitude) = match raw.as_bytes().first() {
            Some(b'+') => (Sign::Grow, &raw[1..]),
            Some(b'-') => (Sign::Shrink, &raw[1..]),
            _ => return Err(InvalidValue::new(raw, SIGNED_SIZE)),
        };
        let Ok(size) = magnitude.parse::<Size>() else {
            return Err(InvalidValue::new(raw, SIGNED_SIZE));
        };
        Ok(Self { sign, size })
    }
}

/// Why the command line is not usable. Every rejection is a usage error:
/// the binary prints the message and exits 2 (spec "Command-line exit
/// codes"), so the exit code is part of the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError {
    message: String,
}

impl UsageError {
    /// The exit code of a usage error (spec "Command-line exit codes").
    pub const EXIT_CODE: u8 = 2;

    fn new(message: impl Into<String>) -> Self {
        Self {
            message: format!("pinwin: {}", message.into()),
        }
    }

    /// The message the binary prints, without a trailing newline.
    #[cfg(test)]
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UsageError {}

/// Parse the arguments after the program name. `run` gets the name and the
/// options the config file is merged with; the client subcommands get the
/// value they send (spec "Run options" and "Command-line client").
pub fn parse_args(args: &[OsString]) -> Result<Command, UsageError> {
    let Some((first, rest)) = args.split_first() else {
        return Err(UsageError::new("expected a subcommand"));
    };
    let subcommand = text("command", first)?;
    match subcommand {
        "run" => parse_run(rest),
        "daemon" => {
            no_args(subcommand, rest)?;
            Ok(Command::Daemon)
        }
        "toggle" | "show" | "hide" | "focus" | "set-edge" | "set-size" | "resize"
        | "set-output" | "set-layer" | "set-reserve" | "set-keyboard" | "status" | "subscribe"
        | "quit" | "reload" => parse_client(subcommand, rest),
        other => Err(UsageError::new(format!("unknown command '{other}'"))),
    }
}

/// Parse the `run` options and the command after the first non-option or
/// `--` (spec "Run options").
fn parse_run(args: &[OsString]) -> Result<Command, UsageError> {
    let mut name = None;
    let mut options = PanelOptions::default();
    let mut index = 0;
    let mut command = None;
    while let Some(argument) = args.get(index) {
        let bytes = argument.as_os_str().as_bytes();
        if bytes == b"--" {
            command = Some(args[index + 1..].to_vec());
            break;
        }
        if !bytes.starts_with(b"--") {
            command = Some(args[index..].to_vec());
            break;
        }
        let option = text("option", argument)?;
        match option {
            "--name" => name = Some(panel("--name", next_value(args, &mut index, option)?)?),
            "--edge" => {
                options.edge = Some(setting(option, next_value(args, &mut index, option)?)?);
            }
            "--size" => {
                options.size = Some(setting(option, next_value(args, &mut index, option)?)?);
            }
            "--reserve" => {
                options.reserve = Some(setting(option, next_value(args, &mut index, option)?)?);
            }
            "--layer" => {
                options.layer = Some(setting(option, next_value(args, &mut index, option)?)?);
            }
            "--keyboard" => {
                options.keyboard = Some(setting(option, next_value(args, &mut index, option)?)?);
            }
            "--output" => {
                let raw = next_value(args, &mut index, option)?;
                options.output = Some(text(option, raw)?.to_owned());
            }
            "--hidden" => options.hidden = Some(true),
            "--focus-ring" => {
                let value: FocusRing = setting(option, next_value(args, &mut index, option)?)?;
                options.focus_ring = Some(value);
            }
            other => return Err(UsageError::new(format!("unknown option '{other}'"))),
        }
        index += 1;
    }

    let command = command.unwrap_or_default();
    let Some(first) = command.first() else {
        return Err(UsageError::new(
            "run: no command; pass -- <command> [args...]",
        ));
    };
    let name = match name {
        Some(name) => name,
        None => {
            PanelName::from_command(first).map_err(|error| UsageError::new(error.to_string()))?
        }
    };
    Ok(Command::Run(Run {
        name,
        options,
        command,
    }))
}

/// Parse one client subcommand (spec "Command-line client").
fn parse_client(subcommand: &str, args: &[OsString]) -> Result<Command, UsageError> {
    let command = match subcommand {
        "toggle" => ClientCommand::Toggle(one_panel(subcommand, args)?),
        "show" => ClientCommand::Show(one_panel(subcommand, args)?),
        "hide" => ClientCommand::Hide(one_panel(subcommand, args)?),
        "focus" => ClientCommand::Focus(one_panel(subcommand, args)?),
        "set-edge" => {
            let (panel, raw) = panel_and_value(subcommand, args, "an edge")?;
            ClientCommand::SetEdge {
                panel,
                edge: setting(subcommand, raw)?,
            }
        }
        "set-size" => {
            let (panel, raw, ms) = panel_value_ms(subcommand, args, "a size")?;
            ClientCommand::SetSize {
                panel,
                size: setting(subcommand, &raw)?,
                ms,
            }
        }
        "resize" => {
            let (panel, raw, ms) = panel_value_ms(subcommand, args, "an amount")?;
            ClientCommand::Resize {
                panel,
                amount: setting(subcommand, &raw)?,
                ms,
            }
        }
        "set-output" => {
            let (panel, raw) = panel_and_value(subcommand, args, "an output name")?;
            ClientCommand::SetOutput {
                panel,
                output: text(subcommand, raw)?.to_owned(),
            }
        }
        "set-layer" => {
            let (panel, raw) = panel_and_value(subcommand, args, "a layer")?;
            ClientCommand::SetLayer {
                panel,
                layer: setting(subcommand, raw)?,
            }
        }
        "set-reserve" => {
            let (panel, raw) = panel_and_value(subcommand, args, "a reserve mode")?;
            ClientCommand::SetReserve {
                panel,
                reserve: setting(subcommand, raw)?,
            }
        }
        "set-keyboard" => {
            let (panel, raw) = panel_and_value(subcommand, args, "a keyboard mode")?;
            ClientCommand::SetKeyboard {
                panel,
                keyboard: setting(subcommand, raw)?,
            }
        }
        "status" => ClientCommand::Status {
            panel: match args {
                [] => None,
                [raw] => Some(panel(subcommand, raw)?),
                _ => {
                    return Err(UsageError::new("status: expected at most one panel name"));
                }
            },
        },
        "subscribe" => {
            no_args(subcommand, args)?;
            ClientCommand::Subscribe
        }
        "quit" => {
            no_args(subcommand, args)?;
            ClientCommand::Quit
        }
        "reload" => {
            no_args(subcommand, args)?;
            ClientCommand::Reload
        }
        other => return Err(UsageError::new(format!("unknown command '{other}'"))),
    };
    Ok(Command::Client(command))
}

/// The next argument as an option's value; the index moves onto it.
fn next_value<'a>(
    args: &'a [OsString],
    index: &mut usize,
    option: &str,
) -> Result<&'a OsStr, UsageError> {
    *index += 1;
    args.get(*index)
        .map(OsString::as_os_str)
        .ok_or_else(|| UsageError::new(format!("{option}: missing value")))
}

/// An argument as UTF-8, with the context a message needs.
fn text<'a>(context: &str, raw: &'a OsStr) -> Result<&'a str, UsageError> {
    raw.to_str()
        .ok_or_else(|| UsageError::new(format!("{context}: expected valid UTF-8")))
}

/// A panel name with the option's context: [`PanelName::parse`] carries
/// none (spec "Panel names").
fn panel(context: &str, raw: &OsStr) -> Result<PanelName, UsageError> {
    let raw = text(context, raw)?;
    PanelName::parse(raw).map_err(|error| UsageError::new(format!("{context}: {error}")))
}

/// A setting value parsed with its config-file grammar.
fn setting<T>(context: &str, raw: &OsStr) -> Result<T, UsageError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    let raw = text(context, raw)?;
    raw.parse()
        .map_err(|error| UsageError::new(format!("{context}: {error}")))
}

/// Exactly one panel name.
fn one_panel(subcommand: &str, args: &[OsString]) -> Result<PanelName, UsageError> {
    match args {
        [raw] => panel(subcommand, raw),
        _ => Err(UsageError::new(format!(
            "{subcommand}: expected one panel name"
        ))),
    }
}

/// A panel name and one value, and nothing else.
fn panel_and_value<'a>(
    subcommand: &str,
    args: &'a [OsString],
    value: &str,
) -> Result<(PanelName, &'a OsStr), UsageError> {
    match args {
        [raw_panel, raw_value] => Ok((panel(subcommand, raw_panel)?, raw_value.as_os_str())),
        _ => Err(UsageError::new(format!(
            "{subcommand}: expected <name> {value}"
        ))),
    }
}

/// A panel name, a value and the optional `--ms N`.
fn panel_value_ms(
    subcommand: &str,
    args: &[OsString],
    value: &str,
) -> Result<(PanelName, OsString, Option<u32>), UsageError> {
    let (raw_panel, rest) = args.split_first().ok_or_else(|| {
        UsageError::new(format!("{subcommand}: expected <name> {value} [--ms N]"))
    })?;
    let (raw_value, rest) = rest.split_first().ok_or_else(|| {
        UsageError::new(format!("{subcommand}: expected <name> {value} [--ms N]"))
    })?;
    let panel = panel(subcommand, raw_panel)?;
    let ms = match rest {
        [] => None,
        [option, raw] if option.as_os_str().as_bytes() == b"--ms" => Some(millis(raw)?),
        _ => {
            return Err(UsageError::new(format!(
                "{subcommand}: expected <name> {value} [--ms N]"
            )));
        }
    };
    Ok((panel, raw_value.clone(), ms))
}

/// A whole number of milliseconds; `0` is the snap animation (spec
/// "Animated size changes").
fn millis(raw: &OsStr) -> Result<u32, UsageError> {
    let raw = text("--ms", raw)?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(UsageError::new(format!(
            "--ms: expected a whole number of milliseconds, got '{raw}'"
        )));
    }
    let Ok(ms) = raw.parse() else {
        return Err(UsageError::new(format!(
            "--ms: expected a whole number of milliseconds, got '{raw}'"
        )));
    };
    Ok(ms)
}

/// A subcommand that takes no arguments.
fn no_args(subcommand: &str, args: &[OsString]) -> Result<(), UsageError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(UsageError::new(format!("{subcommand}: takes no arguments")))
    }
}

#[cfg(test)]
mod tests;
