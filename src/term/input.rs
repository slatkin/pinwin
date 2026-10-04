//! Input-encoding half of pinwin's terminal core (port-to-rust D3): key,
//! mouse, scroll and focus events arrive from the GTK controllers (row 3.7)
//! and are encoded into the pty with libghostty-vt's encoders, exactly as
//! `src/input.zig` did.
//!
//! The encoders read the modes the child set at runtime (kitty keyboard
//! flags, mouse tracking, SGR format) with a `setopt_from_terminal` before
//! every event, so no state is cached here. The only state is the scroll
//! notch accumulator and the last focus/button state, which live on the
//! [`Terminal`] so this module keeps no process globals.
//!
//! No GTK/GDK types appear here: the two GDK-derived values the key encoder
//! needs (the level-0 "unshifted" codepoint for a keycode and the codepoint a
//! keyval produces) arrive in [`KeyInput`] from the GTK layer, which owns
//! those calls.

use std::os::raw::c_char;

use super::Terminal;
use super::keys::{self, Key};
use crate::ghostty_sys::input::{
    GHOSTTY_FOCUS_GAINED, GHOSTTY_FOCUS_LOST, GHOSTTY_KEY_ACTION_PRESS, GHOSTTY_KEY_ACTION_RELEASE,
    GHOSTTY_KEY_ACTION_REPEAT, GHOSTTY_MODE_FOCUS_EVENT, GHOSTTY_MOUSE_ACTION_MOTION,
    GHOSTTY_MOUSE_ACTION_PRESS, GHOSTTY_MOUSE_ACTION_RELEASE, GHOSTTY_MOUSE_BUTTON_EIGHT,
    GHOSTTY_MOUSE_BUTTON_ELEVEN, GHOSTTY_MOUSE_BUTTON_FIVE, GHOSTTY_MOUSE_BUTTON_FOUR,
    GHOSTTY_MOUSE_BUTTON_LEFT, GHOSTTY_MOUSE_BUTTON_MIDDLE, GHOSTTY_MOUSE_BUTTON_NINE,
    GHOSTTY_MOUSE_BUTTON_RIGHT, GHOSTTY_MOUSE_BUTTON_SEVEN, GHOSTTY_MOUSE_BUTTON_SIX,
    GHOSTTY_MOUSE_BUTTON_TEN, GHOSTTY_MOUSE_BUTTON_UNKNOWN,
    GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED, GHOSTTY_MOUSE_ENCODER_OPT_SIZE, GhosttyKeyAction,
    GhosttyMouseAction, GhosttyMouseButton, GhosttyMouseEncoderSize, GhosttyMousePosition,
    ghostty_focus_encode, ghostty_key_encoder_encode, ghostty_key_encoder_setopt_from_terminal,
    ghostty_key_event_set_action, ghostty_key_event_set_consumed_mods, ghostty_key_event_set_key,
    ghostty_key_event_set_mods, ghostty_key_event_set_unshifted_codepoint,
    ghostty_key_event_set_utf8, ghostty_mouse_encoder_encode, ghostty_mouse_encoder_setopt,
    ghostty_mouse_encoder_setopt_from_terminal, ghostty_mouse_event_clear_button,
    ghostty_mouse_event_set_action, ghostty_mouse_event_set_button, ghostty_mouse_event_set_mods,
    ghostty_mouse_event_set_position,
};
use crate::ghostty_sys::terminal::{
    GHOSTTY_TERMINAL_DATA_MODE, GhosttyTerminalModeConfig, ghostty_terminal_get,
};
use crate::ghostty_sys::{GHOSTTY_OUT_OF_SPACE, GHOSTTY_SUCCESS, GhosttyResult};

/// The action a key event describes (`GhosttyKeyAction`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyAction {
    Press,
    Release,
    Repeat,
}

impl KeyAction {
    fn raw(self) -> GhosttyKeyAction {
        match self {
            KeyAction::Press => GHOSTTY_KEY_ACTION_PRESS,
            KeyAction::Release => GHOSTTY_KEY_ACTION_RELEASE,
            KeyAction::Repeat => GHOSTTY_KEY_ACTION_REPEAT,
        }
    }
}

