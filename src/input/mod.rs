//! The GDK input side of pinwin (port-to-rust D3): the controllers attached to
//! the drawing area for key, mouse, scroll and focus, and their translation
//! into the terminal's key/mouse/focus encoders. Ported from `src/input.c`,
//! whose `pinwin_key`/`pinwin_mouse`/`pinwin_scroll`/`pinwin_focus` calls into
//! the Zig core are here direct [`Terminal`] pushes.
//!
//! The GTK-thread wiring lives in [`attach`]; everything it needs is injected
//! through [`InputLinks`], so the module carries no panel state of its own and
//! tests can drive the translation core without GTK (D4, D10). The pure
//! helpers — modifier translation, button mapping and the level-0 keycode
//! lookup — are the GTK-free core the task keeps unit tested.
//!
//! Panics must never cross back into GTK/glib (D5): every signal closure runs
//! its body under `catch_unwind`, latching a poisoned flag. Row 4.2 grows this
//! local guard into the shared helper.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk4::gdk;
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;

use crate::term::Terminal;
use crate::term::input::{KeyAction, KeyInput, Modifiers, MouseAction, MouseButton, ScrollUnit};

/// Everything the controller closures reach outside this module. The links are
/// cloned into every closure; they are all `Rc`/`Arc` handles to GTK-thread
/// state (D4).
#[derive(Clone)]
pub struct InputLinks {
    /// The terminal the encoders push into.
    pub terminal: Rc<RefCell<Terminal>>,
    /// The anim row's draw offset: the drawing shift that keeps the grid
    /// against the docked edge while the surface animates; pointer x is
    /// adjusted by it (`glue_anim_draw_offset`).
    pub draw_offset: Rc<dyn Fn() -> f64>,
    /// Whether the panel holds keyboard focus; render reads it for the focus
    /// accent (`g_focused`).
    pub focused: Rc<Cell<bool>>,
    /// Queue a redraw of the drawing area.
    pub queue_draw: Rc<dyn Fn()>,
    /// Latched when a controller body panicked (D5); the panel consults it.
    pub poisoned: Arc<AtomicBool>,
}

/// Translate GDK modifier state into the encoder's modifier bits
/// (`mods_from_gdk`). Everything GDK can report beyond these five is ignored,
/// exactly like the C.
pub fn mods_from_gdk(state: gdk::ModifierType) -> Modifiers {
    let mut mods = Modifiers::NONE;
    if state.contains(gdk::ModifierType::SHIFT_MASK) {
        mods = mods | Modifiers::SHIFT;
    }
    if state.contains(gdk::ModifierType::CONTROL_MASK) {
        mods = mods | Modifiers::CTRL;
    }
    if state.contains(gdk::ModifierType::ALT_MASK) {
        mods = mods | Modifiers::ALT;
    }
    if state.contains(gdk::ModifierType::SUPER_MASK) {
        mods = mods | Modifiers::SUPER;
    }
    if state.contains(gdk::ModifierType::LOCK_MASK) {
        mods = mods | Modifiers::CAPS_LOCK;
    }
    mods
}

/// Translate a GDK button number into the encoder's button (`pinwin_button_from_gdk`).
/// Anything unrecognised — including the button-less 0 — is [`MouseButton::Unknown`].
pub fn button_from_gdk(button: u32) -> MouseButton {
    match button {
        gdk::BUTTON_PRIMARY => MouseButton::Left,
        gdk::BUTTON_MIDDLE => MouseButton::Middle,
        gdk::BUTTON_SECONDARY => MouseButton::Right,
        8 => MouseButton::Eight,
        9 => MouseButton::Nine,
        _ => MouseButton::Unknown,
    }
}

/// Pick the level-0 keyval for a hardware keycode from `gdk_display_map_keycode`
/// entries `(group, level, keyval)` (`glue_keycode_unshifted_codepoint`'s
/// selection): the entry matching the keyboard layout first, else any level-0
/// entry. `layout` is `gdk_key_event_get_layout` as the key controller last
/// saw it.
pub fn select_unshifted_keyval(entries: &[(i32, i32, u32)], layout: i32) -> Option<u32> {
    for (group, level, keyval) in entries {
        if *group == layout && *level == 0 {
            return Some(*keyval);
        }
    }
    for (_group, level, keyval) in entries {
        if *level == 0 {
            return Some(*keyval);
        }
    }
    None
}

