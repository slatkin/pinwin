//! Key mapping for the key encoder (port-to-rust D3), mirroring what Ghostty's
//! own GTK apprt does (`src/apprt/gtk/key.zig` and `src/input/keycodes.zig` at
//! the pinned ghostty commit): a physical key from the keycode, the same keyval
//! remap Ghostty applies for writing-system keys, and the same modifier keyvals
//! it refuses to encode.
//!
//! Generated from that commit: `KEYCODE_TABLE` from the `xkb` column of
//! `raw_entries` (which is what GTK reports as a keycode), `KEYVAL_TABLE` from
//! `src/apprt/gtk/key.zig`'s `keymap` with the GDK keyval numbers resolved
//! through the GDK headers. Keys neither table names stay
//! [`Key::UNIDENTIFIED`] and travel as text only.

use crate::ghostty_sys::input::{
    GHOSTTY_KEY_A, GHOSTTY_KEY_ALT_LEFT, GHOSTTY_KEY_ALT_RIGHT, GHOSTTY_KEY_ARROW_DOWN,
    GHOSTTY_KEY_ARROW_LEFT, GHOSTTY_KEY_ARROW_RIGHT, GHOSTTY_KEY_ARROW_UP, GHOSTTY_KEY_B,
    GHOSTTY_KEY_BACKQUOTE, GHOSTTY_KEY_BACKSLASH, GHOSTTY_KEY_BACKSPACE, GHOSTTY_KEY_BRACKET_LEFT,
    GHOSTTY_KEY_BRACKET_RIGHT, GHOSTTY_KEY_C, GHOSTTY_KEY_CAPS_LOCK, GHOSTTY_KEY_COMMA,
    GHOSTTY_KEY_CONTEXT_MENU, GHOSTTY_KEY_CONTROL_LEFT, GHOSTTY_KEY_CONTROL_RIGHT,
    GHOSTTY_KEY_COPY, GHOSTTY_KEY_CUT, GHOSTTY_KEY_D, GHOSTTY_KEY_DELETE, GHOSTTY_KEY_DIGIT_0,
    GHOSTTY_KEY_DIGIT_1, GHOSTTY_KEY_DIGIT_2, GHOSTTY_KEY_DIGIT_3, GHOSTTY_KEY_DIGIT_4,
    GHOSTTY_KEY_DIGIT_5, GHOSTTY_KEY_DIGIT_6, GHOSTTY_KEY_DIGIT_7, GHOSTTY_KEY_DIGIT_8,
    GHOSTTY_KEY_DIGIT_9, GHOSTTY_KEY_E, GHOSTTY_KEY_END, GHOSTTY_KEY_ENTER, GHOSTTY_KEY_EQUAL,
    GHOSTTY_KEY_ESCAPE, GHOSTTY_KEY_F, GHOSTTY_KEY_F1, GHOSTTY_KEY_F2, GHOSTTY_KEY_F3,
    GHOSTTY_KEY_F4, GHOSTTY_KEY_F5, GHOSTTY_KEY_F6, GHOSTTY_KEY_F7, GHOSTTY_KEY_F8, GHOSTTY_KEY_F9,
    GHOSTTY_KEY_F10, GHOSTTY_KEY_F11, GHOSTTY_KEY_F12, GHOSTTY_KEY_F13, GHOSTTY_KEY_F14,
    GHOSTTY_KEY_F15, GHOSTTY_KEY_F16, GHOSTTY_KEY_F17, GHOSTTY_KEY_F18, GHOSTTY_KEY_F19,
    GHOSTTY_KEY_F20, GHOSTTY_KEY_F21, GHOSTTY_KEY_F22, GHOSTTY_KEY_F23, GHOSTTY_KEY_F24,
    GHOSTTY_KEY_F25, GHOSTTY_KEY_G, GHOSTTY_KEY_H, GHOSTTY_KEY_HOME, GHOSTTY_KEY_I,
    GHOSTTY_KEY_INSERT, GHOSTTY_KEY_INTL_BACKSLASH, GHOSTTY_KEY_INTL_RO, GHOSTTY_KEY_INTL_YEN,
    GHOSTTY_KEY_J, GHOSTTY_KEY_K, GHOSTTY_KEY_L, GHOSTTY_KEY_M, GHOSTTY_KEY_META_LEFT,
    GHOSTTY_KEY_META_RIGHT, GHOSTTY_KEY_MINUS, GHOSTTY_KEY_N, GHOSTTY_KEY_NUM_LOCK,
    GHOSTTY_KEY_NUMPAD_0, GHOSTTY_KEY_NUMPAD_1, GHOSTTY_KEY_NUMPAD_2, GHOSTTY_KEY_NUMPAD_3,
    GHOSTTY_KEY_NUMPAD_4, GHOSTTY_KEY_NUMPAD_5, GHOSTTY_KEY_NUMPAD_6, GHOSTTY_KEY_NUMPAD_7,
    GHOSTTY_KEY_NUMPAD_8, GHOSTTY_KEY_NUMPAD_9, GHOSTTY_KEY_NUMPAD_ADD, GHOSTTY_KEY_NUMPAD_BEGIN,
    GHOSTTY_KEY_NUMPAD_DECIMAL, GHOSTTY_KEY_NUMPAD_DELETE, GHOSTTY_KEY_NUMPAD_DIVIDE,
    GHOSTTY_KEY_NUMPAD_DOWN, GHOSTTY_KEY_NUMPAD_END, GHOSTTY_KEY_NUMPAD_ENTER,
    GHOSTTY_KEY_NUMPAD_EQUAL, GHOSTTY_KEY_NUMPAD_HOME, GHOSTTY_KEY_NUMPAD_INSERT,
    GHOSTTY_KEY_NUMPAD_LEFT, GHOSTTY_KEY_NUMPAD_MULTIPLY, GHOSTTY_KEY_NUMPAD_PAGE_DOWN,
    GHOSTTY_KEY_NUMPAD_PAGE_UP, GHOSTTY_KEY_NUMPAD_RIGHT, GHOSTTY_KEY_NUMPAD_SEPARATOR,
    GHOSTTY_KEY_NUMPAD_SUBTRACT, GHOSTTY_KEY_NUMPAD_UP, GHOSTTY_KEY_O, GHOSTTY_KEY_P,
    GHOSTTY_KEY_PAGE_DOWN, GHOSTTY_KEY_PAGE_UP, GHOSTTY_KEY_PASTE, GHOSTTY_KEY_PAUSE,
    GHOSTTY_KEY_PERIOD, GHOSTTY_KEY_PRINT_SCREEN, GHOSTTY_KEY_Q, GHOSTTY_KEY_QUOTE, GHOSTTY_KEY_R,
    GHOSTTY_KEY_S, GHOSTTY_KEY_SCROLL_LOCK, GHOSTTY_KEY_SEMICOLON, GHOSTTY_KEY_SHIFT_LEFT,
    GHOSTTY_KEY_SHIFT_RIGHT, GHOSTTY_KEY_SLASH, GHOSTTY_KEY_SPACE, GHOSTTY_KEY_T, GHOSTTY_KEY_TAB,
    GHOSTTY_KEY_U, GHOSTTY_KEY_UNIDENTIFIED, GHOSTTY_KEY_V, GHOSTTY_KEY_W, GHOSTTY_KEY_X,
    GHOSTTY_KEY_Y, GHOSTTY_KEY_Z, GhosttyKey,
};