/// The action a mouse event describes (`GhosttyMouseAction`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

impl MouseAction {
    fn raw(self) -> GhosttyMouseAction {
        match self {
            MouseAction::Press => GHOSTTY_MOUSE_ACTION_PRESS,
            MouseAction::Release => GHOSTTY_MOUSE_ACTION_RELEASE,
            MouseAction::Motion => GHOSTTY_MOUSE_ACTION_MOTION,
        }
    }
}

/// A mouse button (`GhosttyMouseButton`). `Unknown` is the button-less motion
/// case.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Unknown,
    Left,
    Right,
    Middle,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Eleven,
}

impl MouseButton {
    fn raw(self) -> GhosttyMouseButton {
        match self {
            MouseButton::Unknown => GHOSTTY_MOUSE_BUTTON_UNKNOWN,
            MouseButton::Left => GHOSTTY_MOUSE_BUTTON_LEFT,
            MouseButton::Right => GHOSTTY_MOUSE_BUTTON_RIGHT,
            MouseButton::Middle => GHOSTTY_MOUSE_BUTTON_MIDDLE,
            MouseButton::Four => GHOSTTY_MOUSE_BUTTON_FOUR,
            MouseButton::Five => GHOSTTY_MOUSE_BUTTON_FIVE,
            MouseButton::Six => GHOSTTY_MOUSE_BUTTON_SIX,
            MouseButton::Seven => GHOSTTY_MOUSE_BUTTON_SEVEN,
            MouseButton::Eight => GHOSTTY_MOUSE_BUTTON_EIGHT,
            MouseButton::Nine => GHOSTTY_MOUSE_BUTTON_NINE,
            MouseButton::Ten => GHOSTTY_MOUSE_BUTTON_TEN,
            MouseButton::Eleven => GHOSTTY_MOUSE_BUTTON_ELEVEN,
        }
    }
}

/// The unit a scroll delta is reported in (`GdkScrollUnit`): a wheel reports
/// whole notches, a touchpad or high-resolution wheel reports surface units.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollUnit {
    Wheel,
    Surface,
}

/// Keyboard and mouse modifier bits (`GhosttyMods`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers(u16);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(crate::ghostty_sys::input::GHOSTTY_MODS_SHIFT);
    pub const CTRL: Modifiers = Modifiers(crate::ghostty_sys::input::GHOSTTY_MODS_CTRL);
    pub const ALT: Modifiers = Modifiers(crate::ghostty_sys::input::GHOSTTY_MODS_ALT);
    pub const SUPER: Modifiers = Modifiers(crate::ghostty_sys::input::GHOSTTY_MODS_SUPER);
    pub const CAPS_LOCK: Modifiers = Modifiers(crate::ghostty_sys::input::GHOSTTY_MODS_CAPS_LOCK);

    /// The raw modifier bits for the FFI call.
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;

    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

/// One key event as the GTK layer observed it. `unshifted_codepoint` and
/// `keyval_unicode` are the two GDK lookups the encoder needs; both are 0 when
/// the layout has no answer.
#[derive(Clone, Copy, Debug)]
pub struct KeyInput {
    pub action: KeyAction,
    pub keyval: u32,
    pub keycode: u32,
    pub mods: Modifiers,
    pub consumed_mods: Modifiers,
    pub is_modifier: bool,
    /// The level-0 codepoint for the hardware keycode (kitty's "unshifted").
    pub unshifted_codepoint: u32,
    /// The codepoint the keyval produces, 0 for none.
    pub keyval_unicode: u32,
}

/// Per-terminal input state that `src/input.zig` kept in process globals.
#[derive(Default)]
pub struct InputState {
    /// Whether a non-modifier button is currently held (mouse encoder input).
    any_button_pressed: bool,
    /// The focus state last reported to the program. GTK sends a leave at map
    /// time, before any enter; programs expect balanced reports, so only
    /// actual changes are reported.
    focus_gained: bool,
    /// Left/right, up/down remainders of a scroll in notches.
    scroll_acc: [f64; 2],
}

