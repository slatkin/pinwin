//! input.zig — the input-encoding half of pinwin's terminal core: key, mouse,
//! scroll and focus events arrive through pinwin.h from input.c and are
//! encoded into the PTY with libghostty-vt's encoders (design D4).
//!
//! The shared terminal handles live in main.zig.

const std = @import("std");

const c = @import("c.zig").c;
const keys = @import("keys.zig");
const main = @import("main.zig");

/// `ghostty_mode_new(1004, false)`: mode 1004 without the ANSI-mode bit. Taken
/// by hand because the cimport's inline `ghostty_mode_new` does not translate.
const MODE_FOCUS_EVENT: c.GhosttyMode = 1004;

var any_button_pressed = false;
/// The focus state last reported to the program. GTK sends a leave at map
/// time, before any enter; programs expect balanced reports, so only actual
/// changes are reported.
var focus_gained = false;
/// Left/right, up/down remainders of a scroll in notches (see pinwin_scroll).
var scroll_acc: [2]f64 = .{ 0, 0 };

/// Writes an encoded sequence to the PTY. `written` becomes the required size
/// when `GhosttyResult` is GHOSTTY_OUT_OF_SPACE, so the caller retries once
/// with an exact-size buffer instead of dropping the event.
fn writeEncoded(comptime encode: fn ([*c]u8, usize, *usize) c.GhosttyResult, buf: []u8) void {
    var written: usize = 0;
    var result = encode(buf.ptr, buf.len, &written);
    if (result == c.GHOSTTY_OUT_OF_SPACE) {
        const exact = main.allocator.alloc(u8, written) catch return;
        defer main.allocator.free(exact);
        result = encode(exact.ptr, exact.len, &written);
        if (result == c.GHOSTTY_SUCCESS and written > 0) c.glue_pty_write(exact.ptr, written);
        return;
    }
    if (result == c.GHOSTTY_SUCCESS and written > 0) c.glue_pty_write(buf.ptr, written);
}

fn encodeKey(out: [*c]u8, len: usize, written: *usize) c.GhosttyResult {
    return c.ghostty_key_encoder_encode(main.key_encoder, main.key_event, out, len, written);
}

fn encodeMouse(out: [*c]u8, len: usize, written: *usize) c.GhosttyResult {
    return c.ghostty_mouse_encoder_encode(main.mouse_encoder, main.mouse_event, out, len, written);
}

export fn pinwin_key(action: i32, keyval: i32, keycode: i32, mods: u32, consumed_mods: u32, is_modifier: i32) void {
    if (main.term == null) return;

    const key_action: c.GhosttyKeyAction = switch (action) {
        c.PINWIN_KEY_RELEASE => c.GHOSTTY_KEY_ACTION_RELEASE,
        c.PINWIN_KEY_REPEAT => c.GHOSTTY_KEY_ACTION_REPEAT,
        else => c.GHOSTTY_KEY_ACTION_PRESS,
    };

    const keycode_u: u32 = @intCast(@max(keycode, 0));
    const keyval_u: u32 = @intCast(@max(keyval, 0));
    const key = physicalKey(keycode_u, keyval_u, is_modifier != 0);

    c.ghostty_key_event_set_action(main.key_event, key_action);
    c.ghostty_key_event_set_key(main.key_event, key);
    c.ghostty_key_event_set_mods(main.key_event, @intCast(mods));
    c.ghostty_key_event_set_consumed_mods(main.key_event, @intCast(consumed_mods));
    c.ghostty_key_event_set_unshifted_codepoint(main.key_event, c.glue_keycode_unshifted_codepoint(keycode_u));

    var text: [8]u8 = undefined;
    var text_len: usize = 0;
    const codepoint = c.glue_keyval_unicode(keyval_u);
    if (key != c.GHOSTTY_KEY_UNIDENTIFIED and key_action != c.GHOSTTY_KEY_ACTION_RELEASE and
        codepoint >= 0x20 and codepoint <= 0x10ffff)
    {
        text_len = std.unicode.utf8Encode(@intCast(codepoint), &text) catch 0;
    }
    c.ghostty_key_event_set_utf8(main.key_event, &text, text_len);

    c.ghostty_key_encoder_setopt_from_terminal(main.key_encoder, main.term);
    var buf: [256]u8 = undefined;
    if (main.debug_enabled) {
        var kitty: u8 = 0;
        _ = c.ghostty_terminal_get(main.term, c.GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, @ptrCast(&kitty));
        std.debug.print("key action={} keyval=0x{x} keycode={} mods=0x{x} consumed=0x{x} modifier={} text_len={} key={} kitty={}\n", .{ action, keyval, keycode, mods, consumed_mods, is_modifier, text_len, key, kitty });
    }
    writeEncoded(encodeKey, &buf);
}

/// The physical key for a key event, following Ghostty's GTK apprt: the keycode
/// names the key, writing-system keys are remapped from the keyval (a layout or
/// a synthetic keymap can put any character on one keycode), and modifier
/// keyvals never encode.
fn physicalKey(keycode: u32, keyval: u32, is_modifier: bool) c.GhosttyKey {
    var key = keys.keyFromKeycode(keycode);
    if (keys.keyFromKeyval(keyval)) |remapped| {
        if (keys.shouldBeRemappable(key) or keys.shouldBeRemappable(remapped)) key = remapped;
    }
    if (is_modifier or keys.isModifierKeyval(keyval)) return c.GHOSTTY_KEY_UNIDENTIFIED;
    return key;
}

