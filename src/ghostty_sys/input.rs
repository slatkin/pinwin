//! Input encoders: key, mouse and focus (`ghostty/vt/key/*.h`,
//! `mouse/*.h`, `focus.h`).

use std::os::raw::{c_char, c_int, c_void};

use super::terminal::GhosttyTerminal;
use super::{GhosttyAllocator, GhosttyMode, GhosttyResult};

/// Opaque handle to a key encoder (`GhosttyKeyEncoder`, key/encoder.h).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct GhosttyKeyEncoder(pub *mut c_void);

/// Opaque handle to a key event (`GhosttyKeyEvent`, key/event.h).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct GhosttyKeyEvent(pub *mut c_void);

/// Opaque handle to a mouse encoder (`GhosttyMouseEncoder`, mouse/encoder.h).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct GhosttyMouseEncoder(pub *mut c_void);

/// Opaque handle to a mouse event (`GhosttyMouseEvent`, mouse/event.h).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct GhosttyMouseEvent(pub *mut c_void);

/// Key and mouse modifier bits (`GhosttyMods`, key/event.h).
pub type GhosttyMods = u16;

pub const GHOSTTY_MODS_SHIFT: GhosttyMods = 1 << 0;
pub const GHOSTTY_MODS_CTRL: GhosttyMods = 1 << 1;
pub const GHOSTTY_MODS_ALT: GhosttyMods = 1 << 2;
pub const GHOSTTY_MODS_SUPER: GhosttyMods = 1 << 3;
pub const GHOSTTY_MODS_CAPS_LOCK: GhosttyMods = 1 << 4;
pub const GHOSTTY_MODS_NUM_LOCK: GhosttyMods = 1 << 5;
pub const GHOSTTY_MODS_SHIFT_SIDE: GhosttyMods = 1 << 6;
pub const GHOSTTY_MODS_CTRL_SIDE: GhosttyMods = 1 << 7;
pub const GHOSTTY_MODS_ALT_SIDE: GhosttyMods = 1 << 8;
pub const GHOSTTY_MODS_SUPER_SIDE: GhosttyMods = 1 << 9;

/// The kitty keyboard flags stored in the terminal
/// (`GhosttyKittyKeyFlags`, key/encoder.h).
pub type GhosttyKittyKeyFlags = u8;

pub const GHOSTTY_KITTY_KEY_DISABLED: GhosttyKittyKeyFlags = 0;
pub const GHOSTTY_KITTY_KEY_DISAMBIGUATE: GhosttyKittyKeyFlags = 1 << 0;
pub const GHOSTTY_KITTY_KEY_REPORT_EVENTS: GhosttyKittyKeyFlags = 1 << 1;
pub const GHOSTTY_KITTY_KEY_REPORT_ALTERNATES: GhosttyKittyKeyFlags = 1 << 2;
pub const GHOSTTY_KITTY_KEY_REPORT_ALL: GhosttyKittyKeyFlags = 1 << 3;
pub const GHOSTTY_KITTY_KEY_REPORT_ASSOCIATED: GhosttyKittyKeyFlags = 1 << 4;

/// The action a key event describes (`GhosttyKeyAction`, key/event.h).
pub type GhosttyKeyAction = c_int;

pub const GHOSTTY_KEY_ACTION_RELEASE: GhosttyKeyAction = 0;
pub const GHOSTTY_KEY_ACTION_PRESS: GhosttyKeyAction = 1;
pub const GHOSTTY_KEY_ACTION_REPEAT: GhosttyKeyAction = 2;

/// A physical or logical key (`GhosttyKey`, key/event.h).
pub type GhosttyKey = c_int;