/// `gdk_keyval_to_unicode`, 0 when the keyval has no Unicode answer.
fn keyval_unicode(keyval: u32) -> u32 {
    // SAFETY: a pure keyval table lookup that accepts any u32.
    unsafe { gtk4::gdk::ffi::gdk_keyval_to_unicode(keyval) }
}

/// The level-0 codepoint for a hardware keycode (kitty's "unshifted"):
/// `glue_keycode_unshifted_codepoint` against the default display. 0 when
/// there is no display or no mapping.
fn unshifted_codepoint(keycode: u32, layout: i32) -> u32 {
    let Some(display) = gdk::Display::default() else {
        return 0;
    };
    let Some(entries) = display.map_keycode(keycode) else {
        return 0;
    };
    let entries: Vec<(i32, i32, u32)> = entries
        .into_iter()
        .map(|(key, keyval)| (key.group(), key.level(), keyval.into_glib()))
        .collect();
    select_unshifted_keyval(&entries, layout).map_or(0, keyval_unicode)
}

/// Build the encoder's view of one key event and push it (`on_key` +
/// `pinwin_key`). `event` is the controller's current event, absent for
/// synthetic key events; `layout` carries `g_key_layout` across events.
fn on_key(
    controller: &gtk4::EventControllerKey,
    keyval: u32,
    keycode: u32,
    state: gdk::ModifierType,
    action: KeyAction,
    links: &InputLinks,
    layout: &Cell<i32>,
) {
    let event = controller
        .current_event()
        .and_then(|event| event.downcast::<gdk::KeyEvent>().ok());
    let consumed_mods = event.as_ref().map_or(Modifiers::NONE, |event| {
        mods_from_gdk(event.consumed_modifiers())
    });
    let is_modifier = event.as_ref().is_some_and(|event| event.is_modifier());
    if action == KeyAction::Press
        && let Some(event) = event
    {
        layout.set(event.layout() as i32);
    }
    links.terminal.borrow_mut().push_key(KeyInput {
        action,
        keyval,
        keycode,
        mods: mods_from_gdk(state),
        consumed_mods,
        is_modifier,
        unshifted_codepoint: unshifted_codepoint(keycode, layout.get()),
        keyval_unicode: keyval_unicode(keyval),
    });
}

/// Build the encoder's view of one mouse event. `record` marks the press path,
/// which also updates the position the scroll handler reports (`g_last_x`,
/// `g_last_y`).
fn on_mouse(
    gesture: &gtk4::GestureClick,
    x: f64,
    y: f64,
    action: MouseAction,
    record: bool,
    links: &InputLinks,
    position: &Cell<(f64, f64)>,
) {
    let x = x - (links.draw_offset)();
    if record {
        position.set((x, y));
    }
    let button = button_from_gdk(gesture.current_button());
    let mods = mods_from_gdk(gesture.current_event_state());
    links
        .terminal
        .borrow_mut()
        .push_mouse(action, x, y, button, mods);
}

