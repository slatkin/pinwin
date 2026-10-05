//! pinwin's Ghostty-config reader: the terminal font (family and size) and the
//! default background/foreground colours, including `theme` file resolution.
//!
//! This is the Rust port of `fontconfig.c` (port-to-rust D3). The parsing
//! functions are pure: they take the file contents as a `&str` and return
//! plain data, so they are testable without a display or a config file. The
//! thin wrappers at the bottom resolve the XDG config paths and read the
//! files.
//!
//! The C parser is deliberately quirky in places (it was written against
//! Ghostty's real config syntax, and `sscanf` does the tokenizing); the port
//! reproduces those quirks rather than "fixing" them, and the tests pin them.

use std::fs;
use std::path::{Path, PathBuf};

/// The fallback font family when the Ghostty config names none.
pub const DEFAULT_FONT_FAMILY: &str = "monospace";
/// The fallback font size (points * Pango scale, but the config value is in
/// points; see `render`).
pub const DEFAULT_FONT_SIZE: f64 = 11.0;

/// The font selected by the Ghostty config: the first non-empty
/// `font-family` and the last positive `font-size`.
#[derive(Debug, Clone, PartialEq)]
pub struct FontConfig {
    /// `None` when the config names no family; the caller falls back to
    /// [`DEFAULT_FONT_FAMILY`].
    pub family: Option<String>,
    /// Always non-zero; defaults to [`DEFAULT_FONT_SIZE`].
    pub size: f64,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: None,
            size: DEFAULT_FONT_SIZE,
        }
    }
}

impl FontConfig {
    /// The family to draw with: the configured one, else `monospace`.
    #[must_use]
    pub fn effective_family(&self) -> &str {
        self.family.as_deref().unwrap_or(DEFAULT_FONT_FAMILY)
    }
}

/// The terminal's default background and foreground, from the Ghostty config
/// and its `theme` file. The initial values match the C globals in `glue.c`:
/// black background, white foreground.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColours {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
}

impl Default for ThemeColours {
    fn default() -> Self {
        Self {
            background: [0, 0, 0],
            foreground: [255, 255, 255],
        }
    }
}

/// Parse a Ghostty config's font settings. This is `font_config_load`'s loop;
/// the initial values are [`FontConfig::default`].
#[allow(clippy::must_use_candidate, reason = "approved #13: pure loader")]
pub fn parse_font_config(contents: &str) -> FontConfig {
    let mut config = FontConfig::default();

    for line in contents.split('\n') {
        // Ghostty allows whole-line and trailing comments; the rest of the
        // line after the first '#' is dropped.
        let line = match line.find('#') {
            Some(hash) => &line[..hash],
            None => line,
        };
        let Some(eq) = line.find('=') else {
            continue;
        };
        let key = ascii_trim(&line[..eq]);
        let value = ascii_trim(&line[eq + 1..]);
        // A quoted value's closing quote ends it; the opening quote is
        // dropped. An unclosed quote just loses the quote.
        let value = unquote(value);

        if key == "font-family" {
            if !value.is_empty() && config.family.is_none() {
                config.family = Some(value.to_string());
            }
        } else if key == "font-size" {
            let parsed = ascii_strtod(value);
            if parsed > 0.0 {
                config.size = parsed;
            }
        }
    }

    config
}

/// Parse the main Ghostty config's `theme` key into `colours` (this is the
/// first pass of `theme_colours`) and return the theme name when one was set.
///
/// The returned name is the last `theme` value seen, including an empty one;
/// `None` means no `theme` key was present. Any inline `background` /
/// `foreground` line is parsed too, but since the C outer `sscanf` excludes
/// '#', such lines never yield a colour there; the behaviour is kept.
#[must_use]
pub fn parse_theme_config(contents: &str, colours: &mut ThemeColours) -> Option<String> {
    let mut theme: Option<String> = None;

    for line in contents.split('\n') {
        let Some((key, value)) = scan_config_line(line) else {
            continue;
        };
        let value = unquote(ascii_trim(value));

        if key == "theme" {
            theme = Some(truncate_theme_name(value));
        } else if key == "background" || key == "foreground" {
            apply_theme_colour_line(line, colours);
        }
    }

    theme
}

