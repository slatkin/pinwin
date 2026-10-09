//! Key mapping for the key encoder (port-to-rust D3), mirroring what Ghostty's
//! own apprt does (the apprt keymap `key.zig` and the keycode tables
//! `keycodes.zig` at
//! the pinned ghostty commit): a physical key from the keycode, the same keyval
//! remap Ghostty applies for writing-system keys, and the same modifier keyvals
//! it refuses to encode.
//!
//! Generated from that commit: `KEYCODE_TABLE` from the `xkb` column of
//! `raw_entries` (the keycode the seat reports), `KEYVAL_TABLE` from
//! the apprt keymap's `keymap`, keyed on the keyval numbers resolved
//! at generation time. Keys neither table names stay
//! [`Key::UNIDENTIFIED`] and travel as text only.

use libghostty_vt::key;

/// A physical or logical key, wrapping a libghostty `key::Key` value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key(key::Key);

impl Key {
    /// The key no table names; it travels as text only.
    pub const UNIDENTIFIED: Key = Key(key::Key::Unidentified);

    /// The raw `key::Key` value for the FFI call.
    #[must_use]
    pub(crate) const fn raw(self) -> key::Key {
        self.0
    }
}

/// XKB keycode -> physical key (the `xkb` column of Ghostty's `raw_entries`),
/// sorted by keycode.
const KEYCODE_TABLE: &[(u16, key::Key)] = &[
    (0x09, key::Key::Escape),
    (0x0a, key::Key::Digit1),
    (0x0b, key::Key::Digit2),
    (0x0c, key::Key::Digit3),
    (0x0d, key::Key::Digit4),
    (0x0e, key::Key::Digit5),
    (0x0f, key::Key::Digit6),
    (0x10, key::Key::Digit7),
    (0x11, key::Key::Digit8),
    (0x12, key::Key::Digit9),
    (0x13, key::Key::Digit0),
    (0x14, key::Key::Minus),
    (0x15, key::Key::Equal),
    (0x16, key::Key::Backspace),
    (0x17, key::Key::Tab),
    (0x18, key::Key::Q),
    (0x19, key::Key::W),
    (0x1a, key::Key::E),
    (0x1b, key::Key::R),
    (0x1c, key::Key::T),
    (0x1d, key::Key::Y),
    (0x1e, key::Key::U),
    (0x1f, key::Key::I),
    (0x20, key::Key::O),
    (0x21, key::Key::P),
    (0x22, key::Key::BracketLeft),
    (0x23, key::Key::BracketRight),
    (0x24, key::Key::Enter),
    (0x25, key::Key::ControlLeft),
    (0x26, key::Key::A),
    (0x27, key::Key::S),
    (0x28, key::Key::D),
    (0x29, key::Key::F),
    (0x2a, key::Key::G),
    (0x2b, key::Key::H),
    (0x2c, key::Key::J),
    (0x2d, key::Key::K),
    (0x2e, key::Key::L),
    (0x2f, key::Key::Semicolon),
    (0x30, key::Key::Quote),
    (0x31, key::Key::Backquote),
    (0x32, key::Key::ShiftLeft),
    (0x33, key::Key::Backslash),
    (0x34, key::Key::Z),
    (0x35, key::Key::X),
    (0x36, key::Key::C),
    (0x37, key::Key::V),
    (0x38, key::Key::B),
    (0x39, key::Key::N),
    (0x3a, key::Key::M),
    (0x3b, key::Key::Comma),
    (0x3c, key::Key::Period),
    (0x3d, key::Key::Slash),
    (0x3e, key::Key::ShiftRight),
    (0x3f, key::Key::NumpadMultiply),
    (0x40, key::Key::AltLeft),
    (0x41, key::Key::Space),
    (0x42, key::Key::CapsLock),
    (0x43, key::Key::F1),
    (0x44, key::Key::F2),
    (0x45, key::Key::F3),
    (0x46, key::Key::F4),
    (0x47, key::Key::F5),
    (0x48, key::Key::F6),
    (0x49, key::Key::F7),
    (0x4a, key::Key::F8),
    (0x4b, key::Key::F9),
    (0x4c, key::Key::F10),
    (0x4d, key::Key::NumLock),
    (0x4e, key::Key::ScrollLock),
    (0x4f, key::Key::Numpad7),
    (0x50, key::Key::Numpad8),
    (0x51, key::Key::Numpad9),
    (0x52, key::Key::NumpadSubtract),
    (0x53, key::Key::Numpad4),
    (0x54, key::Key::Numpad5),
    (0x55, key::Key::Numpad6),
    (0x56, key::Key::NumpadAdd),
    (0x57, key::Key::Numpad1),
    (0x58, key::Key::Numpad2),
    (0x59, key::Key::Numpad3),
    (0x5a, key::Key::Numpad0),
    (0x5b, key::Key::NumpadDecimal),
    (0x5f, key::Key::F11),
    (0x60, key::Key::F12),
    (0x68, key::Key::NumpadEnter),
    (0x69, key::Key::ControlRight),
    (0x6a, key::Key::NumpadDivide),
    (0x6b, key::Key::PrintScreen),
    (0x6c, key::Key::AltRight),
    (0x6e, key::Key::Home),
    (0x6f, key::Key::ArrowUp),
    (0x70, key::Key::PageUp),
    (0x71, key::Key::ArrowLeft),
    (0x72, key::Key::ArrowRight),
    (0x73, key::Key::End),
    (0x74, key::Key::ArrowDown),
    (0x75, key::Key::PageDown),
    (0x76, key::Key::Insert),
    (0x77, key::Key::Delete),
    (0x7d, key::Key::NumpadEqual),
    (0x7f, key::Key::Pause),
    (0x85, key::Key::MetaLeft),
    (0x86, key::Key::MetaRight),
    (0x87, key::Key::ContextMenu),
    (0x8d, key::Key::Copy),
    (0x8f, key::Key::Paste),
    (0x91, key::Key::Cut),
    (0xbf, key::Key::F13),
    (0xc0, key::Key::F14),
    (0xc1, key::Key::F15),
    (0xc2, key::Key::F16),
    (0xc3, key::Key::F17),
    (0xc4, key::Key::F18),
    (0xc5, key::Key::F19),
    (0xc6, key::Key::F20),
    (0xc7, key::Key::F21),
    (0xc8, key::Key::F22),
    (0xc9, key::Key::F23),
    (0xca, key::Key::F24),
];

