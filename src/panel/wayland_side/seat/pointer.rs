//! The pointer half of the seat side (replace-gtk-with-wayland D8): the
//! pointer events the toolkit's `PointerHandler` delivers are
//! translated into the mouse and scroll pushes the terminal's encoders
//! take.
//!
//! The translation core is the pure [`PointerSide::handle`]: an event kind,
//! the event's surface position and the width tween's draw offset go in, the
//! terminal pushes come out. The toolkit's [`sctk kind`] carries no Wayland
//! object, so the tests construct it directly and the rules run without a
//! display (`port-to-rust` D10); the live-compositor behaviour is exercised
//! on niri.
//!
//! The axis mapping follows the Wayland pointer protocol: a frame with
//! wheel motion — a `value120` sum, or a discrete step on a pre-v8 pointer —
//! reports wheel units divided out of the 120ths (`value120/120`), and a
//! frame with only continuous motion reports the raw surface-space values as
//! surface units, which the terminal counts a tenth of a notch each. Wheel
//! motion wins the frame; the terminal's own notch accumulator absorbs the
//! fractional wheel units of a high-resolution wheel, so no second
//! accumulator lives here.
//!
//! [`sctk kind`]: smithay_client_toolkit::seat::pointer::PointerEventKind

use smithay_client_toolkit::seat::pointer::{
    AxisScroll, BTN_EXTRA, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE, PointerEventKind,
};

use crate::guard::guard;
use crate::term::input::{MouseAction, MouseButton, ScrollUnit};

use super::SeatLinks;
use super::cursor::CursorShape;

/// What one pointer event translates into: the terminal push it feeds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerOutcome {
    /// A [`crate::term::input::MouseAction`] push: a press, a release or a
    /// motion at the adjusted surface position.
    Mouse {
        /// The action the event describes.
        action: MouseAction,
        /// The x position, the draw offset already subtracted.
        x: f64,
        /// The y position.
        y: f64,
        /// The button, [`MouseButton::Unknown`] for motion.
        button: MouseButton,
    },
    /// A scroll push: one axis pair in wheel or surface units.
    Scroll {
        /// The x position, the draw offset already subtracted.
        x: f64,
        /// The y position.
        y: f64,
        /// The horizontal delta in the outcome's unit.
        dx: f64,
        /// The vertical delta in the outcome's unit.
        dy: f64,
        /// The unit the deltas are measured in.
        unit: ScrollUnit,
    },
}

/// The pointer side's dispatch: the cursor-shape device the
/// pointer capability's bind installs (an
/// absent one until then) and the pure translation over it.
#[derive(Debug, Default)]
pub struct PointerSide {
    cursor: CursorShape,
}

impl PointerSide {
    /// Install the cursor-shape device the capability bind produced. A seat
    /// side starts with an absent device, which sets no shape.
    pub fn set_cursor(&mut self, cursor: CursorShape) {
        self.cursor = cursor;
    }

    /// Translate one pointer event into the terminal pushes it feeds. Pure:
    /// no state, so a repeated call answers the same. An associated
    /// function: it reads no pointer-side state.
    ///
    /// Enter and leave translate to no push — they have no mouse
    /// counterpart — and only set the cursor shape (the
    /// [`PointerSide::frame`]). The event's position already tracks the
    /// pointer: the toolkit updates it on enter and motion, so a
    /// release and an axis report the last recorded position.
    #[must_use]
    pub fn handle(
        kind: &PointerEventKind,
        position: (f64, f64),
        draw_offset: f64,
    ) -> Vec<PointerOutcome> {
        // Only the x axis shifts with the tween's draw offset (`x -
        // draw_offset`): the reports must land on the drawn grid.
        let (x, y) = position;
        let x = x - draw_offset;
        match kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Leave { .. } => Vec::new(),
            PointerEventKind::Motion { .. } => vec![PointerOutcome::Mouse {
                action: MouseAction::Motion,
                x,
                y,
                button: MouseButton::Unknown,
            }],
            PointerEventKind::Press { button, .. } => vec![PointerOutcome::Mouse {
                action: MouseAction::Press,
                x,
                y,
                button: button_from_evdev(*button),
            }],
            PointerEventKind::Release { button, .. } => vec![PointerOutcome::Mouse {
                action: MouseAction::Release,
                x,
                y,
                button: button_from_evdev(*button),
            }],
            PointerEventKind::Axis {
                horizontal,
                vertical,
                ..
            } => scroll_outcome(x, y, horizontal, vertical),
        }
    }

    /// One pointer event of a frame, dispatched: the cursor shape on enter
    /// and the translation's pushes into the terminal, all under
    /// the shared guard (D5). `mods` are the keyboard side's last modifier
    /// report — Wayland pointer events carry none — and reach the mouse and
    /// scroll encoders with the event.
    pub fn frame(
        &mut self,
        kind: &PointerEventKind,
        position: (f64, f64),
        links: &SeatLinks,
        mods: crate::term::input::Modifiers,
    ) {
        let poisoned = links.poisoned.clone();
        let _ = guard(&poisoned, || {
            if let PointerEventKind::Enter { serial } = kind {
                self.cursor.set_default(*serial);
            }
            for outcome in Self::handle(kind, position, links.draw_offset.get()) {
                match outcome {
                    PointerOutcome::Mouse {
                        action,
                        x,
                        y,
                        button,
                    } => {
                        links
                            .terminal
                            .borrow_mut()
                            .push_mouse(action, x, y, button, mods);
                    }
                    PointerOutcome::Scroll { x, y, dx, dy, unit } => {
                        links
                            .terminal
                            .borrow_mut()
                            .push_scroll(x, y, dx, dy, unit, mods);
                    }
                }
            }
        });
    }
}

