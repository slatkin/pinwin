//! cells.zig — the frame protocol half of pinwin's terminal core: starting
//! and ending a frame, walking the terminal's cells (with grapheme merging
//! and constraint widths), the cursor and default colours, and the kitty
//! image placements (design D4).
//!
//! The shared terminal handles live in main.zig; the C draw callback drives
//! these functions through pinwin.h.

const std = @import("std");

const c = @import("c.zig").c;
const main = @import("main.zig");

var frame_open = false;
var has_pending = false;
var last_emitted_cp: u32 = 0;
var pending_cell: c.PinwinCell = undefined;
var in_row = false;
var cell_x: i32 = 0;
var cell_y: i32 = 0;
var frame_cursor: c.PinwinCursor = undefined;
var default_bg: c.GhosttyColorRgb = undefined;
var default_fg: c.GhosttyColorRgb = undefined;

/// The kitty placeholder codepoint: a cell carrying it means "draw the image
/// here", with the image id in the cell's foreground colour.
const PLACEHOLDER = 0x10EEEE;

/// A placeholder cell carries only the low 24 bits of the image id in its
/// foreground colour; the high byte travels in the third diacritic, which is
/// the placement id. Origins are therefore keyed by those low 24 bits, and two
/// placements of the same image share the first one's origin.
const placeholder_id_mask: u32 = 0xFF_FFFF;

const PlaceholderOrigin = struct { image_id: u32, row: i32, col: i32 };
const max_placeholders = 8;
var placeholder_origins: [max_placeholders]PlaceholderOrigin = undefined;
var placeholder_count: usize = 0;

fn notePlaceholder(image_id: u32, row: i32, col: i32) void {
    const key = image_id & placeholder_id_mask;
    for (placeholder_origins[0..placeholder_count]) |*origin| {
        if (origin.image_id != key) continue;
        if (row < origin.row or (row == origin.row and col < origin.col)) {
            origin.row = row;
            origin.col = col;
        }
        return;
    }
    if (main.debug_enabled) std.debug.print("placeholder new image_id={} at row={} col={}\n", .{ key, row, col });
    if (placeholder_count == max_placeholders) return;
    placeholder_origins[placeholder_count] = .{ .image_id = key, .row = row, .col = col };
    placeholder_count += 1;
}

fn placeholderOrigin(image_id: u32) ?PlaceholderOrigin {
    const key = image_id & placeholder_id_mask;
    for (placeholder_origins[0..placeholder_count]) |origin| {
        if (origin.image_id == key) return origin;
    }
    return null;
}

var graphics: c.GhosttyKittyGraphics = null;
var images_started = false;

/// Starts a frame. Every draw callback draws a complete frame: GTK asks for a
/// redraw for its own reasons too (focus, exposure, resize), and a frame that
/// skipped the cells it thinks are unchanged would leave those areas blank,
/// because the callback paints the panel's background first.
export fn pinwin_frame_begin() i32 {
    if (main.term == null) return 0;
    if (c.ghostty_render_state_update(main.render_state, main.term) != c.GHOSTTY_SUCCESS) return 0;

    var colors = std.mem.zeroes(c.GhosttyRenderStateColors);
    colors.size = @sizeOf(c.GhosttyRenderStateColors);
    if (c.ghostty_render_state_get(main.render_state, c.GHOSTTY_RENDER_STATE_DATA_COLORS, &colors) == c.GHOSTTY_SUCCESS) {
        default_bg = colors.background;
        default_fg = colors.foreground;
    }

    _ = c.ghostty_render_state_get(main.render_state, c.GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, @ptrCast(&main.row_iter));

    frame_cursor = std.mem.zeroes(c.PinwinCursor);
    var cursor = std.mem.zeroes(c.GhosttyRenderStateCursor);
    cursor.size = @sizeOf(c.GhosttyRenderStateCursor);
    if (c.ghostty_render_state_get(main.render_state, c.GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor) == c.GHOSTTY_SUCCESS and
        cursor.visible and cursor.viewport_has_value)
    {
        frame_cursor.has_value = 1;
        frame_cursor.x = cursor.viewport_x;
        frame_cursor.y = cursor.viewport_y;
        frame_cursor.style = @intCast(cursor.visual_style);
        frame_cursor.wide_tail = @intFromBool(cursor.wide_tail);
    }

    frame_open = true;
    has_pending = false;
    last_emitted_cp = 0;
    in_row = false;
    images_started = false;
    placeholder_count = 0;
    cell_y = -1;
    return 1;
}

