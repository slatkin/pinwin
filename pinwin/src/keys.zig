//! Key mapping for the key encoder, mirroring what Ghostty's own GTK apprt
//! does (`src/apprt/gtk/key.zig` and `src/input/keycodes.zig` at the pinned
//! ghostty commit): a physical key from the keycode, the same keyval remap
//! Ghostty applies for writing-system keys, and the same modifier keyvals it
//! refuses to encode.
//!
//! Generated from that commit: `keycodeTable` from the `xkb` column of
//! `raw_entries` (which is what GTK reports as a keycode), `keyvalTable` from
//! `src/apprt/gtk/key.zig`'s `keymap` with the GDK keyval numbers resolved
//! through the GDK headers. Keys neither table names stay
//! GHOSTTY_KEY_UNIDENTIFIED and travel as text only.

const c = @import("c.zig").c;

const KeycodeEntry = struct { keycode: u16, key: c.GhosttyKey };

const keycodeTable = [_]KeycodeEntry{
    .{ .keycode = 0x09, .key = c.GHOSTTY_KEY_ESCAPE },
    .{ .keycode = 0x0a, .key = c.GHOSTTY_KEY_DIGIT_1 },
    .{ .keycode = 0x0b, .key = c.GHOSTTY_KEY_DIGIT_2 },
    .{ .keycode = 0x0c, .key = c.GHOSTTY_KEY_DIGIT_3 },
    .{ .keycode = 0x0d, .key = c.GHOSTTY_KEY_DIGIT_4 },
    .{ .keycode = 0x0e, .key = c.GHOSTTY_KEY_DIGIT_5 },
    .{ .keycode = 0x0f, .key = c.GHOSTTY_KEY_DIGIT_6 },
    .{ .keycode = 0x10, .key = c.GHOSTTY_KEY_DIGIT_7 },
    .{ .keycode = 0x11, .key = c.GHOSTTY_KEY_DIGIT_8 },
    .{ .keycode = 0x12, .key = c.GHOSTTY_KEY_DIGIT_9 },
    .{ .keycode = 0x13, .key = c.GHOSTTY_KEY_DIGIT_0 },
    .{ .keycode = 0x14, .key = c.GHOSTTY_KEY_MINUS },
    .{ .keycode = 0x15, .key = c.GHOSTTY_KEY_EQUAL },
    .{ .keycode = 0x16, .key = c.GHOSTTY_KEY_BACKSPACE },
    .{ .keycode = 0x17, .key = c.GHOSTTY_KEY_TAB },
    .{ .keycode = 0x18, .key = c.GHOSTTY_KEY_Q },
    .{ .keycode = 0x19, .key = c.GHOSTTY_KEY_W },
    .{ .keycode = 0x1a, .key = c.GHOSTTY_KEY_E },
    .{ .keycode = 0x1b, .key = c.GHOSTTY_KEY_R },
    .{ .keycode = 0x1c, .key = c.GHOSTTY_KEY_T },
    .{ .keycode = 0x1d, .key = c.GHOSTTY_KEY_Y },
    .{ .keycode = 0x1e, .key = c.GHOSTTY_KEY_U },
    .{ .keycode = 0x1f, .key = c.GHOSTTY_KEY_I },
    .{ .keycode = 0x20, .key = c.GHOSTTY_KEY_O },
    .{ .keycode = 0x21, .key = c.GHOSTTY_KEY_P },
    .{ .keycode = 0x22, .key = c.GHOSTTY_KEY_BRACKET_LEFT },
    .{ .keycode = 0x23, .key = c.GHOSTTY_KEY_BRACKET_RIGHT },
    .{ .keycode = 0x24, .key = c.GHOSTTY_KEY_ENTER },
    .{ .keycode = 0x25, .key = c.GHOSTTY_KEY_CONTROL_LEFT },
    .{ .keycode = 0x26, .key = c.GHOSTTY_KEY_A },
    .{ .keycode = 0x27, .key = c.GHOSTTY_KEY_S },
    .{ .keycode = 0x28, .key = c.GHOSTTY_KEY_D },
    .{ .keycode = 0x29, .key = c.GHOSTTY_KEY_F },
    .{ .keycode = 0x2a, .key = c.GHOSTTY_KEY_G },
    .{ .keycode = 0x2b, .key = c.GHOSTTY_KEY_H },
    .{ .keycode = 0x2c, .key = c.GHOSTTY_KEY_J },
    .{ .keycode = 0x2d, .key = c.GHOSTTY_KEY_K },
    .{ .keycode = 0x2e, .key = c.GHOSTTY_KEY_L },
    .{ .keycode = 0x2f, .key = c.GHOSTTY_KEY_SEMICOLON },
    .{ .keycode = 0x30, .key = c.GHOSTTY_KEY_QUOTE },
    .{ .keycode = 0x31, .key = c.GHOSTTY_KEY_BACKQUOTE },
    .{ .keycode = 0x32, .key = c.GHOSTTY_KEY_SHIFT_LEFT },
    .{ .keycode = 0x33, .key = c.GHOSTTY_KEY_BACKSLASH },
    .{ .keycode = 0x34, .key = c.GHOSTTY_KEY_Z },
    .{ .keycode = 0x35, .key = c.GHOSTTY_KEY_X },
    .{ .keycode = 0x36, .key = c.GHOSTTY_KEY_C },
    .{ .keycode = 0x37, .key = c.GHOSTTY_KEY_V },
    .{ .keycode = 0x38, .key = c.GHOSTTY_KEY_B },
    .{ .keycode = 0x39, .key = c.GHOSTTY_KEY_N },
    .{ .keycode = 0x3a, .key = c.GHOSTTY_KEY_M },
    .{ .keycode = 0x3b, .key = c.GHOSTTY_KEY_COMMA },
    .{ .keycode = 0x3c, .key = c.GHOSTTY_KEY_PERIOD },
    .{ .keycode = 0x3d, .key = c.GHOSTTY_KEY_SLASH },
    .{ .keycode = 0x3e, .key = c.GHOSTTY_KEY_SHIFT_RIGHT },
    .{ .keycode = 0x3f, .key = c.GHOSTTY_KEY_NUMPAD_MULTIPLY },
    .{ .keycode = 0x40, .key = c.GHOSTTY_KEY_ALT_LEFT },
    .{ .keycode = 0x41, .key = c.GHOSTTY_KEY_SPACE },
    .{ .keycode = 0x42, .key = c.GHOSTTY_KEY_CAPS_LOCK },
    .{ .keycode = 0x43, .key = c.GHOSTTY_KEY_F1 },
    .{ .keycode = 0x44, .key = c.GHOSTTY_KEY_F2 },
    .{ .keycode = 0x45, .key = c.GHOSTTY_KEY_F3 },
    .{ .keycode = 0x46, .key = c.GHOSTTY_KEY_F4 },
    .{ .keycode = 0x47, .key = c.GHOSTTY_KEY_F5 },
    .{ .keycode = 0x48, .key = c.GHOSTTY_KEY_F6 },
    .{ .keycode = 0x49, .key = c.GHOSTTY_KEY_F7 },
    .{ .keycode = 0x4a, .key = c.GHOSTTY_KEY_F8 },
    .{ .keycode = 0x4b, .key = c.GHOSTTY_KEY_F9 },
    .{ .keycode = 0x4c, .key = c.GHOSTTY_KEY_F10 },
    .{ .keycode = 0x4d, .key = c.GHOSTTY_KEY_NUM_LOCK },
    .{ .keycode = 0x4e, .key = c.GHOSTTY_KEY_SCROLL_LOCK },
    .{ .keycode = 0x4f, .key = c.GHOSTTY_KEY_NUMPAD_7 },
    .{ .keycode = 0x50, .key = c.GHOSTTY_KEY_NUMPAD_8 },
    .{ .keycode = 0x51, .key = c.GHOSTTY_KEY_NUMPAD_9 },
    .{ .keycode = 0x52, .key = c.GHOSTTY_KEY_NUMPAD_SUBTRACT },
    .{ .keycode = 0x53, .key = c.GHOSTTY_KEY_NUMPAD_4 },
    .{ .keycode = 0x54, .key = c.GHOSTTY_KEY_NUMPAD_5 },
    .{ .keycode = 0x55, .key = c.GHOSTTY_KEY_NUMPAD_6 },
    .{ .keycode = 0x56, .key = c.GHOSTTY_KEY_NUMPAD_ADD },
    .{ .keycode = 0x57, .key = c.GHOSTTY_KEY_NUMPAD_1 },
    .{ .keycode = 0x58, .key = c.GHOSTTY_KEY_NUMPAD_2 },
    .{ .keycode = 0x59, .key = c.GHOSTTY_KEY_NUMPAD_3 },
    .{ .keycode = 0x5a, .key = c.GHOSTTY_KEY_NUMPAD_0 },
    .{ .keycode = 0x5b, .key = c.GHOSTTY_KEY_NUMPAD_DECIMAL },
    .{ .keycode = 0x5f, .key = c.GHOSTTY_KEY_F11 },
    .{ .keycode = 0x60, .key = c.GHOSTTY_KEY_F12 },
    .{ .keycode = 0x68, .key = c.GHOSTTY_KEY_NUMPAD_ENTER },
    .{ .keycode = 0x69, .key = c.GHOSTTY_KEY_CONTROL_RIGHT },
    .{ .keycode = 0x6a, .key = c.GHOSTTY_KEY_NUMPAD_DIVIDE },
    .{ .keycode = 0x6b, .key = c.GHOSTTY_KEY_PRINT_SCREEN },
    .{ .keycode = 0x6c, .key = c.GHOSTTY_KEY_ALT_RIGHT },
    .{ .keycode = 0x6e, .key = c.GHOSTTY_KEY_HOME },
    .{ .keycode = 0x6f, .key = c.GHOSTTY_KEY_ARROW_UP },
    .{ .keycode = 0x70, .key = c.GHOSTTY_KEY_PAGE_UP },
    .{ .keycode = 0x71, .key = c.GHOSTTY_KEY_ARROW_LEFT },
    .{ .keycode = 0x72, .key = c.GHOSTTY_KEY_ARROW_RIGHT },
    .{ .keycode = 0x73, .key = c.GHOSTTY_KEY_END },
    .{ .keycode = 0x74, .key = c.GHOSTTY_KEY_ARROW_DOWN },
    .{ .keycode = 0x75, .key = c.GHOSTTY_KEY_PAGE_DOWN },
    .{ .keycode = 0x76, .key = c.GHOSTTY_KEY_INSERT },
    .{ .keycode = 0x77, .key = c.GHOSTTY_KEY_DELETE },
    .{ .keycode = 0x7d, .key = c.GHOSTTY_KEY_NUMPAD_EQUAL },
    .{ .keycode = 0x7f, .key = c.GHOSTTY_KEY_PAUSE },
    .{ .keycode = 0x85, .key = c.GHOSTTY_KEY_META_LEFT },
    .{ .keycode = 0x86, .key = c.GHOSTTY_KEY_META_RIGHT },
    .{ .keycode = 0x87, .key = c.GHOSTTY_KEY_CONTEXT_MENU },
    .{ .keycode = 0x8d, .key = c.GHOSTTY_KEY_COPY },
    .{ .keycode = 0x8f, .key = c.GHOSTTY_KEY_PASTE },
    .{ .keycode = 0x91, .key = c.GHOSTTY_KEY_CUT },
    .{ .keycode = 0xbf, .key = c.GHOSTTY_KEY_F13 },
    .{ .keycode = 0xc0, .key = c.GHOSTTY_KEY_F14 },
    .{ .keycode = 0xc1, .key = c.GHOSTTY_KEY_F15 },
    .{ .keycode = 0xc2, .key = c.GHOSTTY_KEY_F16 },
    .{ .keycode = 0xc3, .key = c.GHOSTTY_KEY_F17 },
    .{ .keycode = 0xc4, .key = c.GHOSTTY_KEY_F18 },
    .{ .keycode = 0xc5, .key = c.GHOSTTY_KEY_F19 },
    .{ .keycode = 0xc6, .key = c.GHOSTTY_KEY_F20 },
    .{ .keycode = 0xc7, .key = c.GHOSTTY_KEY_F21 },
    .{ .keycode = 0xc8, .key = c.GHOSTTY_KEY_F22 },
    .{ .keycode = 0xc9, .key = c.GHOSTTY_KEY_F23 },
    .{ .keycode = 0xca, .key = c.GHOSTTY_KEY_F24 },
};

