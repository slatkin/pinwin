//! The keyboard half of the seat side (replace-gtk-with-wayland D8, row
//! 5.1): the key events the toolkit's `KeyboardHandler` delivers are
//! translated into the same [`KeyInput`] the GDK path builds, from a keymap
//! and state of pinwin's own — the toolkit's `KeyEvent` carries neither the
//! consumed modifiers nor the level-0 keysym `on_key` needs.
//!
//! The row 8.1 wiring calls [`KeyboardSide`] from the `KeyboardHandler` impl
//! the dispatch state carries; the type itself holds no Wayland objects, so
//! every rule here is testable without a display (`port-to-rust` D10).

use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, Modifiers as SctkModifiers, RawModifiers, RepeatInfo,
};

use crate::term::input::{KeyAction, KeyInput, Modifiers};

use super::xkb::XkbKeyboard;

/// The keyboard side's translation state: the xkb keymap and state once the
/// compositor's keymap string has arrived, the modifier bits the last
/// modifiers event reported, and the repeat gate (row 5.2).
#[derive(Debug, Default)]
pub struct KeyboardSide {
    /// `None` until the first `update_keymap`: the keymap precedes the key
    /// events on the wire, so a key event before it is dropped like the GDK
    /// path drops a key with no display to look the keycode up in.
    xkb: Option<XkbKeyboard>,
    /// The encoder's modifier bits of the last modifiers event.
    mods: Modifiers,
    /// The last raw modifiers set and layout, re-applied to a new keymap so
    /// a mid-session keymap update keeps the held modifiers active.
    last_modifiers: Option<(RawModifiers, u32)>,
    /// The repeat gate between the toolkit's calloop repeat and the terminal
    /// (row 5.2).
    repeat: RepeatTracker,
}

impl KeyboardSide {
    /// A keyboard side with no keymap and no modifiers held.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The modifier bits of the last modifiers event: what a key event
    /// carries, and what the pointer row's mouse and scroll encoders will
    /// report.
    #[must_use]
    pub fn mods(&self) -> Modifiers {
        self.mods
    }

    /// One keymap update from the compositor (D8): recompile the xkb keymap
    /// and state, reusing the context once one exists. A string that does
    /// not compile keeps the previous keymap; the held raw modifiers are
    /// re-applied to the new state either way.
    pub fn keymap_updated(&mut self, keymap: &str) {
        match self.xkb.as_mut() {
            Some(xkb) => {
                if xkb.recompile(keymap) {
                    self.reapply_modifiers();
                }
            }
            None => {
                if let Some(xkb) = XkbKeyboard::from_string(keymap) {
                    self.xkb = Some(xkb);
                    self.reapply_modifiers();
                }
            }
        }
    }

    /// Re-apply the last raw modifiers to the current xkb state, so a
    /// keymap change keeps the held modifiers active.
    fn reapply_modifiers(&mut self) {
        if let Some(xkb) = self.xkb.as_mut()
            && let Some((raw, layout)) = self.last_modifiers
        {
            xkb.update_modifiers(raw, layout);
        }
    }

    /// One modifiers event (D8): the raw modifier set and layout feed the
    /// xkb state, and the toolkit's interpreted modifier flags become the
    /// encoder's bits.
    pub fn modifiers_updated(&mut self, raw: RawModifiers, layout: u32, mods: SctkModifiers) {
        self.last_modifiers = Some((raw, layout));
        if let Some(xkb) = &mut self.xkb {
            xkb.update_modifiers(raw, layout);
        }
        self.mods = mods_from_sctk(mods);
    }

    /// A key press: the [`KeyInput`] the GDK path builds, and the repeat
    /// gate arms when the key is no modifier and the keymap repeats it.
    #[must_use]
    pub fn pressed(&mut self, event: &KeyEvent) -> Option<KeyInput> {
        let input = self.key_input(KeyAction::Press, event)?;
        let repeats = self
            .xkb
            .as_ref()
            .is_some_and(|xkb| xkb.key_repeats(event.raw_code));
        self.repeat
            .on_press(event.raw_code, input.is_modifier, repeats);
        Some(input)
    }

