//! The loading half of the Ghostty-config reader: the thin wrappers that
//! resolve the XDG config paths and read the files, split from the parsing
//! half in the parent module (row 8.1). The parsing functions stay pure; a
//! missing or unreadable file falls back to the defaults here.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::{FontConfig, ThemeColours, parse_font_config, parse_theme_config, parse_theme_file};

/// Load the font from `$XDG_CONFIG_HOME/ghostty/config` (default
/// `~/.config/ghostty/config`); a missing or unreadable file falls back to
/// [`FontConfig::default`] (``monospace 11``).
#[must_use]
pub fn load_font_config() -> FontConfig {
    match user_config_dir() {
        Some(config_dir) => load_font_config_at(&config_dir),
        None => FontConfig::default(),
    }
}

/// Load the theme colours from the Ghostty config and, when it names a
/// `theme`, the matching theme file (user themes first, then
/// `/usr/share/ghostty/themes`); a missing config leaves the defaults.
#[allow(clippy::must_use_candidate, reason = "approved #13: pure loader")]
pub fn load_theme_colours() -> ThemeColours {
    match user_config_dir() {
        Some(config_dir) => load_theme_colours_at(&config_dir),
        None => ThemeColours::default(),
    }
}

fn load_font_config_at(config_dir: &Path) -> FontConfig {
    let path = config_dir.join("ghostty").join("config");
    match read_text(&path) {
        Some(contents) => parse_font_config(&contents),
        None => FontConfig::default(),
    }
}

fn load_theme_colours_at(config_dir: &Path) -> ThemeColours {
    let mut colours = ThemeColours::default();

    let config_path = config_dir.join("ghostty").join("config");
    let Some(contents) = read_text(&config_path) else {
        return colours;
    };
    let Some(theme) = parse_theme_config(&contents, &mut colours) else {
        return colours;
    };

    // Theme files live next to the config or in Ghostty's install directory.
    let user_theme = config_dir.join("ghostty").join("themes").join(&theme);
    let theme_contents = read_text(&user_theme)
        .or_else(|| read_text(&Path::new("/usr/share/ghostty/themes").join(&theme)));
    if let Some(contents) = theme_contents {
        parse_theme_file(&contents, &mut colours);
    }

    colours
}

/// The config directory both loaders read from: `$XDG_CONFIG_HOME` when it is
/// set, non-empty and absolute, else `$HOME/.config`; `None` when neither is
/// usable, which leaves both loaders at their defaults.
fn user_config_dir() -> Option<PathBuf> {
    user_config_dir_from(&|key| std::env::var_os(key))
}

/// The rule behind [`user_config_dir`], with the environment injected so the
/// tests never touch the process environment.
fn user_config_dir_from(getenv: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = getenv("XDG_CONFIG_HOME")
        && !dir.is_empty()
        && Path::new(&dir).is_absolute()
    {
        return Some(PathBuf::from(dir));
    }
    let home = getenv("HOME")?;
    Some(PathBuf::from(home).join(".config"))
}

/// `g_file_get_contents`, loosely: read the file if it exists and is
/// readable. Non-UTF-8 bytes are replaced rather than treated as a missing
/// file, since the C parser is byte-oriented.
fn read_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pinwin-fontconfig-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ghostty").join("themes")).expect("temp dir creation");
        dir
    }

    #[test]
    fn load_resolves_config_and_theme_files() {
        let dir = temp_config_dir("resolve");
        fs::write(
            dir.join("ghostty").join("config"),
            "font-family = \"Test Mono\"\nfont-size = 9\ntheme = mytheme\n",
        )
        .expect("config write");
        fs::write(
            dir.join("ghostty").join("themes").join("mytheme"),
            "background = #102030\nforeground = #405060\n",
        )
        .expect("theme write");

        let font = load_font_config_at(&dir);
        assert_eq!(font.family.as_deref(), Some("Test Mono"));
        assert_eq!(font.size, 9.0);

        let colours = load_theme_colours_at(&dir);
        assert_eq!(colours.background, [0x10, 0x20, 0x30]);
        assert_eq!(colours.foreground, [0x40, 0x50, 0x60]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_without_config_uses_defaults() {
        let dir = temp_config_dir("missing");
        let font = load_font_config_at(&dir);
        assert_eq!(font, FontConfig::default());
        assert_eq!(font.effective_family(), "monospace");
        assert_eq!(font.size, 11.0);
        assert_eq!(load_theme_colours_at(&dir), ThemeColours::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_theme_file_keeps_defaults() {
        let dir = temp_config_dir("no-theme-file");
        fs::write(
            dir.join("ghostty").join("config"),
            "theme = does-not-exist-anywhere\n",
        )
        .expect("config write");
        // The user theme and /usr/share both miss; defaults survive.
        assert_eq!(load_theme_colours_at(&dir), ThemeColours::default());
        let _ = fs::remove_dir_all(&dir);
    }

    fn env_with<'a>(entries: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        |key: &str| {
            entries
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(*value))
        }
    }

    #[test]
    fn absolute_xdg_config_home_wins() {
        let getenv = env_with(&[("XDG_CONFIG_HOME", "/opt/ghostty-cfg"), ("HOME", "/home/u")]);
        assert_eq!(
            user_config_dir_from(&getenv),
            Some(PathBuf::from("/opt/ghostty-cfg"))
        );
    }

    #[test]
    fn empty_xdg_config_home_falls_back_to_home() {
        let getenv = env_with(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/u")]);
        assert_eq!(
            user_config_dir_from(&getenv),
            Some(PathBuf::from("/home/u/.config"))
        );
    }

    #[test]
    fn relative_xdg_config_home_falls_back_to_home() {
        let getenv = env_with(&[("XDG_CONFIG_HOME", "rel/cfg"), ("HOME", "/home/u")]);
        assert_eq!(
            user_config_dir_from(&getenv),
            Some(PathBuf::from("/home/u/.config"))
        );
    }

    #[test]
    fn unset_xdg_config_home_uses_home() {
        let getenv = env_with(&[("HOME", "/home/u")]);
        assert_eq!(
            user_config_dir_from(&getenv),
            Some(PathBuf::from("/home/u/.config"))
        );
    }

    #[test]
    fn unusable_environment_leaves_no_directory() {
        let getenv = env_with(&[]);
        assert_eq!(user_config_dir_from(&getenv), None);
    }
}