/// keyval -> logical key, for the writing-system remap and for synthetic
/// keymaps that report every key with one keycode, sorted by keyval.
const KEYVAL_TABLE: &[(u32, key::Key)] = &[
    (0x0020, key::Key::Space),
    (0x0027, key::Key::Quote),
    (0x002c, key::Key::Comma),
    (0x002d, key::Key::Minus),
    (0x002e, key::Key::Period),
    (0x002f, key::Key::Slash),
    (0x0030, key::Key::Digit0),
    (0x0031, key::Key::Digit1),
    (0x0032, key::Key::Digit2),
    (0x0033, key::Key::Digit3),
    (0x0034, key::Key::Digit4),
    (0x0035, key::Key::Digit5),
    (0x0036, key::Key::Digit6),
    (0x0037, key::Key::Digit7),
    (0x0038, key::Key::Digit8),
    (0x0039, key::Key::Digit9),
    (0x003b, key::Key::Semicolon),
    (0x003d, key::Key::Equal),
    (0x005b, key::Key::BracketLeft),
    (0x005c, key::Key::Backslash),
    (0x005d, key::Key::BracketRight),
    (0x0060, key::Key::Backquote),
    (0x0061, key::Key::A),
    (0x0062, key::Key::B),
    (0x0063, key::Key::C),
    (0x0064, key::Key::D),
    (0x0065, key::Key::E),
    (0x0066, key::Key::F),
    (0x0067, key::Key::G),
    (0x0068, key::Key::H),
    (0x0069, key::Key::I),
    (0x006a, key::Key::J),
    (0x006b, key::Key::K),
    (0x006c, key::Key::L),
    (0x006d, key::Key::M),
    (0x006e, key::Key::N),
    (0x006f, key::Key::O),
    (0x0070, key::Key::P),
    (0x0071, key::Key::Q),
    (0x0072, key::Key::R),
    (0x0073, key::Key::S),
    (0x0074, key::Key::T),
    (0x0075, key::Key::U),
    (0x0076, key::Key::V),
    (0x0077, key::Key::W),
    (0x0078, key::Key::X),
    (0x0079, key::Key::Y),
    (0x007a, key::Key::Z),
    (0xff08, key::Key::Backspace),
    (0xff09, key::Key::Tab),
    (0xff0d, key::Key::Enter),
    (0xff13, key::Key::Pause),
    (0xff14, key::Key::ScrollLock),
    (0xff1b, key::Key::Escape),
    (0xff50, key::Key::Home),
    (0xff51, key::Key::ArrowLeft),
    (0xff52, key::Key::ArrowUp),
    (0xff53, key::Key::ArrowRight),
    (0xff54, key::Key::ArrowDown),
    (0xff55, key::Key::PageUp),
    (0xff56, key::Key::PageDown),
    (0xff57, key::Key::End),
    (0xff61, key::Key::PrintScreen),
    (0xff63, key::Key::Insert),
    (0xff7f, key::Key::NumLock),
    (0xff8d, key::Key::NumpadEnter),
    (0xff95, key::Key::NumpadHome),
    (0xff96, key::Key::NumpadLeft),
    (0xff97, key::Key::NumpadUp),
    (0xff98, key::Key::NumpadRight),
    (0xff99, key::Key::NumpadDown),
    (0xff9a, key::Key::NumpadPageUp),
    (0xff9b, key::Key::NumpadPageDown),
    (0xff9c, key::Key::NumpadEnd),
    (0xff9d, key::Key::NumpadBegin),
    (0xff9e, key::Key::NumpadInsert),
    (0xff9f, key::Key::NumpadDelete),
    (0xffaa, key::Key::NumpadMultiply),
    (0xffab, key::Key::NumpadAdd),
    (0xffac, key::Key::NumpadSeparator),
    (0xffad, key::Key::NumpadSubtract),
    (0xffae, key::Key::NumpadDecimal),
    (0xffaf, key::Key::NumpadDivide),
    (0xffb0, key::Key::Numpad0),
    (0xffb1, key::Key::Numpad1),
    (0xffb2, key::Key::Numpad2),
    (0xffb3, key::Key::Numpad3),
    (0xffb4, key::Key::Numpad4),
    (0xffb5, key::Key::Numpad5),
    (0xffb6, key::Key::Numpad6),
    (0xffb7, key::Key::Numpad7),
    (0xffb8, key::Key::Numpad8),
    (0xffb9, key::Key::Numpad9),
    (0xffbd, key::Key::NumpadEqual),
    (0xffbe, key::Key::F1),
    (0xffbf, key::Key::F2),
    (0xffc0, key::Key::F3),
    (0xffc1, key::Key::F4),
    (0xffc2, key::Key::F5),
    (0xffc3, key::Key::F6),
    (0xffc4, key::Key::F7),
    (0xffc5, key::Key::F8),
    (0xffc6, key::Key::F9),
    (0xffc7, key::Key::F10),
    (0xffc8, key::Key::F11),
    (0xffc9, key::Key::F12),
    (0xffca, key::Key::F13),
    (0xffcb, key::Key::F14),
    (0xffcc, key::Key::F15),
    (0xffcd, key::Key::F16),
    (0xffce, key::Key::F17),
    (0xffcf, key::Key::F18),
    (0xffd0, key::Key::F19),
    (0xffd1, key::Key::F20),
    (0xffd2, key::Key::F21),
    (0xffd3, key::Key::F22),
    (0xffd4, key::Key::F23),
    (0xffd5, key::Key::F24),
    (0xffd6, key::Key::F25),
    (0xffe1, key::Key::ShiftLeft),
    (0xffe2, key::Key::ShiftRight),
    (0xffe3, key::Key::ControlLeft),
    (0xffe4, key::Key::ControlRight),
    (0xffe5, key::Key::CapsLock),
    (0xffe9, key::Key::AltLeft),
    (0xffea, key::Key::AltRight),
    (0xffeb, key::Key::MetaLeft),
    (0xffec, key::Key::MetaRight),
    (0xffff, key::Key::Delete),
    (0x1008_ff57, key::Key::Copy),
    (0x1008_ff58, key::Key::Cut),
    (0x1008_ff6d, key::Key::Paste),
];