impl Terminal {
    /// Encode a key event into the pty. Before the terminal exists this is a
    /// no-op (as `main.term == null` was).
    pub fn push_key(&mut self, input: KeyInput) {
        let Some(terminal) = self.terminal() else {
            return;
        };
        let (Some(encoder), Some(event)) = (self.key_encoder(), self.key_event()) else {
            return;
        };

        let action = input.action;
        let key = physical_key(input.keycode, input.keyval, input.is_modifier);
        // SAFETY: the event handle is live and every value is the plain data
        // the setters expect.
        unsafe {
            ghostty_key_event_set_action(event, action.raw());
            ghostty_key_event_set_key(event, key.raw());
            ghostty_key_event_set_mods(event, input.mods.bits());
            ghostty_key_event_set_consumed_mods(event, input.consumed_mods.bits());
            ghostty_key_event_set_unshifted_codepoint(event, input.unshifted_codepoint);
        }

        let mut text = [0u8; 8];
        let mut text_len = 0usize;
        let codepoint = input.keyval_unicode;
        if key != Key::UNIDENTIFIED
            && action != KeyAction::Release
            && (0x20..=0x10_ffff).contains(&codepoint)
            && let Some(ch) = char::from_u32(codepoint)
        {
            text_len = ch.encode_utf8(&mut text).len();
        }
        // SAFETY: `text` is valid for `text_len` bytes.
        unsafe {
            ghostty_key_event_set_utf8(event, text.as_ptr().cast(), text_len);
            ghostty_key_encoder_setopt_from_terminal(encoder, terminal);
        }

        self.write_encoded(|out, len, written| unsafe {
            ghostty_key_encoder_encode(encoder, event, out, len, written)
        });
    }

    /// Encode a mouse button event. `x`/`y` are terminal surface pixels.
    pub fn push_mouse(
        &mut self,
        action: MouseAction,
        x: f64,
        y: f64,
        button: MouseButton,
        mods: Modifiers,
    ) {
        if self.terminal().is_none() {
            return;
        }
        if action == MouseAction::Press && button != MouseButton::Unknown {
            self.input_state.any_button_pressed = true;
        }
        if action == MouseAction::Release {
            self.input_state.any_button_pressed = false;
        }
        self.send_mouse(action, x, y, button, mods);
    }

    /// Encode a scroll delta. A wheel notch is a mouse press with button 4
    /// (up), 5 (down), 6 (left) or 7 (right) in the protocols terminals use,
    /// so it goes through the mouse encoder like any other button.
    ///
    /// A wheel device reports one notch as one unit; touchpads and
    /// free-spinning or high-resolution wheels report surface units, which
    /// count a tenth of a notch each so a single swipe cannot flood the
    /// program with wheel events.
    pub fn push_scroll(
        &mut self,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        unit: ScrollUnit,
        mods: Modifiers,
    ) {
        if self.terminal().is_none() {
            return;
        }

        let scale = if unit == ScrollUnit::Surface {
            0.1
        } else {
            1.0
        };
        self.input_state.scroll_acc[0] += dx * scale;
        self.input_state.scroll_acc[1] += dy * scale;

        while self.input_state.scroll_acc[1] >= 1.0 {
            self.input_state.scroll_acc[1] -= 1.0;
            self.send_mouse(MouseAction::Press, x, y, MouseButton::Five, mods);
        }
        while self.input_state.scroll_acc[1] <= -1.0 {
            self.input_state.scroll_acc[1] += 1.0;
            self.send_mouse(MouseAction::Press, x, y, MouseButton::Four, mods);
        }
        while self.input_state.scroll_acc[0] >= 1.0 {
            self.input_state.scroll_acc[0] -= 1.0;
            self.send_mouse(MouseAction::Press, x, y, MouseButton::Seven, mods);
        }
        while self.input_state.scroll_acc[0] <= -1.0 {
            self.input_state.scroll_acc[0] += 1.0;
            self.send_mouse(MouseAction::Press, x, y, MouseButton::Six, mods);
        }
    }