/// Translate an evdev button code onto the encoder's button:
/// `BTN_LEFT`/`BTN_MIDDLE`/`BTN_RIGHT` map to the primary/middle/secondary
/// numbers, the back button `BTN_SIDE` to the eighth and the forward button
/// `BTN_EXTRA` to the ninth (the button numbers past the old 4-7 scroll
/// ones). `BTN_FORWARD`, `BTN_BACK` and `BTN_TASK` have no encoder number
/// and stay [`MouseButton::Unknown`].
#[must_use]
pub fn button_from_evdev(code: u32) -> MouseButton {
    match code {
        BTN_LEFT => MouseButton::Left,
        BTN_MIDDLE => MouseButton::Middle,
        BTN_RIGHT => MouseButton::Right,
        BTN_SIDE => MouseButton::Eight,
        BTN_EXTRA => MouseButton::Nine,
        _ => MouseButton::Unknown,
    }
}

/// The wheel delta of one axis in whole notches: the frame's `value120` sum
/// divided out of the 120ths, or — on a pre-v8 pointer whose compositor
/// reports discrete steps instead — the step count itself, one notch per
/// step. Zero when the axis carries neither.
#[must_use]
fn wheel_delta(axis: &AxisScroll) -> f64 {
    if axis.value120 != 0 {
        f64::from(axis.value120) / 120.0
    } else if axis.discrete != 0 {
        f64::from(axis.discrete)
    } else {
        0.0
    }
}