/// A physical or logical key, wrapping a libghostty `GhosttyKey` value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key(GhosttyKey);

impl Key {
    /// The key no table names; it travels as text only.
    pub const UNIDENTIFIED: Key = Key(GHOSTTY_KEY_UNIDENTIFIED);

    /// Wrap a raw `GhosttyKey`.
    #[must_use]
    pub const fn new(raw: GhosttyKey) -> Self {
        Key(raw)
    }

    /// The raw `GhosttyKey` value for the FFI call.
    #[must_use]
    pub const fn raw(self) -> GhosttyKey {
        self.0
    }
}

/// XKB keycode -> physical key (the `xkb` column of Ghostty's `raw_entries`),
/// sorted by keycode.
const KEYCODE_TABLE: &[(u16, GhosttyKey)] = &[
    (0x09, GHOSTTY_KEY_ESCAPE),
    (0x0a, GHOSTTY_KEY_DIGIT_1),
    (0x0b, GHOSTTY_KEY_DIGIT_2),
    (0x0c, GHOSTTY_KEY_DIGIT_3),
    (0x0d, GHOSTTY_KEY_DIGIT_4),
    (0x0e, GHOSTTY_KEY_DIGIT_5),
    (0x0f, GHOSTTY_KEY_DIGIT_6),
    (0x10, GHOSTTY_KEY_DIGIT_7),
    (0x11, GHOSTTY_KEY_DIGIT_8),
    (0x12, GHOSTTY_KEY_DIGIT_9),
    (0x13, GHOSTTY_KEY_DIGIT_0),
    (0x14, GHOSTTY_KEY_MINUS),
    (0x15, GHOSTTY_KEY_EQUAL),
    (0x16, GHOSTTY_KEY_BACKSPACE),
    (0x17, GHOSTTY_KEY_TAB),
    (0x18, GHOSTTY_KEY_Q),
    (0x19, GHOSTTY_KEY_W),
    (0x1a, GHOSTTY_KEY_E),
    (0x1b, GHOSTTY_KEY_R),
    (0x1c, GHOSTTY_KEY_T),
    (0x1d, GHOSTTY_KEY_Y),
    (0x1e, GHOSTTY_KEY_U),
    (0x1f, GHOSTTY_KEY_I),
    (0x20, GHOSTTY_KEY_O),
    (0x21, GHOSTTY_KEY_P),
    (0x22, GHOSTTY_KEY_BRACKET_LEFT),
    (0x23, GHOSTTY_KEY_BRACKET_RIGHT),
    (0x24, GHOSTTY_KEY_ENTER),
    (0x25, GHOSTTY_KEY_CONTROL_LEFT),
    (0x26, GHOSTTY_KEY_A),
    (0x27, GHOSTTY_KEY_S),
    (0x28, GHOSTTY_KEY_D),
    (0x29, GHOSTTY_KEY_F),
    (0x2a, GHOSTTY_KEY_G),
    (0x2b, GHOSTTY_KEY_H),
    (0x2c, GHOSTTY_KEY_J),
    (0x2d, GHOSTTY_KEY_K),
    (0x2e, GHOSTTY_KEY_L),
    (0x2f, GHOSTTY_KEY_SEMICOLON),
    (0x30, GHOSTTY_KEY_QUOTE),
    (0x31, GHOSTTY_KEY_BACKQUOTE),
    (0x32, GHOSTTY_KEY_SHIFT_LEFT),
    (0x33, GHOSTTY_KEY_BACKSLASH),
    (0x34, GHOSTTY_KEY_Z),
    (0x35, GHOSTTY_KEY_X),
    (0x36, GHOSTTY_KEY_C),
    (0x37, GHOSTTY_KEY_V),
    (0x38, GHOSTTY_KEY_B),
    (0x39, GHOSTTY_KEY_N),
    (0x3a, GHOSTTY_KEY_M),
    (0x3b, GHOSTTY_KEY_COMMA),
    (0x3c, GHOSTTY_KEY_PERIOD),
    (0x3d, GHOSTTY_KEY_SLASH),
    (0x3e, GHOSTTY_KEY_SHIFT_RIGHT),
    (0x3f, GHOSTTY_KEY_NUMPAD_MULTIPLY),
    (0x40, GHOSTTY_KEY_ALT_LEFT),
    (0x41, GHOSTTY_KEY_SPACE),
    (0x42, GHOSTTY_KEY_CAPS_LOCK),
    (0x43, GHOSTTY_KEY_F1),
    (0x44, GHOSTTY_KEY_F2),
    (0x45, GHOSTTY_KEY_F3),
    (0x46, GHOSTTY_KEY_F4),
    (0x47, GHOSTTY_KEY_F5),
    (0x48, GHOSTTY_KEY_F6),
    (0x49, GHOSTTY_KEY_F7),
    (0x4a, GHOSTTY_KEY_F8),
    (0x4b, GHOSTTY_KEY_F9),
    (0x4c, GHOSTTY_KEY_F10),
    (0x4d, GHOSTTY_KEY_NUM_LOCK),
    (0x4e, GHOSTTY_KEY_SCROLL_LOCK),
    (0x4f, GHOSTTY_KEY_NUMPAD_7),
    (0x50, GHOSTTY_KEY_NUMPAD_8),
    (0x51, GHOSTTY_KEY_NUMPAD_9),
    (0x52, GHOSTTY_KEY_NUMPAD_SUBTRACT),
    (0x53, GHOSTTY_KEY_NUMPAD_4),
    (0x54, GHOSTTY_KEY_NUMPAD_5),
    (0x55, GHOSTTY_KEY_NUMPAD_6),
    (0x56, GHOSTTY_KEY_NUMPAD_ADD),
    (0x57, GHOSTTY_KEY_NUMPAD_1),
    (0x58, GHOSTTY_KEY_NUMPAD_2),
    (0x59, GHOSTTY_KEY_NUMPAD_3),
    (0x5a, GHOSTTY_KEY_NUMPAD_0),
    (0x5b, GHOSTTY_KEY_NUMPAD_DECIMAL),
    (0x5f, GHOSTTY_KEY_F11),
    (0x60, GHOSTTY_KEY_F12),
    (0x68, GHOSTTY_KEY_NUMPAD_ENTER),
    (0x69, GHOSTTY_KEY_CONTROL_RIGHT),
    (0x6a, GHOSTTY_KEY_NUMPAD_DIVIDE),
    (0x6b, GHOSTTY_KEY_PRINT_SCREEN),
    (0x6c, GHOSTTY_KEY_ALT_RIGHT),
    (0x6e, GHOSTTY_KEY_HOME),
    (0x6f, GHOSTTY_KEY_ARROW_UP),
    (0x70, GHOSTTY_KEY_PAGE_UP),
    (0x71, GHOSTTY_KEY_ARROW_LEFT),
    (0x72, GHOSTTY_KEY_ARROW_RIGHT),
    (0x73, GHOSTTY_KEY_END),
    (0x74, GHOSTTY_KEY_ARROW_DOWN),
    (0x75, GHOSTTY_KEY_PAGE_DOWN),
    (0x76, GHOSTTY_KEY_INSERT),
    (0x77, GHOSTTY_KEY_DELETE),
    (0x7d, GHOSTTY_KEY_NUMPAD_EQUAL),
    (0x7f, GHOSTTY_KEY_PAUSE),
    (0x85, GHOSTTY_KEY_META_LEFT),
    (0x86, GHOSTTY_KEY_META_RIGHT),
    (0x87, GHOSTTY_KEY_CONTEXT_MENU),
    (0x8d, GHOSTTY_KEY_COPY),
    (0x8f, GHOSTTY_KEY_PASTE),
    (0x91, GHOSTTY_KEY_CUT),
    (0xbf, GHOSTTY_KEY_F13),
    (0xc0, GHOSTTY_KEY_F14),
    (0xc1, GHOSTTY_KEY_F15),
    (0xc2, GHOSTTY_KEY_F16),
    (0xc3, GHOSTTY_KEY_F17),
    (0xc4, GHOSTTY_KEY_F18),
    (0xc5, GHOSTTY_KEY_F19),
    (0xc6, GHOSTTY_KEY_F20),
    (0xc7, GHOSTTY_KEY_F21),
    (0xc8, GHOSTTY_KEY_F22),
    (0xc9, GHOSTTY_KEY_F23),
    (0xca, GHOSTTY_KEY_F24),
];

