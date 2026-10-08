//! The seat handlers on the panel thread (replace-gtk-with-wayland D8): the
//! sctk `SeatHandler`, `KeyboardHandler` and `PointerHandler` impls the
//! connection's event queue dispatches into, routing the seat's events into
//! the [`SeatSide`] the session holds — the keyboard, pointer, focus and
//! cursor-shape hooks — plus the capability bookkeeping that creates and
//! drops the keyboard, the pointer and the pointer's cursor-shape device.
//!
//! The routing lives in the pure [`route_seat_event`], which the tests
//! drive without a display (`port-to-rust` D10): a seat event belongs to
//! the panel when its surface is the panel's — the reserve takes neither
//! keyboard nor pointer (its empty input region, replace-gtk-with-wayland D3) — and the hook
//! call runs under the shared guard (D5). The live handler bodies stay a
//! few lines each; everything touching a live seat — the capability
//! objects, the toolkit's keymap and modifier events, the repeat timer —
//! is exercised on niri.
//!
//! The toolkit's `wl_seat`, `wl_keyboard` and `wl_pointer` dispatch needs
//! no macros of its own: the blanket `delegate_dispatch2!` in
//! [`super::handlers`] already covers every `Dispatch2` impl the toolkit
//! provides once these traits are implemented here.

use smithay_client_toolkit::seat::keyboard::{
    self, KeyEvent, KeyboardHandler, Modifiers, RawModifiers, RepeatInfo,
};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use wayland_client::protocol::wl_surface;
use wayland_client::protocol::{wl_keyboard::WlKeyboard, wl_pointer::WlPointer, wl_seat::WlSeat};
use wayland_client::{Connection, QueueHandle};

use crate::guard::{Poisoned, guard};

use super::super::seat::SeatSide;
use super::super::seat::cursor::CursorShape;
use super::PanelState;

/// What one capability event asks of the objects the session holds (row
/// 8.1): create the object on an arriving capability nothing is held for,
/// drop the held objects on a leaving one, and change nothing otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapabilityStep {
    /// Create the capability's object.
    Create,
    /// Drop the held object(s) of the capability.
    Drop,
    /// Nothing: the object is already in the wanted state.
    Hold,
}

/// The step for one capability event, from whether the session holds the
/// capability's object and whether the capability is arriving or leaving.
/// The toolkit reports a capability only when it actually changed, so the
/// hold cells are the belt for the handler's own bookkeeping: a repeated
/// arrival never re-creates the keyboard or the pointer. The pointer's
/// step covers its cursor-shape device too: a pointer drop
/// takes the device, a pointer create re-binds it.
#[must_use]
pub(crate) fn capability_step(held: bool, arriving: bool) -> CapabilityStep {
    match (held, arriving) {
        (false, true) => CapabilityStep::Create,
        (true, false) => CapabilityStep::Drop,
        (true, true) | (false, false) => CapabilityStep::Hold,
    }
}

/// The seat's capability objects: the keyboard and the pointer
/// the capability events created, held so a removed capability drops its
/// object and a teardown drops both before the connection ends. The
/// pointer's cursor-shape device is deliberately not held here — it lives
/// in the seat side's pointer half, and a second handle would
/// keep the device alive after the seat side's reset drops it.
#[derive(Debug, Default)]
pub(crate) struct SeatObjects {
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
}

impl SeatObjects {
    /// Hold the keyboard a keyboard capability created, replacing — and so
    /// dropping, its repeat source included — any previous one.
    fn hold_keyboard(&mut self, keyboard: WlKeyboard) {
        self.keyboard = Some(keyboard);
    }

    /// Hold the pointer a pointer capability created, replacing any
    /// previous one.
    fn hold_pointer(&mut self, pointer: WlPointer) {
        self.pointer = Some(pointer);
    }

    /// Drop the keyboard. The toolkit's repeat data dies with it, which
    /// removes the repeat timer; the seat side's own repeat gate
    /// is cleared by the caller's `keyboard_left`.
    fn release_keyboard(&mut self) {
        self.keyboard = None;
    }

    /// Drop the pointer. The caller resets the seat side's cursor shape,
    /// which drops the pointer's cursor-shape device with it.
    fn release_pointer(&mut self) {
        self.pointer = None;
    }

    /// Release everything, the keyboard first so its repeat timer goes
    /// before the pointer and its cursor-shape device. A removed seat and
    /// the teardown end here.
    pub(crate) fn release_all(&mut self) {
        self.release_keyboard();
        self.release_pointer();
    }
}

