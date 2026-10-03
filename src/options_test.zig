//! Layout-core unit tests (design D8), run by `zig build check`.
//!
//! Exercises the GTK-free C core declared in options.h and implemented in
//! options.c: checked side geometry and every pinwin_layout_validate result
//! code.

const std = @import("std");
const testing = std.testing;
const c = @cImport({
    @cInclude("options.h");
});

fn baseLayout() c.PinwinLayout {
    return .{
        .side = c.PINWIN_SIDE_LEFT,
        .cols = 60,
        .top = 0,
        .bottom = 0,
        .left = 0,
        .right = 0,
    };
}

test "side geometry margins follow the docking side" {
    var margin: i32 = -1;
    var reservation: i32 = -1;
    var layout = baseLayout();
    layout.left = 10;
    layout.right = 20;

    layout.side = c.PINWIN_SIDE_LEFT;
    try testing.expectEqual(@as(c_int, 1), c.pinwin_side_geometry(&layout, 100, &margin, &reservation));
    try testing.expectEqual(@as(i32, 10), margin);
    try testing.expectEqual(@as(i32, 130), reservation);

    layout.side = c.PINWIN_SIDE_RIGHT;
    try testing.expectEqual(@as(c_int, 1), c.pinwin_side_geometry(&layout, 100, &margin, &reservation));
    try testing.expectEqual(@as(i32, 20), margin);
    try testing.expectEqual(@as(i32, 130), reservation);
}

test "side geometry rejects overflow and bad input" {
    var margin: i32 = -1;
    var reservation: i32 = -1;
    var layout = baseLayout();

    try testing.expectEqual(@as(c_int, 0), c.pinwin_side_geometry(&layout, -1, &margin, &reservation));
    try testing.expectEqual(@as(c_int, 0), c.pinwin_side_geometry(&layout, 2147483648, &margin, &reservation));

    layout.left = 2147483647;
    layout.right = 2147483647;
    try testing.expectEqual(@as(c_int, 0), c.pinwin_side_geometry(&layout, 0, &margin, &reservation));

    layout = baseLayout();
    layout.side = 2;
    try testing.expectEqual(@as(c_int, 0), c.pinwin_side_geometry(&layout, 1, &margin, &reservation));
}

test "validate accepts a sane layout" {
    var layout = baseLayout();
    layout.right = 12;
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_OK),
        c.pinwin_layout_validate(&layout, 60, 8, 16, 1920, 1080),
    );
}

test "validate reaches GEOM_ERR_SIDE" {
    var layout = baseLayout();
    layout.side = 2;
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_ERR_SIDE),
        c.pinwin_layout_validate(&layout, 60, 8, 16, 1920, 1080),
    );
}

test "validate reaches GEOM_ERR_METRICS" {
    var layout = baseLayout();
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_ERR_METRICS),
        c.pinwin_layout_validate(&layout, 0, 8, 16, 1920, 1080),
    );
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_ERR_METRICS),
        c.pinwin_layout_validate(&layout, 60, 0, 0, 0, 0),
    );
}

test "validate reaches GEOM_ERR_OVERFLOW" {
    var layout = baseLayout();
    // cols * cell_w overflows int32_t in the checked panel width.
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_ERR_OVERFLOW),
        c.pinwin_layout_validate(&layout, 65535, 65536, 16, 1920, 1080),
    );
    layout.left = 2147483647;
    layout.right = 2147483647;
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_ERR_OVERFLOW),
        c.pinwin_layout_validate(&layout, 1, 1, 16, 1920, 1080),
    );
}

test "validate reaches GEOM_ERR_NO_RESERVE" {
    var layout = baseLayout();
    layout.left = -100;
    layout.right = -100;
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_ERR_NO_RESERVE),
        c.pinwin_layout_validate(&layout, 1, 1, 16, 1920, 1080),
    );
}

test "validate reaches GEOM_ERR_NO_WIDTH" {
    var layout = baseLayout();
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_ERR_NO_WIDTH),
        c.pinwin_layout_validate(&layout, 600, 1, 16, 600, 1080),
    );
}

test "validate reaches GEOM_ERR_NO_ROW" {
    var layout = baseLayout();
    layout.top = 1000;
    layout.bottom = 1000;
    try testing.expectEqual(
        @as(c_int, c.PINWIN_GEOM_ERR_NO_ROW),
        c.pinwin_layout_validate(&layout, 1, 1, 16, 1920, 1080),
    );
}

test "accent validate accepts widths 1 through 65535 when enabled" {
    var accent = std.mem.zeroes(c.PinwinAccent);
    accent.enabled = 1;
    accent.width = 1;
    try testing.expectEqual(@as(c_int, 1), c.pinwin_accent_validate(&accent));
    accent.width = 65535;
    try testing.expectEqual(@as(c_int, 1), c.pinwin_accent_validate(&accent));
}

test "accent validate rejects enabled widths outside 1..65535" {
    var accent = std.mem.zeroes(c.PinwinAccent);
    accent.enabled = 1;

    accent.width = 0;
    try testing.expectEqual(@as(c_int, 0), c.pinwin_accent_validate(&accent));
    accent.width = 65536;
    try testing.expectEqual(@as(c_int, 0), c.pinwin_accent_validate(&accent));
    accent.width = -1;
    try testing.expectEqual(@as(c_int, 0), c.pinwin_accent_validate(&accent));
}

test "accent validate ignores width when off and rejects other enabled values" {
    var accent = std.mem.zeroes(c.PinwinAccent);

    accent.enabled = 0;
    accent.width = 0;
    try testing.expectEqual(@as(c_int, 1), c.pinwin_accent_validate(&accent));
    accent.width = -5;
    try testing.expectEqual(@as(c_int, 1), c.pinwin_accent_validate(&accent));

    accent.enabled = 2;
    try testing.expectEqual(@as(c_int, 0), c.pinwin_accent_validate(&accent));
    accent.enabled = -1;
    try testing.expectEqual(@as(c_int, 0), c.pinwin_accent_validate(&accent));
}