/// GDK keyval -> logical key, for the writing-system remap and for synthetic
/// keymaps that report every key with one keycode, sorted by keyval.
const KEYVAL_TABLE: &[(u32, GhosttyKey)] = &[
    (0x0020, GHOSTTY_KEY_SPACE),
    (0x0027, GHOSTTY_KEY_QUOTE),
    (0x002c, GHOSTTY_KEY_COMMA),
    (0x002d, GHOSTTY_KEY_MINUS),
    (0x002e, GHOSTTY_KEY_PERIOD),
    (0x002f, GHOSTTY_KEY_SLASH),
    (0x0030, GHOSTTY_KEY_DIGIT_0),
    (0x0031, GHOSTTY_KEY_DIGIT_1),
    (0x0032, GHOSTTY_KEY_DIGIT_2),
    (0x0033, GHOSTTY_KEY_DIGIT_3),
    (0x0034, GHOSTTY_KEY_DIGIT_4),
    (0x0035, GHOSTTY_KEY_DIGIT_5),
    (0x0036, GHOSTTY_KEY_DIGIT_6),
    (0x0037, GHOSTTY_KEY_DIGIT_7),
    (0x0038, GHOSTTY_KEY_DIGIT_8),
    (0x0039, GHOSTTY_KEY_DIGIT_9),
    (0x003b, GHOSTTY_KEY_SEMICOLON),
    (0x003d, GHOSTTY_KEY_EQUAL),
    (0x005b, GHOSTTY_KEY_BRACKET_LEFT),
    (0x005c, GHOSTTY_KEY_BACKSLASH),
    (0x005d, GHOSTTY_KEY_BRACKET_RIGHT),
    (0x0060, GHOSTTY_KEY_BACKQUOTE),
    (0x0061, GHOSTTY_KEY_A),
    (0x0062, GHOSTTY_KEY_B),
    (0x0063, GHOSTTY_KEY_C),
    (0x0064, GHOSTTY_KEY_D),
    (0x0065, GHOSTTY_KEY_E),
    (0x0066, GHOSTTY_KEY_F),
    (0x0067, GHOSTTY_KEY_G),
    (0x0068, GHOSTTY_KEY_H),
    (0x0069, GHOSTTY_KEY_I),
    (0x006a, GHOSTTY_KEY_J),
    (0x006b, GHOSTTY_KEY_K),
    (0x006c, GHOSTTY_KEY_L),
    (0x006d, GHOSTTY_KEY_M),
    (0x006e, GHOSTTY_KEY_N),
    (0x006f, GHOSTTY_KEY_O),
    (0x0070, GHOSTTY_KEY_P),
    (0x0071, GHOSTTY_KEY_Q),
    (0x0072, GHOSTTY_KEY_R),
    (0x0073, GHOSTTY_KEY_S),
    (0x0074, GHOSTTY_KEY_T),
    (0x0075, GHOSTTY_KEY_U),
    (0x0076, GHOSTTY_KEY_V),
    (0x0077, GHOSTTY_KEY_W),
    (0x0078, GHOSTTY_KEY_X),
    (0x0079, GHOSTTY_KEY_Y),
    (0x007a, GHOSTTY_KEY_Z),
    (0xff08, GHOSTTY_KEY_BACKSPACE),
    (0xff09, GHOSTTY_KEY_TAB),
    (0xff0d, GHOSTTY_KEY_ENTER),
    (0xff13, GHOSTTY_KEY_PAUSE),
    (0xff14, GHOSTTY_KEY_SCROLL_LOCK),
    (0xff1b, GHOSTTY_KEY_ESCAPE),
    (0xff50, GHOSTTY_KEY_HOME),
    (0xff51, GHOSTTY_KEY_ARROW_LEFT),
    (0xff52, GHOSTTY_KEY_ARROW_UP),
    (0xff53, GHOSTTY_KEY_ARROW_RIGHT),
    (0xff54, GHOSTTY_KEY_ARROW_DOWN),
    (0xff55, GHOSTTY_KEY_PAGE_UP),
    (0xff56, GHOSTTY_KEY_PAGE_DOWN),
    (0xff57, GHOSTTY_KEY_END),
    (0xff61, GHOSTTY_KEY_PRINT_SCREEN),
    (0xff63, GHOSTTY_KEY_INSERT),
    (0xff7f, GHOSTTY_KEY_NUM_LOCK),
    (0xff8d, GHOSTTY_KEY_NUMPAD_ENTER),
    (0xff95, GHOSTTY_KEY_NUMPAD_HOME),
    (0xff96, GHOSTTY_KEY_NUMPAD_LEFT),
    (0xff97, GHOSTTY_KEY_NUMPAD_UP),
    (0xff98, GHOSTTY_KEY_NUMPAD_RIGHT),
    (0xff99, GHOSTTY_KEY_NUMPAD_DOWN),
    (0xff9a, GHOSTTY_KEY_NUMPAD_PAGE_UP),
    (0xff9b, GHOSTTY_KEY_NUMPAD_PAGE_DOWN),
    (0xff9c, GHOSTTY_KEY_NUMPAD_END),
    (0xff9d, GHOSTTY_KEY_NUMPAD_BEGIN),
    (0xff9e, GHOSTTY_KEY_NUMPAD_INSERT),
    (0xff9f, GHOSTTY_KEY_NUMPAD_DELETE),
    (0xffaa, GHOSTTY_KEY_NUMPAD_MULTIPLY),
    (0xffab, GHOSTTY_KEY_NUMPAD_ADD),
    (0xffac, GHOSTTY_KEY_NUMPAD_SEPARATOR),
    (0xffad, GHOSTTY_KEY_NUMPAD_SUBTRACT),
    (0xffae, GHOSTTY_KEY_NUMPAD_DECIMAL),
    (0xffaf, GHOSTTY_KEY_NUMPAD_DIVIDE),
    (0xffb0, GHOSTTY_KEY_NUMPAD_0),
    (0xffb1, GHOSTTY_KEY_NUMPAD_1),
    (0xffb2, GHOSTTY_KEY_NUMPAD_2),
    (0xffb3, GHOSTTY_KEY_NUMPAD_3),
    (0xffb4, GHOSTTY_KEY_NUMPAD_4),
    (0xffb5, GHOSTTY_KEY_NUMPAD_5),
    (0xffb6, GHOSTTY_KEY_NUMPAD_6),
    (0xffb7, GHOSTTY_KEY_NUMPAD_7),
    (0xffb8, GHOSTTY_KEY_NUMPAD_8),
    (0xffb9, GHOSTTY_KEY_NUMPAD_9),
    (0xffbd, GHOSTTY_KEY_NUMPAD_EQUAL),
    (0xffbe, GHOSTTY_KEY_F1),
    (0xffbf, GHOSTTY_KEY_F2),
    (0xffc0, GHOSTTY_KEY_F3),
    (0xffc1, GHOSTTY_KEY_F4),
    (0xffc2, GHOSTTY_KEY_F5),
    (0xffc3, GHOSTTY_KEY_F6),
    (0xffc4, GHOSTTY_KEY_F7),
    (0xffc5, GHOSTTY_KEY_F8),
    (0xffc6, GHOSTTY_KEY_F9),
    (0xffc7, GHOSTTY_KEY_F10),
    (0xffc8, GHOSTTY_KEY_F11),
    (0xffc9, GHOSTTY_KEY_F12),
    (0xffca, GHOSTTY_KEY_F13),
    (0xffcb, GHOSTTY_KEY_F14),
    (0xffcc, GHOSTTY_KEY_F15),
    (0xffcd, GHOSTTY_KEY_F16),
    (0xffce, GHOSTTY_KEY_F17),
    (0xffcf, GHOSTTY_KEY_F18),
    (0xffd0, GHOSTTY_KEY_F19),
    (0xffd1, GHOSTTY_KEY_F20),
    (0xffd2, GHOSTTY_KEY_F21),
    (0xffd3, GHOSTTY_KEY_F22),
    (0xffd4, GHOSTTY_KEY_F23),
    (0xffd5, GHOSTTY_KEY_F24),
    (0xffd6, GHOSTTY_KEY_F25),
    (0xffe1, GHOSTTY_KEY_SHIFT_LEFT),
    (0xffe2, GHOSTTY_KEY_SHIFT_RIGHT),
    (0xffe3, GHOSTTY_KEY_CONTROL_LEFT),
    (0xffe4, GHOSTTY_KEY_CONTROL_RIGHT),
    (0xffe5, GHOSTTY_KEY_CAPS_LOCK),
    (0xffe9, GHOSTTY_KEY_ALT_LEFT),
    (0xffea, GHOSTTY_KEY_ALT_RIGHT),
    (0xffeb, GHOSTTY_KEY_META_LEFT),
    (0xffec, GHOSTTY_KEY_META_RIGHT),
    (0xffff, GHOSTTY_KEY_DELETE),
    (0x1008_ff57, GHOSTTY_KEY_COPY),
    (0x1008_ff58, GHOSTTY_KEY_CUT),
    (0x1008_ff6d, GHOSTTY_KEY_PASTE),
];