/// Apply every `background` / `foreground` line of a Ghostty theme file to
/// `colours`. This is `theme_colours`' second pass.
pub fn parse_theme_file(contents: &str, colours: &mut ThemeColours) {
    for line in contents.split('\n') {
        apply_theme_colour_line(line, colours);
    }
}

/// Load the font from `$XDG_CONFIG_HOME/ghostty/config` (default
/// `~/.config/ghostty/config`); a missing or unreadable file falls back to
/// [`FontConfig::default`] (``monospace 11``).
#[must_use]
pub fn load_font_config() -> FontConfig {
    load_font_config_at(&user_config_dir())
}

/// Load the theme colours from the Ghostty config and, when it names a
/// `theme`, the matching theme file (user themes first, then
/// `/usr/share/ghostty/themes`); a missing config leaves the defaults.
#[allow(clippy::must_use_candidate, reason = "approved #13: pure loader")]
pub fn load_theme_colours() -> ThemeColours {
    load_theme_colours_at(&user_config_dir())
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

/// `g_get_user_config_dir`: `$XDG_CONFIG_HOME` when set and non-empty (`GLib`
/// uses it verbatim, even when relative), else `$HOME/.config`, falling back
/// to the passwd entry when `$HOME` is unset. Delegating keeps the fallback
/// identical to the C code; the parsing functions stay pure.
fn user_config_dir() -> PathBuf {
    gtk4::glib::user_config_dir()
}

/// `g_file_get_contents`, loosely: read the file if it exists and is
/// readable. Non-UTF-8 bytes are replaced rather than treated as a missing
/// file, since the C parser is byte-oriented.
fn read_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `theme_colour_parse`: read a `background` / `foreground` line of a theme
/// file and, when its value is a hex colour, update `colours`.
fn apply_theme_colour_line(line: &str, colours: &mut ThemeColours) {
    let Some((key, value)) = scan_theme_colour_line(line) else {
        return;
    };
    if !value.starts_with('#') {
        return;
    }
    let hex = &value.as_bytes()[1..];
    let mut cursor = 0;
    let (Some(r), Some(g), Some(b)) = (
        scan_hex_pair(hex, &mut cursor),
        scan_hex_pair(hex, &mut cursor),
        scan_hex_pair(hex, &mut cursor),
    ) else {
        return;
    };

    // Hex pairs are range-checked to 0-255 at parse, so no truncation.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "approved #13: hex pair range-checked to 0-255"
    )]
    let rgb = [r as u8, g as u8, b as u8];
    if key == "background" {
        colours.background = rgb;
    } else if key == "foreground" {
        colours.foreground = rgb;
    }
}

/// The C `sscanf(line, " %31[a-z-] = ")` prefix: the key (lowercase letters
/// and hyphens, at most 31), an `=`, and the byte offset where the value
/// starts (leading whitespace skipped). `None` when the prefix does not
/// match, including a key longer than 31 characters.
fn scan_key_prefix(line: &str) -> Option<(&str, usize)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    let key_start = i;
    while i < bytes.len()
        && (bytes[i].is_ascii_lowercase() || bytes[i] == b'-')
        && i - key_start < 31
    {
        i += 1;
    }
    if i == key_start {
        return None;
    }
    // A 32nd key character means the key ran past the `%31` width, so the
    // following literal '=' cannot match.
    if i - key_start == 31 && i < bytes.len() && (bytes[i].is_ascii_lowercase() || bytes[i] == b'-')
    {
        return None;
    }
    let key_end = i;

    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'=' {
        return None;
    }
    i += 1;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    Some((&line[key_start..key_end], i))
}

