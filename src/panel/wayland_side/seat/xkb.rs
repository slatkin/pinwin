//! The seat's own xkbcommon keymap and state (replace-gtk-with-wayland D8):
//! the toolkit hands the compositor's keymap over as a string and
//! its [`sctk KeyEvent`] carries only the raw keycode, the keysym and the
//! text. The [`crate::term::input::KeyInput`] needs three more values — the consumed
//! modifiers, the level-0 "unshifted" codepoint and the modifier flag — and
//! D8 has the keyboard handler build them from a keymap and
//! state of its own, fed by the same `update_keymap` string and the same
//! `RawModifiers` and layout `update_modifiers` receives.
//!
//! The numberings line up: a Wayland keycode plus 8 is the XKB keycode the
//! state lookups and the terminal's `key_from_keycode` both take, X keysyms
//! are the keyvals the encoder sees, and `xkbcommon`'s
//! `keysym_to_utf32` maps a keysym onto its Unicode codepoint.
//!
//! [`sctk KeyEvent`]: smithay_client_toolkit::seat::keyboard::KeyEvent

use smithay_client_toolkit::seat::keyboard::RawModifiers;
use xkbcommon::xkb::{
    self, Context, KEYMAP_COMPILE_NO_FLAGS, KEYMAP_FORMAT_TEXT_V1, Keycode, Keymap,
    STATE_MODS_EFFECTIVE, State,
};

use crate::term::input::Modifiers;

/// The keymap string compiles in the `XKB_V1` text format with no flags, the
/// way the toolkit's own state does.
const COMPILE_FLAGS: u32 = KEYMAP_COMPILE_NO_FLAGS;

/// The canonical XKB modifier masks: `xkb_state_key_get_consumed_mods`
/// answers a mask over the eight canonical modifiers, bit `i` = modifier
/// index `i` (Shift, Lock, Control, Mod1, Mod2, Mod3, Mod4, Mod5).
const XKB_SHIFT: u32 = 1 << 0;
const XKB_LOCK: u32 = 1 << 1;
const XKB_CONTROL: u32 = 1 << 2;
const XKB_MOD1: u32 = 1 << 3;
const XKB_MOD2: u32 = 1 << 4;
const XKB_MOD4: u32 = 1 << 6;

/// One key event's translation facts, minus the action the caller knows.
#[derive(Clone, Copy, Debug)]
pub struct KeyFacts {
    /// The XKB keycode: the Wayland keycode plus 8, the value
    /// `key_from_keycode` and the state lookups both take.
    pub keycode: u32,
    /// The keysym of the event, the [`crate::term::input::KeyInput`]'s keyval.
    pub keyval: u32,
    /// The modifiers consumed by producing the event's keysym, mapped onto
    /// the encoder's bits.
    pub consumed_mods: Modifiers,
    /// The level-0 codepoint of the key in the current layout (kitty's
    /// "unshifted"), 0 when the layout has no answer.
    pub unshifted_codepoint: u32,
    /// The codepoint the keysym produces, 0 for none.
    pub keyval_unicode: u32,
}

/// A compiled keymap and its live state, plus the layout the modifiers event
/// last reported. Owns its libxkbcommon objects; the field order drops the
/// state before the keymap and the keymap before the context, the order
/// libxkbcommon requires.
pub struct XkbKeyboard {
    state: State,
    keymap: Keymap,
    context: Context,
    /// The layout (group) the last modifiers event reported.
    layout: u32,
}

impl std::fmt::Debug for XkbKeyboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XkbKeyboard")
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

impl XkbKeyboard {
    /// Compile a keymap from the compositor's keymap string (D8). `None`
    /// when the string does not compile: the keyboard side keeps its
    /// previous keymap, the way a failed keymap event is dropped.
    #[must_use]
    pub fn from_string(keymap: &str) -> Option<Self> {
        let context = Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = Keymap::new_from_string(
            &context,
            keymap.to_owned(),
            KEYMAP_FORMAT_TEXT_V1,
            COMPILE_FLAGS,
        )?;
        let state = State::new(&keymap);
        Some(XkbKeyboard {
            state,
            keymap,
            context,
            layout: 0,
        })
    }

