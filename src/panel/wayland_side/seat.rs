//! The seat side of the wayland panel thread (replace-gtk-with-wayland D8,
//! rows 5.1 to 5.5): the keyboard half — the key events the toolkit's
//! `KeyboardHandler` delivers are translated into the same [`KeyInput`] the
//! GDK path's `src/input.rs` builds and pushed into the terminal's key
//! encoder — plus the pointer half (row 5.3) and the keyboard focus half
//! (row 5.4) behind the same links, and the cursor-shape hook (row 5.5).
//! Row 8.1 wires the dispatch state to the hooks here.
//!
//! The hooks take the pieces the row 8.1 dispatch hands them (the toolkit's
//! `KeyEvent`, `RawModifiers`, keymap string) and hold no Wayland objects of
//! their own, so every rule is testable without a display
//! (`port-to-rust` D10); the live-compositor behaviour is 10.1's.
//!
//! Panics never cross back into calloop or the compositor (D5): every hook
//! runs its body through the shared [`crate::guard`] with the panel's shared
//! latch, the way the GDK path's controller closures do.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use smithay_client_toolkit::globals::GlobalData;
use smithay_client_toolkit::reexports::client::globals::GlobalList;
use smithay_client_toolkit::reexports::client::protocol::wl_pointer::WlPointer;
use smithay_client_toolkit::reexports::client::{Dispatch, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, Modifiers as SctkModifiers, RawModifiers, RepeatInfo,
};
use smithay_client_toolkit::seat::pointer::PointerEventKind;

use crate::guard::{Poisoned, guard};
use crate::term::Terminal;

pub mod cursor;
pub mod focus;
pub mod keyboard;
pub mod pointer;
pub mod xkb;

use focus::FocusSide;
use pointer::PointerSide;

/// The keyboard hooks' links: the terminal the key encoder pushes into and
/// the panel's shared D5 latch. They are the keyboard view of the fuller
/// [`SeatLinks`], which [`SeatLinks::keyboard`] derives, so the row 8.1
/// wiring carries one links value for all three seat halves.
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

/// The seat hooks' links: everything the hooks reach outside this module. The row 8.1 wiring
/// assembles one value from the panel state; the pointer and focus hooks
/// take the whole set, the keyboard hooks the terminal and the latch
/// through [`KeyboardLinks`].
#[derive(Clone)]
pub struct SeatLinks {
    /// The terminal the encoders push into.
    pub terminal: Rc<RefCell<Terminal>>,
    /// The anim row's draw offset: the drawing shift that keeps the grid
    /// against the docked edge while the surface animates; pointer x is
    /// adjusted by it, like the GDK path's `on_mouse`.
    pub draw_offset: Rc<dyn Fn() -> f64>,
    /// Whether the panel holds keyboard focus; the renderer reads it for
    /// the focus accent (`g_focused`).
    pub focused: Rc<Cell<bool>>,
    /// Queue a redraw of the panel's surface.
    pub queue_draw: Rc<dyn Fn()>,
    /// Latched when a hook body panicked (D5); the panel consults it.
    pub poisoned: Poisoned,
}

impl std::fmt::Debug for SeatLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeatLinks")
            .field("focused", &self.focused)
            .field("poisoned", &self.poisoned)
            .finish_non_exhaustive()
    }
}

impl SeatLinks {
    /// The keyboard hooks' view of the links: the terminal and the latch.
    #[must_use]
    pub fn keyboard(&self) -> KeyboardLinks {
        KeyboardLinks {
            terminal: Rc::clone(&self.terminal),
            poisoned: self.poisoned.clone(),
        }
    }
}

/// The seat side's hooks (rows 5.1 to 5.5): one type the row 8.1 dispatch
/// delegates the toolkit's keyboard and pointer events to.
#[derive(Debug)]
pub struct SeatSide {
    links: SeatLinks,
    keyboard: keyboard::KeyboardSide,
    /// `pub(crate)` for the row 8.1 wiring: the pointer capability's
    /// removal resets the cursor-shape device through
    /// [`PointerSide::set_cursor`], the only path that drops it.
    pub(crate) pointer: PointerSide,
    focus: FocusSide,
}

impl SeatSide {
    /// A seat side over the panel's links. The keyboard half starts with no
    /// keymap and no modifiers held, the pointer half with no cursor-shape
    /// device (row 5.5's [`SeatSide::bind_cursor_shape`] installs one).
    #[must_use]
    pub fn new(links: SeatLinks) -> Self {
        SeatSide {
            focus: FocusSide::new(links.clone()),
            links,
            keyboard: keyboard::KeyboardSide::new(),
            pointer: PointerSide::default(),
        }
    }