const KeyvalEntry = struct { keyval: u32, key: c.GhosttyKey };

const keyvalTable = [_]KeyvalEntry{
    .{ .keyval = 0x0020, .key = c.GHOSTTY_KEY_SPACE },
    .{ .keyval = 0x0027, .key = c.GHOSTTY_KEY_QUOTE },
    .{ .keyval = 0x002c, .key = c.GHOSTTY_KEY_COMMA },
    .{ .keyval = 0x002d, .key = c.GHOSTTY_KEY_MINUS },
    .{ .keyval = 0x002e, .key = c.GHOSTTY_KEY_PERIOD },
    .{ .keyval = 0x002f, .key = c.GHOSTTY_KEY_SLASH },
    .{ .keyval = 0x0030, .key = c.GHOSTTY_KEY_DIGIT_0 },
    .{ .keyval = 0x0031, .key = c.GHOSTTY_KEY_DIGIT_1 },
    .{ .keyval = 0x0032, .key = c.GHOSTTY_KEY_DIGIT_2 },
    .{ .keyval = 0x0033, .key = c.GHOSTTY_KEY_DIGIT_3 },
    .{ .keyval = 0x0034, .key = c.GHOSTTY_KEY_DIGIT_4 },
    .{ .keyval = 0x0035, .key = c.GHOSTTY_KEY_DIGIT_5 },
    .{ .keyval = 0x0036, .key = c.GHOSTTY_KEY_DIGIT_6 },
    .{ .keyval = 0x0037, .key = c.GHOSTTY_KEY_DIGIT_7 },
    .{ .keyval = 0x0038, .key = c.GHOSTTY_KEY_DIGIT_8 },
    .{ .keyval = 0x0039, .key = c.GHOSTTY_KEY_DIGIT_9 },
    .{ .keyval = 0x003b, .key = c.GHOSTTY_KEY_SEMICOLON },
    .{ .keyval = 0x003d, .key = c.GHOSTTY_KEY_EQUAL },
    .{ .keyval = 0x005b, .key = c.GHOSTTY_KEY_BRACKET_LEFT },
    .{ .keyval = 0x005c, .key = c.GHOSTTY_KEY_BACKSLASH },
    .{ .keyval = 0x005d, .key = c.GHOSTTY_KEY_BRACKET_RIGHT },
    .{ .keyval = 0x0060, .key = c.GHOSTTY_KEY_BACKQUOTE },
    .{ .keyval = 0x0061, .key = c.GHOSTTY_KEY_A },
    .{ .keyval = 0x0062, .key = c.GHOSTTY_KEY_B },
    .{ .keyval = 0x0063, .key = c.GHOSTTY_KEY_C },
    .{ .keyval = 0x0064, .key = c.GHOSTTY_KEY_D },
    .{ .keyval = 0x0065, .key = c.GHOSTTY_KEY_E },
    .{ .keyval = 0x0066, .key = c.GHOSTTY_KEY_F },
    .{ .keyval = 0x0067, .key = c.GHOSTTY_KEY_G },
    .{ .keyval = 0x0068, .key = c.GHOSTTY_KEY_H },
    .{ .keyval = 0x0069, .key = c.GHOSTTY_KEY_I },
    .{ .keyval = 0x006a, .key = c.GHOSTTY_KEY_J },
    .{ .keyval = 0x006b, .key = c.GHOSTTY_KEY_K },
    .{ .keyval = 0x006c, .key = c.GHOSTTY_KEY_L },
    .{ .keyval = 0x006d, .key = c.GHOSTTY_KEY_M },
    .{ .keyval = 0x006e, .key = c.GHOSTTY_KEY_N },
    .{ .keyval = 0x006f, .key = c.GHOSTTY_KEY_O },
    .{ .keyval = 0x0070, .key = c.GHOSTTY_KEY_P },
    .{ .keyval = 0x0071, .key = c.GHOSTTY_KEY_Q },
    .{ .keyval = 0x0072, .key = c.GHOSTTY_KEY_R },
    .{ .keyval = 0x0073, .key = c.GHOSTTY_KEY_S },
    .{ .keyval = 0x0074, .key = c.GHOSTTY_KEY_T },
    .{ .keyval = 0x0075, .key = c.GHOSTTY_KEY_U },
    .{ .keyval = 0x0076, .key = c.GHOSTTY_KEY_V },
    .{ .keyval = 0x0077, .key = c.GHOSTTY_KEY_W },
    .{ .keyval = 0x0078, .key = c.GHOSTTY_KEY_X },
    .{ .keyval = 0x0079, .key = c.GHOSTTY_KEY_Y },
    .{ .keyval = 0x007a, .key = c.GHOSTTY_KEY_Z },
    .{ .keyval = 0xff08, .key = c.GHOSTTY_KEY_BACKSPACE },
    .{ .keyval = 0xff09, .key = c.GHOSTTY_KEY_TAB },
    .{ .keyval = 0xff0d, .key = c.GHOSTTY_KEY_ENTER },
    .{ .keyval = 0xff13, .key = c.GHOSTTY_KEY_PAUSE },
    .{ .keyval = 0xff14, .key = c.GHOSTTY_KEY_SCROLL_LOCK },
    .{ .keyval = 0xff1b, .key = c.GHOSTTY_KEY_ESCAPE },
    .{ .keyval = 0xff50, .key = c.GHOSTTY_KEY_HOME },
    .{ .keyval = 0xff51, .key = c.GHOSTTY_KEY_ARROW_LEFT },
    .{ .keyval = 0xff52, .key = c.GHOSTTY_KEY_ARROW_UP },
    .{ .keyval = 0xff53, .key = c.GHOSTTY_KEY_ARROW_RIGHT },
    .{ .keyval = 0xff54, .key = c.GHOSTTY_KEY_ARROW_DOWN },
    .{ .keyval = 0xff55, .key = c.GHOSTTY_KEY_PAGE_UP },
    .{ .keyval = 0xff56, .key = c.GHOSTTY_KEY_PAGE_DOWN },
    .{ .keyval = 0xff57, .key = c.GHOSTTY_KEY_END },
    .{ .keyval = 0xff61, .key = c.GHOSTTY_KEY_PRINT_SCREEN },
    .{ .keyval = 0xff63, .key = c.GHOSTTY_KEY_INSERT },
    .{ .keyval = 0xff7f, .key = c.GHOSTTY_KEY_NUM_LOCK },
    .{ .keyval = 0xff8d, .key = c.GHOSTTY_KEY_NUMPAD_ENTER },
    .{ .keyval = 0xff95, .key = c.GHOSTTY_KEY_NUMPAD_HOME },
    .{ .keyval = 0xff96, .key = c.GHOSTTY_KEY_NUMPAD_LEFT },
    .{ .keyval = 0xff97, .key = c.GHOSTTY_KEY_NUMPAD_UP },
    .{ .keyval = 0xff98, .key = c.GHOSTTY_KEY_NUMPAD_RIGHT },
    .{ .keyval = 0xff99, .key = c.GHOSTTY_KEY_NUMPAD_DOWN },
    .{ .keyval = 0xff9a, .key = c.GHOSTTY_KEY_NUMPAD_PAGE_UP },
    .{ .keyval = 0xff9b, .key = c.GHOSTTY_KEY_NUMPAD_PAGE_DOWN },
    .{ .keyval = 0xff9c, .key = c.GHOSTTY_KEY_NUMPAD_END },
    .{ .keyval = 0xff9d, .key = c.GHOSTTY_KEY_NUMPAD_BEGIN },
    .{ .keyval = 0xff9e, .key = c.GHOSTTY_KEY_NUMPAD_INSERT },
    .{ .keyval = 0xff9f, .key = c.GHOSTTY_KEY_NUMPAD_DELETE },
    .{ .keyval = 0xffaa, .key = c.GHOSTTY_KEY_NUMPAD_MULTIPLY },
    .{ .keyval = 0xffab, .key = c.GHOSTTY_KEY_NUMPAD_ADD },
    .{ .keyval = 0xffac, .key = c.GHOSTTY_KEY_NUMPAD_SEPARATOR },
    .{ .keyval = 0xffad, .key = c.GHOSTTY_KEY_NUMPAD_SUBTRACT },
    .{ .keyval = 0xffae, .key = c.GHOSTTY_KEY_NUMPAD_DECIMAL },
    .{ .keyval = 0xffaf, .key = c.GHOSTTY_KEY_NUMPAD_DIVIDE },
    .{ .keyval = 0xffb0, .key = c.GHOSTTY_KEY_NUMPAD_0 },
    .{ .keyval = 0xffb1, .key = c.GHOSTTY_KEY_NUMPAD_1 },
    .{ .keyval = 0xffb2, .key = c.GHOSTTY_KEY_NUMPAD_2 },
    .{ .keyval = 0xffb3, .key = c.GHOSTTY_KEY_NUMPAD_3 },
    .{ .keyval = 0xffb4, .key = c.GHOSTTY_KEY_NUMPAD_4 },
    .{ .keyval = 0xffb5, .key = c.GHOSTTY_KEY_NUMPAD_5 },
    .{ .keyval = 0xffb6, .key = c.GHOSTTY_KEY_NUMPAD_6 },
    .{ .keyval = 0xffb7, .key = c.GHOSTTY_KEY_NUMPAD_7 },
    .{ .keyval = 0xffb8, .key = c.GHOSTTY_KEY_NUMPAD_8 },
    .{ .keyval = 0xffb9, .key = c.GHOSTTY_KEY_NUMPAD_9 },
    .{ .keyval = 0xffbd, .key = c.GHOSTTY_KEY_NUMPAD_EQUAL },
    .{ .keyval = 0xffbe, .key = c.GHOSTTY_KEY_F1 },
    .{ .keyval = 0xffbf, .key = c.GHOSTTY_KEY_F2 },
    .{ .keyval = 0xffc0, .key = c.GHOSTTY_KEY_F3 },
    .{ .keyval = 0xffc1, .key = c.GHOSTTY_KEY_F4 },
    .{ .keyval = 0xffc2, .key = c.GHOSTTY_KEY_F5 },
    .{ .keyval = 0xffc3, .key = c.GHOSTTY_KEY_F6 },
    .{ .keyval = 0xffc4, .key = c.GHOSTTY_KEY_F7 },
    .{ .keyval = 0xffc5, .key = c.GHOSTTY_KEY_F8 },
    .{ .keyval = 0xffc6, .key = c.GHOSTTY_KEY_F9 },
    .{ .keyval = 0xffc7, .key = c.GHOSTTY_KEY_F10 },
    .{ .keyval = 0xffc8, .key = c.GHOSTTY_KEY_F11 },
    .{ .keyval = 0xffc9, .key = c.GHOSTTY_KEY_F12 },
    .{ .keyval = 0xffca, .key = c.GHOSTTY_KEY_F13 },
    .{ .keyval = 0xffcb, .key = c.GHOSTTY_KEY_F14 },
    .{ .keyval = 0xffcc, .key = c.GHOSTTY_KEY_F15 },
    .{ .keyval = 0xffcd, .key = c.GHOSTTY_KEY_F16 },
    .{ .keyval = 0xffce, .key = c.GHOSTTY_KEY_F17 },
    .{ .keyval = 0xffcf, .key = c.GHOSTTY_KEY_F18 },
    .{ .keyval = 0xffd0, .key = c.GHOSTTY_KEY_F19 },
    .{ .keyval = 0xffd1, .key = c.GHOSTTY_KEY_F20 },
    .{ .keyval = 0xffd2, .key = c.GHOSTTY_KEY_F21 },
    .{ .keyval = 0xffd3, .key = c.GHOSTTY_KEY_F22 },
    .{ .keyval = 0xffd4, .key = c.GHOSTTY_KEY_F23 },
    .{ .keyval = 0xffd5, .key = c.GHOSTTY_KEY_F24 },
    .{ .keyval = 0xffd6, .key = c.GHOSTTY_KEY_F25 },
    .{ .keyval = 0xffe1, .key = c.GHOSTTY_KEY_SHIFT_LEFT },
    .{ .keyval = 0xffe2, .key = c.GHOSTTY_KEY_SHIFT_RIGHT },
    .{ .keyval = 0xffe3, .key = c.GHOSTTY_KEY_CONTROL_LEFT },
    .{ .keyval = 0xffe4, .key = c.GHOSTTY_KEY_CONTROL_RIGHT },
    .{ .keyval = 0xffe5, .key = c.GHOSTTY_KEY_CAPS_LOCK },
    .{ .keyval = 0xffe9, .key = c.GHOSTTY_KEY_ALT_LEFT },
    .{ .keyval = 0xffea, .key = c.GHOSTTY_KEY_ALT_RIGHT },
    .{ .keyval = 0xffeb, .key = c.GHOSTTY_KEY_META_LEFT },
    .{ .keyval = 0xffec, .key = c.GHOSTTY_KEY_META_RIGHT },
    .{ .keyval = 0xffff, .key = c.GHOSTTY_KEY_DELETE },
    .{ .keyval = 0x1008ff57, .key = c.GHOSTTY_KEY_COPY },
    .{ .keyval = 0x1008ff58, .key = c.GHOSTTY_KEY_CUT },
    .{ .keyval = 0x1008ff6d, .key = c.GHOSTTY_KEY_PASTE },
};