    /// A key release: the [`KeyInput`], which the encoder sends only when
    /// the child asked for release reports; the repeat gate stops repeating
    /// the released key.
    #[must_use]
    pub fn released(&mut self, event: &KeyEvent) -> Option<KeyInput> {
        self.repeat.on_release(event.raw_code);
        self.key_input(KeyAction::Release, event)
    }

    /// A repeat from the toolkit's calloop repeat source (row 5.2): the
    /// [`KeyInput`] when the gate still has the key armed, `None` when the
    /// repeat must not reach the terminal — released, left, a modifier, or
    /// a compositor repeat rate of 0.
    #[must_use]
    pub fn repeated(&mut self, event: &KeyEvent) -> Option<KeyInput> {
        if self.repeat.accepts_repeat(event.raw_code) {
            self.key_input(KeyAction::Repeat, event)
        } else {
            None
        }
    }

    /// The keyboard left the surface (row 5.2): the repeat stops.
    pub fn left(&mut self) {
        self.repeat.on_leave();
    }

    /// One `repeat_info` update (row 5.2): the compositor's rate decides
    /// whether any key repeats; a rate of 0 disables it.
    pub fn repeat_info_updated(&mut self, info: &RepeatInfo) {
        self.repeat
            .set_enabled(matches!(info, RepeatInfo::Repeat { .. }));
    }

    /// Build the encoder's view of one key event (D8): the same
    /// [`KeyInput`] the GDK path's `on_key` fills, from the xkb state's
    /// lookups and the event's own keysym. `None` while no keymap has
    /// arrived.
    #[must_use]
    fn key_input(&self, action: KeyAction, event: &KeyEvent) -> Option<KeyInput> {
        let xkb = self.xkb.as_ref()?;
        let facts = xkb.facts(event.raw_code, event.keysym.raw());
        Some(KeyInput {
            action,
            keyval: facts.keyval,
            keycode: facts.keycode,
            mods: self.mods,
            consumed_mods: facts.consumed_mods,
            is_modifier: crate::term::keys::is_modifier_keyval(facts.keyval),
            unshifted_codepoint: facts.unshifted_codepoint,
            keyval_unicode: facts.keyval_unicode,
        })
    }
}

/// Map the toolkit's interpreted modifier flags onto the encoder's bits.
/// Every flag the toolkit reports has an encoder bit, num lock included —
/// the GDK path's `mods_from_gdk` has no num-lock mask and drops it, so the
/// seat path reports one modifier more.
#[must_use]
pub fn mods_from_sctk(mods: SctkModifiers) -> Modifiers {
    let mut out = Modifiers::NONE;
    if mods.shift {
        out = out | Modifiers::SHIFT;
    }
    if mods.ctrl {
        out = out | Modifiers::CTRL;
    }
    if mods.alt {
        out = out | Modifiers::ALT;
    }
    if mods.logo {
        out = out | Modifiers::SUPER;
    }
    if mods.caps_lock {
        out = out | Modifiers::CAPS_LOCK;
    }
    if mods.num_lock {
        out = out | Modifiers::NUM_LOCK;
    }
    out
}

/// The repeat gate between the toolkit's calloop repeat and the terminal
/// (row 5.2). The toolkit schedules the repeats and stops them on release
/// and on keyboard leave itself; this gate re-checks the same rules so a
/// repeat only reaches the terminal while its key is armed, is no modifier,
/// repeats under the keymap, and the compositor's repeat rate is non-zero.
#[derive(Debug, Default)]
pub struct RepeatTracker {
    /// The raw keycode of the armed key, if any.
    armed: Option<u32>,
    /// Whether the compositor's repeat rate is non-zero (D8: a rate of 0
    /// means no key repeats). `false` until the first `repeat_info` arrives,
    /// the toolkit's own default.
    enabled: bool,
}

