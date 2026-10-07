//! The host's command-line parsing (`host/main.c`'s argv contract,
//! port-to-rust D8), plus the `--toggle [name]` and `--show [name]` client
//! modes of `replace-gtk-with-wayland` D4 and `serve-instance-socket`:
//! `--toggle`/`--show` before `--` ask a running panel to show or hide
//! itself, or to show itself, anything else is the command to run in the
//! panel. `--` ends option parsing, so `-- --toggle` runs a command
//! literally named `--toggle`, and an unknown `--`-prefixed option is
//! rejected.

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

use pinwin::instance::{InstanceName, Request};

/// The shell used when `$SHELL` is unset or empty.
const FALLBACK_SHELL: &str = "/bin/sh";

/// What the host's arguments ask for. `--toggle [name]` and
/// `--show [name]` before `--` select the client mode that asks a running
/// panel to show or hide itself, or to show itself; anything else is the
/// command to run in the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Host {
        /// An empty command means "use the default shell".
        command: Vec<OsString>,
    },
    Client {
        /// What to ask the running panel to do.
        request: Request,
        /// `None` is the default instance name.
        name: Option<InstanceName>,
    },
}

/// The client option's request, when the first argument is one.
fn client_request(arg: &OsStr) -> Option<Request> {
    match arg.as_bytes() {
        b"--toggle" => Some(Request::Toggle),
        b"--show" => Some(Request::Show),
        _ => None,
    }
}

/// Parse the host's arguments. The optional `--toggle`/`--show` name is the
/// next argument, whatever it is; the client mode runs no command.
pub(crate) fn parse_args(args: &[OsString]) -> Result<Mode, String> {
    let mut rest = args;
    if let Some(first) = rest.first() {
        if let Some(request) = client_request(first) {
            return Ok(Mode::Client {
                request,
                name: rest
                    .get(1)
                    .map(|raw| {
                        // The library reports the rejection without a context
                        // label (`serve-instance-socket` D5); this adds the
                        // option's.
                        InstanceName::parse(&raw.to_string_lossy()).map_err(|error| {
                            format!("pinwin: {}: {error}", first.to_string_lossy())
                        })
                    })
                    .transpose()?,
            });
        } else if first.as_bytes() == b"--" {
            rest = &rest[1..];
        } else if first.as_bytes().starts_with(b"--") {
            return Err(format!(
                "pinwin: unknown option '{}'",
                first.to_string_lossy()
            ));
        }
    }
    Ok(Mode::Host {
        command: rest.to_vec(),
    })
}