/// Keyvals that only ever mean "a modifier is held".
const modifierKeyvals = [_]u32{
    0xfe01, 0xfe02, 0xfe03, 0xfe04, 0xfe05, 0xfe06, 0xfe07, 0xfe11, 0xfe12, 0xfe13, 0xff7e,
    0xff7f, 0xffe1, 0xffe2, 0xffe3, 0xffe4, 0xffe5, 0xffe6, 0xffe7, 0xffe8, 0xffe9, 0xffea,
    0xffeb, 0xffec, 0xffed, 0xffee,
};

const remappableKeys = [_]c.GhosttyKey{
    c.GHOSTTY_KEY_A, c.GHOSTTY_KEY_B, c.GHOSTTY_KEY_BACKQUOTE, c.GHOSTTY_KEY_BACKSLASH,
    c.GHOSTTY_KEY_BRACKET_LEFT, c.GHOSTTY_KEY_BRACKET_RIGHT, c.GHOSTTY_KEY_C,
    c.GHOSTTY_KEY_COMMA, c.GHOSTTY_KEY_D, c.GHOSTTY_KEY_DIGIT_0, c.GHOSTTY_KEY_DIGIT_1,
    c.GHOSTTY_KEY_DIGIT_2, c.GHOSTTY_KEY_DIGIT_3, c.GHOSTTY_KEY_DIGIT_4,
    c.GHOSTTY_KEY_DIGIT_5, c.GHOSTTY_KEY_DIGIT_6, c.GHOSTTY_KEY_DIGIT_7,
    c.GHOSTTY_KEY_DIGIT_8, c.GHOSTTY_KEY_DIGIT_9, c.GHOSTTY_KEY_E, c.GHOSTTY_KEY_EQUAL,
    c.GHOSTTY_KEY_F, c.GHOSTTY_KEY_G, c.GHOSTTY_KEY_H, c.GHOSTTY_KEY_I,
    c.GHOSTTY_KEY_INTL_BACKSLASH, c.GHOSTTY_KEY_INTL_RO, c.GHOSTTY_KEY_INTL_YEN,
    c.GHOSTTY_KEY_J, c.GHOSTTY_KEY_K, c.GHOSTTY_KEY_L, c.GHOSTTY_KEY_M, c.GHOSTTY_KEY_MINUS,
    c.GHOSTTY_KEY_N, c.GHOSTTY_KEY_O, c.GHOSTTY_KEY_P, c.GHOSTTY_KEY_PERIOD,
    c.GHOSTTY_KEY_Q, c.GHOSTTY_KEY_QUOTE, c.GHOSTTY_KEY_R, c.GHOSTTY_KEY_S,
    c.GHOSTTY_KEY_SEMICOLON, c.GHOSTTY_KEY_SLASH, c.GHOSTTY_KEY_T, c.GHOSTTY_KEY_U,
    c.GHOSTTY_KEY_V, c.GHOSTTY_KEY_W, c.GHOSTTY_KEY_X, c.GHOSTTY_KEY_Y, c.GHOSTTY_KEY_Z,
};

