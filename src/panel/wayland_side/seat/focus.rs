//! The keyboard-focus half of the seat side (replace-gtk-with-wayland D8):
//! the keyboard enter and leave the toolkit's `KeyboardHandler`
//! delivers drive the focus accent and the focus reports — the enter and
//! the leave are their only triggers.
//!
//! The enter sets the links' focused flag the renderer reads for the accent,
//! queues a redraw so the accent appears in the next frame, and reports the
//! focus gain to the terminal's focus encoder; the leave undoes all three.
//! The encoder reports only actual changes to a program that enabled focus
//! reporting (mode 1004), so a balanced enter/leave pair reaches the child
//! as `CSI I` and `CSI O`.
//!
//! Panics never cross back into calloop or the compositor (D5): both hooks
//! run their bodies through the shared [`crate::guard`] with the links'
//! latch.

use crate::guard::guard;

use super::SeatLinks;

/// The keyboard-focus hooks: one small type over the
/// links — the terminal handle, the focused flag, the redraw and the latch.
#[derive(Debug)]
pub struct FocusSide {
    links: SeatLinks,
}

impl FocusSide {
    /// A focus side over the panel's links.
    #[must_use]
    pub fn new(links: SeatLinks) -> Self {
        FocusSide { links }
    }

    /// The keyboard entered the surface: the accent's flag goes up, the
    /// redraw queues the accent into the next frame, and the focus gain is
    /// reported.
    pub fn entered(&mut self) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.links.focused.set(true);
            (self.links.queue_draw)();
            self.links.terminal.borrow_mut().push_focus(true);
        });
    }

    /// The keyboard left the surface: the accent's flag goes down, the
    /// redraw queues its removal, and the focus loss is reported.
    pub fn left(&mut self) {
        let poisoned = self.links.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.links.focused.set(false);
            (self.links.queue_draw)();
            self.links.terminal.borrow_mut().push_focus(false);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::term::{DecodedPng, PngDecoder, PtySink, Terminal};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
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

    /// How often the redraw closure ran.
    type Redraws = Rc<Cell<u32>>;

    /// A focus side over a real display-free terminal (40x24 at 8x16) with
    /// focus reporting enabled, plus the flag, the redraw counter and the
    /// pty bytes.
    type FocusFixture = (FocusSide, Rc<Cell<bool>>, Redraws, Arc<Mutex<Vec<u8>>>);

    fn focus_side() -> FocusFixture {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(
            GuardPoisoned::new(),
            RecordingSink(Arc::clone(&writes)),
            NoDecoder,
            || (),
        );
        assert!(terminal.push_size(40, 24, 8, 16));
        terminal.push_pty_data(b"\x1b[?1004h");
        let focused = Rc::new(Cell::new(false));
        let redraws: Redraws = Rc::new(Cell::new(0));
        let redraw_count = Rc::clone(&redraws);
        let links = SeatLinks {
            terminal: Rc::new(RefCell::new(terminal)),
            draw_offset: Rc::new(|| 0.0),
            focused: Rc::clone(&focused),
            queue_draw: Rc::new(move || redraw_count.set(redraw_count.get() + 1)),
            poisoned: GuardPoisoned::new(),
        };
        (FocusSide::new(links), focused, redraws, writes)
    }

    fn take(writes: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
        std::mem::take(&mut *writes.lock().expect("sink lock"))
    }

    /// An enter turns the focused flag on, queues the redraw and reports the
    /// focus gain; the leave undoes all three.
    #[test]
    fn enter_and_leave_drive_the_flag_the_redraw_and_the_reports() {
        let (mut focus, focused, redraws, writes) = focus_side();
        focus.entered();
        assert!(focused.get(), "the enter set the flag");
        assert_eq!(redraws.get(), 1, "the enter queued one redraw");
        assert_eq!(take(&writes), b"\x1b[I");

        focus.left();
        assert!(!focused.get(), "the leave cleared the flag");
        assert_eq!(redraws.get(), 2, "the leave queued one redraw");
        assert_eq!(take(&writes), b"\x1b[O");
    }

    /// Only actual changes are reported: a second enter with no leave in
    /// between queues the redraw but writes nothing, the encoder's balanced
    /// report rule.
    #[test]
    fn a_second_enter_reports_nothing() {
        let (mut focus, focused, _redraws, writes) = focus_side();
        focus.entered();
        focus.entered();
        assert!(focused.get());
        assert_eq!(take(&writes), b"\x1b[I", "only the first enter reported");
    }

    /// Without mode 1004 the flags and redraws still run, but the pty
    /// receives nothing: the encoder gates the reports.
    #[test]
    fn the_reports_are_gated_by_mode_1004() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(
            GuardPoisoned::new(),
            RecordingSink(Arc::clone(&writes)),
            NoDecoder,
            || (),
        );
        assert!(terminal.push_size(40, 24, 8, 16));
        let focused = Rc::new(Cell::new(false));
        let links = SeatLinks {
            terminal: Rc::new(RefCell::new(terminal)),
            draw_offset: Rc::new(|| 0.0),
            focused: Rc::clone(&focused),
            queue_draw: Rc::new(|| ()),
            poisoned: GuardPoisoned::new(),
        };
        let mut focus = FocusSide::new(links);
        focus.entered();
        assert!(focused.get(), "the flag runs without the mode");
        assert_eq!(take(&writes), Vec::new(), "no report without mode 1004");
    }

    /// A panicking hook body is caught and latches the shared flag (D5), and
    /// a latched side runs no further hooks: the enter and the leave both
    /// short-circuit, so the flag never moves.
    #[test]
    fn a_panicking_hook_is_contained() {
        let (mut side, focused, poisoned) = latched_side();
        side.entered();
        assert!(poisoned.is_poisoned(), "the latch stays latched");
        assert!(!focused.get(), "the latched enter set nothing");
        side.left();
        assert!(!focused.get(), "the latched leave set nothing");
    }

    /// A focus side whose latch starts latched (D5's short-circuit path),
    /// with its flag readable.
    fn latched_side() -> (FocusSide, Rc<Cell<bool>>, GuardPoisoned) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let terminal = Terminal::new(
            GuardPoisoned::new(),
            RecordingSink(writes),
            NoDecoder,
            || (),
        );
        let poisoned = GuardPoisoned::latched();
        let focused = Rc::new(Cell::new(false));
        let links = SeatLinks {
            terminal: Rc::new(RefCell::new(terminal)),
            draw_offset: Rc::new(|| 0.0),
            focused: Rc::clone(&focused),
            queue_draw: Rc::new(|| ()),
            poisoned: poisoned.clone(),
        };
        (FocusSide::new(links), focused, poisoned)
    }
}