/// Attach the key, mouse, scroll and focus controllers to the drawing area
/// (`attach_controllers`). Call once, on the GTK thread, after the area exists
/// and the links are composed.
pub fn attach(area: &gtk4::DrawingArea, links: InputLinks) {
    let key = gtk4::EventControllerKey::new();
    key.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let pressed = links.clone();
    let pressed_layout = Rc::new(Cell::new(0i32));
    let released = links.clone();
    let released_layout = pressed_layout.clone();
    key.connect_key_pressed(move |controller, keyval, keycode, state| {
        guarded(&pressed.poisoned, || {
            on_key(
                controller,
                keyval.into_glib(),
                keycode,
                state,
                KeyAction::Press,
                &pressed,
                &pressed_layout,
            );
        });
        // The key controller must not swallow the event: the terminal encodes
        // everything it receives (on_key returned TRUE).
        gtk4::glib::Propagation::Stop
    });
    key.connect_key_released(move |controller, keyval, keycode, state| {
        guarded(&released.poisoned, || {
            on_key(
                controller,
                keyval.into_glib(),
                keycode,
                state,
                KeyAction::Release,
                &released,
                &released_layout,
            );
        });
    });
    area.add_controller(key);

    // Button 0 listens to every button, as the C's
    // gtk_gesture_single_set_button(click, 0) did.
    let click = gtk4::GestureClick::new();
    click.set_button(0);
    // The pointer position the scroll handler reports (g_last_x/g_last_y):
    // presses and motion update it, release does not.
    let position = Rc::new(Cell::new((0.0f64, 0.0f64)));
    let pressed = links.clone();
    let pressed_position = position.clone();
    let released = links.clone();
    let released_position = position.clone();
    click.connect_pressed(move |gesture, _n_press, x, y| {
        guarded(&pressed.poisoned, || {
            on_mouse(
                gesture,
                x,
                y,
                MouseAction::Press,
                true,
                &pressed,
                &pressed_position,
            );
        });
    });
    click.connect_released(move |gesture, _n_press, x, y| {
        guarded(&released.poisoned, || {
            on_mouse(
                gesture,
                x,
                y,
                MouseAction::Release,
                false,
                &released,
                &released_position,
            );
        });
    });
    area.add_controller(click);

    let motion = gtk4::EventControllerMotion::new();
    let motion_links = links.clone();
    let motion_position = position.clone();
    motion.connect_motion(move |controller, x, y| {
        guarded(&motion_links.poisoned, || {
            let x = x - (motion_links.draw_offset)();
            motion_position.set((x, y));
            let mods = mods_from_gdk(controller.current_event_state());
            motion_links.terminal.borrow_mut().push_mouse(
                MouseAction::Motion,
                x,
                y,
                MouseButton::Unknown,
                mods,
            );
        });
    });
    area.add_controller(motion);

    // The flags gate the axes: DISCRETE alone delivers no scroll events.
    // VERTICAL and HORIZONTAL make a wheel notch exactly one unit, which is
    // what the mouse encoder turns into a wheel button.
    let scroll = gtk4::EventControllerScroll::new(
        gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::HORIZONTAL,
    );
    let scroll_links = links.clone();
    let scroll_position = position.clone();
    scroll.connect_scroll(move |controller, dx, dy| {
        guarded(&scroll_links.poisoned, || {
            let (x, y) = scroll_position.get();
            let unit = scroll_unit_from_gdk(controller.unit());
            let mods = mods_from_gdk(controller.current_event_state());
            scroll_links
                .terminal
                .borrow_mut()
                .push_scroll(x, y, dx, dy, unit, mods);
        });
        // The terminal encodes everything it receives (on_scroll returned TRUE).
        gtk4::glib::Propagation::Stop
    });
    area.add_controller(scroll);

    let focus = gtk4::EventControllerFocus::new();
    let entered = links.clone();
    focus.connect_enter(move |_controller| {
        guarded(&entered.poisoned, || {
            entered.focused.set(true);
            (entered.queue_draw)();
            entered.terminal.borrow_mut().push_focus(true);
        });
    });
    let left = links.clone();
    focus.connect_leave(move |_controller| {
        guarded(&left.poisoned, || {
            left.focused.set(false);
            (left.queue_draw)();
            left.terminal.borrow_mut().push_focus(false);
        });
    });
    area.add_controller(focus);
}

/// Map the GDK scroll unit onto the terminal encoder's (`input.c` passes the
/// raw `GdkScrollUnit` through and the Zig port scaled only
/// `PINWIN_SCROLL_UNIT_SURFACE`, `src/input.zig`): only a surface delta counts
/// a tenth of a notch, so a wheel click — and any other unit this gdk build
/// reports, such as a discrete one — is a whole notch. Mapping the catch-all
/// to `Wheel` keeps an unrecognised unit from silently shrinking to surface
/// scale.
fn scroll_unit_from_gdk(unit: gdk::ScrollUnit) -> ScrollUnit {
    match unit {
        gdk::ScrollUnit::Surface => ScrollUnit::Surface,
        _ => ScrollUnit::Wheel,
    }
}