/// Revisit the captured cells after painting all backgrounds, so glyphs may
/// extend into neighboring cells without being covered by their backgrounds.
export fn pinwin_frame_rewind() void {
    if (!frame_open) return;
    _ = c.ghostty_render_state_get(main.render_state, c.GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, @ptrCast(&main.row_iter));
    has_pending = false;
    last_emitted_cp = 0;
    in_row = false;
    placeholder_count = 0;
    cell_y = -1;
}

/// The next raw cell of the frame, before grapheme joining. False at the end
/// of the grid.
fn nextRawCell(out: [*c]c.PinwinCell) bool {
    while (true) {
        if (in_row) {
            if (c.ghostty_render_state_row_cells_next(main.cells)) {
                fillCell(out);
                cell_x += 1;
                return true;
            }
            in_row = false;
        }

        if (!c.ghostty_render_state_row_iterator_next(main.row_iter)) return false;
        // The whole grid is walked every frame (see design D5): the draw
        // callback repaints the panel from scratch, so a row that is not
        // visited would be left blank.
        var viewport_y: i32 = 0;
        if (c.ghostty_render_state_row_get(main.row_iter, c.GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y, &viewport_y) != c.GHOSTTY_SUCCESS)
            continue;
        cell_y = viewport_y;
        if (c.ghostty_render_state_row_get(main.row_iter, c.GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, @ptrCast(&main.cells)) != c.GHOSTTY_SUCCESS)
            continue;
        in_row = true;
        cell_x = 0;
    }
}

/// Codepoints that extend the previous cell's cluster. libghostty-vt already
/// joins ordinary combining marks, but it leaves emoji modifiers, variation
/// selectors, keycaps and the base after a zero-width joiner in their own
/// cells, so an emoji arrives as two or three cells and each half is drawn on
/// its own. Ghostty's own renderer shows them as one glyph, so pinwin joins
/// them back together here (see design D5).
fn graphemeExtend(cp: u32) bool {
    return switch (cp) {
        0x200D, // zero-width joiner
        0xFE0E, 0xFE0F, // variation selectors 15 and 16
        0x20E3, // combining enclosing keycap
        0x1F3FB...0x1F3FF, // emoji skin tone modifiers
        0xE0020...0xE007F, // tag characters (subdivision flags)
        => true,
        else => false,
    };
}

fn isRegionalIndicator(cp: u32) bool {
    return cp >= 0x1F1E6 and cp <= 0x1F1FF;
}

fn firstCodepoint(text: []const u8) u32 {
    var view = std.unicode.Utf8View.init(text) catch return 0;
    var it = view.iterator();
    return it.nextCodepoint() orelse 0;
}

fn endsWithZwj(text: []const u8) bool {
    var view = std.unicode.Utf8View.init(text) catch return false;
    var it = view.iterator();
    var last: u32 = 0;
    while (it.nextCodepoint()) |cp| last = cp;
    return last == 0x200D;
}

fn countRegionals(text: []const u8) u32 {
    var view = std.unicode.Utf8View.init(text) catch return 0;
    var it = view.iterator();
    var n: u32 = 0;
    while (it.nextCodepoint()) |cp| {
        if (isRegionalIndicator(cp)) n += 1;
    }
    return n;
}

/// Join `src` into `dst` when the two cells are halves of one grapheme
/// cluster. The joined cell keeps `dst`'s position and width; `src` keeps its
/// colours (its background is still painted) but loses its text.
fn mergeGrapheme(dst: [*c]c.PinwinCell, src: [*c]c.PinwinCell) bool {
    if (dst.*.len <= 0 or src.*.len <= 0) return false;
    const src_text = src.*.text[0..@intCast(src.*.len)];
    const dst_text = dst.*.text[0..@intCast(dst.*.len)];
    const first = firstCodepoint(src_text);
    const odd_regionals = isRegionalIndicator(first) and
        isRegionalIndicator(firstCodepoint(dst_text)) and
        countRegionals(dst_text) % 2 == 1;

    if (!endsWithZwj(dst_text) and !graphemeExtend(first) and !odd_regionals) return false;
    if (dst.*.len + src.*.len > dst.*.text.len) return false;

    @memcpy(dst.*.text[@intCast(dst.*.len)..][0..src_text.len], src_text);
    dst.*.len += src.*.len;
    src.*.len = 0;
    return true;
}