    /// Recompile from a new keymap string, reusing this keyboard's context
    /// (D8). `false` — keeping the previous keymap and state — when the
    /// string does not compile.
    pub fn recompile(&mut self, keymap: &str) -> bool {
        let Some(new_keymap) = Keymap::new_from_string(
            &self.context,
            keymap.to_owned(),
            KEYMAP_FORMAT_TEXT_V1,
            COMPILE_FLAGS,
        ) else {
            return false;
        };
        // The new state is built from the new keymap before either field is
        // replaced, so the old state never outlives the old keymap it points
        // at: the state assignment drops it first, then the keymap one.
        let new_state = State::new(&new_keymap);
        self.state = new_state;
        self.keymap = new_keymap;
        true
    }

    /// Apply one modifiers event (D8): the raw modifier set the compositor
    /// sent and the layout (group) it reports, the same update the toolkit's
    /// own state applies.
    pub fn update_modifiers(&mut self, raw: RawModifiers, layout: u32) {
        self.layout = layout;
        self.state
            .update_mask(raw.depressed, raw.latched, raw.locked, 0, 0, layout);
    }

    /// Whether the key repeats under this keymap: the calloop repeat source
    /// arms only such keys, and the seat's repeat tracker re-checks it.
    #[must_use]
    pub fn key_repeats(&self, raw_code: u32) -> bool {
        let Some(keycode) = keycode(raw_code) else {
            return false;
        };
        self.keymap.key_repeats(keycode)
    }

    /// The keysym this state answers for a key right now, with the
    /// modifiers and layout the last modifiers event applied. A repeat
    /// carries this, not the toolkit event's cached press-time keysym:
    /// the toolkit refreshes only a repeat's text when the modifiers
    /// change, so a Shift tapped while a key is held would otherwise keep
    /// repeating the base letter. `None` for a keycode outside the
    /// protocol's range.
    #[must_use]
    pub fn state_keysym(&self, raw_code: u32) -> Option<u32> {
        let keycode = keycode(raw_code)?;
        Some(self.state.key_get_one_sym(keycode).raw())
    }

    /// The facts of one key event (D8). `keysym` is the event's keysym, the
    /// value the [`crate::term::input::KeyInput`] carries as its keyval; the rest is looked up in
    /// this state.
    #[must_use]
    pub fn facts(&self, raw_code: u32, keysym: u32) -> KeyFacts {
        KeyFacts {
            keycode: keycode(raw_code).map_or(raw_code, Keycode::raw),
            keyval: keysym,
            consumed_mods: self.consumed_mods(raw_code),
            unshifted_codepoint: self.unshifted_codepoint(raw_code),
            keyval_unicode: xkb::keysym_to_utf32(keysym.into()),
        }
    }

    /// The modifiers this key's keysym translation consumed, mapped onto the
    /// encoder's bits: Shift on a shifted key, caps lock where it selects the
    /// level, and the control/alt/super/num masks when the keymap used them.
    /// The xkb consumed mask covers the key type's own modifiers even when
    /// they are not held (a plain letter press reports Shift), so it is
    /// intersected with the effective modifier state — a modifier can only
    /// be consumed while it is held, which is what the intersection
    /// reports.
    #[must_use]
    fn consumed_mods(&self, raw_code: u32) -> Modifiers {
        let Some(keycode) = keycode(raw_code) else {
            return Modifiers::NONE;
        };
        let consumed = self.state.key_get_consumed_mods(keycode);
        let effective = self.state.serialize_mods(STATE_MODS_EFFECTIVE);
        mods_from_xkb_mask(consumed & effective)
    }