pub const GHOSTTY_KEY_UNIDENTIFIED: GhosttyKey = 0;
pub const GHOSTTY_KEY_BACKQUOTE: GhosttyKey = 1;
pub const GHOSTTY_KEY_BACKSLASH: GhosttyKey = 2;
pub const GHOSTTY_KEY_BRACKET_LEFT: GhosttyKey = 3;
pub const GHOSTTY_KEY_BRACKET_RIGHT: GhosttyKey = 4;
pub const GHOSTTY_KEY_COMMA: GhosttyKey = 5;
pub const GHOSTTY_KEY_DIGIT_0: GhosttyKey = 6;
pub const GHOSTTY_KEY_DIGIT_1: GhosttyKey = 7;
pub const GHOSTTY_KEY_DIGIT_2: GhosttyKey = 8;
pub const GHOSTTY_KEY_DIGIT_3: GhosttyKey = 9;
pub const GHOSTTY_KEY_DIGIT_4: GhosttyKey = 10;
pub const GHOSTTY_KEY_DIGIT_5: GhosttyKey = 11;
pub const GHOSTTY_KEY_DIGIT_6: GhosttyKey = 12;
pub const GHOSTTY_KEY_DIGIT_7: GhosttyKey = 13;
pub const GHOSTTY_KEY_DIGIT_8: GhosttyKey = 14;
pub const GHOSTTY_KEY_DIGIT_9: GhosttyKey = 15;
pub const GHOSTTY_KEY_EQUAL: GhosttyKey = 16;
pub const GHOSTTY_KEY_INTL_BACKSLASH: GhosttyKey = 17;
pub const GHOSTTY_KEY_INTL_RO: GhosttyKey = 18;
pub const GHOSTTY_KEY_INTL_YEN: GhosttyKey = 19;
pub const GHOSTTY_KEY_A: GhosttyKey = 20;
pub const GHOSTTY_KEY_B: GhosttyKey = 21;
pub const GHOSTTY_KEY_C: GhosttyKey = 22;
pub const GHOSTTY_KEY_D: GhosttyKey = 23;
pub const GHOSTTY_KEY_E: GhosttyKey = 24;
pub const GHOSTTY_KEY_F: GhosttyKey = 25;
pub const GHOSTTY_KEY_G: GhosttyKey = 26;
pub const GHOSTTY_KEY_H: GhosttyKey = 27;
pub const GHOSTTY_KEY_I: GhosttyKey = 28;
pub const GHOSTTY_KEY_J: GhosttyKey = 29;
pub const GHOSTTY_KEY_K: GhosttyKey = 30;
pub const GHOSTTY_KEY_L: GhosttyKey = 31;
pub const GHOSTTY_KEY_M: GhosttyKey = 32;
pub const GHOSTTY_KEY_N: GhosttyKey = 33;
pub const GHOSTTY_KEY_O: GhosttyKey = 34;
pub const GHOSTTY_KEY_P: GhosttyKey = 35;
pub const GHOSTTY_KEY_Q: GhosttyKey = 36;
pub const GHOSTTY_KEY_R: GhosttyKey = 37;
pub const GHOSTTY_KEY_S: GhosttyKey = 38;
pub const GHOSTTY_KEY_T: GhosttyKey = 39;
pub const GHOSTTY_KEY_U: GhosttyKey = 40;
pub const GHOSTTY_KEY_V: GhosttyKey = 41;
pub const GHOSTTY_KEY_W: GhosttyKey = 42;
pub const GHOSTTY_KEY_X: GhosttyKey = 43;
pub const GHOSTTY_KEY_Y: GhosttyKey = 44;
pub const GHOSTTY_KEY_Z: GhosttyKey = 45;
pub const GHOSTTY_KEY_MINUS: GhosttyKey = 46;
pub const GHOSTTY_KEY_PERIOD: GhosttyKey = 47;
pub const GHOSTTY_KEY_QUOTE: GhosttyKey = 48;
pub const GHOSTTY_KEY_SEMICOLON: GhosttyKey = 49;
pub const GHOSTTY_KEY_SLASH: GhosttyKey = 50;
pub const GHOSTTY_KEY_ALT_LEFT: GhosttyKey = 51;
pub const GHOSTTY_KEY_ALT_RIGHT: GhosttyKey = 52;
pub const GHOSTTY_KEY_BACKSPACE: GhosttyKey = 53;
pub const GHOSTTY_KEY_CAPS_LOCK: GhosttyKey = 54;
pub const GHOSTTY_KEY_CONTEXT_MENU: GhosttyKey = 55;
pub const GHOSTTY_KEY_CONTROL_LEFT: GhosttyKey = 56;
pub const GHOSTTY_KEY_CONTROL_RIGHT: GhosttyKey = 57;
pub const GHOSTTY_KEY_ENTER: GhosttyKey = 58;
pub const GHOSTTY_KEY_META_LEFT: GhosttyKey = 59;
pub const GHOSTTY_KEY_META_RIGHT: GhosttyKey = 60;
pub const GHOSTTY_KEY_SHIFT_LEFT: GhosttyKey = 61;
pub const GHOSTTY_KEY_SHIFT_RIGHT: GhosttyKey = 62;
pub const GHOSTTY_KEY_SPACE: GhosttyKey = 63;
pub const GHOSTTY_KEY_TAB: GhosttyKey = 64;
pub const GHOSTTY_KEY_CONVERT: GhosttyKey = 65;
pub const GHOSTTY_KEY_KANA_MODE: GhosttyKey = 66;
pub const GHOSTTY_KEY_NON_CONVERT: GhosttyKey = 67;
pub const GHOSTTY_KEY_DELETE: GhosttyKey = 68;
pub const GHOSTTY_KEY_END: GhosttyKey = 69;
pub const GHOSTTY_KEY_HELP: GhosttyKey = 70;
pub const GHOSTTY_KEY_HOME: GhosttyKey = 71;
pub const GHOSTTY_KEY_INSERT: GhosttyKey = 72;
pub const GHOSTTY_KEY_PAGE_DOWN: GhosttyKey = 73;
pub const GHOSTTY_KEY_PAGE_UP: GhosttyKey = 74;
pub const GHOSTTY_KEY_ARROW_DOWN: GhosttyKey = 75;
pub const GHOSTTY_KEY_ARROW_LEFT: GhosttyKey = 76;
pub const GHOSTTY_KEY_ARROW_RIGHT: GhosttyKey = 77;
pub const GHOSTTY_KEY_ARROW_UP: GhosttyKey = 78;
pub const GHOSTTY_KEY_NUM_LOCK: GhosttyKey = 79;
pub const GHOSTTY_KEY_NUMPAD_0: GhosttyKey = 80;
pub const GHOSTTY_KEY_NUMPAD_1: GhosttyKey = 81;
pub const GHOSTTY_KEY_NUMPAD_2: GhosttyKey = 82;
pub const GHOSTTY_KEY_NUMPAD_3: GhosttyKey = 83;
pub const GHOSTTY_KEY_NUMPAD_4: GhosttyKey = 84;
pub const GHOSTTY_KEY_NUMPAD_5: GhosttyKey = 85;
pub const GHOSTTY_KEY_NUMPAD_6: GhosttyKey = 86;
pub const GHOSTTY_KEY_NUMPAD_7: GhosttyKey = 87;
pub const GHOSTTY_KEY_NUMPAD_8: GhosttyKey = 88;
pub const GHOSTTY_KEY_NUMPAD_9: GhosttyKey = 89;
pub const GHOSTTY_KEY_NUMPAD_ADD: GhosttyKey = 90;
pub const GHOSTTY_KEY_NUMPAD_BACKSPACE: GhosttyKey = 91;
pub const GHOSTTY_KEY_NUMPAD_CLEAR: GhosttyKey = 92;
pub const GHOSTTY_KEY_NUMPAD_CLEAR_ENTRY: GhosttyKey = 93;
pub const GHOSTTY_KEY_NUMPAD_COMMA: GhosttyKey = 94;
pub const GHOSTTY_KEY_NUMPAD_DECIMAL: GhosttyKey = 95;
pub const GHOSTTY_KEY_NUMPAD_DIVIDE: GhosttyKey = 96;
pub const GHOSTTY_KEY_NUMPAD_ENTER: GhosttyKey = 97;
pub const GHOSTTY_KEY_NUMPAD_EQUAL: GhosttyKey = 98;
pub const GHOSTTY_KEY_NUMPAD_MEMORY_ADD: GhosttyKey = 99;
pub const GHOSTTY_KEY_NUMPAD_MEMORY_CLEAR: GhosttyKey = 100;
pub const GHOSTTY_KEY_NUMPAD_MEMORY_RECALL: GhosttyKey = 101;
pub const GHOSTTY_KEY_NUMPAD_MEMORY_STORE: GhosttyKey = 102;
pub const GHOSTTY_KEY_NUMPAD_MEMORY_SUBTRACT: GhosttyKey = 103;
pub const GHOSTTY_KEY_NUMPAD_MULTIPLY: GhosttyKey = 104;
pub const GHOSTTY_KEY_NUMPAD_PAREN_LEFT: GhosttyKey = 105;
pub const GHOSTTY_KEY_NUMPAD_PAREN_RIGHT: GhosttyKey = 106;
pub const GHOSTTY_KEY_NUMPAD_SUBTRACT: GhosttyKey = 107;
pub const GHOSTTY_KEY_NUMPAD_SEPARATOR: GhosttyKey = 108;
pub const GHOSTTY_KEY_NUMPAD_UP: GhosttyKey = 109;
pub const GHOSTTY_KEY_NUMPAD_DOWN: GhosttyKey = 110;
pub const GHOSTTY_KEY_NUMPAD_RIGHT: GhosttyKey = 111;
pub const GHOSTTY_KEY_NUMPAD_LEFT: GhosttyKey = 112;
pub const GHOSTTY_KEY_NUMPAD_BEGIN: GhosttyKey = 113;
pub const GHOSTTY_KEY_NUMPAD_HOME: GhosttyKey = 114;
pub const GHOSTTY_KEY_NUMPAD_END: GhosttyKey = 115;
pub const GHOSTTY_KEY_NUMPAD_INSERT: GhosttyKey = 116;
pub const GHOSTTY_KEY_NUMPAD_DELETE: GhosttyKey = 117;
pub const GHOSTTY_KEY_NUMPAD_PAGE_UP: GhosttyKey = 118;
pub const GHOSTTY_KEY_NUMPAD_PAGE_DOWN: GhosttyKey = 119;
pub const GHOSTTY_KEY_ESCAPE: GhosttyKey = 120;
pub const GHOSTTY_KEY_F1: GhosttyKey = 121;
pub const GHOSTTY_KEY_F2: GhosttyKey = 122;
pub const GHOSTTY_KEY_F3: GhosttyKey = 123;
pub const GHOSTTY_KEY_F4: GhosttyKey = 124;
pub const GHOSTTY_KEY_F5: GhosttyKey = 125;
pub const GHOSTTY_KEY_F6: GhosttyKey = 126;
pub const GHOSTTY_KEY_F7: GhosttyKey = 127;
pub const GHOSTTY_KEY_F8: GhosttyKey = 128;
pub const GHOSTTY_KEY_F9: GhosttyKey = 129;
pub const GHOSTTY_KEY_F10: GhosttyKey = 130;
pub const GHOSTTY_KEY_F11: GhosttyKey = 131;
pub const GHOSTTY_KEY_F12: GhosttyKey = 132;
pub const GHOSTTY_KEY_F13: GhosttyKey = 133;
pub const GHOSTTY_KEY_F14: GhosttyKey = 134;
pub const GHOSTTY_KEY_F15: GhosttyKey = 135;
pub const GHOSTTY_KEY_F16: GhosttyKey = 136;
pub const GHOSTTY_KEY_F17: GhosttyKey = 137;
pub const GHOSTTY_KEY_F18: GhosttyKey = 138;
pub const GHOSTTY_KEY_F19: GhosttyKey = 139;
pub const GHOSTTY_KEY_F20: GhosttyKey = 140;
pub const GHOSTTY_KEY_F21: GhosttyKey = 141;
pub const GHOSTTY_KEY_F22: GhosttyKey = 142;
pub const GHOSTTY_KEY_F23: GhosttyKey = 143;
pub const GHOSTTY_KEY_F24: GhosttyKey = 144;
pub const GHOSTTY_KEY_F25: GhosttyKey = 145;
pub const GHOSTTY_KEY_FN: GhosttyKey = 146;
pub const GHOSTTY_KEY_FN_LOCK: GhosttyKey = 147;
pub const GHOSTTY_KEY_PRINT_SCREEN: GhosttyKey = 148;
pub const GHOSTTY_KEY_SCROLL_LOCK: GhosttyKey = 149;
pub const GHOSTTY_KEY_PAUSE: GhosttyKey = 150;
pub const GHOSTTY_KEY_BROWSER_BACK: GhosttyKey = 151;
pub const GHOSTTY_KEY_BROWSER_FAVORITES: GhosttyKey = 152;
pub const GHOSTTY_KEY_BROWSER_FORWARD: GhosttyKey = 153;
pub const GHOSTTY_KEY_BROWSER_HOME: GhosttyKey = 154;
pub const GHOSTTY_KEY_BROWSER_REFRESH: GhosttyKey = 155;
pub const GHOSTTY_KEY_BROWSER_SEARCH: GhosttyKey = 156;
pub const GHOSTTY_KEY_BROWSER_STOP: GhosttyKey = 157;
pub const GHOSTTY_KEY_EJECT: GhosttyKey = 158;
pub const GHOSTTY_KEY_LAUNCH_APP_1: GhosttyKey = 159;
pub const GHOSTTY_KEY_LAUNCH_APP_2: GhosttyKey = 160;
pub const GHOSTTY_KEY_LAUNCH_MAIL: GhosttyKey = 161;
pub const GHOSTTY_KEY_MEDIA_PLAY_PAUSE: GhosttyKey = 162;
pub const GHOSTTY_KEY_MEDIA_SELECT: GhosttyKey = 163;
pub const GHOSTTY_KEY_MEDIA_STOP: GhosttyKey = 164;
pub const GHOSTTY_KEY_MEDIA_TRACK_NEXT: GhosttyKey = 165;
pub const GHOSTTY_KEY_MEDIA_TRACK_PREVIOUS: GhosttyKey = 166;
pub const GHOSTTY_KEY_POWER: GhosttyKey = 167;
pub const GHOSTTY_KEY_SLEEP: GhosttyKey = 168;
pub const GHOSTTY_KEY_AUDIO_VOLUME_DOWN: GhosttyKey = 169;
pub const GHOSTTY_KEY_AUDIO_VOLUME_MUTE: GhosttyKey = 170;
pub const GHOSTTY_KEY_AUDIO_VOLUME_UP: GhosttyKey = 171;
pub const GHOSTTY_KEY_WAKE_UP: GhosttyKey = 172;
pub const GHOSTTY_KEY_COPY: GhosttyKey = 173;
pub const GHOSTTY_KEY_CUT: GhosttyKey = 174;
pub const GHOSTTY_KEY_PASTE: GhosttyKey = 175;