    /// Report a focus change. Focus reports are sent only when the program
    /// enabled mode 1004; the encoder has no terminal to ask, so the mode is
    /// checked here.
    pub fn push_focus(&mut self, gained: bool) {
        let Some(terminal) = self.terminal() else {
            return;
        };

        if gained == self.input_state.focus_gained {
            return;
        }
        self.input_state.focus_gained = gained;

        let mut config = GhosttyTerminalModeConfig {
            mode: GHOSTTY_MODE_FOCUS_EVENT,
            value: false,
        };
        // SAFETY: the terminal is live and `config` is the struct the
        // `GHOSTTY_TERMINAL_DATA_MODE` query writes.
        let result = unsafe {
            ghostty_terminal_get(
                terminal,
                GHOSTTY_TERMINAL_DATA_MODE,
                (&mut config as *mut GhosttyTerminalModeConfig).cast(),
            )
        };
        if result != GHOSTTY_SUCCESS || !config.value {
            return;
        }

        let mut buf = [0u8; 8];
        let mut written = 0usize;
        let event = if gained {
            GHOSTTY_FOCUS_GAINED
        } else {
            GHOSTTY_FOCUS_LOST
        };
        // SAFETY: `buf` is an 8-byte buffer and `written` is a valid out
        // pointer.
        let result = unsafe {
            ghostty_focus_encode(event, buf.as_mut_ptr().cast(), buf.len(), &mut written)
        };
        if result == GHOSTTY_SUCCESS && written > 0 {
            self.ctx.sink.write_pty(&buf[..written]);
        }
    }

    /// Encode one mouse event; the caller has already updated
    /// `any_button_pressed`.
    fn send_mouse(
        &mut self,
        action: MouseAction,
        x: f64,
        y: f64,
        button: MouseButton,
        mods: Modifiers,
    ) {
        let Some(terminal) = self.terminal() else {
            return;
        };
        let (Some(encoder), Some(event)) = (self.mouse_encoder(), self.mouse_event()) else {
            return;
        };

        let mut size = GhosttyMouseEncoderSize {
            size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
            screen_width: u32::from(self.ctx.cols) * self.ctx.cell_w,
            screen_height: u32::from(self.ctx.rows) * self.ctx.cell_h,
            cell_width: self.ctx.cell_w,
            cell_height: self.ctx.cell_h,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        let mut pressed = self.input_state.any_button_pressed;
        // SAFETY: both handles are live; `size` and `pressed` are the sized
        // values the option ids expect.
        unsafe {
            ghostty_mouse_encoder_setopt_from_terminal(encoder, terminal);
            ghostty_mouse_encoder_setopt(
                encoder,
                GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&mut size as *mut GhosttyMouseEncoderSize).cast(),
            );
            ghostty_mouse_encoder_setopt(
                encoder,
                GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                (&mut pressed as *mut bool).cast(),
            );
            ghostty_mouse_event_set_action(event, action.raw());
            if button == MouseButton::Unknown {
                ghostty_mouse_event_clear_button(event);
            } else {
                ghostty_mouse_event_set_button(event, button.raw());
            }
            ghostty_mouse_event_set_mods(event, mods.bits());
            ghostty_mouse_event_set_position(
                event,
                GhosttyMousePosition {
                    x: x as f32,
                    y: y as f32,
                },
            );
        }

        self.write_encoded(|out, len, written| unsafe {
            ghostty_mouse_encoder_encode(encoder, event, out, len, written)
        });
    }

    /// Write an encoded sequence to the pty. `written` becomes the required
    /// size on `GHOSTTY_OUT_OF_SPACE`, so retry once with an exact-size buffer
    /// instead of dropping the event (as `input.zig`'s `writeEncoded` did).
    fn write_encoded(
        &mut self,
        mut encode: impl FnMut(*mut c_char, usize, *mut usize) -> GhosttyResult,
    ) {
        let mut buf = [0u8; 256];
        let mut written = 0usize;
        let mut result = encode(buf.as_mut_ptr().cast(), buf.len(), &mut written);
        if result == GHOSTTY_OUT_OF_SPACE {
            let mut exact = vec![0u8; written];
            result = encode(exact.as_mut_ptr().cast(), exact.len(), &mut written);
            if result == GHOSTTY_SUCCESS && written > 0 {
                self.ctx.sink.write_pty(&exact[..written]);
            }
            return;
        }
        if result == GHOSTTY_SUCCESS && written > 0 {
            self.ctx.sink.write_pty(&buf[..written]);
        }
    }
}