/// Keyvals that only ever mean "a modifier is held".
const MODIFIER_KEYVALS: &[u32] = &[
    0xfe01, 0xfe02, 0xfe03, 0xfe04, 0xfe05, 0xfe06, 0xfe07, 0xfe11, 0xfe12, 0xfe13, 0xff7e, 0xff7f,
    0xffe1, 0xffe2, 0xffe3, 0xffe4, 0xffe5, 0xffe6, 0xffe7, 0xffe8, 0xffe9, 0xffea, 0xffeb, 0xffec,
    0xffed, 0xffee,
];

/// Writing-system keys (W3C section 3.1.1): the keyval, not the keycode,
/// decides which key the user meant.
const REMAPPABLE_KEYS: &[key::Key] = &[
    key::Key::A,
    key::Key::B,
    key::Key::Backquote,
    key::Key::Backslash,
    key::Key::BracketLeft,
    key::Key::BracketRight,
    key::Key::C,
    key::Key::Comma,
    key::Key::D,
    key::Key::Digit0,
    key::Key::Digit1,
    key::Key::Digit2,
    key::Key::Digit3,
    key::Key::Digit4,
    key::Key::Digit5,
    key::Key::Digit6,
    key::Key::Digit7,
    key::Key::Digit8,
    key::Key::Digit9,
    key::Key::E,
    key::Key::Equal,
    key::Key::F,
    key::Key::G,
    key::Key::H,
    key::Key::I,
    key::Key::IntlBackslash,
    key::Key::IntlRo,
    key::Key::IntlYen,
    key::Key::J,
    key::Key::K,
    key::Key::L,
    key::Key::M,
    key::Key::Minus,
    key::Key::N,
    key::Key::O,
    key::Key::P,
    key::Key::Period,
    key::Key::Q,
    key::Key::Quote,
    key::Key::R,
    key::Key::S,
    key::Key::Semicolon,
    key::Key::Slash,
    key::Key::T,
    key::Key::U,
    key::Key::V,
    key::Key::W,
    key::Key::X,
    key::Key::Y,
    key::Key::Z,
];

