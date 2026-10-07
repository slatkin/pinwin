//! The host's command-line parsing (`host/main.c`'s argv contract,
//! port-to-rust D8), plus the `--toggle [name]` client mode of
//! `replace-gtk-with-wayland` D4: `--toggle` before `--` asks a running
//! panel to show or hide itself, anything else is the command to run in the
//! panel. `--` ends option parsing, so `-- --toggle` runs a command
//! literally named `--toggle`, and an unknown `--`-prefixed option is
//! rejected.

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

use pinwin::instance::InstanceName;

/// The shell used when `$SHELL` is unset or empty.
const FALLBACK_SHELL: &str = "/bin/sh";

/// What the host's arguments ask for. `--toggle [name]` before `--` selects
/// the client mode that asks a running panel to show or hide itself;
/// anything else is the command to run in the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Host {
        /// An empty command means "use the default shell".
        command: Vec<OsString>,
    },
    Toggle {
        /// `None` is the default instance name.
        name: Option<InstanceName>,
    },
}

/// Parse the host's arguments. The optional `--toggle` name is the next
/// argument, whatever it is; the client mode runs no command.
pub(crate) fn parse_args(args: &[OsString]) -> Result<Mode, String> {
    let mut rest = args;
    if let Some(first) = rest.first() {
        if first.as_bytes() == b"--toggle" {
            return Ok(Mode::Toggle {
                name: rest
                    .get(1)
                    .map(|raw| InstanceName::parse("--toggle", &raw.to_string_lossy()))
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
            Ok(Mode::Toggle { name: None })
        );
        assert_eq!(
            parse_args(&os(&["--toggle", "notes"])),
            Ok(Mode::Toggle {
                name: Some(InstanceName::parse("--toggle", "notes").expect("valid"))
            })
        );

        // The same name check as `PINWIN_NAME`, under the `--toggle` label:
        // a bad name, the empty one included, is an exit-2 error. The exact
        // wording is `InstanceName::parse`'s contract, asserted in `instance.rs`;
        // here only the error class holds: the rejection names the option.
        for raw in ["", "a/b"] {
            assert!(
                matches!(
                    parse_args(&os(&["--toggle", raw])),
                    Err(message) if message.contains("pinwin: --toggle:")
                ),
                "raw = {raw:?}"
            );
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