/// The C `sscanf(line, " %31[a-z-] = %255[^#]", ...)`: a key and a value
/// running up to the first '#' (or 255 characters). Fails when the value is
/// empty, which is how a `background = #...` line gets skipped.
fn scan_config_line(line: &str) -> Option<(&str, &str)> {
    let (key, mut i) = scan_key_prefix(line)?;
    let bytes = line.as_bytes();
    let value_start = i;
    while i < bytes.len() && bytes[i] != b'#' && i - value_start < 255 {
        i += 1;
    }
    if i == value_start {
        return None;
    }
    let value_end = floor_char_boundary(line, i);
    Some((key, &line[value_start..value_end]))
}

/// The C `sscanf(line, " %31[a-z-] = %31s", ...)`: a key and a
/// whitespace-delimited value (at most 31 characters).
fn scan_theme_colour_line(line: &str) -> Option<(&str, &str)> {
    let (key, mut i) = scan_key_prefix(line)?;
    let bytes = line.as_bytes();
    let value_start = i;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && i - value_start < 31 {
        i += 1;
    }
    if i == value_start {
        return None;
    }
    let value_end = floor_char_boundary(line, i);
    Some((key, &line[value_start..value_end]))
}

/// One `%2x` conversion: an optional sign and one or two hex digits. Like the
/// C `%x` assigned to an `unsigned`, a negative value wraps to 32 bits, then
/// truncates to a colour byte.
fn scan_hex_pair(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let start = *cursor;
    let mut negative = false;
    if *cursor < bytes.len() && (bytes[*cursor] == b'+' || bytes[*cursor] == b'-') {
        negative = bytes[*cursor] == b'-';
        *cursor += 1;
    }

    let digits_start = *cursor;
    let mut value: u32 = 0;
    while *cursor < bytes.len() && *cursor - start < 2 && bytes[*cursor].is_ascii_hexdigit() {
        value = value * 16 + hex_value(bytes[*cursor]);
        *cursor += 1;
    }
    if *cursor == digits_start {
        return None;
    }

    Some(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}

fn hex_value(byte: u8) -> u32 {
    match byte {
        b'0'..=b'9' => u32::from(byte - b'0'),
        b'a'..=b'f' => u32::from(byte - b'a' + 10),
        b'A'..=b'F' => u32::from(byte - b'A' + 10),
        _ => 0,
    }
}

/// Drop a leading and trailing double quote, as the C config parsers do. The
/// closing quote is the last one in the remainder; an unclosed quote just
/// drops the opener.
fn unquote(value: &str) -> &str {
    if let Some(rest) = value.strip_prefix('"') {
        match rest.rfind('"') {
            Some(end) => &rest[..end],
            None => rest,
        }
    } else {
        value
    }
}

/// Clamp `end` down to the nearest `char` boundary of `value`: the byte caps
/// above reproduce C's `%255[^#]` / `%31s` widths, but slicing a `str` there
/// must not cut a multi-byte character in half.
fn floor_char_boundary(value: &str, mut end: usize) -> usize {
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// `snprintf(theme_name, 128, "%s", value)`: at most 127 bytes, cut on a
/// character boundary so the result stays a valid `String`.
fn truncate_theme_name(value: &str) -> String {
    if value.len() <= 127 {
        return value.to_string();
    }
    value[..floor_char_boundary(value, 127)].to_string()
}

/// `g_ascii_strstrip`: trim ASCII whitespace only (the C code never trims
/// Unicode whitespace).
fn ascii_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_ascii_whitespace())
}