/// The action a mouse event describes (`GhosttyMouseAction`, mouse/event.h).
pub type GhosttyMouseAction = c_int;

pub const GHOSTTY_MOUSE_ACTION_PRESS: GhosttyMouseAction = 0;
pub const GHOSTTY_MOUSE_ACTION_RELEASE: GhosttyMouseAction = 1;
pub const GHOSTTY_MOUSE_ACTION_MOTION: GhosttyMouseAction = 2;

/// A mouse button (`GhosttyMouseButton`, mouse/event.h).
pub type GhosttyMouseButton = c_int;

pub const GHOSTTY_MOUSE_BUTTON_UNKNOWN: GhosttyMouseButton = 0;
pub const GHOSTTY_MOUSE_BUTTON_LEFT: GhosttyMouseButton = 1;
pub const GHOSTTY_MOUSE_BUTTON_RIGHT: GhosttyMouseButton = 2;
pub const GHOSTTY_MOUSE_BUTTON_MIDDLE: GhosttyMouseButton = 3;
pub const GHOSTTY_MOUSE_BUTTON_FOUR: GhosttyMouseButton = 4;
pub const GHOSTTY_MOUSE_BUTTON_FIVE: GhosttyMouseButton = 5;
pub const GHOSTTY_MOUSE_BUTTON_SIX: GhosttyMouseButton = 6;
pub const GHOSTTY_MOUSE_BUTTON_SEVEN: GhosttyMouseButton = 7;
pub const GHOSTTY_MOUSE_BUTTON_EIGHT: GhosttyMouseButton = 8;
pub const GHOSTTY_MOUSE_BUTTON_NINE: GhosttyMouseButton = 9;
pub const GHOSTTY_MOUSE_BUTTON_TEN: GhosttyMouseButton = 10;
pub const GHOSTTY_MOUSE_BUTTON_ELEVEN: GhosttyMouseButton = 11;