fn lookup(comptime T: type, comptime table: []const T, target: u32, comptime field: []const u8) ?usize {
    var lo: usize = 0;
    var hi: usize = table.len;
    while (lo < hi) {
        const mid = lo + (hi - lo) / 2;
        if (@field(table[mid], field) < target) lo = mid + 1 else hi = mid;
    }
    if (lo < table.len and @field(table[lo], field) == target) return lo;
    return null;
}

/// XKB keycode -> physical key.
pub fn keyFromKeycode(keycode: u32) c.GhosttyKey {
    const i = lookup(KeycodeEntry, &keycodeTable, keycode, "keycode") orelse
        return c.GHOSTTY_KEY_UNIDENTIFIED;
    return keycodeTable[i].key;
}

/// GDK keyval -> logical key, for the writing-system remap and for synthetic
/// keymaps that report every key with one keycode.
pub fn keyFromKeyval(keyval: u32) ?c.GhosttyKey {
    const i = lookup(KeyvalEntry, &keyvalTable, keyval, "keyval") orelse return null;
    return keyvalTable[i].key;
}

pub fn isModifierKeyval(keyval: u32) bool {
    var lo: usize = 0;
    var hi: usize = modifierKeyvals.len;
    while (lo < hi) {
        const mid = lo + (hi - lo) / 2;
        if (modifierKeyvals[mid] < keyval) lo = mid + 1 else hi = mid;
    }
    return lo < modifierKeyvals.len and modifierKeyvals[lo] == keyval;
}

/// Writing-system keys (W3C section 3.1.1): the keyval, not the keycode,
/// decides which key the user meant.
pub fn shouldBeRemappable(key: c.GhosttyKey) bool {
    for (remappableKeys) |k| {
        if (k == key) return true;
    }
    return false;
}