/// Codepoints Ghostty treats as "symbol-like" (renderer/cell.zig isSymbol,
/// which uses uucode's is_symbol property). Private use areas and the symbol
/// blocks, as documented there.
fn isSymbol(cp: u32) bool {
    return switch (cp) {
        0x2190...0x21FF, // arrows
        0x2460...0x24FF, // enclosed alphanumerics
        0x2600...0x26FF, // miscellaneous symbols
        0x2700...0x27BF, // dingbats
        0xE000...0xF8FF, // private use area
        0x1F100...0x1F1FF, // enclosed alphanumeric supplement
        0x1F300...0x1F5FF, // miscellaneous symbols and pictographs
        0x1F600...0x1F64F, // emoticons
        0x1F680...0x1F6FF, // transport and map symbols
        0xF0000...0xFFFFD, // private use plane 15
        0x100000...0x10FFFD, // private use plane 16
        => true,
        else => false,
    };
}

/// Terminal graphics rather than icons: box drawing, blocks, legacy computing
/// and Powerline (renderer/cell.zig isGraphicsElement).
fn isGraphicsElement(cp: u32) bool {
    return switch (cp) {
        0x2500...0x257F, 0x2580...0x259F, 0xE0B0...0xE0D7, 0x1FB00...0x1FBFF, 0x1CC00...0x1CEBF => true,
        else => false,
    };
}

fn isSpaceCodepoint(cp: u32) bool {
    return cp == 0x20 or cp == 0x2002;
}

/// How many cells this glyph may use once its Nerd Font constraint is applied
/// (renderer/cell.zig constraintWidth): a symbol may extend into the next cell
/// when that cell is empty, so icons don't get squeezed into one cell.
fn constraintWidth(cell: *const c.PinwinCell, prev_cp: u32, next_cp: u32, at_row_end: bool) u32 {
    const cp = firstCodepoint(cell.*.text[0..@intCast(cell.len)]);
    if (cell.wide != c.PINWIN_WIDE_NARROW) return 2;
    if (!isSymbol(cp)) return 1;
    if (at_row_end) return 1;
    if (prev_cp != 0 and isSymbol(prev_cp) and !isGraphicsElement(prev_cp)) return 1;
    if (next_cp == 0 or isSpaceCodepoint(next_cp)) return 2;
    return 1;
}

/// Cells, with a one-cell lookahead so that a grapheme cluster split over
/// several cells is drawn as a single glyph at the first of them.
export fn pinwin_cell_next(out: [*c]c.PinwinCell) i32 {
    if (!frame_open) return 0;

    var next: c.PinwinCell = std.mem.zeroes(c.PinwinCell);
    while (nextRawCell(&next)) {
        if (!has_pending) {
            pending_cell = next;
            has_pending = true;
            continue;
        }
        if (next.len == 0) {
            // A blank or wide-glyph spacer cell cannot join a cluster, but it
            // must not flush the pending cell either: a wide emoji is followed
            // by one of these before the modifier that belongs to it. Cells
            // are painted by position, so emitting it first is harmless.
            out.* = next;
            return 1;
        }
        if (mergeGrapheme(&pending_cell, &next)) continue;
        emitPending(out, next);
        pending_cell = next;
        return 1;
    }

    if (has_pending) {
        has_pending = false;
        emitPending(out, next);
        return 1;
    }
    return 0;
}

/// Give the pending cell its constraint width (it needs the cell to its right)
/// and hand it to the caller.
fn emitPending(out: [*c]c.PinwinCell, next: c.PinwinCell) void {
    const same_row = next.len > 0 and next.y == pending_cell.y;
    const next_cp: u32 = if (same_row) firstCodepoint(next.text[0..@intCast(next.len)]) else 0;
    const prev_cp: u32 = if (pending_cell.x == 0) 0 else last_emitted_cp;

    pending_cell.cw = @intCast(constraintWidth(&pending_cell, prev_cp, next_cp, !same_row));
    out.* = pending_cell;
    last_emitted_cp = firstCodepoint(pending_cell.text[0..@intCast(pending_cell.len)]);
}

