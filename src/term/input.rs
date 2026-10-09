//! Input-encoding half of pinwin's terminal core (port-to-rust D3): key,
//! mouse, scroll and focus events arrive from the panel thread's seat
//! handlers (`panel::wayland_side::seat`) and are encoded into the pty with
//! libghostty-vt's encoders, exactly as `src/input.zig` did — now through
//! the `libghostty-vt` crate's safe API (adopt-libghostty-rs A6).
//!
//! The encoders read the modes the child set at runtime (kitty keyboard
//! flags, mouse tracking, SGR format) with `set_options_from_terminal`
//! before every event, so no state is cached here. The only state is the
//! scroll notch accumulator and the last focus/button state, which live on
//! the [`Terminal`] so this module keeps no process globals.
//!
//! No windowing-toolkit types appear here: the two toolkit-derived values
//! the key encoder needs (the level-0 "unshifted" codepoint for a keycode
//! and the codepoint a keyval produces) arrive in [`KeyInput`] from the
//! seat's translation, which owns those calls.

use std::cell::RefCell;

use libghostty_vt as vt;

use super::keys::{self, Key};
use super::{PtySink, Terminal};

/// The action a key event describes (`key::Action`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyAction {
    Press,
    Release,
    Repeat,
}

impl KeyAction {
    fn raw(self) -> vt::key::Action {
        match self {
            KeyAction::Press => vt::key::Action::Press,
            KeyAction::Release => vt::key::Action::Release,
            KeyAction::Repeat => vt::key::Action::Repeat,
        }
    }
}

/// The action a mouse event describes (`mouse::Action`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

impl MouseAction {
    fn raw(self) -> vt::mouse::Action {
        match self {
            MouseAction::Press => vt::mouse::Action::Press,
            MouseAction::Release => vt::mouse::Action::Release,
            MouseAction::Motion => vt::mouse::Action::Motion,
        }
    }
}

/// A mouse button (`mouse::Button`). `Unknown` is the button-less motion
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
    fn raw(self) -> Option<vt::mouse::Button> {
        match self {
            MouseButton::Unknown => None,
            MouseButton::Left => Some(vt::mouse::Button::Left),
            MouseButton::Right => Some(vt::mouse::Button::Right),
            MouseButton::Middle => Some(vt::mouse::Button::Middle),
            MouseButton::Four => Some(vt::mouse::Button::Four),
            MouseButton::Five => Some(vt::mouse::Button::Five),
            MouseButton::Six => Some(vt::mouse::Button::Six),
            MouseButton::Seven => Some(vt::mouse::Button::Seven),
            MouseButton::Eight => Some(vt::mouse::Button::Eight),
            MouseButton::Nine => Some(vt::mouse::Button::Nine),
            MouseButton::Ten => Some(vt::mouse::Button::Ten),
            MouseButton::Eleven => Some(vt::mouse::Button::Eleven),
        }
    }
}

/// The unit a scroll delta is reported in: a wheel reports
/// whole notches, a touchpad or high-resolution wheel reports surface units.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollUnit {
    Wheel,
    Surface,
}

