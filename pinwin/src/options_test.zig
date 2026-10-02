//! Layout-core unit tests (design D8), run by `zig build check`.
//!
//! Exercises the GTK-free C core declared in options.h and implemented in
//! options.c: strict cols/gutter parsing, checked side geometry and every
//! pinwin_layout_validate result code.

const std = @import("std");
const testing = std.testing;
const c = @cImport({
    @cInclude("options.h");
});

fn parseCols(text: [*c]const u8) struct { ok: c_int, value: i32 } {
    var value: i32 = -1;
    const ok = c.pinwin_parse_cols(text, &value);
    return .{ .ok = ok, .value = value };
}

fn parseGutter(text: [*c]const u8) struct { ok: c_int, value: i32 } {
    var value: i32 = -1;
    const ok = c.pinwin_parse_gutter(text, &value);
    return .{ .ok = ok, .value = value };
}

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

test "cols parsing accepts 1..=65535" {
    try testing.expectEqual(@as(c_int, 1), parseCols("1").ok);
    try testing.expectEqual(@as(i32, 1), parseCols("1").value);
    try testing.expectEqual(@as(c_int, 1), parseCols("65535").ok);
    try testing.expectEqual(@as(i32, 65535), parseCols("65535").value);
    try testing.expectEqual(@as(c_int, 1), parseCols("60").ok);
    try testing.expectEqual(@as(i32, 60), parseCols("60").value);
}

test "cols parsing rejects zero, overflow, mixed and empty" {
    try testing.expectEqual(@as(c_int, 0), parseCols("0").ok);
    try testing.expectEqual(@as(c_int, 0), parseCols("65536").ok);
    try testing.expectEqual(@as(c_int, 0), parseCols("12x").ok);
    try testing.expectEqual(@as(c_int, 0), parseCols("").ok);
}

test "cols parsing rejects sign and whitespace" {
    try testing.expectEqual(@as(c_int, 0), parseCols("-12").ok);
    try testing.expectEqual(@as(c_int, 0), parseCols("+12").ok);
    try testing.expectEqual(@as(c_int, 0), parseCols(" 12").ok);
    try testing.expectEqual(@as(c_int, 0), parseCols("12 ").ok);
    try testing.expectEqual(@as(c_int, 0), parseCols("1 2").ok);
}

test "gutter parsing accepts optional sign and digits" {
    try testing.expectEqual(@as(c_int, 1), parseGutter("0").ok);
    try testing.expectEqual(@as(i32, 0), parseGutter("0").value);
    try testing.expectEqual(@as(c_int, 1), parseGutter("12").ok);
    try testing.expectEqual(@as(i32, 12), parseGutter("12").value);
    try testing.expectEqual(@as(c_int, 1), parseGutter("-5").ok);
    try testing.expectEqual(@as(i32, -5), parseGutter("-5").value);
}

test "gutter parsing accepts int32 bounds" {
    try testing.expectEqual(@as(c_int, 1), parseGutter("2147483647").ok);
    try testing.expectEqual(@as(i32, 2147483647), parseGutter("2147483647").value);
    try testing.expectEqual(@as(c_int, 1), parseGutter("-2147483648").ok);
    try testing.expectEqual(@as(i32, -2147483648), parseGutter("-2147483648").value);
}

test "gutter parsing rejects malformed and out-of-int32" {
    try testing.expectEqual(@as(c_int, 0), parseGutter("").ok);
    try testing.expectEqual(@as(c_int, 0), parseGutter("-").ok);
    try testing.expectEqual(@as(c_int, 0), parseGutter("12x").ok);
    try testing.expectEqual(@as(c_int, 0), parseGutter("+12").ok);
    try testing.expectEqual(@as(c_int, 0), parseGutter(" 12").ok);
    try testing.expectEqual(@as(c_int, 0), parseGutter("2147483648").ok);
    try testing.expectEqual(@as(c_int, 0), parseGutter("-2147483649").ok);
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

test "default layout is the launch baseline" {
    const layout = c.pinwin_layout_default(80, 12);
    try testing.expectEqual(@as(i32, c.PINWIN_SIDE_LEFT), layout.side);
    try testing.expectEqual(@as(i32, 80), layout.cols);
    try testing.expectEqual(@as(i32, 0), layout.top);
    try testing.expectEqual(@as(i32, 0), layout.bottom);
    try testing.expectEqual(@as(i32, 0), layout.left);
    try testing.expectEqual(@as(i32, 12), layout.right);

    const negative = c.pinwin_layout_default(80, -4);
    try testing.expectEqual(@as(i32, 0), negative.right);
}