fn fillCell(out: [*c]c.PinwinCell) void {
    out.* = std.mem.zeroes(c.PinwinCell);
    out.*.x = cell_x;
    out.*.y = cell_y;

    var is_placeholder = false;
    var grapheme_len: u32 = 0;
    _ = c.ghostty_render_state_row_cells_get(main.cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN, &grapheme_len);
    if (grapheme_len > 0) {
        var codepoints: [8]u32 = undefined;
        const count = @min(grapheme_len, codepoints.len);
        _ = c.ghostty_render_state_row_cells_get(main.cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF, &codepoints);
        is_placeholder = codepoints[0] == PLACEHOLDER;
        if (!is_placeholder) {
            var len: usize = 0;
            for (codepoints[0..count]) |codepoint| {
                var utf8: [4]u8 = undefined;
                const written = std.unicode.utf8Encode(@intCast(codepoint), &utf8) catch continue;
                if (len + written > out.*.text.len) break;
                @memcpy(out.*.text[len..][0..written], utf8[0..written]);
                len += written;
            }
            out.*.len = @intCast(len);
        }
    }

    // The cell's width: a wide glyph owns two columns and the tail cell after
    // it must not be drawn (GHOSTTY_CELL_WIDE_SPACER_TAIL).
    var raw: c.GhosttyCell = 0;
    var wide: c.GhosttyCellWide = c.GHOSTTY_CELL_WIDE_NARROW;
    if (c.ghostty_render_state_row_cells_get(main.cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &raw) == c.GHOSTTY_SUCCESS)
        _ = c.ghostty_cell_get(raw, c.GHOSTTY_CELL_DATA_WIDE, &wide);
    out.*.wide = @intCast(wide);

    var style = std.mem.zeroes(c.GhosttyStyle);
    style.size = @sizeOf(c.GhosttyStyle);
    _ = c.ghostty_render_state_row_cells_get(main.cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style);
    // The placeholder's image id is the cell's foreground colour, and the
    // resolved-colour getter has no value for placeholder cells, so take it
    // from the style itself.
    var style_fg_rgb: ?u32 = null;
    if (style.fg_color.tag == c.GHOSTTY_STYLE_COLOR_RGB) {
        const rgb = style.fg_color.value.rgb;
        style_fg_rgb = (@as(u32, rgb.r) << 16) | (@as(u32, rgb.g) << 8) | rgb.b;
    }

    var flags: u32 = 0;
    if (style.bold) flags |= c.PINWIN_BOLD;
    if (style.italic) flags |= c.PINWIN_ITALIC;
    if (style.inverse) flags |= c.PINWIN_INVERSE;
    if (style.faint) flags |= c.PINWIN_FAINT;
    if (style.invisible) flags |= c.PINWIN_INVISIBLE;
    if (style.strikethrough) flags |= c.PINWIN_STRIKETHROUGH;
    if (style.underline != 0) flags |= c.PINWIN_UNDERLINE;
    out.*.flags = flags;

    var fg = default_fg;
    var bg = default_bg;
    var has_fg = c.ghostty_render_state_row_cells_get(main.cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &fg) == c.GHOSTTY_SUCCESS;
    var has_bg = c.ghostty_render_state_row_cells_get(main.cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &bg) == c.GHOSTTY_SUCCESS;

    if (style.inverse) {
        const swap = fg;
        fg = bg;
        bg = swap;
        const swap_has = has_fg;
        has_fg = has_bg;
        has_bg = swap_has;
        // Both sides are "explicit" under inverse: one of them is the default.
        has_fg = true;
        has_bg = true;
    }

    if (is_placeholder) {
        // No glyph: the image is drawn at this cell instead, and the cell's
        // foreground colour is the image id.
        if (style_fg_rgb) |image_id| notePlaceholder(image_id, cell_y, cell_x);
    } else if (has_fg) {
        out.*.has_fg = 1;
        out.*.fr = fg.r;
        out.*.fg = fg.g;
        out.*.fb = fg.b;
    }
    if (has_bg) {
        out.*.has_bg = 1;
        out.*.br = bg.r;
        out.*.bg = bg.g;
        out.*.bb = bg.b;
    }
}

export fn pinwin_cursor(out: [*c]c.PinwinCursor) i32 {
    if (!frame_open or frame_cursor.has_value == 0) return 0;
    out.* = frame_cursor;
    return 1;
}

export fn pinwin_colors(bg: [*c]u8, fg: [*c]u8) void {
    bg[0] = default_bg.r;
    bg[1] = default_bg.g;
    bg[2] = default_bg.b;
    fg[0] = default_fg.r;
    fg[1] = default_fg.g;
    fg[2] = default_fg.b;
}