/// The exact-match index of `target` in a table sorted by its first field,
/// using the same lower-bound search `keys.zig` used.
fn lookup<T>(table: &[T], target: u32, field: impl Fn(&T) -> u32) -> Option<usize> {
    let mut lo = 0usize;
    let mut hi = table.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if field(&table[mid]) < target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    (lo < table.len() && field(&table[lo]) == target).then_some(lo)
}

/// XKB keycode -> physical key.
#[must_use]
pub fn key_from_keycode(keycode: u32) -> Key {
    match lookup(KEYCODE_TABLE, keycode, |entry| u32::from(entry.0)) {
        Some(i) => Key(KEYCODE_TABLE[i].1),
        None => Key::UNIDENTIFIED,
    }
}

/// keyval -> logical key, for the writing-system remap and for synthetic
/// keymaps that report every key with one keycode.
#[must_use]
pub fn key_from_keyval(keyval: u32) -> Option<Key> {
    lookup(KEYVAL_TABLE, keyval, |entry| entry.0).map(|i| Key(KEYVAL_TABLE[i].1))
}

/// Whether a keyval only ever means "a modifier is held".
#[must_use]
pub fn is_modifier_keyval(keyval: u32) -> bool {
    lookup(MODIFIER_KEYVALS, keyval, |value| *value).is_some()
}

/// Writing-system keys (W3C section 3.1.1): the keyval, not the keycode,
/// decides which key the user meant.
#[must_use]
pub fn should_be_remappable(key: Key) -> bool {
    REMAPPABLE_KEYS.contains(&key.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table is sorted by its lookup key and free of duplicates: the
    /// binary search is only correct while that holds.
    #[test]
    fn tables_are_sorted_and_unique() {
        for pair in KEYCODE_TABLE.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "keycode table out of order at {:x}",
                pair[0].0
            );
        }
        for pair in KEYVAL_TABLE.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "keyval table out of order at {:x}",
                pair[0].0
            );
        }
        for pair in MODIFIER_KEYVALS.windows(2) {
            assert!(pair[0] < pair[1], "modifier table out of order");
        }
    }

    /// The first and last entries of each table resolve.
    #[test]
    fn first_and_last_entries_resolve() {
        let (keycode, key) = KEYCODE_TABLE[0];
        assert_eq!(key_from_keycode(u32::from(keycode)), Key(key));
        let (keycode, key) = KEYCODE_TABLE[KEYCODE_TABLE.len() - 1];
        assert_eq!(key_from_keycode(u32::from(keycode)), Key(key));

        let (keyval, key) = KEYVAL_TABLE[0];
        assert_eq!(key_from_keyval(keyval), Some(Key(key)));
        let (keyval, key) = KEYVAL_TABLE[KEYVAL_TABLE.len() - 1];
        assert_eq!(key_from_keyval(keyval), Some(Key(key)));

        assert!(is_modifier_keyval(MODIFIER_KEYVALS[0]));
        assert!(is_modifier_keyval(
            MODIFIER_KEYVALS[MODIFIER_KEYVALS.len() - 1]
        ));
    }

    /// Unknown keycodes stay unidentified; unknown keyvals resolve to `None`
    /// and are not modifiers.
    #[test]
    fn unknown_values_are_unidentified() {
        assert_eq!(key_from_keycode(0), Key::UNIDENTIFIED);
        assert_eq!(key_from_keycode(0xffff), Key::UNIDENTIFIED);
        assert_eq!(key_from_keyval(0), None);
        assert_eq!(key_from_keyval(0xffff_ffff), None);
        assert!(!is_modifier_keyval(0));
        assert!(!is_modifier_keyval(0xffff_ffff));
    }

    /// Known keyvals and writing-system keys resolve and remap.
    #[test]
    fn known_keys_resolve() {
        assert_eq!(key_from_keycode(0x18), Key(key::Key::Q));
        assert_eq!(key_from_keyval(0x0071), Some(Key(key::Key::Q)));
        assert_eq!(key_from_keyval(0x0061), Some(Key(key::Key::A)));
        assert!(should_be_remappable(Key(key::Key::A)));
        assert!(should_be_remappable(Key(key::Key::IntlRo)));
        assert!(!should_be_remappable(Key(key::Key::Escape)));
    }
}
