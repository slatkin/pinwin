//! The hand-written command line (replace-terminal-with-app-host D13, D15):
//! `pinwin run` with its options, `pinwin daemon`, and one client subcommand
//! for each daemon request (spec "Run options" and "Command-line client").
//!
//! The parser is pure and display-free (`port-to-rust` D10): it turns the
//! arguments into a [`Command`], which carries the resolved panel name, the
//! [`PanelOptions`] overrides of the config file and the command to run, or
//! one [`Request`]. It opens no socket and starts nothing; rows 5.x
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

use crate::config::{InvalidValue, PanelOptions, parse_digits};
use crate::geometry::{ReserveMode, Size};
use crate::ipc::Request;
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
    Client(Request),
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
        client => client_command(client, rest).map(Command::Client),
    }
}

/// Parse the `run` options and the command after the first non-option or
/// `--` (spec "Run options").
fn parse_run(args: &[OsString]) -> Result<Command, UsageError> {
    let (name, options, command) = run_arguments(args)?;
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

/// Walk the arguments before the command, collecting the options; the
/// command is everything after the first argument that is not an option, or
/// after `--` (spec "Run options").
fn run_arguments(
    args: &[OsString],
) -> Result<(Option<PanelName>, PanelOptions, Vec<OsString>), UsageError> {
    let mut name = None;
    let mut options = PanelOptions::default();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        let bytes = argument.as_os_str().as_bytes();
        if bytes == b"--" {
            return Ok((name, options, args[index + 1..].to_vec()));
        }
        if !bytes.starts_with(b"--") {
            return Ok((name, options, args[index..].to_vec()));
        }
        run_option(args, &mut index, argument, &mut name, &mut options)?;
        index += 1;
    }
    Ok((name, options, Vec::new()))
}

/// One `--option` of `run`, whose value is the next argument; it changes
/// the name or the options directly (spec "Run options").
fn run_option(
    args: &[OsString],
    index: &mut usize,
    argument: &OsStr,
    name: &mut Option<PanelName>,
    options: &mut PanelOptions,
) -> Result<(), UsageError> {
    let option = text("option", argument)?;
    match option {
        "--name" => {
            let raw = next_value(args, index, option)?;
            *name = Some(panel(option, raw)?);
        }
        "--output" => {
            let raw = next_value(args, index, option)?;
            options.output = Some(text(option, raw)?.to_owned());
        }
        "--hidden" => options.hidden = Some(true),
        "--edge" => setting_option(args, index, option, &mut options.edge)?,
        "--size" => setting_option(args, index, option, &mut options.size)?,
        "--reserve" => setting_option(args, index, option, &mut options.reserve)?,
        "--layer" => setting_option(args, index, option, &mut options.layer)?,
        "--keyboard" => setting_option(args, index, option, &mut options.keyboard)?,
        "--focus-ring" => setting_option(args, index, option, &mut options.focus_ring)?,
        other => return Err(UsageError::new(format!("unknown option '{other}'"))),
    }
    Ok(())
}

/// A `run` option whose value parses with the config file's grammar.
fn setting_option<T>(
    args: &[OsString],
    index: &mut usize,
    option: &str,
    slot: &mut Option<T>,
) -> Result<(), UsageError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    let raw = next_value(args, index, option)?;
    *slot = Some(setting(option, raw)?);
    Ok(())
}

/// The command one client subcommand asks for; each helper owns its own
/// arguments and its own usage message.
fn client_command(subcommand: &str, args: &[OsString]) -> Result<Request, UsageError> {
    match subcommand {
        "toggle" => panel_command(|panel| Request::Toggle { panel }, subcommand, args),
        "show" => panel_command(|panel| Request::Show { panel }, subcommand, args),
        "hide" => panel_command(|panel| Request::Hide { panel }, subcommand, args),
        "focus" => panel_command(|panel| Request::Focus { panel }, subcommand, args),
        "set-edge" => setting_command(subcommand, args, "an edge", |panel, edge| {
            Request::SetEdge { panel, edge }
        }),
        "set-size" => set_size(subcommand, args),
        "resize" => resize(subcommand, args),
        "set-output" => set_output(subcommand, args),
        "set-layer" => setting_command(subcommand, args, "a layer", |panel, layer| {
            Request::SetLayer { panel, layer }
        }),
        "set-reserve" => setting_command(subcommand, args, "a reserve mode", |panel, reserve| {
            Request::SetReserve { panel, reserve }
        }),
        "set-keyboard" => {
            setting_command(subcommand, args, "a keyboard mode", |panel, keyboard| {
                Request::SetKeyboard { panel, keyboard }
            })
        }
        "status" => status_command(subcommand, args),
        "subscribe" => no_arg_command(Request::Subscribe, subcommand, args),
        "quit" => no_arg_command(Request::Quit, subcommand, args),
        "reload" => no_arg_command(Request::Reload, subcommand, args),
        other => Err(unknown_command(other)),
    }
}