/// The default command when the host gave none: `$SHELL`, else `/bin/sh`.
pub(crate) fn default_command() -> Vec<OsString> {
    let shell = env::var_os("SHELL").filter(|shell| !shell.is_empty());
    vec![shell.unwrap_or_else(|| OsStr::new(FALLBACK_SHELL).to_os_string())]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parse_args` mirrors `host/main.c`'s argv handling for the command.
    #[test]
    fn parse_args_separates_options_from_the_command() {
        let os = |slice: &[&str]| -> Vec<OsString> {
            slice
                .iter()
                .map(OsStr::new)
                .map(OsStr::to_os_string)
                .collect()
        };

        // No arguments, and `--` alone: the default shell.
        assert_eq!(parse_args(&os(&[])), Ok(Mode::Host { command: vec![] }));
        assert_eq!(parse_args(&os(&["--"])), Ok(Mode::Host { command: vec![] }));

        // A command passes through; `--` ends option parsing.
        assert_eq!(
            parse_args(&os(&["htop"])),
            Ok(Mode::Host {
                command: os(&["htop"])
            })
        );
        assert_eq!(
            parse_args(&os(&["--", "htop", "-x"])),
            Ok(Mode::Host {
                command: os(&["htop", "-x"])
            })
        );

        // A `--`-prefixed unknown option is rejected, `--` after the first
        // argument is just a command argument.
        assert_eq!(
            parse_args(&os(&["--frobnicate"])),
            Err("pinwin: unknown option '--frobnicate'".to_owned())
        );
        assert_eq!(
            parse_args(&os(&["htop", "--help"])),
            Ok(Mode::Host {
                command: os(&["htop", "--help"])
            })
        );
    }

    /// `--toggle [name]` before `--` selects the client mode with the same
    /// name check; after `--` it is an ordinary command argument.
    #[test]
    fn parse_args_parses_the_toggle_mode() {
        let os = |slice: &[&str]| -> Vec<OsString> {
            slice
                .iter()
                .map(OsStr::new)
                .map(OsStr::to_os_string)
                .collect()
        };

        // No name is the default instance.
        assert_eq!(
            parse_args(&os(&["--toggle"])),
            Ok(Mode::Client {
                request: Request::Toggle,
                name: None
            })
        );
        assert_eq!(
            parse_args(&os(&["--toggle", "notes"])),
            Ok(Mode::Client {
                request: Request::Toggle,
                name: Some(InstanceName::parse("notes").expect("valid"))
            })
        );

        // The same name check as `PINWIN_NAME`, under the option's label:
        // a bad name, the empty one included, is an exit-2 error. The exact
        // wording is `InstanceName::parse`'s contract, asserted in `instance.rs`;
        // here only the error class holds: the rejection names the option.
        for option in ["--toggle", "--show"] {
            for raw in ["", "a/b"] {
                assert!(
                    matches!(
                        parse_args(&os(&[option, raw])),
                        Err(message) if message.starts_with(&format!("pinwin: {option}:"))
                    ),
                    "option = {option:?}, raw = {raw:?}"
                );
            }
        }

        // `--` ends option parsing: a command literally named `--toggle`
        // runs.
        assert_eq!(
            parse_args(&os(&["--", "--toggle"])),
            Ok(Mode::Host {
                command: os(&["--toggle"])
            })
        );
    }

    /// `--show [name]` selects the show client mode with the same shape as
    /// `--toggle`: no name is the default instance, a name is checked under
    /// the `--show` label, and after `--` the word is an ordinary command
    /// argument.
    #[test]
    fn parse_args_parses_the_show_mode() {
        let os = |slice: &[&str]| -> Vec<OsString> {
            slice
                .iter()
                .map(OsStr::new)
                .map(OsStr::to_os_string)
                .collect()
        };

        assert_eq!(
            parse_args(&os(&["--show"])),
            Ok(Mode::Client {
                request: Request::Show,
                name: None
            })
        );
        assert_eq!(
            parse_args(&os(&["--show", "notes"])),
            Ok(Mode::Client {
                request: Request::Show,
                name: Some(InstanceName::parse("notes").expect("valid"))
            })
        );

        // The invalid name is rejected under the `--show` label, exit 2.
        assert!(
            matches!(
                parse_args(&os(&["--show", "a/b"])),
                Err(message) if message.starts_with("pinwin: --show:")
            ),
            "the rejection names the option"
        );

        // `--` ends option parsing: a command literally named `--show` runs.
        assert_eq!(
            parse_args(&os(&["--", "--show"])),
            Ok(Mode::Host {
                command: os(&["--show"])
            })
        );
    }

    /// The default command is `$SHELL`, else `/bin/sh`.
    #[test]
    fn default_command_prefers_the_shell_variable() {
        // SAFETY: single-threaded test process; the variable is restored
        // before the test ends so the other tests see the real environment.
        unsafe {
            env::set_var("SHELL", "/bin/zsh");
        };
        assert_eq!(
            default_command(),
            vec![OsStr::new("/bin/zsh").to_os_string()]
        );
        // SAFETY: single-threaded test process; the variable is restored
        // before the test ends so the other tests see the real environment.
        unsafe {
            env::set_var("SHELL", "");
        };
        assert_eq!(
            default_command(),
            vec![OsStr::new(FALLBACK_SHELL).to_os_string()]
        );
        // SAFETY: single-threaded test process; the variable is restored
        // before the test ends so the other tests see the real environment.
        unsafe {
            env::remove_var("SHELL");
        };
        assert_eq!(
            default_command(),
            vec![OsStr::new(FALLBACK_SHELL).to_os_string()]
        );
    }
}