/// Keyvals that only ever mean "a modifier is held".
const MODIFIER_KEYVALS: &[u32] = &[
    0xfe01, 0xfe02, 0xfe03, 0xfe04, 0xfe05, 0xfe06, 0xfe07, 0xfe11, 0xfe12, 0xfe13, 0xff7e, 0xff7f,
    0xffe1, 0xffe2, 0xffe3, 0xffe4, 0xffe5, 0xffe6, 0xffe7, 0xffe8, 0xffe9, 0xffea, 0xffeb, 0xffec,
    0xffed, 0xffee,
];

/// Writing-system keys (W3C section 3.1.1): the keyval, not the keycode,
/// decides which key the user meant.
const REMAPPABLE_KEYS: &[GhosttyKey] = &[
    GHOSTTY_KEY_A,
    GHOSTTY_KEY_B,
    GHOSTTY_KEY_BACKQUOTE,
    GHOSTTY_KEY_BACKSLASH,
    GHOSTTY_KEY_BRACKET_LEFT,
    GHOSTTY_KEY_BRACKET_RIGHT,
    GHOSTTY_KEY_C,
    GHOSTTY_KEY_COMMA,
    GHOSTTY_KEY_D,
    GHOSTTY_KEY_DIGIT_0,
    GHOSTTY_KEY_DIGIT_1,
    GHOSTTY_KEY_DIGIT_2,
    GHOSTTY_KEY_DIGIT_3,
    GHOSTTY_KEY_DIGIT_4,
    GHOSTTY_KEY_DIGIT_5,
    GHOSTTY_KEY_DIGIT_6,
    GHOSTTY_KEY_DIGIT_7,
    GHOSTTY_KEY_DIGIT_8,
    GHOSTTY_KEY_DIGIT_9,
    GHOSTTY_KEY_E,
    GHOSTTY_KEY_EQUAL,
    GHOSTTY_KEY_F,
    GHOSTTY_KEY_G,
    GHOSTTY_KEY_H,
    GHOSTTY_KEY_I,
    GHOSTTY_KEY_INTL_BACKSLASH,
    GHOSTTY_KEY_INTL_RO,
    GHOSTTY_KEY_INTL_YEN,
    GHOSTTY_KEY_J,
    GHOSTTY_KEY_K,
    GHOSTTY_KEY_L,
    GHOSTTY_KEY_M,
    GHOSTTY_KEY_MINUS,
    GHOSTTY_KEY_N,
    GHOSTTY_KEY_O,
    GHOSTTY_KEY_P,
    GHOSTTY_KEY_PERIOD,
    GHOSTTY_KEY_Q,
    GHOSTTY_KEY_QUOTE,
    GHOSTTY_KEY_R,
    GHOSTTY_KEY_S,
    GHOSTTY_KEY_SEMICOLON,
    GHOSTTY_KEY_SLASH,
    GHOSTTY_KEY_T,
    GHOSTTY_KEY_U,
    GHOSTTY_KEY_V,
    GHOSTTY_KEY_W,
    GHOSTTY_KEY_X,
    GHOSTTY_KEY_Y,
    GHOSTTY_KEY_Z,
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

/// GDK keyval -> logical key, for the writing-system remap and for synthetic
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
        assert_eq!(key_from_keycode(0x18), Key(GHOSTTY_KEY_Q));
        assert_eq!(key_from_keyval(0x0071), Some(Key(GHOSTTY_KEY_Q)));
        assert_eq!(key_from_keyval(0x0061), Some(Key(GHOSTTY_KEY_A)));
        assert!(should_be_remappable(Key(GHOSTTY_KEY_A)));
        assert!(should_be_remappable(Key(GHOSTTY_KEY_INTL_RO)));
        assert!(!should_be_remappable(Key(GHOSTTY_KEY_ESCAPE)));
    }
}