/// A client command that names one panel and nothing else.
fn panel_command(
    build: fn(PanelName) -> Request,
    subcommand: &str,
    args: &[OsString],
) -> Result<Request, UsageError> {
    Ok(build(one_panel(subcommand, args)?))
}

/// A client command that takes no arguments.
fn no_arg_command(
    command: Request,
    subcommand: &str,
    args: &[OsString],
) -> Result<Request, UsageError> {
    no_args(subcommand, args)?;
    Ok(command)
}

/// A `set-edge`, `set-layer`, `set-reserve` or `set-keyboard` command: a
/// panel name and one value in the config file's grammar.
fn setting_command<T>(
    subcommand: &str,
    args: &[OsString],
    value: &str,
    build: fn(PanelName, T) -> Request,
) -> Result<Request, UsageError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    let (panel, raw) = panel_and_value(subcommand, args, value)?;
    Ok(build(panel, setting(subcommand, raw)?))
}

/// `set-size <name> <size> [--reserve push|cover] [--ms N]`.
fn set_size(subcommand: &str, args: &[OsString]) -> Result<Request, UsageError> {
    let (panel, raw, reserve, ms) = panel_value_options(subcommand, args, "a size")?;
    Ok(Request::SetSize {
        panel,
        size: setting(subcommand, &raw)?,
        reserve,
        ms,
    })
}

/// `resize <name> <amount> [--reserve push|cover] [--ms N]`.
fn resize(subcommand: &str, args: &[OsString]) -> Result<Request, UsageError> {
    let (panel, raw, reserve, ms) = panel_value_options(subcommand, args, "an amount")?;
    Ok(Request::Resize {
        panel,
        by: setting(subcommand, &raw)?,
        reserve,
        ms,
    })
}

/// `set-output <name> <output>`.
fn set_output(subcommand: &str, args: &[OsString]) -> Result<Request, UsageError> {
    let (panel, raw) = panel_and_value(subcommand, args, "an output name")?;
    Ok(Request::SetOutput {
        panel,
        output: text(subcommand, raw)?.to_owned(),
    })
}

/// `status [<name>]` (spec "Status").
fn status_command(subcommand: &str, args: &[OsString]) -> Result<Request, UsageError> {
    let panel = match args {
        [] => None,
        [raw] => Some(panel(subcommand, raw)?),
        _ => {
            return Err(UsageError::new("status: expected at most one panel name"));
        }
    };
    Ok(Request::Status { panel })
}

/// The message of a subcommand the parser does not know.
fn unknown_command(subcommand: &str) -> UsageError {
    UsageError::new(format!("unknown command '{subcommand}'"))
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

/// A panel name, a value, and the optional `--reserve push|cover` and
/// `--ms N` in any order after them (D4).
fn panel_value_options(
    subcommand: &str,
    args: &[OsString],
    value: &str,
) -> Result<(PanelName, OsString, Option<ReserveMode>, Option<u32>), UsageError> {
    let expected = || {
        UsageError::new(format!(
            "{subcommand}: expected <name> {value} [--reserve push|cover] [--ms N]"
        ))
    };
    let (raw_panel, rest) = args.split_first().ok_or_else(expected)?;
    let (raw_value, mut rest) = rest.split_first().ok_or_else(expected)?;
    let panel = panel(subcommand, raw_panel)?;
    let mut reserve = None;
    let mut ms = None;
    while let Some((option, tail)) = rest.split_first() {
        let (raw, next) = tail.split_first().ok_or_else(expected)?;
        match text("option", option)? {
            "--reserve" => reserve = Some(setting("--reserve", raw)?),
            "--ms" => ms = Some(millis(raw)?),
            _ => return Err(expected()),
        }
        rest = next;
    }
    Ok((panel, raw_value.clone(), reserve, ms))
}

/// A whole number of milliseconds; `0` is the snap animation (spec
/// "Animated size changes").
fn millis(raw: &OsStr) -> Result<u32, UsageError> {
    let raw = text("--ms", raw)?;
    parse_digits(raw).ok_or_else(|| {
        UsageError::new(format!(
            "--ms: expected a whole number of milliseconds, got '{raw}'"
        ))
    })
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
