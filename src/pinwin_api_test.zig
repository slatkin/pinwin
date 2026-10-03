//! ABI contract test (design D8), run by `zig build check`.
//!
//! Pins the Phase 1 (design D5) contract of pinwin_apply_layout without ever
//! starting the panel: an invalid layout is PINWIN_ERR_INVALID on the caller's
//! thread, a structurally valid layout is PINWIN_ERR_NOT_RUNNING because no
//! GTK thread was ever started, and the two codes are distinct. The test never
//! calls pinwin_start, so no GTK thread exists and no display is needed.

const std = @import("std");
const testing = std.testing;
const c = @cImport({
    @cInclude("pinwin_api.h");
});

fn layout(side: i32, cols: i32) c.PinwinLayout {
    return .{
        .side = side,
        .cols = cols,
        .top = 0,
        .bottom = 0,
        .left = 0,
        .right = 0,
    };
}

test "apply_layout rejects invalid layouts with PINWIN_ERR_INVALID" {
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), c.pinwin_apply_layout(null));

    const valid = layout(c.PINWIN_SIDE_LEFT, 60);

    var bad_side: c.PinwinLayout = valid;
    bad_side.side = 2;
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), c.pinwin_apply_layout(&bad_side));

    var negative_side: c.PinwinLayout = valid;
    negative_side.side = -1;
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), c.pinwin_apply_layout(&negative_side));

    var zero_cols: c.PinwinLayout = valid;
    zero_cols.cols = 0;
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), c.pinwin_apply_layout(&zero_cols));

    var huge_cols: c.PinwinLayout = valid;
    huge_cols.cols = 65536;
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), c.pinwin_apply_layout(&huge_cols));
}

test "apply_layout without a running panel is PINWIN_ERR_NOT_RUNNING" {
    // pinwin_start was never called, so the GTK thread does not exist. A
    // structurally valid layout must not be INVALID: it reports not-running.
    const left = layout(c.PINWIN_SIDE_LEFT, 60);
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_NOT_RUNNING), c.pinwin_apply_layout(&left));

    const right = layout(c.PINWIN_SIDE_RIGHT, 1);
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_NOT_RUNNING), c.pinwin_apply_layout(&right));
}

test "invalid and not-running results are distinct codes" {
    try testing.expect(c.PINWIN_ERR_INVALID != c.PINWIN_ERR_NOT_RUNNING);

    var zero_cols = layout(c.PINWIN_SIDE_LEFT, 0);
    const invalid = c.pinwin_apply_layout(&zero_cols);
    var valid = layout(c.PINWIN_SIDE_LEFT, 60);
    const not_running = c.pinwin_apply_layout(&valid);

    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), invalid);
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_NOT_RUNNING), not_running);
    try testing.expect(invalid != not_running);
}

test "apply_layout_animated shares the invalid and not-running contract" {
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), c.pinwin_apply_layout_animated(null, 200));

    var zero_cols = layout(c.PINWIN_SIDE_LEFT, 0);
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), c.pinwin_apply_layout_animated(&zero_cols, 200));

    var bad_side = layout(c.PINWIN_SIDE_LEFT, 60);
    bad_side.side = 2;
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_INVALID), c.pinwin_apply_layout_animated(&bad_side, 200));

    // No panel was started: a valid layout is not-running for any duration,
    // including zero (snap) and one beyond the clamp.
    const valid = layout(c.PINWIN_SIDE_LEFT, 60);
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_NOT_RUNNING), c.pinwin_apply_layout_animated(&valid, 0));
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_NOT_RUNNING), c.pinwin_apply_layout_animated(&valid, c.PINWIN_ANIM_DEFAULT_MS));
    try testing.expectEqual(@as(c_int, c.PINWIN_ERR_NOT_RUNNING), c.pinwin_apply_layout_animated(&valid, 60000));
}

// The ABI drives a real terminal here (the only test that creates one), so it
// stays last in this file: `term` is process-global and every other test in
// this file requires that it was never created.

extern var g_pty_fd: c_int;
extern fn pinwin_pty_data(data: [*c]const u8, len: usize) void;
extern fn pinwin_size(cols: i32, rows: i32, cw: i32, ch: i32) c_int;

test "pty data arriving before the terminal is replayed, not dropped" {
    // The host forks its child before pinwin_start, so a child's startup
    // queries land before the first draw creates the terminal. Regression:
    // dropping them made fish wait ~10s for query replies and then warn. The
    // query goes in before pinwin_size, the DA1 reply must come out of the
    // real pty fd after it.
    var fds: [2]i32 = undefined;
    if (std.os.linux.pipe(&fds) != 0) return error.PipeFailed;
    defer _ = std.os.linux.close(fds[0]);
    defer _ = std.os.linux.close(fds[1]);
    g_pty_fd = fds[1];
    defer g_pty_fd = -1;

    pinwin_pty_data("\x1b[c", 3); // DA1: term is null here, so this buffers
    try testing.expectEqual(@as(c_int, 0), pinwin_size(40, 24, 8, 16));

    var buf: [64]u8 = undefined;
    // Poll with a timeout so a regression (no reply) fails instead of hanging.
    var pfd: std.os.linux.pollfd = .{ .fd = fds[0], .events = std.os.linux.POLL.IN, .revents = 0 };
    const ready = std.os.linux.poll(@ptrCast(&pfd), 1, 5000);
    try testing.expect(ready == 1);
    const n = std.os.linux.read(fds[0], &buf, buf.len);
    try testing.expect(n >= 6);
    try testing.expectEqualSlices(u8, "\x1b[?6", buf[0..4]);
}

// ---- SIGWINCH contract ------------------------------------------------------
// "Resize of the panel results in SIGWINCH delivered to the process after the
// winsize is updated" (pinwin-panel delta, add-sigwinch-raise).

var sigwinch_seen = false;

fn onSigwinch(_: std.posix.SIG) callconv(.c) void {
    @atomicStore(bool, &sigwinch_seen, true, .seq_cst);
}

extern "c" fn posix_openpt(flags: c_int) c_int;
extern "c" fn grantpt(fd: c_int) c_int;
extern "c" fn unlockpt(fd: c_int) c_int;
extern fn glue_pty_resize(cols: c_int, rows: c_int) void;

test "winsize update delivers SIGWINCH to the process" {
    var act: std.posix.Sigaction = std.mem.zeroes(std.posix.Sigaction);
    act.handler.handler = onSigwinch;
    var old: std.posix.Sigaction = undefined;
    std.posix.sigaction(std.posix.SIG.WINCH, &act, &old);
    defer std.posix.sigaction(std.posix.SIG.WINCH, &old, null);

    // TIOCSWINSZ needs a real pty master; a pipe rejects it (ENOTTY).
    const master = posix_openpt(2); // O_RDWR
    try testing.expect(master >= 0);
    try testing.expectEqual(@as(c_int, 0), grantpt(master));
    try testing.expectEqual(@as(c_int, 0), unlockpt(master));
    defer _ = std.os.linux.close(master);
    g_pty_fd = master;
    defer g_pty_fd = -1;

    sigwinch_seen = false;
    glue_pty_resize(40, 24);
    try testing.expect(@atomicLoad(bool, &sigwinch_seen, .seq_cst));

    // No pty attached: no winsize update, no signal.
    g_pty_fd = -1;
    sigwinch_seen = false;
    glue_pty_resize(40, 24);
    try testing.expect(!@atomicLoad(bool, &sigwinch_seen, .seq_cst));
}