/// A mouse encoder option id (`GhosttyMouseEncoderOption`, mouse/encoder.h).
pub type GhosttyMouseEncoderOption = c_int;

pub const GHOSTTY_MOUSE_ENCODER_OPT_SIZE: GhosttyMouseEncoderOption = 2;
pub const GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED: GhosttyMouseEncoderOption = 3;

/// A mouse position in terminal surface pixels (`GhosttyMousePosition`,
/// mouse/event.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyMousePosition {
    pub x: f32,
    pub y: f32,
}

/// The mouse encoder's surface size (`GhosttyMouseEncoderSize`,
/// mouse/encoder.h). A sized struct: set `size` to
/// `size_of::<GhosttyMouseEncoderSize>()` first.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyMouseEncoderSize {
    pub size: usize,
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

/// A focus event (`GhosttyFocusEvent`, focus.h).
pub type GhosttyFocusEvent = c_int;

pub const GHOSTTY_FOCUS_GAINED: GhosttyFocusEvent = 0;
pub const GHOSTTY_FOCUS_LOST: GhosttyFocusEvent = 1;

/// The mode number for focus reporting (mode 1004), without the ANSI bit.
pub const GHOSTTY_MODE_FOCUS_EVENT: GhosttyMode = super::ghostty_mode_new(1004, false);