/// Keyboard and mouse modifier bits (`key::Mods`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers(u16);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(vt::key::Mods::SHIFT.bits());
    pub const CTRL: Modifiers = Modifiers(vt::key::Mods::CTRL.bits());
    pub const ALT: Modifiers = Modifiers(vt::key::Mods::ALT.bits());
    pub const SUPER: Modifiers = Modifiers(vt::key::Mods::SUPER.bits());
    pub const CAPS_LOCK: Modifiers = Modifiers(vt::key::Mods::CAPS_LOCK.bits());
    /// Only the seat's modifier report sets this bit
    /// (`replace-gtk-with-wayland` D8).
    pub const NUM_LOCK: Modifiers = Modifiers(vt::key::Mods::NUM_LOCK.bits());

    /// The raw modifier bits for the encoder call.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl Modifiers {
    /// The crate's modifier bitmask for the encoders.
    pub(crate) fn raw(self) -> vt::key::Mods {
        vt::key::Mods::from_bits_retain(self.0)
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;

    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

/// One key event as the seat layer observed it. `unshifted_codepoint` and
/// `keyval_unicode` are the two layout lookups the encoder needs; both are 0
/// when the layout has no answer.
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
#[derive(Default, Debug)]
pub struct InputState {
    /// Whether a non-modifier button is currently held (mouse encoder input).
    any_button_pressed: bool,
    /// The focus state last reported to the program. The compositor sends a
    /// leave at map
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
        let Some(handles) = self.handles.as_mut() else {
            return;
        };

        let action = input.action;
        let key = physical_key(input.keycode, input.keyval, input.is_modifier);
        let event = &mut handles.key_event;
        event.set_action(action.raw());
        event.set_key(key.raw());
        event.set_mods(input.mods.raw());
        event.set_consumed_mods(input.consumed_mods.raw());
        // An unshifted codepoint of 0 means the layout had no answer; the
        // encoder's codepoint is a `char`, so such an event skips the setter
        // (adopt-libghostty-rs A6).
        if let Some(unshifted) = char::from_u32(input.unshifted_codepoint) {
            event.set_unshifted_codepoint(unshifted);
        }

        // UTF-8 text: the unmodified character the key produces, never a C0
        // control; the encoder derives modifier sequences from the logical
        // key and mods, not from this text. `set_utf8` makes the event copy
        // the `String` (adopt-libghostty-rs A6 accepts the cost).
        let text = if key != Key::UNIDENTIFIED
            && action != KeyAction::Release
            && (0x20..=0x10_ffff).contains(&input.keyval_unicode)
            && let Some(ch) = char::from_u32(input.keyval_unicode)
        {
            Some(ch.to_string())
        } else {
            None
        };
        event.set_utf8(text);

        handles
            .key_encoder
            .set_options_from_terminal(&handles.terminal);
        write_encoded(&self.shared.sink, |buf| {
            handles.key_encoder.encode(&handles.key_event, buf)
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
        if self.handles.is_none() {
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
        if self.handles.is_none() {
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
        let Some(handles) = self.handles.as_ref() else {
            return;
        };

        if gained == self.input_state.focus_gained {
            return;
        }
        self.input_state.focus_gained = gained;

        let Ok(enabled) = handles.terminal.mode(vt::terminal::Mode::FOCUS_EVENT) else {
            return;
        };
        if !enabled {
            return;
        }

        let mut buf = [0u8; 8];
        let event = if gained {
            vt::focus::Event::Gained
        } else {
            vt::focus::Event::Lost
        };
        if let Ok(written) = event.encode(&mut buf)
            && written > 0
        {
            self.shared.sink.borrow_mut().write_pty(&buf[..written]);
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
        let Some(handles) = self.handles.as_mut() else {
            return;
        };
        let metrics = self.shared.metrics.get();

        handles
            .mouse_encoder
            .set_options_from_terminal(&handles.terminal);
        handles.mouse_encoder.set_size(vt::mouse::EncoderSize {
            screen_width: u32::from(metrics.cols) * metrics.cell_w,
            screen_height: u32::from(metrics.rows) * metrics.cell_h,
            cell_width: metrics.cell_w,
            cell_height: metrics.cell_h,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        });
        handles
            .mouse_encoder
            .set_any_button_pressed(self.input_state.any_button_pressed);

        let event = &mut handles.mouse_event;
        event.set_action(action.raw());
        event.set_button(button.raw());
        event.set_mods(mods.raw());
        // Approved per-instance (#13): event coordinates are
        // screen-bounded; the encoder takes f32.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "approved #13: screen-bounded event coordinates into the f32 encoder ABI"
        )]
        event.set_position(vt::mouse::Position {
            x: x as f32,
            y: y as f32,
        });

        write_encoded(&self.shared.sink, |buf| {
            handles.mouse_encoder.encode(&handles.mouse_event, buf)
        });
    }
}

/// Write an encoded sequence to the pty. The stack buffer takes the common
/// case; on `OutOfSpace { required }` the encode is retried once into an
/// exact-size `Vec` instead of dropping the event (as `input.zig`'s
/// `writeEncoded` did; adopt-libghostty-rs A6 never uses `encode_to_vec`,
/// whose grow path reserves too little).
fn write_encoded(
    sink: &RefCell<Box<dyn PtySink>>,
    mut encode: impl FnMut(&mut [u8]) -> vt::error::Result<usize>,
) {
    let mut buf = [0u8; 256];
    match encode(&mut buf) {
        Ok(written) if written > 0 => {
            sink.borrow_mut().write_pty(&buf[..written]);
        }
        Err(vt::error::Error::OutOfSpace { required }) => {
            let mut exact = vec![0u8; required];
            if let Ok(written) = encode(&mut exact)
                && written > 0
            {
                sink.borrow_mut().write_pty(&exact[..written]);
            }
        }
        _ => {}
    }
}

/// The physical key for a key event, following Ghostty's apprt keymap: the
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
    use crate::term::{DecodedPng, PngDecoder};
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
            RecordingSink(Arc::clone(&writes)),
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
        assert_eq!(take(&writes), Vec::new());
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
            RecordingSink(Arc::clone(&writes)),
            NoDecoder,
            || {},
        );
        terminal.push_key(key(KeyAction::Press));
        assert_eq!(take(&writes), Vec::new());
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
        assert_eq!(take(&writes), Vec::new());
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
        assert_eq!(take(&writes), Vec::new());
        terminal.push_focus(false);
        assert_eq!(take(&writes), b"\x1b[O");

        let (mut terminal, writes) = new_terminal();
        terminal.push_focus(true);
        assert_eq!(take(&writes), Vec::new());
    }
}
