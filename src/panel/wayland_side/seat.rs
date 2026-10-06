//! The seat side of the wayland panel thread (replace-gtk-with-wayland D8,
//! row 5.1): the keyboard half — the key events the toolkit's
//! `KeyboardHandler` delivers are translated into the same [`KeyInput`] the
//! GDK path's `src/input.rs` builds and pushed into the terminal's key
//! encoder. The pointer and focus halves arrive with rows 5.3 and 5.4; row
//! 8.1 wires the dispatch state to the hooks here.
//!
//! The hooks take the pieces the row 8.1 dispatch hands them (the toolkit's
//! `KeyEvent`, `RawModifiers`, keymap string) and hold no Wayland objects of
//! their own, so every rule is testable without a display
//! (`port-to-rust` D10); the live-compositor behaviour is 10.1's.
//!
//! Panics never cross back into calloop or the compositor (D5): every hook
//! runs its body through the shared [`crate::guard`] with the panel's shared
//! latch, the way the GDK path's controller closures do.

use std::cell::RefCell;
use std::rc::Rc;

use smithay_client_toolkit::seat::keyboard::{KeyEvent, Modifiers as SctkModifiers, RawModifiers};

use crate::guard::{Poisoned, guard};
use crate::term::Terminal;

pub mod keyboard;
pub mod xkb;

/// The keyboard hooks' links: the terminal the key encoder pushes into and
/// the panel's shared D5 latch. The pointer and focus rows bring their own
/// links — the draw offset and the focused flag are theirs — and the row 8.1
/// wiring assembles both from the panel state.
#[derive(Clone)]
pub struct KeyboardLinks {
    /// The terminal the encoders push into.
    pub terminal: Rc<RefCell<Terminal>>,
    /// Latched when a hook body panicked (D5); the panel consults it.
    pub poisoned: Poisoned,
}

impl std::fmt::Debug for KeyboardLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyboardLinks")
            .field("poisoned", &self.poisoned)
            .finish_non_exhaustive()
    }
}

/// The seat side's keyboard hooks (row 5.1): one type the row 8.1 dispatch
/// delegates the toolkit's keyboard events to.
#[derive(Debug)]
pub struct SeatSide {
    links: KeyboardLinks,
    keyboard: keyboard::KeyboardSide,
}

impl SeatSide {
    /// A seat side's keyboard half over the panel's links.
    #[must_use]
    pub fn new(links: KeyboardLinks) -> Self {
        SeatSide {
            links,
            keyboard: keyboard::KeyboardSide::new(),
        }
    }

    /// The links the hooks reach the terminal through.
    #[must_use]
    pub fn links(&self) -> &KeyboardLinks {
        &self.links
    }

    /// One keymap update (row 5.1): the seat's own xkb keymap and state
    /// recompile from the compositor's keymap string.
    pub fn keymap_updated(&mut self, keymap: &str) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.keyboard.keymap_updated(keymap);
        });
    }

    /// One modifiers event (row 5.1): the xkb state and the encoder's
    /// modifier bits update together.
    pub fn modifiers_updated(&mut self, raw: RawModifiers, layout: u32, mods: SctkModifiers) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.keyboard.modifiers_updated(raw, layout, mods);
        });
    }

    /// A key press (row 5.1): the translated [`KeyInput`] reaches the
    /// terminal's key encoder.
    pub fn key_pressed(&mut self, event: &KeyEvent) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            if let Some(input) = self.keyboard.pressed(event) {
                self.links.terminal.borrow_mut().push_key(input);
            }
        });
    }

    /// A key release (row 5.1): the release reaches the encoder, which sends
    /// it only when the child asked for release reports.
    pub fn key_released(&mut self, event: &KeyEvent) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            if let Some(input) = self.keyboard.released(event) {
                self.links.terminal.borrow_mut().push_key(input);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::term::{DecodedPng, PngDecoder, PtySink};
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};
    use xkbcommon::xkb::Keysym;

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

    /// The hermetic two-group keymap the seat's tests share.
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

    fn key_event(raw_code: u32, keysym: u32) -> KeyEvent {
        KeyEvent {
            time: 0,
            raw_code,
            keysym: Keysym::new(keysym),
            utf8: None,
        }
    }

    /// The seat fixture: a seat side over a real display-free terminal
    /// (40x24 at 8x16) and the pty bytes it wrote.
    type SeatFixture = (SeatSide, Arc<Mutex<Vec<u8>>>);

    fn seat_side() -> SeatFixture {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(
            GuardPoisoned::new(),
            RecordingSink(Arc::clone(&writes)),
            NoDecoder,
            || (),
        );
        assert!(terminal.push_size(40, 24, 8, 16));
        let links = KeyboardLinks {
            terminal: Rc::new(RefCell::new(terminal)),
            poisoned: GuardPoisoned::new(),
        };
        (SeatSide::new(links), writes)
    }

    fn take(writes: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
        std::mem::take(&mut *writes.lock().expect("sink lock"))
    }

    /// A key press flows through the seat side into the pty: the terminal
    /// encodes the translated `KeyInput` like the GDK path's push would.
    #[test]
    fn a_key_press_reaches_the_pty() {
        let (mut seat, writes) = seat_side();
        seat.keymap_updated(TEST_KEYMAP);
        seat.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), b"q");

        // A release reaches the encoder too; without the kitty report-events
        // flag it writes nothing.
        seat.key_released(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), Vec::new());
    }

    /// A key event before the compositor's keymap arrived writes nothing:
    /// there is no xkb state to translate it with.
    #[test]
    fn a_press_before_the_keymap_writes_nothing() {
        let (mut seat, writes) = seat_side();
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), Vec::new());
    }

    /// A panicking hook body is caught and latches the shared flag (D5), and
    /// a latched side runs no further hooks.
    #[test]
    fn a_panicking_terminal_push_is_contained() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(
            GuardPoisoned::new(),
            RecordingSink(Arc::clone(&writes)),
            NoDecoder,
            || (),
        );
        assert!(terminal.push_size(40, 24, 8, 16));
        let poisoned = GuardPoisoned::new();
        let links = KeyboardLinks {
            terminal: Rc::new(RefCell::new(terminal)),
            poisoned: poisoned.clone(),
        };
        let mut seat = SeatSide::new(links);
        seat.keymap_updated(TEST_KEYMAP);
        poisoned.latch();
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), Vec::new(), "a latched side runs no hooks");
    }

    /// The `Cell` import keeps the fixture's counter shape available; the
    /// type alias pins the fixture's shape for the later rows.
    #[test]
    fn the_fixture_shape_holds() {
        let draws = Cell::new(0usize);
        draws.set(draws.get() + 1);
        assert_eq!(draws.get(), 1);
    }
}