/// Route one seat event into the seat side: the hook runs when
/// the event belongs to the panel (`panel` — the reserve takes neither
/// keyboard nor pointer, and a torn-down session has no surfaces) and the
/// session is bound, always under the shared guard (D5). Returns the
/// hook's result, `None` when it did not run. Pure over the surface
/// decision and the seat side, so the tests drive the routing without a
/// display (`port-to-rust` D10); the live decision is one comparison in
/// the handler.
fn route_seat_event<R>(
    side: Option<&mut SeatSide>,
    panel: bool,
    poisoned: &Poisoned,
    hook: impl FnOnce(&mut SeatSide) -> R,
) -> Option<R> {
    if !panel {
        return None;
    }
    guard(poisoned, move || side.map(hook)).ok().flatten()
}

/// Run one seat hook on the panel's seat side: the events that carry no
/// surface of their own — the key, modifier, keymap and repeat-info
/// events of the panel's own keyboard — always belong to the panel, so
/// only the bound session and the latch decide. Returns the hook's
/// result, `None` when the session is unbound or the latch is set.
fn seat_hook<R>(state: &mut PanelState, hook: impl FnOnce(&mut SeatSide) -> R) -> Option<R> {
    let poisoned = state.poisoned.clone();
    let side = state.session.as_mut().map(|session| &mut session.seat_side);
    route_seat_event(side, true, &poisoned, hook)
}

impl PanelState {
    /// The seat side the hooks route into, when the session is bound.
    fn seat_side(&mut self) -> Option<&mut SeatSide> {
        self.session.as_mut().map(|session| &mut session.seat_side)
    }

    /// Whether a seat event's surface is the panel's: the reserve takes
    /// neither keyboard nor pointer (its empty input region, replace-gtk-with-wayland D3) and
    /// a torn-down session has no surfaces left to match.
    fn on_panel_surface(&self, surface: &wl_surface::WlSurface) -> bool {
        self.session
            .as_ref()
            .and_then(|session| session.surfaces.as_ref())
            .is_some_and(|surfaces| surfaces.is_panel_surface(surface))
    }

    /// One capability event: create the capability's object on
    /// arrival, drop the held one on removal, and re-create nothing. The
    /// capability events dispatch from the loop, after the bind stored the
    /// session.
    fn on_capability(
        &mut self,
        qh: &QueueHandle<Self>,
        seat: &WlSeat,
        capability: Capability,
        arriving: bool,
    ) {
        let held = match capability {
            Capability::Keyboard => self
                .session
                .as_ref()
                .is_some_and(|session| session.seat_objects.keyboard.is_some()),
            Capability::Pointer => self
                .session
                .as_ref()
                .is_some_and(|session| session.seat_objects.pointer.is_some()),
            // The panel takes no touch input; the capability changes
            // nothing here. Future capabilities change nothing either —
            // the enum is non-exhaustive and the panel grows no new input
            // half without a change that says so.
            _ => return,
        };
        match capability_step(held, arriving) {
            CapabilityStep::Create => self.create_capability(qh, seat, capability),
            CapabilityStep::Drop => self.drop_capability(capability),
            CapabilityStep::Hold => {}
        }
    }

    /// Create the object one arriving capability asks for.
    fn create_capability(&mut self, qh: &QueueHandle<Self>, seat: &WlSeat, capability: Capability) {
        match capability {
            Capability::Keyboard => {
                let Some(session) = self.session.as_mut() else {
                    return;
                };
                // The repeat callback: the toolkit's repeat timer
                // runs it with the dispatch state, and the repeat becomes a
                // `key_repeated` hook call under the shared guard (D5) —
                // the same hook the toolkit's own repeat delivery takes
                // (`repeat_key` below), so both roads agree.
                let callback: keyboard::repeat::RepeatCallback<PanelState> = Box::new(
                    |state: &mut PanelState, _keyboard: &WlKeyboard, event: KeyEvent| {
                        seat_hook(state, |side| side.key_repeated(&event));
                    },
                );
                // A seat that reports the capability but refuses the
                // object, or a dead one, leaves the panel without that
                // half's input, the way an absent capability degrades
                // (port-to-rust D3) — the library never exits over an
                // environment failure.
                if let Ok(keyboard) = session.seat.get_keyboard_with_repeat(
                    qh,
                    seat,
                    None,
                    session.loop_handle.clone(),
                    callback,
                ) {
                    session.seat_objects.hold_keyboard(keyboard);
                }
            }
            Capability::Pointer => {
                let Some(session) = self.session.as_mut() else {
                    return;
                };
                // The pointer without a theme: the cursor shape comes from
                // the cursor-shape protocol, not from loaded
                // surfaces, so `get_pointer` needs no shm or theme. The
                // same degradation as the keyboard's covers a refusal.
                if let Ok(pointer) = session.seat.get_pointer(qh, seat) {
                    // The cursor-shape device is created once per pointer,
                    // at the capability's arrival; the enter
                    // serial the shape request needs travels in the enter
                    // event the pointer handler routes.
                    session
                        .seat_side
                        .bind_cursor_shape(&session.globals, qh, &pointer);
                    session.seat_objects.hold_pointer(pointer);
                }
            }
            _ => {}
        }
    }