/// The physical key for a key event, following Ghostty's GTK apprt: the
/// keycode names the key, writing-system keys are remapped from the keyval (a
/// layout or a synthetic keymap can put any character on one keycode), and
/// modifier keyvals never encode.
fn physical_key(keycode: u32, keyval: u32, is_modifier: bool) -> Key {
    let mut key = keys::key_from_keycode(keycode);
    if let Some(remapped) = keys::key_from_keyval(keyval)
        && (keys::should_be_remappable(key) || keys::should_be_remappable(remapped))
    {
        key = remapped;
    }
    if is_modifier || keys::is_modifier_keyval(keyval) {
        return Key::UNIDENTIFIED;
    }
    key
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::{DecodedPng, PngDecoder, PtySink};
    use std::sync::{Arc, Mutex};

    /// Records every pty write so a test can assert the encoded bytes.
    struct RecordingSink(Arc<Mutex<Vec<u8>>>);

    impl PtySink for RecordingSink {
        fn write_pty(&mut self, data: &[u8]) {
            self.0.lock().expect("sink lock").extend_from_slice(data);
        }
    }

    /// Rejects every image; these tests do not exercise PNG decoding.
    struct NoDecoder;

    impl PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
            None
        }
    }

    /// A terminal sized 40x24 at 8x16 pixels, plus the bytes it wrote.
    fn new_terminal() -> (Terminal, Arc<Mutex<Vec<u8>>>) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(
            crate::guard::Poisoned::new(),
            RecordingSink(writes.clone()),
            NoDecoder,
            || {},
        );
        assert!(terminal.push_size(40, 24, 8, 16));
        (terminal, writes)
    }

    /// Take everything written since the last call.
    fn take(writes: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
        std::mem::take(&mut *writes.lock().expect("sink lock"))
    }

    /// A key event for `a`, no modifiers, with the given action.
    fn key(action: KeyAction) -> KeyInput {
        KeyInput {
            action,
            keyval: u32::from('a'),
            keycode: 0x26,
            mods: Modifiers::NONE,
            consumed_mods: Modifiers::NONE,
            is_modifier: false,
            unshifted_codepoint: u32::from('a'),
            keyval_unicode: u32::from('a'),
        }
    }

    /// A plain 'a' press, no kitty flags: legacy text encoding.
    #[test]
    fn plain_key_press_encodes_text() {
        let (mut terminal, writes) = new_terminal();
        terminal.push_key(key(KeyAction::Press));
        assert_eq!(take(&writes), b"a");
    }

    /// Shift+a encodes the shifted text, not the base key.
    #[test]
    fn shifted_key_press_encodes_shifted_text() {
        let (mut terminal, writes) = new_terminal();
        terminal.push_key(KeyInput {
            keyval: u32::from('A'),
            mods: Modifiers::SHIFT,
            consumed_mods: Modifiers::SHIFT,
            keyval_unicode: u32::from('A'),
            ..key(KeyAction::Press)
        });
        assert_eq!(take(&writes), b"A");
    }

    /// A release with no kitty report-events flag writes nothing.
    #[test]
    fn release_without_kitty_report_events_writes_nothing() {
        let (mut terminal, writes) = new_terminal();
        terminal.push_key(key(KeyAction::Release));
        assert!(take(&writes).is_empty());
    }

    /// The encoder picks up the terminal's kitty keyboard flags per event:
    /// report-all turns a press into `CSI 97 u` and report-events turns a
    /// release into `CSI 97 ; 1 : 3 u`.
    #[test]
    fn kitty_keyboard_flags_change_encoding() {
        let (mut terminal, writes) = new_terminal();
        terminal.push_pty_data(b"\x1b[>8u");
        terminal.push_key(key(KeyAction::Press));
        assert_eq!(take(&writes), b"\x1b[97u");

        let (mut terminal, writes) = new_terminal();
        terminal.push_pty_data(b"\x1b[>3u");
        terminal.push_key(key(KeyAction::Release));
        assert_eq!(take(&writes), b"\x1b[97;1:3u");
    }

    /// A key event before the terminal exists is dropped.
    #[test]
    fn key_before_init_is_dropped() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(
            crate::guard::Poisoned::new(),
            RecordingSink(writes.clone()),
            NoDecoder,
            || {},
        );
        terminal.push_key(key(KeyAction::Press));
        assert!(take(&writes).is_empty());
    }

    /// Mouse press/release/motion in SGR cells: the left button at (24,16)
    /// pixels is column 4 row 2, motion has the motion bit and the shift
    /// modifiers.
    #[test]
    fn mouse_press_release_motion_use_sgr() {
        let (mut terminal, writes) = new_terminal();
        terminal.push_pty_data(b"\x1b[?1003h\x1b[?1006h");
        terminal.push_mouse(
            MouseAction::Press,
            24.0,
            16.0,
            MouseButton::Left,
            Modifiers::NONE,
        );
        assert_eq!(take(&writes), b"\x1b[<0;4;2M");
        terminal.push_mouse(
            MouseAction::Release,
            24.0,
            16.0,
            MouseButton::Left,
            Modifiers::NONE,
        );
        assert_eq!(take(&writes), b"\x1b[<0;4;2m");
        terminal.push_mouse(
            MouseAction::Motion,
            8.0,
            32.0,
            MouseButton::Unknown,
            Modifiers::SHIFT,
        );
        assert_eq!(take(&writes), b"\x1b[<39;2;3M");
    }

    /// A wheel notch is one press of buttons 5/4/7/6, and surface units
    /// coalesce a tenth of a notch each until a whole one is due.
    #[test]
    fn scroll_notches_and_surface_coalescing() {
        let (mut terminal, writes) = new_terminal();
        terminal.push_pty_data(b"\x1b[?1000h\x1b[?1006h");
        terminal.push_scroll(24.0, 16.0, 0.0, 1.0, ScrollUnit::Wheel, Modifiers::NONE);
        assert_eq!(take(&writes), b"\x1b[<65;4;2M");
        terminal.push_scroll(24.0, 16.0, 0.0, -1.0, ScrollUnit::Wheel, Modifiers::NONE);
        assert_eq!(take(&writes), b"\x1b[<64;4;2M");
        terminal.push_scroll(24.0, 16.0, 1.0, 0.0, ScrollUnit::Wheel, Modifiers::NONE);
        assert_eq!(take(&writes), b"\x1b[<67;4;2M");
        terminal.push_scroll(24.0, 16.0, -1.0, 0.0, ScrollUnit::Wheel, Modifiers::NONE);
        assert_eq!(take(&writes), b"\x1b[<66;4;2M");

        terminal.push_scroll(24.0, 16.0, 0.0, 5.0, ScrollUnit::Surface, Modifiers::NONE);
        assert!(take(&writes).is_empty());
        terminal.push_scroll(24.0, 16.0, 0.0, 5.0, ScrollUnit::Surface, Modifiers::NONE);
        assert_eq!(take(&writes), b"\x1b[<65;4;2M");
    }

    /// Focus reports need mode 1004; without it nothing is written and only
    /// actual changes are reported.
    #[test]
    fn focus_reports_are_gated_by_mode_1004() {
        let (mut terminal, writes) = new_terminal();
        terminal.push_pty_data(b"\x1b[?1004h");
        terminal.push_focus(true);
        assert_eq!(take(&writes), b"\x1b[I");
        terminal.push_focus(true);
        assert!(take(&writes).is_empty());
        terminal.push_focus(false);
        assert_eq!(take(&writes), b"\x1b[O");

        let (mut terminal, writes) = new_terminal();
        terminal.push_focus(true);
        assert!(take(&writes).is_empty());
    }
}