    /// The level-0 codepoint for a hardware keycode (kitty's "unshifted"),
    /// the level-0 keysym of the modifiers event's layout when the key has
    /// one, else the first layout that does. 0 when no layout has an answer.
    #[must_use]
    fn unshifted_codepoint(&self, raw_code: u32) -> u32 {
        let Some(keycode) = keycode(raw_code) else {
            return 0;
        };
        let layouts = self.keymap.num_layouts_for_key(keycode);
        // The modifiers event's layout first, then the first layout with a
        // level-0 keysym.
        if self.layout < layouts
            && let Some(sym) = self
                .keymap
                .key_get_syms_by_level(keycode, self.layout, 0)
                .first()
        {
            return xkb::keysym_to_utf32(*sym);
        }
        for layout in 0..layouts {
            if let Some(sym) = self
                .keymap
                .key_get_syms_by_level(keycode, layout, 0)
                .first()
            {
                return xkb::keysym_to_utf32(*sym);
            }
        }
        0
    }
}

/// The XKB keycode of one Wayland key event: the raw keycode plus 8, the
/// offset the state lookups and the terminal's `key_from_keycode` both
/// expect. `None` on the overflow an out-of-protocol
/// keycode would need.
#[must_use]
fn keycode(raw_code: u32) -> Option<Keycode> {
    let offset = raw_code.checked_add(8)?;
    (offset <= xkb::KEYCODE_MAX).then_some(Keycode::new(offset))
}