    /// Drop the objects one leaving capability holds.
    fn drop_capability(&mut self, capability: Capability) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match capability {
            Capability::Keyboard => {
                // The dropped keyboard takes the toolkit's repeat source
                // with it; `keyboard_left` stops the seat side's own repeat
                // gate and reports the focus loss the gone keyboard ends.
                session.seat_objects.release_keyboard();
                session.seat_side.keyboard_left();
            }
            Capability::Pointer => {
                // The cursor-shape device is the pointer's: the
                // reset drops it with the pointer, so no stale device
                // outlives the capability it was made for.
                session.seat_objects.release_pointer();
                session.seat_side.pointer.set_cursor(CursorShape::absent());
            }
            _ => {}
        }
    }

    /// Release the seat's objects at the teardown, in a defined order: the
    /// keyboard first — its repeat source
    /// dies with it, removing the repeat timer — then the
    /// pointer, whose cursor-shape device dies through the seat side's
    /// reset. The connection outlives the loop's last dispatch,
    /// so the destroy requests still reach the compositor. The focus
    /// accent needs no leave report here: the teardown ends the panel and
    /// its child together, and the flag and the encoder die with the
    /// thread.
    pub(crate) fn release_seat_objects(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.seat_objects.release_all();
        session.seat_side.pointer.set_cursor(CursorShape::absent());
    }

    /// A seat is gone — and with it, the toolkit's docs say, every
    /// capability object created from it: drop what we hold and end the
    /// keyboard and pointer state the way their capability removals would.
    fn on_seat_removed(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.seat_objects.release_all();
        session.seat_side.keyboard_left();
        session.seat_side.pointer.set_cursor(CursorShape::absent());
    }
}

impl SeatHandler for PanelState {
    fn seat_state(&mut self) -> &mut SeatState {
        // Only reachable once the session is bound: the seat state is
        // created at the bind and its events dispatch through the queue the
        // loop dispatches, like the registry and output accessors in
        // [`super::handlers`].
        &mut self
            .session
            .as_mut()
            .expect("seat events dispatch only after the bind")
            .seat
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {
        // Nothing to hold yet: the capability events that follow create the
        // keyboard and the pointer (the toolkit's docs).
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.on_capability(qh, &seat, capability, true);
        });
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.on_capability(qh, &seat, capability, false);
        });
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || self.on_seat_removed());
    }
}

impl KeyboardHandler for PanelState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[keyboard::Keysym],
    ) {
        // Only the panel's enter drives the accent and the focus reports;
        // the reserve never takes the keyboard.
        if self.on_panel_surface(surface) {
            seat_hook(self, SeatSide::keyboard_entered);
        }
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        // The leave stops the repeat and drops the accent with its
        // focus-loss report.
        if self.on_panel_surface(surface) {
            seat_hook(self, SeatSide::keyboard_left);
        }
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        seat_hook(self, |side| side.key_pressed(&event));
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        seat_hook(self, |side| side.key_released(&event));
    }

    fn repeat_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        // The compositor's own repeat delivery; the calloop repeat callback
        // covers the compositors that do not send one.
        seat_hook(self, |side| side.key_repeated(&event));
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        raw_modifiers: RawModifiers,
        layout: u32,
    ) {
        seat_hook(self, |side| {
            side.modifiers_updated(raw_modifiers, layout, modifiers);
        });
    }

    fn update_keymap(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        keymap: keyboard::Keymap<'_>,
    ) {
        seat_hook(self, |side| side.keymap_updated(&keymap.as_string()));
    }

    fn update_repeat_info(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        info: RepeatInfo,
    ) {
        seat_hook(self, |side| side.repeat_info_updated(&info));
    }
}