export fn pinwin_image_next(out: [*c]c.PinwinImage) i32 {
    if (!frame_open) return 0;

    if (!images_started) {
        images_started = true;
        const got = c.ghostty_terminal_get(main.term, c.GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, @ptrCast(&graphics));
        if (main.debug_enabled) std.debug.print("img: graphics get={} handle={}\n", .{ got, graphics != null });
        if (got != c.GHOSTTY_SUCCESS) return 0;
        if (graphics == null) return 0;
        const it = c.ghostty_kitty_graphics_get(graphics, c.GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR, @ptrCast(&main.placement_iter));
        if (main.debug_enabled) std.debug.print("img: iterator get={} handle={}\n", .{ it, main.placement_iter != null });
        if (it != c.GHOSTTY_SUCCESS) return 0;
    }
    if (main.placement_iter == null) return 0;

    while (c.ghostty_kitty_graphics_placement_next(main.placement_iter)) {
        var image_id: u32 = 0;
        var z: i32 = 0;
        var is_virtual = false;
        if (c.ghostty_kitty_graphics_placement_get(main.placement_iter, c.GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID, &image_id) != c.GHOSTTY_SUCCESS)
            continue;
        _ = c.ghostty_kitty_graphics_placement_get(main.placement_iter, c.GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z, &z);
        _ = c.ghostty_kitty_graphics_placement_get(main.placement_iter, c.GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL, &is_virtual);

        if (main.debug_enabled) std.debug.print("img: placement id={} virtual={} z={}\n", .{ image_id, is_virtual, z });
        const image = c.ghostty_kitty_graphics_image(graphics, image_id);
        if (image == null) continue;

        var pixels: [*c]const u8 = null;
        var image_w: u32 = 0;
        var image_h: u32 = 0;
        var generation: u64 = 0;
        const kinds = [_]c.GhosttyKittyGraphicsImageData{
            c.GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR,
            c.GHOSTTY_KITTY_IMAGE_DATA_WIDTH,
            c.GHOSTTY_KITTY_IMAGE_DATA_HEIGHT,
            c.GHOSTTY_KITTY_IMAGE_DATA_GENERATION,
        };
        var values = [_]?*anyopaque{
            @ptrCast(&pixels),
            @ptrCast(&image_w),
            @ptrCast(&image_h),
            @ptrCast(&generation),
        };
        if (c.ghostty_kitty_graphics_image_get_multi(image, kinds.len, &kinds, &values, null) != c.GHOSTTY_SUCCESS)
            continue;
        if (pixels == null or image_w == 0 or image_h == 0) continue;

        out.* = std.mem.zeroes(c.PinwinImage);
        out.*.image_id = image_id;
        out.*.generation = @intCast(generation);
        out.*.z = z;
        out.*.image_w = @intCast(image_w);
        out.*.image_h = @intCast(image_h);
        out.*.pixels = pixels;

        if (is_virtual) {
            // A unicode-placeholder placement has no viewport position of its
            // own: it is drawn where its placeholder cells are, at the image's
            // own pixel size (the program sizes it to that cell box).
            const origin = placeholderOrigin(image_id) orelse {
                if (main.debug_enabled) std.debug.print("img: no placeholder origin for {}\n", .{image_id});
                continue;
            };
            if (main.debug_enabled) std.debug.print("img: virtual origin row={} col={} size={}x{}\n", .{ origin.row, origin.col, image_w, image_h });
            out.*.x = origin.col * @as(i32, @intCast(main.cell_w));
            out.*.y = origin.row * @as(i32, @intCast(main.cell_h));
            out.*.w = @intCast(image_w);
            out.*.h = @intCast(image_h);
            out.*.sw = @intCast(image_w);
            out.*.sh = @intCast(image_h);
            return 1;
        }

        var info = std.mem.zeroes(c.GhosttyKittyGraphicsPlacementRenderInfo);
        info.size = @sizeOf(c.GhosttyKittyGraphicsPlacementRenderInfo);
        if (c.ghostty_kitty_graphics_placement_render_info(main.placement_iter, image, main.term, &info) != c.GHOSTTY_SUCCESS)
            continue;
        if (!info.viewport_visible) continue;

        out.*.x = info.viewport_col * @as(i32, @intCast(main.cell_w));
        out.*.y = info.viewport_row * @as(i32, @intCast(main.cell_h));
        out.*.w = @intCast(info.pixel_width);
        out.*.h = @intCast(info.pixel_height);
        out.*.sx = @intCast(info.source_x);
        out.*.sy = @intCast(info.source_y);
        out.*.sw = @intCast(info.source_width);
        out.*.sh = @intCast(info.source_height);
        return 1;
    }
    return 0;
}

export fn pinwin_frame_end() void {
    if (!frame_open) return;
    frame_open = false;
    _ = c.ghostty_render_state_clean(main.render_state);
}