impl RepeatTracker {
    /// Arm (or re-arm) the gate for a key press. A modifier key and a key
    /// the keymap does not repeat never arm it.
    pub fn on_press(&mut self, raw_code: u32, is_modifier: bool, repeats: bool) {
        self.armed = (!is_modifier && repeats).then_some(raw_code);
    }

    /// A release stops the repeat of the released key; another key's repeat
    /// (the toolkit repeats one key at a time) is untouched.
    pub fn on_release(&mut self, raw_code: u32) {
        if self.armed == Some(raw_code) {
            self.armed = None;
        }
    }

    /// A keyboard leave stops any repeat.
    pub fn on_leave(&mut self) {
        self.armed = None;
    }

    /// The compositor's repeat rate changed; a rate of 0 disables repeat.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether a repeat of this key may reach the terminal.
    #[must_use]
    pub fn accepts_repeat(&self, raw_code: u32) -> bool {
        self.enabled && self.armed == Some(raw_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xkbcommon::xkb::Keysym;

    /// The hermetic keymap from the xkb module's tests, through the keyboard
    /// side's own keymap path.
    const TEST_KEYMAP: &str = r#"
xkb_keymap {
    xkb_keycodes "pinwin" {
        minimum = 8;
        maximum = 255;
        <Q> = 24;
        <LCTL> = 37;
        <LFSH> = 50;
    };
    xkb_types "pinwin" {
        type "TWO_LEVEL" {
            modifiers = Shift;
            map[Shift] = 2;
            level_name[Level1] = "Base";
            level_name[Level2] = "Shift";
        };
    };
    xkb_compat "pinwin" {
    };
    xkb_symbols "pinwin" {
        key <Q> { type = "TWO_LEVEL", [ q, Q ] };
        key <LCTL> { [ Control_L ] };
        key <LFSH> { [ Shift_L ] };
        modifier_map Control { <LCTL> };
        modifier_map Shift { <LFSH> };
    };
};
"#;

    const RAW_Q: u32 = 16;
    const RAW_CTRL: u32 = 29;

    /// One key event for the tests: the keysym matches the keymap's base
    /// level for the keycode, as the toolkit's state would answer.
    fn key_event(raw_code: u32, keysym: u32) -> KeyEvent {
        KeyEvent {
            time: 0,
            raw_code,
            keysym: Keysym::new(keysym),
            utf8: None,
        }
    }

    /// The toolkit's modifier flags map onto the encoder's bits, num lock
    /// included.
    #[test]
    fn sctk_modifiers_map_onto_the_encoder_bits() {
        let none = SctkModifiers::default();
        assert_eq!(mods_from_sctk(none), Modifiers::NONE);
        assert_eq!(
            mods_from_sctk(SctkModifiers {
                shift: true,
                ..none
            }),
            Modifiers::SHIFT
        );
        assert_eq!(
            mods_from_sctk(SctkModifiers { ctrl: true, ..none }),
            Modifiers::CTRL
        );
        assert_eq!(
            mods_from_sctk(SctkModifiers { alt: true, ..none }),
            Modifiers::ALT
        );
        assert_eq!(
            mods_from_sctk(SctkModifiers { logo: true, ..none }),
            Modifiers::SUPER
        );
        assert_eq!(
            mods_from_sctk(SctkModifiers {
                caps_lock: true,
                ..none
            }),
            Modifiers::CAPS_LOCK
        );
        assert_eq!(
            mods_from_sctk(SctkModifiers {
                num_lock: true,
                ..none
            }),
            Modifiers::NUM_LOCK
        );
        assert_eq!(
            mods_from_sctk(SctkModifiers {
                shift: true,
                ctrl: true,
                alt: true,
                logo: true,
                caps_lock: true,
                num_lock: true,
            }),
            Modifiers::SHIFT
                | Modifiers::CTRL
                | Modifiers::ALT
                | Modifiers::SUPER
                | Modifiers::CAPS_LOCK
                | Modifiers::NUM_LOCK
        );
    }

    /// A press after the keymap arrives builds the same `KeyInput` the GDK
    /// path builds: the +8 keycode, the keysym, the held modifiers, no
    /// consumed modifiers for a plain letter, and both codepoints.
    #[test]
    fn a_press_after_the_keymap_builds_the_gdk_shape() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());
        let input = keyboard
            .pressed(&key_event(RAW_Q, 0x71))
            .expect("the keymap is live");
        assert_eq!(input.action, KeyAction::Press);
        assert_eq!(input.keycode, 24);
        assert_eq!(input.keyval, 0x71);
        assert_eq!(input.mods, Modifiers::NONE);
        assert_eq!(input.consumed_mods, Modifiers::NONE);
        assert!(!input.is_modifier);
        assert_eq!(input.unshifted_codepoint, u32::from('q'));
        assert_eq!(input.keyval_unicode, u32::from('q'));
    }

