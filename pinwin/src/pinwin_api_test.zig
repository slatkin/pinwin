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