/// Map one canonical XKB modifier mask onto the encoder's bits: Shift, Lock
/// (caps lock), Control, Mod1 (alt), Mod2 (num lock) and Mod4 (super). Mod3
/// and Mod5 have no encoder bit and stay unmapped.
#[must_use]
fn mods_from_xkb_mask(mask: u32) -> Modifiers {
    let mut mods = Modifiers::NONE;
    if mask & XKB_SHIFT != 0 {
        mods = mods | Modifiers::SHIFT;
    }
    if mask & XKB_LOCK != 0 {
        mods = mods | Modifiers::CAPS_LOCK;
    }
    if mask & XKB_CONTROL != 0 {
        mods = mods | Modifiers::CTRL;
    }
    if mask & XKB_MOD1 != 0 {
        mods = mods | Modifiers::ALT;
    }
    if mask & XKB_MOD2 != 0 {
        mods = mods | Modifiers::NUM_LOCK;
    }
    if mask & XKB_MOD4 != 0 {
        mods = mods | Modifiers::SUPER;
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hermetic two-group keymap: a letter with its shift pair, a dead
    /// key, the two shift/ctrl modifiers, and a second group that puts other
    /// letters on the same keys. No xkeyboard-config include, so the test
    /// runs on any machine libxkbcommon is installed on.
    const TEST_KEYMAP: &str = r#"
xkb_keymap {
    xkb_keycodes "pinwin" {
        minimum = 8;
        maximum = 255;
        <Q> = 24;
        <AC10> = 47;
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
        name[Group1] = "First";
        name[Group2] = "Second";
        key <Q> { type = "TWO_LEVEL", [ q, Q ], [ z, Z ] };
        key <AC10> { type = "TWO_LEVEL", [ dead_acute, dead_grave ] };
        key <LCTL> { [ Control_L ] };
        key <LFSH> { [ Shift_L ] };
        modifier_map Control { <LCTL> };
        modifier_map Shift { <LFSH> };
    };
};
"#;

    const SHIFT_DEPRESSED: u32 = 1 << 0;
    const CONTROL_DEPRESSED: u32 = 1 << 2;

    /// The Wayland keycode of the keymap's `Q` key: evdev 16 plus 8.
    const RAW_Q: u32 = 16;
    /// The Wayland keycode of the dead key (evdev semicolon plus 8).
    const RAW_DEAD: u32 = 39;
    /// The Wayland keycode of the left control key (evdev 29 plus 8).
    const RAW_CTRL: u32 = 29;

    fn keyboard() -> XkbKeyboard {
        XkbKeyboard::from_string(TEST_KEYMAP).expect("the test keymap compiles")
    }

    fn mods(depressed: u32) -> RawModifiers {
        RawModifiers {
            depressed,
            latched: 0,
            locked: 0,
        }
    }

    /// A letter press: the keycode carries the +8 offset, the keysym is the
    /// base letter, nothing is consumed and both codepoints are the letter.
    #[test]
    fn a_letter_press_translates_to_its_base_letter() {
        let keyboard = keyboard();
        let facts = keyboard.facts(RAW_Q, 0x71);
        assert_eq!(facts.keycode, 24);
        assert_eq!(facts.keyval, 0x71);
        assert_eq!(facts.consumed_mods, Modifiers::NONE);
        assert_eq!(facts.unshifted_codepoint, u32::from('q'));
        assert_eq!(facts.keyval_unicode, u32::from('q'));
    }

    /// A shifted letter: the keysym is the shifted letter, its codepoint is
    /// the shifted one, the unshifted codepoint stays the base letter and
    /// shift counts as consumed.
    #[test]
    fn a_shifted_letter_consumes_shift_and_keeps_the_unshifted_codepoint() {
        let mut keyboard = keyboard();
        keyboard.update_modifiers(mods(SHIFT_DEPRESSED), 0);
        let facts = keyboard.facts(RAW_Q, 0x51);
        assert_eq!(facts.keyval, 0x51);
        assert_eq!(facts.keyval_unicode, u32::from('Q'));
        assert_eq!(facts.unshifted_codepoint, u32::from('q'));
        assert_eq!(facts.consumed_mods, Modifiers::SHIFT);
    }

    /// Ctrl+letter: control is not consumed by producing the letter's keysym,
    /// so the encoder sees the whole Ctrl+key combination (the xkb rule the
    /// consumed-modifier value carries).
    #[test]
    fn ctrl_is_not_consumed_by_a_letter() {
        let mut keyboard = keyboard();
        keyboard.update_modifiers(mods(CONTROL_DEPRESSED), 0);
        let facts = keyboard.facts(RAW_Q, 0x71);
        assert_eq!(facts.keyval, 0x71);
        assert_eq!(facts.consumed_mods, Modifiers::NONE);
    }

    /// A modifier key alone: its keysym is a modifier keyval the encoder
    /// refuses to encode, and it produces no codepoint.
    #[test]
    fn a_modifier_key_produces_no_codepoint() {
        let keyboard = keyboard();
        let facts = keyboard.facts(RAW_CTRL, 0xffe3);
        assert_eq!(facts.keycode, 37);
        assert_eq!(facts.keyval, 0xffe3);
        assert_eq!(facts.keyval_unicode, 0);
        assert_eq!(facts.unshifted_codepoint, 0);
        assert_eq!(facts.consumed_mods, Modifiers::NONE);
    }

    /// A dead key: the keysym is a dead keysym with no Unicode answer, so
    /// the keyval codepoint is 0 (the encoder's own range check then drops
    /// the synthetic text).
    #[test]
    fn a_dead_key_has_no_unicode_answer() {
        let keyboard = keyboard();
        let facts = keyboard.facts(RAW_DEAD, 0xfe51);
        assert_eq!(facts.keycode, 47);
        assert_eq!(facts.keyval, 0xfe51);
        assert_eq!(facts.keyval_unicode, 0);
        assert_eq!(facts.unshifted_codepoint, 0);
    }

    /// A non-US layout group: with the second group active, the same key
    /// produces the second group's letters and the unshifted codepoint
    /// follows the active layout.
    #[test]
    fn a_second_group_moves_the_letter_and_the_unshifted_codepoint() {
        let mut keyboard = keyboard();
        keyboard.update_modifiers(mods(0), 1);
        let facts = keyboard.facts(RAW_Q, 0x7a);
        assert_eq!(facts.keyval, 0x7a);
        assert_eq!(facts.keyval_unicode, u32::from('z'));
        assert_eq!(facts.unshifted_codepoint, u32::from('z'));

        // Back to the first group: the base letter returns.
        keyboard.update_modifiers(mods(0), 0);
        let facts = keyboard.facts(RAW_Q, 0x71);
        assert_eq!(facts.unshifted_codepoint, u32::from('q'));
    }

    /// A layout index the key does not have falls back to the first layout
    /// with a level-0 keysym.
    #[test]
    fn an_out_of_range_layout_falls_back_to_the_first_group() {
        let mut keyboard = keyboard();
        keyboard.update_modifiers(mods(0), 7);
        let facts = keyboard.facts(RAW_Q, 0x71);
        assert_eq!(facts.unshifted_codepoint, u32::from('q'));
    }

    /// Only keys the keymap marks repeating repeat (the calloop repeat and
    /// the seat's tracker both ask the keymap); a keycode outside the
    /// protocol's range repeats nothing.
    #[test]
    fn repeat_follows_the_keymap_and_rejects_out_of_range_keycodes() {
        let keyboard = keyboard();
        assert!(keyboard.key_repeats(RAW_Q));
        // u32::MAX + 8 overflows; the offset must refuse it, not wrap.
        assert!(!keyboard.key_repeats(u32::MAX));
    }

    /// A keymap string that does not compile is refused: the keyboard side
    /// keeps its previous keymap instead of swapping in a broken one.
    #[test]
    fn a_broken_keymap_string_is_refused() {
        assert!(XkbKeyboard::from_string("not a keymap").is_none());
    }

    /// The canonical modifier masks map onto the encoder's bits, and the
    /// masks a key never consumes (Mod3, Mod5) stay unmapped.
    #[test]
    fn xkb_masks_map_onto_the_encoder_bits() {
        assert_eq!(mods_from_xkb_mask(0), Modifiers::NONE);
        assert_eq!(mods_from_xkb_mask(XKB_SHIFT), Modifiers::SHIFT);
        assert_eq!(mods_from_xkb_mask(XKB_LOCK), Modifiers::CAPS_LOCK);
        assert_eq!(mods_from_xkb_mask(XKB_CONTROL), Modifiers::CTRL);
        assert_eq!(mods_from_xkb_mask(XKB_MOD1), Modifiers::ALT);
        assert_eq!(mods_from_xkb_mask(XKB_MOD2), Modifiers::NUM_LOCK);
        assert_eq!(mods_from_xkb_mask(XKB_MOD4), Modifiers::SUPER);
        assert_eq!(
            mods_from_xkb_mask(XKB_SHIFT | XKB_CONTROL | XKB_MOD4),
            Modifiers::SHIFT | Modifiers::CTRL | Modifiers::SUPER
        );
        // Mod3 (1 << 5) and Mod5 (1 << 7) have no encoder bit.
        assert_eq!(mods_from_xkb_mask(1 << 5 | 1 << 7), Modifiers::NONE);
    }

    /// With the system's keymap names available (libxkbcommon plus
    /// xkeyboard-config), a real US keymap answers the same facts for `a`;
    /// skipped with a message where xkeyboard-config is absent.
    #[test]
    fn a_real_us_keymap_answers_the_same_letter_facts() {
        let context = Context::new(xkb::CONTEXT_NO_FLAGS);
        let Some(keymap) = Keymap::new_from_names(
            &context,
            "evdev",
            "pc105",
            "us",
            "",
            None,
            KEYMAP_COMPILE_NO_FLAGS,
        ) else {
            println!("skipping: xkeyboard-config is not available for the US keymap test");
            return;
        };
        let state = State::new(&keymap);
        let _ = state;
        // The `a` key: evdev 30 plus 8.
        let keycode = Keycode::new(38);
        let keysym = state.key_get_one_sym(keycode);
        assert_eq!(keysym.raw(), 0x61);
        assert_eq!(xkb::keysym_to_utf32(keysym), u32::from('a'));
    }
}