/// The scroll push for one axis frame. Wheel motion wins the frame — both
/// axes go through the wheel units, and the continuous values a wheel frame
/// also carries are dropped. Without wheel motion, the continuous
/// surface-space values are the surface units (a touchpad swipe, or a wheel
/// behind a compositor that reports neither `value120` nor discrete steps,
/// whose per-notch value the terminal's tenth-of-a-notch count still turns
/// into one notch). A frame with neither — an axis stop, or an empty merge —
/// pushes nothing: the terminal would accumulate a zero delta.
#[must_use]
fn scroll_outcome(
    x: f64,
    y: f64,
    horizontal: &AxisScroll,
    vertical: &AxisScroll,
) -> Vec<PointerOutcome> {
    let dx = wheel_delta(horizontal);
    let dy = wheel_delta(vertical);
    if dx != 0.0 || dy != 0.0 {
        return vec![PointerOutcome::Scroll {
            x,
            y,
            dx,
            dy,
            unit: ScrollUnit::Wheel,
        }];
    }
    let (dx, dy) = (horizontal.absolute, vertical.absolute);
    if dx != 0.0 || dy != 0.0 {
        return vec![PointerOutcome::Scroll {
            x,
            y,
            dx,
            dy,
            unit: ScrollUnit::Surface,
        }];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay_client_toolkit::seat::pointer::{BTN_BACK, BTN_FORWARD, BTN_TASK};

    /// One axis frame with the given continuous, discrete and 120th values.
    fn axis(absolute: f64, discrete: i32, value120: i32) -> AxisScroll {
        AxisScroll {
            absolute,
            discrete,
            value120,
            relative_direction: None,
            stop: false,
        }
    }

    fn kind_press(button: u32) -> PointerEventKind {
        PointerEventKind::Press {
            time: 0,
            button,
            serial: 0,
        }
    }

    fn kind_release(button: u32) -> PointerEventKind {
        PointerEventKind::Release {
            time: 0,
            button,
            serial: 0,
        }
    }

    fn kind_axis(horizontal: AxisScroll, vertical: AxisScroll) -> PointerEventKind {
        PointerEventKind::Axis {
            time: 0,
            horizontal,
            vertical,
            source: None,
        }
    }

    /// One outcome helper: a single push is the only shape these tests
    /// produce.
    fn one(_side: PointerSide, kind: &PointerEventKind, draw_offset: f64) -> PointerOutcome {
        let outcomes = PointerSide::handle(kind, (50.0, 20.0), draw_offset);
        assert_eq!(outcomes.len(), 1, "one push per event");
        outcomes[0]
    }

    /// The evdev buttons map onto the encoder's numbers:
    /// `BTN_LEFT`/`BTN_MIDDLE`/`BTN_RIGHT`, back (`BTN_SIDE`) as the
    /// eighth and forward (`BTN_EXTRA`) as the ninth button, everything else
    /// unknown.
    #[test]
    fn evdev_buttons_map_like_the_composition() {
        assert_eq!(button_from_evdev(BTN_LEFT), MouseButton::Left);
        assert_eq!(button_from_evdev(BTN_MIDDLE), MouseButton::Middle);
        assert_eq!(button_from_evdev(BTN_RIGHT), MouseButton::Right);
        assert_eq!(button_from_evdev(BTN_SIDE), MouseButton::Eight);
        assert_eq!(button_from_evdev(BTN_EXTRA), MouseButton::Nine);
        // The other codes the toolkit names have no encoder number.
        assert_eq!(button_from_evdev(BTN_FORWARD), MouseButton::Unknown);
        assert_eq!(button_from_evdev(BTN_BACK), MouseButton::Unknown);
        assert_eq!(button_from_evdev(BTN_TASK), MouseButton::Unknown);
        assert_eq!(button_from_evdev(0x118), MouseButton::Unknown);
    }

    /// Motion and presses carry the draw offset on x only: the tween's
    /// drawing shift moves the grid against the docked edge, and the mouse
    /// position follows the drawn grid.
    #[test]
    fn motion_and_presses_adjust_x_by_the_draw_offset() {
        let motion = PointerEventKind::Motion { time: 0 };
        assert_eq!(
            one(PointerSide::default(), &motion, 7.0),
            PointerOutcome::Mouse {
                action: MouseAction::Motion,
                x: 43.0,
                y: 20.0,
                button: MouseButton::Unknown,
            }
        );
        assert_eq!(
            one(PointerSide::default(), &kind_press(BTN_LEFT), 7.0),
            PointerOutcome::Mouse {
                action: MouseAction::Press,
                x: 43.0,
                y: 20.0,
                button: MouseButton::Left,
            }
        );
        // Zero offset is the plain position.
        assert_eq!(
            one(PointerSide::default(), &kind_press(BTN_LEFT), 0.0),
            PointerOutcome::Mouse {
                action: MouseAction::Press,
                x: 50.0,
                y: 20.0,
                button: MouseButton::Left,
            }
        );
    }

    /// A release carries its button and position.
    #[test]
    fn a_release_carries_its_button() {
        assert_eq!(
            one(PointerSide::default(), &kind_release(BTN_RIGHT), 0.0),
            PointerOutcome::Mouse {
                action: MouseAction::Release,
                x: 50.0,
                y: 20.0,
                button: MouseButton::Right,
            }
        );
    }

    /// Enter and leave translate to no push.
    #[test]
    fn enter_and_leave_produce_no_pushes() {
        let enter = PointerEventKind::Enter { serial: 7 };
        let leave = PointerEventKind::Leave { serial: 7 };
        assert_eq!(PointerSide::handle(&enter, (50.0, 20.0), 0.0), Vec::new());
        assert_eq!(PointerSide::handle(&leave, (50.0, 20.0), 0.0), Vec::new());
    }

    /// A wheel frame's `value120` sums divide into wheel notches: 120 is one
    /// notch, 240 two, and a high-resolution wheel's fraction accumulates in
    /// the terminal's own notch accumulator.
    #[test]
    fn value120_sums_map_to_wheel_units() {
        for (value120, expected) in [(120, 1.0), (240, 2.0), (15, 0.125), (-120, -1.0)] {
            let side = PointerSide::default();
            let kind = kind_axis(axis(0.0, 0, 0), axis(0.0, 0, value120));
            assert_eq!(
                one(side, &kind, 0.0),
                PointerOutcome::Scroll {
                    x: 50.0,
                    y: 20.0,
                    dx: 0.0,
                    dy: expected,
                    unit: ScrollUnit::Wheel,
                },
                "value120 of {value120}"
            );
        }
    }

    /// A horizontal wheel frame maps the same way onto dx.
    #[test]
    fn a_horizontal_wheel_maps_onto_dx() {
        let side = PointerSide::default();
        let kind = kind_axis(axis(0.0, 0, -120), axis(0.0, 0, 0));
        assert_eq!(
            one(side, &kind, 0.0),
            PointerOutcome::Scroll {
                x: 50.0,
                y: 20.0,
                dx: -1.0,
                dy: 0.0,
                unit: ScrollUnit::Wheel,
            }
        );
    }

    /// A pre-v8 pointer's discrete steps are wheel notches, one notch per
    /// step.
    #[test]
    fn discrete_steps_map_to_wheel_units() {
        let side = PointerSide::default();
        let kind = kind_axis(axis(0.0, 0, 0), axis(0.0, 2, 0));
        assert_eq!(
            one(side, &kind, 0.0),
            PointerOutcome::Scroll {
                x: 50.0,
                y: 20.0,
                dx: 0.0,
                dy: 2.0,
                unit: ScrollUnit::Wheel,
            }
        );
    }

    /// Continuous values — a touchpad swipe — are surface units, which the
    /// terminal counts a tenth of a notch each.
    #[test]
    fn continuous_values_map_to_surface_units() {
        let side = PointerSide::default();
        let kind = kind_axis(axis(0.0, 0, 0), axis(37.5, 0, 0));
        assert_eq!(
            one(side, &kind, 0.0),
            PointerOutcome::Scroll {
                x: 50.0,
                y: 20.0,
                dx: 0.0,
                dy: 37.5,
                unit: ScrollUnit::Surface,
            }
        );
    }

    /// Wheel motion wins the frame: the continuous values a wheel frame also
    /// carries are dropped.
    #[test]
    fn a_wheel_frame_wins_over_continuous_values() {
        let side = PointerSide::default();
        let kind = kind_axis(axis(5.0, 0, 0), axis(10.0, 0, 120));
        assert_eq!(
            one(side, &kind, 0.0),
            PointerOutcome::Scroll {
                x: 50.0,
                y: 20.0,
                dx: 0.0,
                dy: 1.0,
                unit: ScrollUnit::Wheel,
            }
        );
    }

    /// An axis stop, or a frame with neither wheel motion nor continuous
    /// values, pushes nothing: the terminal would accumulate a zero delta.
    #[test]
    fn a_stop_frame_pushes_nothing() {
        assert_eq!(
            PointerSide::handle(
                &kind_axis(axis(0.0, 0, 0), axis(0.0, 0, 0)),
                (50.0, 20.0),
                0.0
            ),
            Vec::new()
        );
    }

    /// The translation is pure: a repeated call answers the same, so a
    /// compositor frame replay changes nothing beyond its pushes.
    #[test]
    fn the_translation_is_stateless() {
        let kind = kind_axis(axis(0.0, 0, 0), axis(0.0, 0, 120));
        let first = PointerSide::handle(&kind, (50.0, 20.0), 0.0);
        let second = PointerSide::handle(&kind, (50.0, 20.0), 0.0);
        assert_eq!(first, second);
    }

    /// A panicking frame body is caught and latches the shared flag (D5),
    /// and a latched side runs no further frames.
    #[test]
    fn a_panicking_frame_is_contained() {
        use crate::guard::Poisoned as GuardPoisoned;
        use crate::term::{DecodedPng, PngDecoder, PtySink, Terminal};
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        use std::sync::{Arc, Mutex};

        struct RecordingSink(Arc<Mutex<Vec<u8>>>);

        impl PtySink for RecordingSink {
            fn write_pty(&mut self, data: &[u8]) {
                self.0.lock().expect("sink lock").extend_from_slice(data);
            }
        }

        struct NoDecoder;

        impl PngDecoder for NoDecoder {
            fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
                None
            }
        }

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
            draw_offset: Rc::new(Cell::new(0.0)),
            focused: Rc::new(Cell::new(false)),
            queue_draw: Rc::new(Cell::new(false)),
            poisoned: poisoned.clone(),
        };
        links
            .terminal
            .borrow_mut()
            .push_pty_data(b"\x1b[?1000h\x1b[?1006h");
        let mut side = PointerSide::default();
        side.frame(
            &kind_press(BTN_LEFT),
            (50.0, 20.0),
            &links,
            crate::term::input::Modifiers::NONE,
        );
        assert_eq!(
            take_writes(&writes),
            b"\x1b[<0;7;2M",
            "the left press at (50, 20) reports cell (7, 2) on the 8x16 grid"
        );

        poisoned.latch();
        side.frame(
            &kind_release(BTN_LEFT),
            (50.0, 20.0),
            &links,
            crate::term::input::Modifiers::NONE,
        );
        assert_eq!(
            take_writes(&writes),
            Vec::new(),
            "a latched side runs no frames"
        );
    }

    fn take_writes(writes: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> Vec<u8> {
        std::mem::take(&mut *writes.lock().expect("sink lock"))
    }
}
