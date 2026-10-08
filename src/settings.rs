//! The host settings parsed from the environment (`host/main.c`'s
//! environment contract, port-to-rust D8): the numbers, the keyboard mode,
//! the accent, the starting zone, and — new with `keyboard-focus-request` —
//! `PINWIN_NAME`, the instance name the instance socket is published under.
//!
//! Every value lands in the library's argument types (`port-to-rust` D6: the
//! invalid classes are unrepresentable, so the only rejections left are the
//! env strings themselves), and every rejection here is an exit-2 error the
//! caller prints before any surface opens. `get` accessors instead of the
//! process environment keep the parsing testable (port-to-rust D10).

use std::num::NonZeroU16;

use pinwin::layout::{Accent, Keyboard};

use pinwin::instance::InstanceName;

/// Which layout class the panel starts in (`PINWIN_ZONE`,
/// `replace-gtk-with-wayland` D4): `reserve` starts with a pushing layout,
/// so tiled windows sit beside the panel and a toggle moves them; `overlay`
/// starts with a covering layout, so nothing is ever reserved and a toggle
/// moves no window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Zone {
    Reserve,
    Overlay,
}

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
    /// The instance name the instance socket is published under.
    pub(crate) name: InstanceName,
    /// The layout class the panel starts in.
    pub(crate) zone: Zone,
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
    let Ok(value) = u32::from_str_radix(hex, 16) else {
        return Err(bad());
    };
    // Six hex digits fit in the low 24 bits: the big-endian bytes' first is
    // zero and the rest are the three channels.
    let [_, red, green, blue] = value.to_be_bytes();
    Ok([red, green, blue])
}

/// `PINWIN_ZONE`: `reserve` (the default) starts with a pushing layout and
/// `overlay` with a covering one; anything else is an exit-2 error.
fn zone(raw: Option<&str>) -> Result<Zone, String> {
    match raw.filter(|raw| !raw.is_empty()) {
        None | Some("reserve") => Ok(Zone::Reserve),
        Some("overlay") => Ok(Zone::Overlay),
        Some(raw) => Err(format!(
            "pinwin: PINWIN_ZONE: expected reserve or overlay, got '{raw}'"
        )),
    }
}

/// `PINWIN_NAME`: the instance name the instance socket is published under.
/// Unset is the default name; a set value — the empty one included — must be
/// a valid [`InstanceName`], or the host exits 2 before any surface opens.
fn instance_name(raw: Option<&str>) -> Result<InstanceName, String> {
    match raw {
        None => Ok(InstanceName::default_instance()),
        Some(raw) => {
            InstanceName::parse(raw).map_err(|error| format!("pinwin: PINWIN_NAME: {error}"))
        }
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
            NonZeroU16::new(u16::try_from(width).expect("width in 1..=65535"))
                .expect("width in 1..=65535"),
        ))
    } else {
        None
    };
    Ok(Settings {
        cols: NonZeroU16::new(u16::try_from(cols).expect("cols in 1..=65535"))
            .expect("cols in 1..=65535"),
        right: i32::try_from(right).expect("gutter in 0..=65535"),
        keyboard,
        accent,
        name: instance_name(get("PINWIN_NAME").as_deref())?,
        zone: zone(get("PINWIN_ZONE").as_deref())?,
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
                zone: Zone::Reserve,
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
                zone: Zone::Reserve,
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
                zone: Zone::Reserve,
            })
        );

        // A keyboard mode that the type cannot express is still an env error.
        let map = HashMap::from([("PINWIN_KEYBOARD".to_owned(), "sometimes".to_owned())]);
        call(&map).unwrap_err();
    }

    /// `zone` accepts `reserve` (default) and `overlay`, and rejects
    /// everything else — `both`, the spec's example invalid value.
    #[test]
    fn zone_parses_reserve_and_overlay() {
        assert_eq!(zone(None), Ok(Zone::Reserve));
        assert_eq!(zone(Some("")), Ok(Zone::Reserve));
        assert_eq!(zone(Some("reserve")), Ok(Zone::Reserve));
        assert_eq!(zone(Some("overlay")), Ok(Zone::Overlay));

        for raw in ["both", "push", "cover", "OVERLAY"] {
            let error = zone(Some(raw)).expect_err(raw);
            assert_eq!(
                error,
                format!("pinwin: PINWIN_ZONE: expected reserve or overlay, got '{raw}'"),
                "raw = {raw:?}"
            );
        }
    }

    /// `PINWIN_ZONE` flows through `read_settings` into the startup
    /// arguments: unset is the pushing default and `overlay` is carried as
    /// the covering start.
    #[test]
    fn read_settings_parses_the_zone() {
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(call(&empty).expect("defaults").zone, Zone::Reserve);

        let map = HashMap::from([("PINWIN_ZONE".to_owned(), "overlay".to_owned())]);
        assert_eq!(call(&map).expect("overlay").zone, Zone::Overlay);

        let map = HashMap::from([("PINWIN_ZONE".to_owned(), "reserve".to_owned())]);
        assert_eq!(call(&map).expect("reserve").zone, Zone::Reserve);

        let map = HashMap::from([("PINWIN_ZONE".to_owned(), "both".to_owned())]);
        let error = call(&map).expect_err("both");
        assert_eq!(
            error,
            "pinwin: PINWIN_ZONE: expected reserve or overlay, got 'both'"
        );
    }

    /// `PINWIN_NAME` parses into the validated instance name: unset is the
    /// default, a valid name passes, and a bad name — the empty one
    /// included — is an exit-2 error naming the variable.
    #[test]
    fn read_settings_parses_the_instance_name() {
        // Unset is the default instance.
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(
            call(&empty).expect("defaults").name,
            InstanceName::default_instance()
        );

        // A valid name, and the 64-character boundary.
        let map = HashMap::from([("PINWIN_NAME".to_owned(), "notes".to_owned())]);
        assert_eq!(
            call(&map).expect("valid").name,
            InstanceName::parse("notes").expect("valid")
        );
        let long = "a".repeat(64);
        let map = HashMap::from([("PINWIN_NAME".to_owned(), long.clone())]);
        assert_eq!(
            call(&map).expect("64 characters").name,
            InstanceName::parse(&long).expect("valid")
        );

        // A bad name is an exit-2 error before any surface opens; an empty
        // value is bad too, not the default. The exact wording is
        // `InstanceName::parse`'s contract, asserted in `instance.rs`; here only
        // the error class holds: the rejection names the variable.
        for raw in ["", "a/b", &"a".repeat(65), "a b"] {
            let map = HashMap::from([("PINWIN_NAME".to_owned(), raw.to_owned())]);
            let error = call(&map).expect_err(raw);
            assert!(
                error.contains("pinwin: PINWIN_NAME:"),
                "raw = {raw:?}, error = {error}"
            );
        }
    }
}