    /// Shifted input carries the held modifiers and the consumed shift: the
    /// `Modifiers` bits the encoder takes reach it unchanged.
    #[test]
    fn a_shifted_press_carries_the_modifiers_and_the_consumed_shift() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.modifiers_updated(
            RawModifiers {
                depressed: 1,
                latched: 0,
                locked: 0,
            },
            0,
            SctkModifiers {
                shift: true,
                ..SctkModifiers::default()
            },
        );
        let input = keyboard
            .pressed(&key_event(RAW_Q, 0x51))
            .expect("the keymap is live");
        assert_eq!(input.mods, Modifiers::SHIFT);
        assert_eq!(input.consumed_mods, Modifiers::SHIFT);
        assert_eq!(input.keyval_unicode, u32::from('Q'));
        assert_eq!(input.unshifted_codepoint, u32::from('q'));
    }

    /// A key event before the compositor's keymap arrived is dropped: there
    /// is no xkb state to look the consumed modifiers and the unshifted
    /// codepoint up in.
    #[test]
    fn a_press_before_the_keymap_is_dropped() {
        let mut keyboard = KeyboardSide::new();
        assert!(keyboard.pressed(&key_event(RAW_Q, 0x71)).is_none());
    }

    /// A broken keymap string keeps the previous keymap live: the next press
    /// still translates.
    #[test]
    fn a_broken_keymap_keeps_the_previous_one() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.keymap_updated("not a keymap");
        assert!(keyboard.pressed(&key_event(RAW_Q, 0x71)).is_some());
    }

    /// A keymap update re-applies the held raw modifiers, so a shift held
    /// across a keymap change still shifts the next key.
    #[test]
    fn a_keymap_update_keeps_the_held_modifiers() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.modifiers_updated(
            RawModifiers {
                depressed: 1,
                latched: 0,
                locked: 0,
            },
            0,
            SctkModifiers {
                shift: true,
                ..SctkModifiers::default()
            },
        );
        keyboard.keymap_updated(TEST_KEYMAP);
        let input = keyboard
            .pressed(&key_event(RAW_Q, 0x51))
            .expect("the new keymap is live");
        assert_eq!(input.mods, Modifiers::SHIFT, "the held shift survived");
        assert_eq!(input.consumed_mods, Modifiers::SHIFT);
    }

    /// A modifier key arrives flagged: the encoder's own modifier keyval
    /// table marks it, so the encoder refuses to encode it as a key.
    #[test]
    fn a_modifier_press_is_flagged() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());
        let input = keyboard
            .pressed(&key_event(RAW_CTRL, 0xffe3))
            .expect("the keymap is live");
        assert!(input.is_modifier);
    }

    /// A rate the compositor sent, for the repeat tests.
    fn repeat_info() -> RepeatInfo {
        RepeatInfo::Repeat {
            rate: std::num::NonZeroU32::new(25).expect("test rate"),
            delay: 250,
        }
    }

    /// A repeating key arms the gate and its repeats pass (row 5.2).
    #[test]
    fn a_repeating_key_arms_and_repeats() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.repeat_info_updated(&repeat_info());
        let press = keyboard
            .pressed(&key_event(RAW_Q, 0x71))
            .expect("the keymap is live");
        assert_eq!(press.action, KeyAction::Press);
        let repeat = keyboard
            .repeated(&key_event(RAW_Q, 0x71))
            .expect("the repeat is accepted");
        assert_eq!(repeat.action, KeyAction::Repeat);
        assert_eq!(repeat.keycode, 24);
    }

    /// A release stops the repeat: the toolkit stops its timer, and the
    /// gate refuses a late repeat of the released key.
    #[test]
    fn a_release_stops_the_repeat() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.repeat_info_updated(&repeat_info());
        let _ = keyboard.pressed(&key_event(RAW_Q, 0x71));
        let _ = keyboard.released(&key_event(RAW_Q, 0x71));
        assert!(keyboard.repeated(&key_event(RAW_Q, 0x71)).is_none());
    }

    /// A keyboard leave stops any repeat (row 5.2's stop-on-leave rule).
    #[test]
    fn a_keyboard_leave_stops_the_repeat() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.repeat_info_updated(&repeat_info());
        let _ = keyboard.pressed(&key_event(RAW_Q, 0x71));
        keyboard.left();
        assert!(keyboard.repeated(&key_event(RAW_Q, 0x71)).is_none());
    }

    /// A modifier key never repeats: the gate refuses to arm it even though
    /// the keymap marks the key repeating.
    #[test]
    fn a_modifier_key_never_repeats() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.repeat_info_updated(&repeat_info());
        let _ = keyboard.pressed(&key_event(RAW_CTRL, 0xffe3));
        assert!(keyboard.repeated(&key_event(RAW_CTRL, 0xffe3)).is_none());
    }

    /// A key the keymap does not repeat never arms the gate.
    #[test]
    fn a_non_repeating_key_never_arms() {
        let mut tracker = RepeatTracker::default();
        tracker.set_enabled(true);
        tracker.on_press(RAW_Q, false, false);
        assert!(!tracker.accepts_repeat(RAW_Q));
    }

    /// A compositor repeat rate of 0 disables repeat even for an armed key
    /// (the spec's "Repeat disabled" scenario).
    #[test]
    fn a_zero_repeat_rate_disables_repeat() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        keyboard.repeat_info_updated(&repeat_info());
        let _ = keyboard.pressed(&key_event(RAW_Q, 0x71));
        keyboard.repeat_info_updated(&RepeatInfo::Disable);
        assert!(keyboard.repeated(&key_event(RAW_Q, 0x71)).is_none());
    }

    /// Until a `repeat_info` arrives nothing repeats, the toolkit's own
    /// default.
    #[test]
    fn repeat_stays_disabled_without_a_repeat_info() {
        let mut keyboard = KeyboardSide::new();
        keyboard.keymap_updated(TEST_KEYMAP);
        let _ = keyboard.pressed(&key_event(RAW_Q, 0x71));
        assert!(keyboard.repeated(&key_event(RAW_Q, 0x71)).is_none());
    }

    /// A release of another key does not stop the armed key's repeat: the
    /// toolkit repeats one key at a time, and only its own release ends it.
    #[test]
    fn another_keys_release_keeps_the_armed_repeat() {
        let mut tracker = RepeatTracker::default();
        tracker.set_enabled(true);
        tracker.on_press(RAW_Q, false, true);
        tracker.on_release(RAW_CTRL);
        assert!(tracker.accepts_repeat(RAW_Q));
    }
}