impl PointerHandler for PanelState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            // The reserve takes no pointer (its empty input region): only
            // the panel's events reach the seat side. An enter
            // among them sets the default cursor shape through the seat
            // side's own frame handling — the serial the request
            // needs travels in the event kind.
            let panel = self.on_panel_surface(&event.surface);
            let poisoned = self.poisoned.clone();
            let side = self.seat_side();
            route_seat_event(side, panel, &poisoned, |side| {
                side.pointer_frame(&event.kind, event.position);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Poisoned as GuardPoisoned;
    use crate::layout::{Keyboard, Layout, Side};
    use crate::panel::Inner;
    use crate::panel::handshake::Handshake;
    use crate::term::{DecodedPng, PngDecoder, PtySink, Terminal};
    use smithay_client_toolkit::seat::keyboard::RepeatInfo as SctkRepeatInfo;
    use smithay_client_toolkit::seat::pointer::{BTN_LEFT, PointerEventKind};
    use std::cell::{Cell, RefCell};
    use std::num::{NonZeroU16, NonZeroU32};
    use std::rc::Rc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
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
            keysym: keyboard::Keysym::new(keysym),
            utf8: None,
        }
    }

    /// The seat fixture: a seat side over a real display-free terminal
    /// (40x24 at 8x16) with the test keymap loaded, plus the terminal
    /// handle the tests push mode bytes through, the pty bytes it wrote,
    /// the focus flag and the latch.
    struct SeatFixture {
        side: SeatSide,
        terminal: Rc<RefCell<Terminal>>,
        writes: Arc<Mutex<Vec<u8>>>,
        focused: Rc<Cell<bool>>,
        poisoned: GuardPoisoned,
    }

    fn seat_fixture() -> SeatFixture {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let terminal = Rc::new(RefCell::new(Terminal::new(
            GuardPoisoned::new(),
            RecordingSink(Arc::clone(&writes)),
            NoDecoder,
            || (),
        )));
        assert!(terminal.borrow_mut().push_size(40, 24, 8, 16));
        let focused = Rc::new(Cell::new(false));
        let poisoned = GuardPoisoned::new();
        let links = super::super::super::seat::SeatLinks {
            // The side and the fixture share the one terminal: the links
            // hold one clone, the tests hold the other.
            terminal: Rc::clone(&terminal),
            draw_offset: Rc::new(|| 0.0),
            focused: Rc::clone(&focused),
            queue_draw: Rc::new(|| ()),
            poisoned: poisoned.clone(),
        };
        let mut side = SeatSide::new(links);
        side.keymap_updated(TEST_KEYMAP);
        SeatFixture {
            side,
            terminal,
            writes,
            focused,
            poisoned,
        }
    }

    fn take(writes: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
        std::mem::take(&mut *writes.lock().expect("sink lock"))
    }

    /// One toolkit pointer press for the routing tests.
    fn press_kind() -> PointerEventKind {
        PointerEventKind::Press {
            time: 0,
            button: BTN_LEFT,
            serial: 0,
        }
    }

    /// The capability step table: create on an arrival nothing is held
    /// for, drop on a removal of a held capability, and change nothing
    /// otherwise — a repeated arrival re-creates nothing, and a removal
    /// with nothing held drops nothing.
    #[test]
    fn the_capability_step_table_covers_the_four_cells() {
        assert_eq!(capability_step(false, true), CapabilityStep::Create);
        assert_eq!(capability_step(true, false), CapabilityStep::Drop);
        assert_eq!(
            capability_step(true, true),
            CapabilityStep::Hold,
            "a repeated arrival re-creates nothing"
        );
        assert_eq!(capability_step(false, false), CapabilityStep::Hold);
    }

    /// A pointer event whose surface decision says panel routes into the
    /// seat side and reaches the pty; the same event for the reserve —
    /// which takes no pointer — changes nothing (the surface filter).
    #[test]
    fn a_pointer_event_routes_for_the_panel_and_not_for_the_reserve() {
        let mut fixture = seat_fixture();
        fixture
            .terminal
            .borrow_mut()
            .push_pty_data(b"\x1b[?1003h\x1b[?1006h");
        route_seat_event(Some(&mut fixture.side), true, &fixture.poisoned, |side| {
            side.pointer_frame(&press_kind(), (50.0, 20.0));
        });
        assert_eq!(take(&fixture.writes), b"\x1b[<0;7;2M", "the panel press");

        route_seat_event(Some(&mut fixture.side), false, &fixture.poisoned, |side| {
            side.pointer_frame(&press_kind(), (50.0, 20.0));
        });
        assert_eq!(
            take(&fixture.writes),
            Vec::new(),
            "the reserve takes no pointer"
        );

        // An unbound session has no seat side to route into either.
        route_seat_event(None, true, &fixture.poisoned, |side| {
            side.pointer_frame(&press_kind(), (50.0, 20.0));
        });
        assert_eq!(take(&fixture.writes), Vec::new());
    }

    /// A keyboard enter and leave route through the same panel gate (row
    /// 5.4): the enter turns the focus flag on, the leave turns it off,
    /// and a reserve event does neither.
    #[test]
    fn the_keyboard_focus_hooks_route_through_the_panel_gate() {
        let mut fixture = seat_fixture();
        fixture.terminal.borrow_mut().push_pty_data(b"\x1b[?1004h");
        route_seat_event(Some(&mut fixture.side), true, &fixture.poisoned, |side| {
            side.keyboard_entered();
        });
        assert!(fixture.focused.get(), "the panel enter focused the panel");

        route_seat_event(Some(&mut fixture.side), false, &fixture.poisoned, |side| {
            side.keyboard_entered();
        });
        assert!(fixture.focused.get(), "a reserve enter changes nothing");

        route_seat_event(Some(&mut fixture.side), true, &fixture.poisoned, |side| {
            side.keyboard_left();
        });
        assert!(
            !fixture.focused.get(),
            "the panel leave unfocused the panel"
        );
    }

    /// A routed repeat reaches the pty as a repeat report, and a
    /// latched panel runs no further hooks (D5).
    #[test]
    fn a_routed_repeat_reaches_the_pty_and_the_latch_stops_the_hooks() {
        let mut fixture = seat_fixture();
        fixture.terminal.borrow_mut().push_pty_data(b"\x1b[>11u");
        fixture.side.repeat_info_updated(&SctkRepeatInfo::Repeat {
            rate: NonZeroU32::new(25).expect("test rate"),
            delay: 250,
        });
        fixture
            .side
            .modifiers_updated(RawModifiers::default(), 0, Modifiers::default());
        fixture.side.key_pressed(&key_event(RAW_Q, 0x71));
        assert_eq!(take(&fixture.writes), b"\x1b[113u", "the press went first");

        route_seat_event(Some(&mut fixture.side), true, &fixture.poisoned, |side| {
            side.key_repeated(&key_event(RAW_Q, 0x71));
        });
        assert_eq!(take(&fixture.writes), b"\x1b[113;1:2u", "the repeat report");

        fixture.poisoned.latch();
        route_seat_event(Some(&mut fixture.side), true, &fixture.poisoned, |side| {
            side.key_repeated(&key_event(RAW_Q, 0x71));
        });
        assert_eq!(
            take(&fixture.writes),
            Vec::new(),
            "a latched panel runs no hooks"
        );
    }

    /// `seat_hook` on a headless state runs nothing — no session, no seat
    /// side — and a latched panel runs nothing even with a session, the
    /// two no-run paths the key-event handlers rely on (D5, D10).
    #[test]
    fn the_key_hooks_run_nothing_on_a_headless_or_latched_state() {
        let (tx, _rx) = mpsc::channel();
        let mut state = PanelState::headless(
            Handshake::new(tx),
            Poisoned::new(),
            test_inner(),
            test_startup(),
            crate::layout::CellSize::new(9, 16).expect("test cell size is non-zero"),
        );
        let ran = Cell::new(false);
        let result = seat_hook(&mut state, |_side| {
            ran.set(true);
        });
        assert!(result.is_none(), "a headless state has no seat side");
        assert!(!ran.get(), "no session, no hook");

        state.poisoned.latch();
        let result = seat_hook(&mut state, |_side| {
            ran.set(true);
        });
        assert!(result.is_none(), "a latched panel runs no hooks");
        assert!(!ran.get(), "the latch stopped the hook");
    }

    /// A live handle state like a started panel's, for the state-level
    /// tests.
    fn test_inner() -> Arc<Inner> {
        Arc::new(Inner {
            poisoned: GuardPoisoned::new(),
            live: AtomicBool::new(true),
        })
    }

    /// A startup for the state-level tests; the placeholder fd is fine
    /// because the headless state touches no pty.
    fn test_startup() -> crate::panel::wayland_side::Startup {
        crate::panel::wayland_side::Startup::new(
            -1,
            Layout::new(
                Side::Left,
                NonZeroU16::new(40).expect("test columns"),
                0,
                0,
                0,
                0,
            ),
            Keyboard::OnDemand,
            None,
        )
    }
}