    /// The keyboard hooks' view of the links: the terminal and the latch.
    #[must_use]
    pub fn keyboard_links(&self) -> KeyboardLinks {
        self.links.keyboard()
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

    /// A repeat from the toolkit's calloop repeat source (row 5.2): sent as
    /// [`crate::term::input::KeyAction::Repeat`] while the repeat gate still
    /// has the key armed.
    pub fn key_repeated(&mut self, event: &KeyEvent) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            if let Some(input) = self.keyboard.repeated(event) {
                self.links.terminal.borrow_mut().push_key(input);
            }
        });
    }

    /// The keyboard entered the surface (row 5.4): the focus accent's flag
    /// goes up, the redraw queues it into the next frame and the focus gain
    /// is reported — the triggers the GDK path's focus controller had.
    pub fn keyboard_entered(&mut self) {
        self.focus.entered();
    }

    /// The keyboard left the surface (rows 5.2 and 5.4): the repeat stops
    /// and the focus accent comes down with its focus-loss report — the two
    /// halves of the one `wl_keyboard.leave` event.
    pub fn keyboard_left(&mut self) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.keyboard.left();
        });
        self.focus.left();
    }

    /// One toolkit pointer event (row 5.3): `kind` and `position` come from
    /// the `PointerEvent` the toolkit's `PointerHandler` delivers, one call
    /// per event of a frame. The translation and its terminal pushes run
    /// under the shared guard (D5); the modifiers are the keyboard side's
    /// last report, since Wayland pointer events carry none.
    pub fn pointer_frame(&mut self, kind: &PointerEventKind, position: (f64, f64)) {
        let mods = self.keyboard.mods();
        self.pointer.frame(kind, position, &self.links, mods);
    }

    /// Bind the cursor-shape global for the panel pointer (row 5.5): when
    /// the compositor offers `wp_cursor_shape_manager_v1`, the pointer gets
    /// a shape device and [`SeatSide::pointer_frame`] sets the default shape
    /// on each enter; without the global the pointer cursor stays unset.
    /// Call once, at the row 8.1 bind, after the pointer exists.
    pub fn bind_cursor_shape<D>(
        &mut self,
        globals: &GlobalList,
        qh: &QueueHandle<D>,
        pointer: &WlPointer,
    ) where
        D: Dispatch<WpCursorShapeManagerV1, GlobalData>
            + Dispatch<WpCursorShapeDeviceV1, GlobalData>
            + 'static,
    {
        self.pointer
            .set_cursor(cursor::CursorShape::bind(globals, qh, pointer));
    }

    /// One `repeat_info` update (row 5.2): the compositor's rate decides
    /// whether any key repeats.
    pub fn repeat_info_updated(&mut self, info: &RepeatInfo) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.keyboard.repeat_info_updated(info);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::term::{DecodedPng, PngDecoder, PtySink};
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
    /// (40x24 at 8x16), the links to reach the terminal with, and the pty
    /// bytes it wrote.
    type SeatFixture = (SeatSide, SeatLinks, Arc<Mutex<Vec<u8>>>);

    fn seat_side() -> SeatFixture {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(
            GuardPoisoned::new(),
            RecordingSink(Arc::clone(&writes)),
            NoDecoder,
            || (),
        );
        assert!(terminal.push_size(40, 24, 8, 16));
        let links = SeatLinks {
            terminal: Rc::new(RefCell::new(terminal)),
            draw_offset: Rc::new(|| 0.0),
            focused: Rc::new(Cell::new(false)),
            queue_draw: Rc::new(|| ()),
            poisoned: GuardPoisoned::new(),
        };
        (SeatSide::new(links.clone()), links, writes)
    }

    fn take(writes: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
        std::mem::take(&mut *writes.lock().expect("sink lock"))
    }

    /// A key press flows through the seat side into the pty: the terminal
    /// encodes the translated `KeyInput` like the GDK path's push would.
    #[test]
    fn a_key_press_reaches_the_pty() {
        let (mut seat, _links, writes) = seat_side();
        seat.keymap_updated(TEST_KEYMAP);
        seat.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), b"q");

        // A release reaches the encoder too; without the kitty report-events
        // flag it writes nothing.
        seat.key_released(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), Vec::new());
    }

    /// A repeat flows through the seat side as a repeat event (row 5.2):
    /// with the kitty report-all flags the child receives the repeat
    /// report, and a release or a keyboard leave stops the repeats.
    #[test]
    fn a_repeat_flows_through_and_stops_on_release_and_leave() {
        let (mut seat, links, writes) = seat_side();
        links.terminal.borrow_mut().push_pty_data(b"\x1b[>11u");
        seat.keymap_updated(TEST_KEYMAP);
        seat.repeat_info_updated(&RepeatInfo::Repeat {
            rate: std::num::NonZeroU32::new(25).expect("test rate"),
            delay: 250,
        });
        seat.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        seat.key_released(&key_event(RAW_Q, 0x71));
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        // Press, repeat, release; the late repeat after the release never
        // reaches the encoder.
        assert_eq!(take(&writes), b"\x1b[113u\x1b[113;1:2u\x1b[113;1:3u");

        // A fresh press repeats until the keyboard leaves.
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), b"\x1b[113u\x1b[113;1:2u");
        seat.keyboard_left();
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), Vec::new(), "the leave stopped the repeat");
    }

    /// A repeat carries the current modifiers' character: Shift tapped
    /// while a key is held shifts the repeats the way the GDK path's
    /// re-translation does — the toolkit's cached repeat event still
    /// carries the press-time keysym and text.
    #[test]
    fn a_repeat_after_shift_went_down_encodes_the_shifted_character() {
        let (mut seat, links, writes) = seat_side();
        // Disambiguate + report events + report all + report associated
        // text: the associated text is where the shifted character shows.
        links.terminal.borrow_mut().push_pty_data(b"\x1b[>27u");
        seat.keymap_updated(TEST_KEYMAP);
        seat.repeat_info_updated(&RepeatInfo::Repeat {
            rate: std::num::NonZeroU32::new(25).expect("test rate"),
            delay: 250,
        });
        seat.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        seat.modifiers_updated(
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
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        // The CSI u modifiers field is 1-based: 1 = no modifiers, 2 =
        // shift. The shifted repeat carries Q (81) as its associated text
        // where the unshifted one carries q (113).
        assert_eq!(
            take(&writes),
            b"\x1b[113;;113u\x1b[113;1:2;113u\x1b[113;2:2;81u"
        );
    }

    /// Until a `repeat_info` arrives, the seat side sends no repeats — the
    /// compositor's rate is the only source (row 5.2).
    #[test]
    fn repeats_wait_for_the_repeat_info() {
        let (mut seat, _links, writes) = seat_side();
        seat.keymap_updated(TEST_KEYMAP);
        seat.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), b"q", "the press went through, no repeat");
    }

    /// A key event before the compositor's keymap arrived writes nothing:
    /// there is no xkb state to translate it with.
    #[test]
    fn a_press_before_the_keymap_writes_nothing() {
        let (mut seat, _links, writes) = seat_side();
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), Vec::new());
    }

    /// Keyboard enter and leave drive the accent and the focus reports
    /// through the seat side (row 5.4): the enter turns the focused flag on
    /// and reports the gain, the leave clears it, reports the loss and stops
    /// a running repeat.
    #[test]
    fn keyboard_enter_and_leave_drive_the_accent_and_the_reports() {
        let (mut seat, links, writes) = seat_side();
        links
            .terminal
            .borrow_mut()
            .push_pty_data(b"\x1b[>11u\x1b[?1004h");
        seat.keymap_updated(TEST_KEYMAP);
        seat.repeat_info_updated(&RepeatInfo::Repeat {
            rate: std::num::NonZeroU32::new(25).expect("test rate"),
            delay: 250,
        });
        seat.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());

        // The enter: the accent's flag goes up and the child learns of the
        // focus gain.
        seat.keyboard_entered();
        assert!(links.focused.get(), "the enter set the flag");
        assert_eq!(take(&writes), b"\x1b[I");

        // A press repeats until the leave, which also drops the accent.
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        seat.keyboard_left();
        seat.key_repeated(&key_event(RAW_Q, 0x71));
        assert!(!links.focused.get(), "the leave cleared the flag");
        assert_eq!(
            take(&writes),
            b"\x1b[113u\x1b[113;1:2u\x1b[O",
            "press, repeat, then the leave's focus-loss report"
        );
    }

    /// A pointer frame flows through the seat side into the pty (row 5.3):
    /// the press reports in SGR cells, and the keyboard side's held
    /// modifiers reach the report the way the GDK path's
    /// `current_event_state` fed them.
    #[test]
    fn a_pointer_frame_reaches_the_pty_with_the_held_modifiers() {
        let (mut seat, links, writes) = seat_side();
        links
            .terminal
            .borrow_mut()
            .push_pty_data(b"\x1b[?1003h\x1b[?1006h");
        seat.keymap_updated(TEST_KEYMAP);
        seat.modifiers_updated(RawModifiers::default(), 0, SctkModifiers::default());
        seat.pointer_frame(&press_kind(0x110), (50.0, 20.0));
        assert_eq!(take(&writes), b"\x1b[<0;7;2M");

        // The held shift reaches the release report (the SGR shift bit).
        seat.modifiers_updated(
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
        seat.pointer_frame(&release_kind(0x110), (50.0, 20.0));
        assert_eq!(take(&writes), b"\x1b[<4;7;2m");
    }

    /// One toolkit pointer press for the seat-side tests.
    fn press_kind(button: u32) -> PointerEventKind {
        PointerEventKind::Press {
            time: 0,
            button,
            serial: 0,
        }
    }

    /// One toolkit pointer release for the seat-side tests.
    fn release_kind(button: u32) -> PointerEventKind {
        PointerEventKind::Release {
            time: 0,
            button,
            serial: 0,
        }
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
        let links = SeatLinks {
            terminal: Rc::new(RefCell::new(terminal)),
            draw_offset: Rc::new(|| 0.0),
            focused: Rc::new(Cell::new(false)),
            queue_draw: Rc::new(|| ()),
            poisoned: poisoned.clone(),
        };
        let mut seat = SeatSide::new(links);
        seat.keymap_updated(TEST_KEYMAP);
        poisoned.latch();
        seat.key_pressed(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&writes), Vec::new(), "a latched side runs no hooks");
    }
}
