//! The host settings parsed from the environment (`host/main.c`'s
//! environment contract, port-to-rust D8): the numbers, the keyboard mode,
//! the accent, and — new with `keyboard-focus-request` — `PINWIN_NAME`, the
//! instance name the focus socket is published under.
//!
//! Every value lands in the library's argument types (`port-to-rust` D6: the
//! invalid classes are unrepresentable, so the only rejections left are the
//! env strings themselves), and every rejection here is an exit-2 error the
//! caller prints before any surface opens. `get` accessors instead of the
//! process environment keep the parsing testable (port-to-rust D10).

use std::num::NonZeroU16;

use pinwin::layout::{Accent, Keyboard};

use crate::ipc::InstanceName;

/// The default column count and gutter (`host/main.c`'s fallbacks).
const DEFAULT_COLS: i64 = 40;
const DEFAULT_GUTTER: i64 = 0;

/// The default accent colour, matching the niri focus ring so the panel's
/// highlight reads like the tiling one (`host/main.c`'s `accent_color`).
const DEFAULT_ACCENT_RGB: [u8; 3] = [0xda, 0xbc, 0x7f];

/// The host settings in the library's argument types. The fields are
/// crate-visible because the validity invariant lives in their types (D6),
/// not in a runtime check; nothing outside the binary reads them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) cols: NonZeroU16,
    /// The right gutter; the C host docks left with `left = 0`.
    pub(crate) right: i32,
    pub(crate) keyboard: Keyboard,
    pub(crate) accent: Option<Accent>,
    /// The instance name the focus socket is published under.
    pub(crate) name: InstanceName,
}

/// One environment variable as a whole number in `min..=max`; unset or empty
/// is the fallback. `get` is the accessor so tests can run without touching
/// the process environment.
fn env_number(
    name: &str,
    raw: Option<&str>,
    fallback: i64,
    min: i64,
    max: i64,
) -> Result<i64, String> {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return Ok(fallback);
    };
    // `strtol`-shaped: an optional sign, then digits, then end of string. A
    // value that does not fit in `i64` cannot be in range either.
    let parsed = raw.parse::<i64>();
    match parsed {
        Ok(value) if (min..=max).contains(&value) => Ok(value),
        _ => Err(format!(
            "pinwin: {name}: expected a number from {min} to {max}, got '{raw}'"
        )),
    }
}

/// `PINWIN_KEYBOARD`: `on-demand` (the default), `exclusive` or `none`.
fn keyboard_mode(raw: Option<&str>) -> Result<Keyboard, String> {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return Ok(Keyboard::OnDemand);
    };
    Keyboard::parse(raw).ok_or_else(|| {
        format!("pinwin: PINWIN_KEYBOARD: expected on-demand, exclusive or none, got '{raw}'")
    })
}

/// `PINWIN_ACCENT`: `on` (the default) or `off`.
fn accent_enabled(raw: Option<&str>) -> Result<bool, String> {
    match raw.filter(|raw| !raw.is_empty()) {
        None | Some("on") => Ok(true),
        Some("off") => Ok(false),
        Some(raw) => Err(format!(
            "pinwin: PINWIN_ACCENT: expected on or off, got '{raw}'"
        )),
    }
}