fn sendMouse(action: c.GhosttyMouseAction, x: f64, y: f64, button: i32, mods: u32) void {
    if (main.term == null) return;

    c.ghostty_mouse_encoder_setopt_from_terminal(main.mouse_encoder, main.term);
    var size = std.mem.zeroes(c.GhosttyMouseEncoderSize);
    size.size = @sizeOf(c.GhosttyMouseEncoderSize);
    size.screen_width = @as(u32, main.grid_cols) * main.cell_w;
    size.screen_height = @as(u32, main.grid_rows) * main.cell_h;
    size.cell_width = main.cell_w;
    size.cell_height = main.cell_h;
    c.ghostty_mouse_encoder_setopt(main.mouse_encoder, c.GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &size);
    var pressed = any_button_pressed;
    c.ghostty_mouse_encoder_setopt(main.mouse_encoder, c.GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED, &pressed);

    c.ghostty_mouse_event_set_action(main.mouse_event, action);
    if (button == c.PINWIN_MOUSE_UNKNOWN) {
        c.ghostty_mouse_event_clear_button(main.mouse_event);
    } else {
        c.ghostty_mouse_event_set_button(main.mouse_event, @intCast(button));
    }
    c.ghostty_mouse_event_set_mods(main.mouse_event, @intCast(mods));
    var position = std.mem.zeroes(c.GhosttyMousePosition);
    position.x = @floatCast(x);
    position.y = @floatCast(y);
    c.ghostty_mouse_event_set_position(main.mouse_event, position);

    var buf: [64]u8 = undefined;
    writeEncoded(encodeMouse, &buf);
}

export fn pinwin_mouse(action: i32, x: f64, y: f64, button: i32, mods: u32) void {
    if (main.term == null) return;
    const mouse_action: c.GhosttyMouseAction = switch (action) {
        c.PINWIN_MOUSE_RELEASE => c.GHOSTTY_MOUSE_ACTION_RELEASE,
        c.PINWIN_MOUSE_MOTION => c.GHOSTTY_MOUSE_ACTION_MOTION,
        else => c.GHOSTTY_MOUSE_ACTION_PRESS,
    };
    if (main.debug_enabled)
        std.debug.print("mouse action={} x={d:.1} y={d:.1} button={} mods=0x{x}\n", .{ action, x, y, button, mods });
    if (mouse_action == c.GHOSTTY_MOUSE_ACTION_PRESS and button != c.PINWIN_MOUSE_UNKNOWN)
        any_button_pressed = true;
    if (mouse_action == c.GHOSTTY_MOUSE_ACTION_RELEASE)
        any_button_pressed = false;
    sendMouse(mouse_action, x, y, button, mods);
}

/// A wheel notch is a mouse press with button 4 (up), 5 (down), 6 (left) or
/// 7 (right) in the protocols terminals use, so it goes through the mouse
/// encoder like any other button.
///
/// A wheel device reports one notch as one unit; touchpads and free-spinning
/// or high-resolution wheels report surface units, which count a tenth of a
/// notch each so a single swipe cannot flood the program with wheel events.
export fn pinwin_scroll(x: f64, y: f64, dx: f64, dy: f64, unit: i32, mods: u32) void {
    if (main.term == null) return;

    if (main.debug_enabled)
        std.debug.print("scroll x={d:.1} y={d:.1} dx={d:.3} dy={d:.3} unit={} mods=0x{x}\n", .{ x, y, dx, dy, unit, mods });

    const scale: f64 = if (unit == c.PINWIN_SCROLL_UNIT_SURFACE) 0.1 else 1.0;
    scroll_acc[0] += dx * scale;
    scroll_acc[1] += dy * scale;

    while (scroll_acc[1] >= 1.0) {
        scroll_acc[1] -= 1.0;
        sendMouse(c.GHOSTTY_MOUSE_ACTION_PRESS, x, y, c.PINWIN_MOUSE_FIVE, mods);
    }
    while (scroll_acc[1] <= -1.0) {
        scroll_acc[1] += 1.0;
        sendMouse(c.GHOSTTY_MOUSE_ACTION_PRESS, x, y, c.PINWIN_MOUSE_FOUR, mods);
    }
    while (scroll_acc[0] >= 1.0) {
        scroll_acc[0] -= 1.0;
        sendMouse(c.GHOSTTY_MOUSE_ACTION_PRESS, x, y, c.PINWIN_MOUSE_SEVEN, mods);
    }
    while (scroll_acc[0] <= -1.0) {
        scroll_acc[0] += 1.0;
        sendMouse(c.GHOSTTY_MOUSE_ACTION_PRESS, x, y, c.PINWIN_MOUSE_SIX, mods);
    }
}

/// Focus reports are sent only when the program enabled mode 1004; the
/// encoder has no terminal to ask, so the mode is checked here (task 4.3).
export fn pinwin_focus(gained: i32) void {
    if (main.term == null) return;

    const now_focused = gained != 0;
    if (now_focused == focus_gained) return;
    focus_gained = now_focused;

    var config = std.mem.zeroes(c.GhosttyTerminalModeConfig);
    config.mode = MODE_FOCUS_EVENT;
    if (c.ghostty_terminal_get(main.term, c.GHOSTTY_TERMINAL_DATA_MODE, &config) != c.GHOSTTY_SUCCESS) return;
    if (main.debug_enabled)
        std.debug.print("focus gained={} mode1004={}\n", .{ gained != 0, config.value });
    if (!config.value) return;

    var buf: [8]u8 = undefined;
    var written: usize = 0;
    const event: c.GhosttyFocusEvent = if (gained != 0) c.GHOSTTY_FOCUS_GAINED else c.GHOSTTY_FOCUS_LOST;
    if (c.ghostty_focus_encode(event, &buf, buf.len, &written) == c.GHOSTTY_SUCCESS and written > 0)
        c.glue_pty_write(&buf, written);
}