unsafe extern "C" {
    /// Create a key encoder with `allocator`, or the default allocator when
    /// it is NULL (key/encoder.h).
    pub fn ghostty_key_encoder_new(
        allocator: *const GhosttyAllocator,
        encoder: *mut GhosttyKeyEncoder,
    ) -> GhosttyResult;

    /// Copy the encoder options the terminal's current modes imply
    /// (key/encoder.h).
    pub fn ghostty_key_encoder_setopt_from_terminal(
        encoder: GhosttyKeyEncoder,
        terminal: GhosttyTerminal,
    );

    /// Encode `event` into `out_buf`; on `GHOSTTY_OUT_OF_SPACE`, `out_len` is
    /// the required size (key/encoder.h).
    pub fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;

    /// Create a key event with `allocator`, or the default allocator when it
    /// is NULL (key/event.h).
    pub fn ghostty_key_event_new(
        allocator: *const GhosttyAllocator,
        event: *mut GhosttyKeyEvent,
    ) -> GhosttyResult;

    /// Set the key event's action (key/event.h).
    pub fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: GhosttyKeyAction);

    /// Set the key event's key (key/event.h).
    pub fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: GhosttyKey);

    /// Set the modifier bits (key/event.h).
    pub fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: GhosttyMods);

    /// Set the mods the platform already consumed (key/event.h).
    pub fn ghostty_key_event_set_consumed_mods(event: GhosttyKeyEvent, consumed_mods: GhosttyMods);

    /// Set the event's UTF-8 text (key/event.h).
    pub fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: *const c_char, len: usize);

    /// Set the codepoint the key produces without shift (key/event.h).
    pub fn ghostty_key_event_set_unshifted_codepoint(event: GhosttyKeyEvent, codepoint: u32);

    /// Create a mouse encoder with `allocator`, or the default allocator when
    /// it is NULL (mouse/encoder.h).
    pub fn ghostty_mouse_encoder_new(
        allocator: *const GhosttyAllocator,
        encoder: *mut GhosttyMouseEncoder,
    ) -> GhosttyResult;

    /// Set a mouse encoder option; `value`'s type depends on `option`
    /// (mouse/encoder.h).
    pub fn ghostty_mouse_encoder_setopt(
        encoder: GhosttyMouseEncoder,
        option: GhosttyMouseEncoderOption,
        value: *const c_void,
    );

    /// Copy the encoder options the terminal's current modes imply
    /// (mouse/encoder.h).
    pub fn ghostty_mouse_encoder_setopt_from_terminal(
        encoder: GhosttyMouseEncoder,
        terminal: GhosttyTerminal,
    );

    /// Encode `event` into `out_buf`; on `GHOSTTY_OUT_OF_SPACE`, `out_len` is
    /// the required size (mouse/encoder.h).
    pub fn ghostty_mouse_encoder_encode(
        encoder: GhosttyMouseEncoder,
        event: GhosttyMouseEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;

    /// Create a mouse event with `allocator`, or the default allocator when
    /// it is NULL (mouse/event.h).
    pub fn ghostty_mouse_event_new(
        allocator: *const GhosttyAllocator,
        event: *mut GhosttyMouseEvent,
    ) -> GhosttyResult;

    /// Set the mouse event's action (mouse/event.h).
    pub fn ghostty_mouse_event_set_action(event: GhosttyMouseEvent, action: GhosttyMouseAction);

    /// Set the mouse event's button (mouse/event.h).
    pub fn ghostty_mouse_event_set_button(event: GhosttyMouseEvent, button: GhosttyMouseButton);

    /// Clear the mouse event's button, as motion has none (mouse/event.h).
    pub fn ghostty_mouse_event_clear_button(event: GhosttyMouseEvent);

    /// Set the modifier bits (mouse/event.h).
    pub fn ghostty_mouse_event_set_mods(event: GhosttyMouseEvent, mods: GhosttyMods);

    /// Set the mouse position (mouse/event.h).
    pub fn ghostty_mouse_event_set_position(
        event: GhosttyMouseEvent,
        position: GhosttyMousePosition,
    );

    /// Encode a focus event into `buf` (focus.h).
    pub fn ghostty_focus_encode(
        event: GhosttyFocusEvent,
        buf: *mut c_char,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GhosttyResult;

    /// Free a key encoder created with `ghostty_key_encoder_new`
    /// (key/encoder.h).
    pub fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);

    /// Free a key event created with `ghostty_key_event_new` (key/event.h).
    pub fn ghostty_key_event_free(event: GhosttyKeyEvent);

    /// Free a mouse encoder created with `ghostty_mouse_encoder_new`
    /// (mouse/encoder.h).
    pub fn ghostty_mouse_encoder_free(encoder: GhosttyMouseEncoder);

    /// Free a mouse event created with `ghostty_mouse_event_new`
    /// (mouse/event.h).
    pub fn ghostty_mouse_event_free(event: GhosttyMouseEvent);
}