/// `PINWIN_ACCENT_COLOR`: `#RRGGBB` or `RRGGBB`, six hex digits; unset or
/// empty is the default colour.
fn accent_color(raw: Option<&str>) -> Result<[u8; 3], String> {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return Ok(DEFAULT_ACCENT_RGB);
    };
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    let bad = || format!("pinwin: PINWIN_ACCENT_COLOR: expected #RRGGBB, got '{raw}'");
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let value = u32::from_str_radix(hex, 16).map_err(|_| bad())?;
    Ok([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

/// `PINWIN_NAME`: the instance name the focus socket is published under.
/// Unset or empty is the default name, like every other variable here; a
/// non-empty value must be a valid [`InstanceName`], or the host exits 2
/// before any surface opens.
fn instance_name(raw: Option<&str>) -> Result<InstanceName, String> {
    match raw.filter(|raw| !raw.is_empty()) {
        None => Ok(InstanceName::default_instance()),
        Some(raw) => InstanceName::parse("PINWIN_NAME", raw),
    }
}

/// Read the whole environment contract into [`Settings`]. `get` returns the
/// raw value of a variable (`None` when unset); the errors carry the same
/// messages the C host printed, and the caller exits 2 with them before any
/// surface opens.
pub(crate) fn read_settings(get: impl Fn(&str) -> Option<String>) -> Result<Settings, String> {
    let cols = env_number("COLS", get("COLS").as_deref(), DEFAULT_COLS, 1, 65535)?;
    let right = env_number("GUTTER", get("GUTTER").as_deref(), DEFAULT_GUTTER, 0, 65535)?;
    let keyboard = keyboard_mode(get("PINWIN_KEYBOARD").as_deref())?;
    // `PINWIN_ACCENT_WIDTH` is read only when the accent is on.
    let accent = if accent_enabled(get("PINWIN_ACCENT").as_deref())? {
        let rgb = accent_color(get("PINWIN_ACCENT_COLOR").as_deref())?;
        let width = env_number(
            "PINWIN_ACCENT_WIDTH",
            get("PINWIN_ACCENT_WIDTH").as_deref(),
            1,
            1,
            65535,
        )?;
        Some(Accent::new(
            rgb,
            NonZeroU16::new(width as u16).expect("width in 1..=65535"),
        ))
    } else {
        None
    };
    Ok(Settings {
        cols: NonZeroU16::new(cols as u16).expect("cols in 1..=65535"),
        right: right as i32,
        keyboard,
        accent,
        name: instance_name(get("PINWIN_NAME").as_deref())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A plain helper, so the map borrow stays a plain borrow.
    fn call(map: &HashMap<String, String>) -> Result<Settings, String> {
        read_settings(|name| map.get(name).cloned())
    }

    /// `env_number` around its fallback, range and error message.
    #[test]
    fn env_number_parses_and_rejects() {
        assert_eq!(env_number("COLS", None, 40, 1, 65535), Ok(40));
        assert_eq!(env_number("COLS", Some(""), 40, 1, 65535), Ok(40));
        assert_eq!(env_number("COLS", Some("60"), 40, 1, 65535), Ok(60));
        assert_eq!(env_number("COLS", Some("+2"), 40, 1, 65535), Ok(2));
        assert_eq!(env_number("COLS", Some("1"), 40, 1, 65535), Ok(1));
        assert_eq!(env_number("COLS", Some("65535"), 40, 1, 65535), Ok(65535));

        for raw in [
            "0",
            "65536",
            "-1",
            "4o",
            "12x",
            " 4",
            "4 ",
            "0x10",
            "99999999999999999999",
        ] {
            let error = env_number("COLS", Some(raw), 40, 1, 65535).expect_err(raw);
            assert_eq!(
                error,
                format!("pinwin: COLS: expected a number from 1 to 65535, got '{raw}'"),
                "raw = {raw:?}"
            );
        }
    }

    /// `keyboard_mode` accepts the three modes with `on-demand` defaulting.
    #[test]
    fn keyboard_mode_parses_the_three_modes() {
        assert_eq!(keyboard_mode(None), Ok(Keyboard::OnDemand));
        assert_eq!(keyboard_mode(Some("")), Ok(Keyboard::OnDemand));
        assert_eq!(keyboard_mode(Some("on-demand")), Ok(Keyboard::OnDemand));
        assert_eq!(keyboard_mode(Some("exclusive")), Ok(Keyboard::Exclusive));
        assert_eq!(keyboard_mode(Some("none")), Ok(Keyboard::None));
        let error = keyboard_mode(Some("sometimes")).expect_err("invalid");
        assert_eq!(
            error,
            "pinwin: PINWIN_KEYBOARD: expected on-demand, exclusive or none, got 'sometimes'"
        );
    }

    /// `accent_enabled` accepts `on` (default) and `off`.
    #[test]
    fn accent_enabled_parses_on_and_off() {
        assert!(accent_enabled(None).expect("unset"));
        assert!(accent_enabled(Some("")).expect("empty"));
        assert!(accent_enabled(Some("on")).expect("on"));
        assert!(!accent_enabled(Some("off")).expect("off"));
        let error = accent_enabled(Some("1")).expect_err("invalid");
        assert_eq!(error, "pinwin: PINWIN_ACCENT: expected on or off, got '1'");
    }

    /// `accent_color` accepts `#RRGGBB` and `RRGGBB` and defaults.
    #[test]
    fn accent_color_parses_hex_triplets() {
        assert_eq!(accent_color(None), Ok(DEFAULT_ACCENT_RGB));
        assert_eq!(accent_color(Some("")), Ok(DEFAULT_ACCENT_RGB));
        assert_eq!(accent_color(Some("#dabc7f")), Ok([0xda, 0xbc, 0x7f]));
        assert_eq!(accent_color(Some("dabc7f")), Ok([0xda, 0xbc, 0x7f]));
        assert_eq!(accent_color(Some("#DABC7F")), Ok([0xda, 0xbc, 0x7f]));
        assert_eq!(accent_color(Some("#000000")), Ok([0, 0, 0]));

        for raw in ["#dabc7", "dabc7f0", "#zdbc7f", "#dabc7f ", "##dabc7", "-1"] {
            let error = accent_color(Some(raw)).expect_err(raw);
            assert_eq!(
                error,
                format!("pinwin: PINWIN_ACCENT_COLOR: expected #RRGGBB, got '{raw}'"),
                "raw = {raw:?}"
            );
        }
    }

    /// `read_settings` composes the variables into the library's types and
    /// reads `PINWIN_ACCENT_WIDTH` only when the accent is on.
    #[test]
    fn read_settings_builds_the_startup_arguments() {
        // All defaults.
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(
            call(&empty),
            Ok(Settings {
                cols: NonZeroU16::new(40).expect("40"),
                right: 0,
                keyboard: Keyboard::OnDemand,
                accent: Some(Accent::new(
                    DEFAULT_ACCENT_RGB,
                    NonZeroU16::new(1).expect("1")
                )),
                name: InstanceName::default_instance(),
            })
        );

        // Everything set; an invalid `PINWIN_ACCENT_WIDTH` is still read
        // because the accent is on.
        let map = HashMap::from([
            ("COLS".to_owned(), "120".to_owned()),
            ("GUTTER".to_owned(), "8".to_owned()),
            ("PINWIN_KEYBOARD".to_owned(), "exclusive".to_owned()),
            ("PINWIN_ACCENT_COLOR".to_owned(), "0a0b0c".to_owned()),
            ("PINWIN_ACCENT_WIDTH".to_owned(), "3".to_owned()),
        ]);
        assert_eq!(
            call(&map),
            Ok(Settings {
                cols: NonZeroU16::new(120).expect("120"),
                right: 8,
                keyboard: Keyboard::Exclusive,
                accent: Some(Accent::new(
                    [0x0a, 0x0b, 0x0c],
                    NonZeroU16::new(3).expect("3")
                )),
                name: InstanceName::default_instance(),
            })
        );

        // The accent off: no colour or width is read, even invalid ones.
        let map = HashMap::from([
            ("PINWIN_ACCENT".to_owned(), "off".to_owned()),
            ("PINWIN_ACCENT_COLOR".to_owned(), "nope".to_owned()),
            ("PINWIN_ACCENT_WIDTH".to_owned(), "0".to_owned()),
        ]);
        assert_eq!(
            call(&map),
            Ok(Settings {
                cols: NonZeroU16::new(40).expect("40"),
                right: 0,
                keyboard: Keyboard::OnDemand,
                accent: None,
                name: InstanceName::default_instance(),
            })
        );

        // A keyboard mode that the type cannot express is still an env error.
        let map = HashMap::from([("PINWIN_KEYBOARD".to_owned(), "sometimes".to_owned())]);
        assert!(call(&map).is_err());
    }

    /// `PINWIN_NAME` parses into the validated instance name: unset or empty
    /// is the default, a valid name passes, and a bad name is an exit-2
    /// error naming the variable.
    #[test]
    fn read_settings_parses_the_instance_name() {
        // Unset and empty are the default instance.
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(
            call(&empty).expect("defaults").name,
            InstanceName::default_instance()
        );
        let map = HashMap::from([("PINWIN_NAME".to_owned(), String::new())]);
        assert_eq!(
            call(&map).expect("empty is the default").name,
            InstanceName::default_instance()
        );

        // A valid name, and the 64-character boundary.
        let map = HashMap::from([("PINWIN_NAME".to_owned(), "notes".to_owned())]);
        assert_eq!(
            call(&map).expect("valid").name,
            InstanceName::parse("PINWIN_NAME", "notes").expect("valid")
        );
        let long = "a".repeat(64);
        let map = HashMap::from([("PINWIN_NAME".to_owned(), long.clone())]);
        assert_eq!(
            call(&map).expect("64 characters").name,
            InstanceName::parse("PINWIN_NAME", &long).expect("valid")
        );

        // A bad name is an exit-2 error before any surface opens.
        for raw in ["a/b", &"a".repeat(65), "a b"] {
            let map = HashMap::from([("PINWIN_NAME".to_owned(), raw.to_owned())]);
            let error = call(&map).expect_err(raw);
            assert_eq!(
                error,
                format!(
                    "pinwin: PINWIN_NAME: expected a name of 1..=64 characters from \
                     [A-Za-z0-9_-], got '{raw}'"
                ),
                "raw = {raw:?}"
            );
        }
    }
}