/// `g_ascii_strtod`: parse a leading number, locale-independently, ignoring
/// any trailing text. Returns 0.0 when there is no number.
fn ascii_strtod(value: &str) -> f64 {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    let mut sign = 1.0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        if bytes[i] == b'-' {
            sign = -1.0;
        }
        i += 1;
    }

    let rest = &value[i..];
    let lower = rest.to_ascii_lowercase();
    if lower.starts_with("infinity") || lower.starts_with("inf") {
        return sign * f64::INFINITY;
    }
    if lower.starts_with("nan") {
        return f64::NAN;
    }
    if lower.starts_with("0x") {
        if let Some(parsed) = parse_hex_float(&rest[2..]) {
            return sign * parsed;
        }
        // `strtod("0x")` parses the leading `0` and stops at `x`.
        return sign * 0.0;
    }

    let bytes = rest.as_bytes();
    let mut i = 0;
    let mut int_digits = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        int_digits += 1;
    }
    let mut frac_digits = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            frac_digits += 1;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return sign * 0.0;
    }

    let mut end = i;
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        // An `e` with no exponent digits is not part of the number.
        if j > exp_start {
            end = j;
        }
    }

    sign * rest[..end].parse::<f64>().unwrap_or(0.0)
}

/// A C99 hex float without the `0x`: hex digits, an optional fraction, and an
/// optional `p` binary exponent. `None` when there are no hex digits.
fn parse_hex_float(rest: &str) -> Option<f64> {
    let bytes = rest.as_bytes();
    let mut i = 0;
    let mut mantissa = 0.0;
    let mut any_digits = false;

    while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
        mantissa = mantissa * 16.0 + f64::from(hex_value(bytes[i]));
        i += 1;
        any_digits = true;
    }
    let mut exponent: i32 = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
            mantissa = mantissa * 16.0 + f64::from(hex_value(bytes[i]));
            exponent -= 4;
            i += 1;
            any_digits = true;
        }
    }
    if !any_digits {
        return None;
    }

    if i < bytes.len() && (bytes[i] == b'p' || bytes[i] == b'P') {
        let mut j = i + 1;
        let mut exp_sign = 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            if bytes[j] == b'-' {
                exp_sign = -1;
            }
            j += 1;
        }
        let digits_start = j;
        let mut p = 0i32;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            p = p
                .saturating_mul(10)
                .saturating_add(i32::from(bytes[j] - b'0'));
            j += 1;
        }
        if j > digits_start {
            exponent = exponent.saturating_add(exp_sign * p);
        }
    }

    Some(mantissa * 2f64.powi(exponent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn temp_config_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("pinwin-fontconfig-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ghostty").join("themes")).unwrap();
        dir
    }

    #[test]
    fn configured_font_family_and_size() {
        let config = parse_font_config(
            "# a comment\nfont-family = \"JetBrains Mono Nerd Font\"\nfont-size = 12.5\n",
        );
        assert_eq!(config.family.as_deref(), Some("JetBrains Mono Nerd Font"));
        assert_eq!(config.size, 12.5);
    }

    #[test]
    fn first_family_wins_and_empty_values_are_skipped() {
        let config = parse_font_config(
            "font-family =\nfont-family = \"\"\nfont-family = Foo Bar\nfont-family = Later\n",
        );
        assert_eq!(config.family.as_deref(), Some("Foo Bar"));
    }

    #[test]
    fn last_positive_font_size_wins_and_non_positive_is_ignored() {
        let config =
            parse_font_config("font-size = 12\nfont-size = 0\nfont-size = -3\nfont-size = 14\n");
        assert_eq!(config.size, 14.0);

        let ignored = parse_font_config("font-size = 0\nfont-size = -3\n");
        assert_eq!(ignored.size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn comments_quotes_and_whitespace() {
        // A '#' starts a comment anywhere, including inside a quoted value.
        let config = parse_font_config("font-family = \"Foo#Bar\"\nfont-size = 12 # trailing\n");
        assert_eq!(config.family.as_deref(), Some("Foo"));
        assert_eq!(config.size, 12.0);

        let spaced = parse_font_config("  font-family\t=\tFoo  \r\n");
        assert_eq!(spaced.family.as_deref(), Some("Foo"));

        let equals_in_value = parse_font_config("font-family=Foo=Bar\n");
        assert_eq!(equals_in_value.family.as_deref(), Some("Foo=Bar"));
    }

    #[test]
    fn missing_config_falls_back_to_monospace_11() {
        let config = parse_font_config("");
        assert_eq!(config, FontConfig::default());
        assert_eq!(config.effective_family(), "monospace");
        assert_eq!(config.size, 11.0);
    }

    #[test]
    fn font_size_uses_ascii_strtod_semantics() {
        let cases = [
            ("12.5", 12.5),
            ("1e2", 100.0),
            ("0x10", 16.0),
            ("0x1.8p1", 3.0),
            (".5", 0.5),
            ("5.", 5.0),
            ("+3", 3.0),
            ("12,5", 12.0),
            ("1e", 1.0),
            ("inf", f64::INFINITY),
            ("Infinity", f64::INFINITY),
            ("1e999", f64::INFINITY),
        ];
        for (input, expected) in cases {
            let config = parse_font_config(&format!("font-size = {input}\n"));
            assert_eq!(config.size, expected, "font-size = {input}");
        }

        // NaN is not > 0, so the default is kept.
        let nan = parse_font_config("font-size = nan\n");
        assert_eq!(nan.size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn theme_file_sets_background_and_foreground() {
        let mut colours = ThemeColours::default();
        parse_theme_file(
            "background = #1e1e2e\nforeground = #cdd6f4\ncursor-color = #f5e0dc\n",
            &mut colours,
        );
        assert_eq!(colours.background, [0x1e, 0x1e, 0x2e]);
        assert_eq!(colours.foreground, [0xcd, 0xd6, 0xf4]);
    }

    #[test]
    fn theme_config_names_theme_and_last_one_wins() {
        let mut colours = ThemeColours::default();
        let theme = parse_theme_config(
            "theme = catppuccin\n# theme = ignored\ntheme = \"mocha\"\n",
            &mut colours,
        );
        assert_eq!(theme.as_deref(), Some("mocha"));

        let none = parse_theme_config("font-family = Foo\n", &mut colours);
        assert_eq!(none, None);

        let empty = parse_theme_config("theme = \"\"\n", &mut colours);
        assert_eq!(empty.as_deref(), Some(""));
    }

    #[test]
    fn inline_background_line_in_the_config_is_skipped() {
        // The C outer sscanf excludes '#', so an inline hex colour never
        // reaches theme_colour_parse; the theme file is what colours the
        // panel. Pin the quirk.
        let mut colours = ThemeColours::default();
        let theme = parse_theme_config(
            "theme = t\nbackground = #111111\nforeground = \"#222222\"\n",
            &mut colours,
        );
        assert_eq!(theme.as_deref(), Some("t"));
        assert_eq!(colours, ThemeColours::default());
    }

    #[test]
    fn theme_colour_hex_quirks() {
        // `%2x%2x%2x` needs five hex digits (2+2+1) to succeed.
        let mut colours = ThemeColours::default();
        parse_theme_file("background = #12345\n", &mut colours);
        assert_eq!(colours.background, [0x12, 0x34, 0x05]);

        let mut too_short = ThemeColours::default();
        parse_theme_file("background = #1234\n", &mut colours);
        assert_eq!(colours.background, [0x12, 0x34, 0x05]);
        parse_theme_file("background = #1234\n", &mut too_short);
        assert_eq!(too_short, ThemeColours::default());

        // A 6-digit colour is read two digits at a time, and an 8-digit one
        // ignores the trailing pair.
        let mut eight = ThemeColours::default();
        parse_theme_file("foreground = #12345678\n", &mut eight);
        assert_eq!(eight.foreground, [0x12, 0x34, 0x56]);

        // Uppercase hex and signs work; `#` followed by a space fails.
        let mut upper = ThemeColours::default();
        parse_theme_file("background = #ABCDEF\n", &mut upper);
        assert_eq!(upper.background, [0xab, 0xcd, 0xef]);

        let mut signed = ThemeColours::default();
        parse_theme_file("background = #-abcde\n", &mut signed);
        assert_eq!(signed.background, [246, 188, 222]);
    }

    #[test]
    fn theme_key_syntax_and_comments() {
        let mut colours = ThemeColours::default();
        // Uppercase keys, `_`, and a leading `0x` are not accepted.
        parse_theme_file(
            "BACKGROUND = #1e1e2e\nback_ground = #1e1e2e\nbackground = #0x1234\n",
            &mut colours,
        );
        assert_eq!(colours, ThemeColours::default());

        // Trailing text after the colour token is ignored.
        parse_theme_file("background = #1e1e2e trailing\n", &mut colours);
        assert_eq!(colours.background, [0x1e, 0x1e, 0x2e]);
    }

    #[test]
    fn load_resolves_config_and_theme_files() {
        let dir = temp_config_dir("resolve");
        fs::write(
            dir.join("ghostty").join("config"),
            "font-family = \"Test Mono\"\nfont-size = 9\ntheme = mytheme\n",
        )
        .unwrap();
        fs::write(
            dir.join("ghostty").join("themes").join("mytheme"),
            "background = #102030\nforeground = #405060\n",
        )
        .unwrap();

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
        .unwrap();
        // The user theme and /usr/share both miss; defaults survive.
        assert_eq!(load_theme_colours_at(&dir), ThemeColours::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_name_is_truncated_to_127_bytes() {
        let name = "x".repeat(200);
        let mut colours = ThemeColours::default();
        let theme = parse_theme_config(&format!("theme = {name}\n"), &mut colours);
        assert_eq!(theme.as_deref(), Some("x".repeat(127).as_str()));
    }

    #[test]
    fn config_value_scan_does_not_split_a_multibyte_char_at_the_255_byte_cap() {
        // The `%255[^#]` width cuts the value at byte 255; a CJK value that
        // straddles that offset must truncate on a boundary instead of
        // panicking. Prefix lengths that are not multiples of 3 shift the
        // straddle past the cap.
        for prefix in [1usize, 2, 4, 5] {
            let value = format!("{}{}", "a".repeat(prefix), "漢".repeat(90));
            let line = format!("theme = {value}");
            let (key, scanned) = scan_config_line(&line).expect("line should scan");
            assert_eq!(key, "theme");
            assert!(scanned.len() <= 255, "prefix {prefix}: {}", scanned.len());
            assert!(scanned.is_char_boundary(scanned.len()));
            assert!(value.starts_with(scanned));
        }
    }

    #[test]
    fn cjk_font_family_and_theme_values_over_255_bytes_do_not_panic() {
        // A `font-family` line with a long CJK value is scanned (and
        // discarded) on the way to reading `theme`; it must not panic.
        let family = "漢".repeat(90);
        let mut colours = ThemeColours::default();
        let theme = parse_theme_config(&format!("font-family = {family}\n"), &mut colours);
        assert_eq!(theme, None);

        // A long CJK theme name is cut at 255 bytes, then at 127; both cuts
        // land on a char boundary.
        let name = format!("a{}", "漢".repeat(100));
        let mut colours = ThemeColours::default();
        let theme =
            parse_theme_config(&format!("theme = {name}\n"), &mut colours).expect("theme key");
        assert_eq!(theme, format!("a{}", "漢".repeat(42)));
    }

    #[test]
    fn theme_colour_scan_does_not_split_a_multibyte_char_at_the_31_byte_cap() {
        // The `%31s` width cuts the colour value at byte 31; a CJK value that
        // straddles that offset must truncate on a boundary, and the line
        // must simply not produce a colour.
        for prefix in [0usize, 1, 2, 4, 5] {
            let value = format!("{}{}", "a".repeat(prefix), "漢".repeat(20));
            let line = format!("background = {value}");
            let (key, scanned) = scan_theme_colour_line(&line).expect("line should scan");
            assert_eq!(key, "background");
            assert!(scanned.len() <= 31, "prefix {prefix}: {}", scanned.len());
            assert!(scanned.is_char_boundary(scanned.len()));
            assert!(value.starts_with(scanned));

            let mut colours = ThemeColours::default();
            parse_theme_file(&format!("background = {value}\n"), &mut colours);
            assert_eq!(colours, ThemeColours::default());
        }
    }
}