/// The local D5 guard, matching `term::callbacks` and `pty` until row 4.2
/// lifts the shared helper: run `body`, latching `poisoned` and returning
/// `None` when it panics.
fn guarded<T>(poisoned: &AtomicBool, body: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => Some(value),
        Err(_) => {
            poisoned.store(true, Ordering::Relaxed);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::glib::translate::FromGlib;

    #[test]
    fn mods_translate_the_five_gdk_masks() {
        assert_eq!(mods_from_gdk(gdk::ModifierType::empty()), Modifiers::NONE);
        assert_eq!(
            mods_from_gdk(gdk::ModifierType::SHIFT_MASK),
            Modifiers::SHIFT
        );
        assert_eq!(
            mods_from_gdk(gdk::ModifierType::CONTROL_MASK),
            Modifiers::CTRL
        );
        assert_eq!(mods_from_gdk(gdk::ModifierType::ALT_MASK), Modifiers::ALT);
        assert_eq!(
            mods_from_gdk(gdk::ModifierType::SUPER_MASK),
            Modifiers::SUPER
        );
        assert_eq!(
            mods_from_gdk(gdk::ModifierType::LOCK_MASK),
            Modifiers::CAPS_LOCK
        );
    }

    /// Unrelated GDK modifier bits (scroll lock, num lock, a held mouse
    /// button) translate to nothing, like the C's explicit five tests.
    #[test]
    fn mods_ignore_everything_beyond_the_five_masks() {
        let combined = gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::LOCK_MASK
            | gdk::ModifierType::HYPER_MASK
            | gdk::ModifierType::BUTTON1_MASK
            | gdk::ModifierType::META_MASK;
        assert_eq!(
            mods_from_gdk(combined),
            Modifiers::SHIFT
                | Modifiers::CTRL
                | Modifiers::ALT
                | Modifiers::SUPER
                | Modifiers::CAPS_LOCK
        );
    }

    #[test]
    fn buttons_map_primary_middle_secondary_and_back_forward() {
        assert_eq!(button_from_gdk(gdk::BUTTON_PRIMARY), MouseButton::Left);
        assert_eq!(button_from_gdk(gdk::BUTTON_MIDDLE), MouseButton::Middle);
        assert_eq!(button_from_gdk(gdk::BUTTON_SECONDARY), MouseButton::Right);
        assert_eq!(button_from_gdk(8), MouseButton::Eight);
        assert_eq!(button_from_gdk(9), MouseButton::Nine);
        // Everything else is unknown, including the button-less 0 and a
        // fourth physical button.
        assert_eq!(button_from_gdk(0), MouseButton::Unknown);
        assert_eq!(button_from_gdk(4), MouseButton::Unknown);
        assert_eq!(button_from_gdk(12), MouseButton::Unknown);
    }

    /// The level-0 selection prefers the entry matching the keyboard layout
    /// and falls back to any level-0 entry.
    #[test]
    fn unshifted_selection_prefers_the_layout_then_any_level_zero() {
        let entries = [(1, 0, 0x61), (1, 1, 0x41), (0, 0, 0x71), (0, 1, 0x51)];
        assert_eq!(select_unshifted_keyval(&entries, 1), Some(0x61));
        assert_eq!(select_unshifted_keyval(&entries, 0), Some(0x71));
        // A layout with no level-0 entry falls back to another layout's
        // level-0 entry.
        assert_eq!(select_unshifted_keyval(&entries, 7), Some(0x61));
        // No level-0 entry anywhere: none.
        let shifted = [(1, 1, 0x41), (2, 2, 0x42)];
        assert_eq!(select_unshifted_keyval(&shifted, 1), None);
        assert_eq!(select_unshifted_keyval(&[], 0), None);
    }

    /// The keyval-to-unicode table lookup matches GDK's C behaviour: ASCII
    /// keys map; a keysym like escape maps to its control codepoint (the
    /// encoder's own 0x20..=0x10ffff range check filters that later), and
    /// unknown keyvals answer 0.
    #[test]
    fn keyval_unicode_matches_the_c_helper() {
        assert_eq!(keyval_unicode(0x61), u32::from('a'));
        assert_eq!(keyval_unicode(0x41), u32::from('A'));
        assert_eq!(keyval_unicode(0xff1b), 0x1b);
        assert_eq!(keyval_unicode(0x0), 0);
    }

    /// Only a surface delta shrinks to a tenth of a notch; a wheel click — and
    /// anything this gdk build reports that is not a surface delta, such as a
    /// discrete unit — is a whole notch, like the C's surface-only 0.1 scale.
    #[test]
    fn scroll_units_map_surface_only() {
        assert_eq!(
            scroll_unit_from_gdk(gdk::ScrollUnit::Wheel),
            ScrollUnit::Wheel
        );
        assert_eq!(
            scroll_unit_from_gdk(gdk::ScrollUnit::Surface),
            ScrollUnit::Surface
        );
        // The bindings map an unrecognised raw unit onto `__Unknown`; the C
        // passed such a value through unscaled, so it stays a whole notch.
        // SAFETY: 2 is not a valid `GdkScrollUnit` in this gdk build, which is
        // exactly the case under test — the `from_glib` fallback arm.
        let discrete = unsafe { gdk::ScrollUnit::from_glib(2) };
        assert_eq!(discrete, gdk::ScrollUnit::__Unknown(2));
        assert_eq!(scroll_unit_from_gdk(discrete), ScrollUnit::Wheel);
    }
}
