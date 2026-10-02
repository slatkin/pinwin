//! pinwin — a terminal docked at the left edge of one monitor via
//! wlr-layer-shell, running one command (mbv by default).
//!
//! Layout: `main.zig` owns the terminal (libghostty-vt), the render state and
//! the input encoders; `glue.c` owns GTK4, gtk4-layer-shell, Pango, cairo,
//! GdkPixbuf and the PTY. `pinwin.h` is the whole interface between them and
//! the only C header besides `ghostty/vt.h` that this file imports: Zig's
//! translate-c cannot consume the GTK4 headers (see pinwin.h).
//!
//! Notes recorded while pinning libghostty-vt (ghostty-org/ghostty
//! 3a3047f6b62a791fd8b12d9f07a85b3d2160370b) — task 1.3 of
//! `add-pinwin-panel`. See design.md D2/D7/D8.
//!
//! Terminal (include/ghostty/vt/terminal.h):
//!   ghostty_terminal_new(allocator, &term, cols, rows)
//!   ghostty_terminal_resize(term, cols, rows, cell_w, cell_h)
//!   ghostty_terminal_vt_write(term, ptr, len)
//!   ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_*, &value)
//!   ghostty_terminal_get(term, GHOSTTY_TERMINAL_DATA_*, &out)
//!
//! Effect callbacks (installed with ghostty_terminal_set, sized-struct ABI):
//!   write_pty:        void (*)(GhosttyTerminal, void*, const uint8_t*, size_t)
//!   size (CSI 14/16/18 t): bool (*)(GhosttyTerminal, void*, GhosttySizeReportSize*)
//!   device_attributes: bool (*)(GhosttyTerminal, void*, GhosttyDeviceAttributes*)
//! The size/device_attributes callbacks return false to ignore the query.
//!
//! DA1/DA2/DA3 values are Ghostty's own (src/termio/stream_handler.zig
//! deviceAttributes), not the library defaults:
//!   primary   conformance_level 62 (level 2), features {22 ansi_color, 52 clipboard}
//!   secondary device_type 1 (VT220), firmware_version 10, rom_cartridge 0
//!   tertiary  unit_id 0
//! -> CSI c replies `\x1b[?62;22;52c`, CSI > c replies `\x1b[>1;10;0c`.
//!
//! Input encoders (key/encoder.h, mouse/encoder.h, focus.h). Both encoders are
//! driven straight from terminal state, so modes the program changes at runtime
//! (kitty flags, mouse tracking, SGR format) are picked up per event:
//!   ghostty_key_encoder_setopt_from_terminal(enc, term)
//!   ghostty_mouse_encoder_setopt_from_terminal(enc, term)
//!   ghostty_mouse_encoder_setopt(enc, GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &size)
//!   ghostty_key_event_new / _set_action / _set_key / _set_mods / _set_utf8 /
//!     _set_unshifted_codepoint
//!   ghostty_key_encoder_encode(enc, event, buf, buf_len, &written)
//!   ghostty_focus_encode(GHOSTTY_FOCUS_GAINED|LOST, buf, len, &written)
//!
//! Kitty graphics (sys.h, kitty_graphics.h): enabled only when
//! GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT is non-zero *and* a PNG
//! decoder is installed with ghostty_sys_set(GHOSTTY_SYS_OPT_DECODE_PNG, fn),
//! whose signature is
//!   bool (*)(void* userdata, const GhosttyAllocator*, const uint8_t* data,
//!            size_t data_len, GhosttySysImage* out)
//! (out->data allocated with ghostty_alloc(allocator, n), RGBA). Placements are
//! read via GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS ->
//! ghostty_kitty_graphics_get(..._DATA_PLACEMENT_ITERATOR, &iter),
//! ghostty_kitty_graphics_placement_next(iter) and
//! ghostty_kitty_graphics_placement_render_info(iter, image, term, &info).

const std = @import("std");

const c = @import("c.zig").c;
const keys = @import("keys.zig");

const DEFAULT_COLS: u32 = 40;
const DEFAULT_GUTTER: u32 = 0;
const DEFAULT_KEYBOARD = "on-demand";

/// Ghostty's own default for `image-storage-limit`, and what mbv's posters fit in.
const KITTY_STORAGE_LIMIT: u64 = 320 * 1024 * 1024;

/// `ghostty_mode_new(1004, false)`: mode 1004 without the ANSI-mode bit. Taken
/// by hand because the cimport's inline `ghostty_mode_new` does not translate.
const MODE_FOCUS_EVENT: c.GhosttyMode = 1004;

const allocator = std.heap.c_allocator;

var term: c.GhosttyTerminal = null;
var render_state: c.GhosttyRenderState = null;
var row_iter: c.GhosttyRenderStateRowIterator = null;
var cells: c.GhosttyRenderStateRowCells = null;
var key_encoder: c.GhosttyKeyEncoder = null;
var key_event: c.GhosttyKeyEvent = null;
var mouse_encoder: c.GhosttyMouseEncoder = null;
var mouse_event: c.GhosttyMouseEvent = null;

var grid_cols: u16 = DEFAULT_COLS;
var grid_rows: u16 = 24;
var cell_w: u32 = 1;
var cell_h: u32 = 1;

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
    if (debug_enabled) std.debug.print("placeholder new image_id={} at row={} col={}\n", .{ key, row, col });
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
var placement_iter: c.GhosttyKittyGraphicsPlacementIterator = null;
var images_started = false;

var any_button_pressed = false;
/// The focus state last reported to the program. GTK sends a leave at map
/// time, before any enter; programs expect balanced reports, so only actual
/// changes are reported.
var focus_gained = false;
/// Left/right, up/down remainders of a scroll in notches (see pinwin_scroll).
var scroll_acc: [2]f64 = .{ 0, 0 };
/// PINWIN_DEBUG=1 routes libghostty-vt's own log through stderr.
var debug_enabled = false;

pub fn main(init: std.process.Init.Minimal) void {
    const cols = envSetting(init.environ, "COLS", DEFAULT_COLS, true);
    const gutter = envSetting(init.environ, "GUTTER", DEFAULT_GUTTER, false);
    const keyboard = keyboardMode(init.environ);

    const argv = commandArgv(init.args) catch |err| {
        std.debug.print("pinwin: {s}\n", .{@errorName(err)});
        std.process.exit(1);
    };

    debug_enabled = std.process.Environ.getPosix(init.environ, "PINWIN_DEBUG") != null;

    // Process-global, and must be installed before the terminal exists.
    _ = c.ghostty_sys_set(c.GHOSTTY_SYS_OPT_DECODE_PNG, @ptrCast(&decodePng));
    if (debug_enabled) _ = c.ghostty_sys_set(c.GHOSTTY_SYS_OPT_LOG, @ptrCast(&c.ghostty_sys_log_stderr));

    if (c.glue_init(@intCast(cols), @intCast(gutter), keyboard) == 0) std.process.exit(1);
    c.glue_start(@ptrCast(argv.ptr));
}

/// `COLS` and `GUTTER`, per design D9: an error names the variable and exits 2
/// before any surface exists.
fn envSetting(environ: std.process.Environ, name: []const u8, default: u32, must_be_positive: bool) u32 {
    const raw = std.process.Environ.getPosix(environ, name) orelse return default;
    const value = std.fmt.parseInt(u32, raw, 10) catch {
        std.debug.print("pinwin: {s}: expected a non-negative integer, got '{s}'\n", .{ name, raw });
        std.process.exit(2);
    };
    if (must_be_positive and value == 0) {
        std.debug.print("pinwin: {s}: must be greater than 0\n", .{name});
        std.process.exit(2);
    }
    return value;
}

/// `PINWIN_KEYBOARD`: layer-shell keyboard interactivity. `on-demand` (the
/// default) means the panel only gets the keyboard after a click; `exclusive`
/// takes it as soon as the panel opens; `none` never takes it.
fn keyboardMode(environ: std.process.Environ) i32 {
    const raw = std.process.Environ.getPosix(environ, "PINWIN_KEYBOARD") orelse return c.PINWIN_KEYBOARD_ON_DEMAND;
    if (std.mem.eql(u8, raw, "on-demand")) return c.PINWIN_KEYBOARD_ON_DEMAND;
    if (std.mem.eql(u8, raw, "exclusive")) return c.PINWIN_KEYBOARD_EXCLUSIVE;
    if (std.mem.eql(u8, raw, "none")) return c.PINWIN_KEYBOARD_NONE;
    std.debug.print("pinwin: PINWIN_KEYBOARD: expected on-demand, exclusive or none, got '{s}'\n", .{raw});
    std.process.exit(2);
}

/// The command to run in the panel, NUL-terminated for execvp: pinwin's own
/// arguments, or `mbv` when it has none.
fn commandArgv(args: std.process.Args) ![:null]?[*:0]const u8 {
    var list: std.ArrayList(?[*:0]const u8) = .empty;
    var iterator = std.process.Args.iterate(args);
    _ = iterator.skip(); // argv[0], pinwin itself
    while (iterator.next()) |arg| try list.append(allocator, arg.ptr);
    if (list.items.len == 0) try list.append(allocator, "mbv");
    try list.append(allocator, null);
    return list.items[0 .. list.items.len - 1 :null];
}

// ---- terminal -------------------------------------------------------------

fn ensureTerminal() !void {
    if (term != null) return;

    if (c.ghostty_terminal_new(null, &term, grid_cols, grid_rows) != c.GHOSTTY_SUCCESS)
        return error.TerminalNewFailed;
    _ = c.ghostty_terminal_set(term, c.GHOSTTY_TERMINAL_OPT_WRITE_PTY, @ptrCast(&writePty));
    _ = c.ghostty_terminal_set(term, c.GHOSTTY_TERMINAL_OPT_SIZE, @ptrCast(&sizeReport));
    _ = c.ghostty_terminal_set(term, c.GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES, @ptrCast(&deviceAttributes));

    var storage_limit: u64 = KITTY_STORAGE_LIMIT;
    _ = c.ghostty_terminal_set(term, c.GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT, &storage_limit);

    if (c.ghostty_render_state_new(null, &render_state) != c.GHOSTTY_SUCCESS)
        return error.RenderStateNewFailed;
    if (c.ghostty_render_state_row_iterator_new(null, &row_iter) != c.GHOSTTY_SUCCESS)
        return error.RowIteratorNewFailed;
    if (c.ghostty_render_state_row_cells_new(null, &cells) != c.GHOSTTY_SUCCESS)
        return error.RowCellsNewFailed;
    if (c.ghostty_kitty_graphics_placement_iterator_new(null, &placement_iter) != c.GHOSTTY_SUCCESS)
        return error.PlacementIteratorNewFailed;

    if (c.ghostty_key_encoder_new(null, &key_encoder) != c.GHOSTTY_SUCCESS)
        return error.KeyEncoderNewFailed;
    if (c.ghostty_key_event_new(null, &key_event) != c.GHOSTTY_SUCCESS)
        return error.KeyEventNewFailed;
    if (c.ghostty_mouse_encoder_new(null, &mouse_encoder) != c.GHOSTTY_SUCCESS)
        return error.MouseEncoderNewFailed;
    if (c.ghostty_mouse_event_new(null, &mouse_event) != c.GHOSTTY_SUCCESS)
        return error.MouseEventNewFailed;
}

fn writePty(_: c.GhosttyTerminal, _: ?*anyopaque, data: [*c]const u8, len: usize) callconv(.c) void {
    c.glue_pty_write(data, len);
}

fn sizeReport(_: c.GhosttyTerminal, _: ?*anyopaque, out: *c.GhosttySizeReportSize) callconv(.c) bool {
    out.rows = grid_rows;
    out.columns = grid_cols;
    out.cell_width = cell_w;
    out.cell_height = cell_h;
    return true;
}

fn deviceAttributes(_: c.GhosttyTerminal, _: ?*anyopaque, out: *c.GhosttyDeviceAttributes) callconv(.c) bool {
    out.primary.conformance_level = 62; // level 2, like Ghostty
    out.primary.features[0] = 22; // ansi color
    out.primary.features[1] = 52; // clipboard
    out.primary.num_features = 2;
    out.secondary.device_type = 1;
    out.secondary.firmware_version = 10;
    out.secondary.rom_cartridge = 0;
    out.tertiary.unit_id = 0;
    return true;
}

fn decodePng(
    _: ?*anyopaque,
    ghostty_allocator: *const c.GhosttyAllocator,
    data: [*c]const u8,
    data_len: usize,
    out: *c.GhosttySysImage,
) callconv(.c) bool {
    var pixels: [*c]u8 = null;
    var width: u32 = 0;
    var height: u32 = 0;

    if (c.glue_decode_png(data, data_len, &pixels, &width, &height) == 0) return false;
    defer std.c.free(pixels);

    const len = @as(usize, width) * @as(usize, height) * 4;
    const buf = c.ghostty_alloc(ghostty_allocator, len);
    if (buf == null) return false;
    @memcpy(buf[0..len], pixels[0..len]);

    out.width = width;
    out.height = height;
    out.data = buf;
    out.data_len = len;
    return true;
}

/// Called by glue.c whenever the grid changes.
export fn pinwin_size(cols: i32, rows: i32, cw: i32, ch: i32) void {
    grid_cols = @intCast(@max(cols, 1));
    grid_rows = @intCast(@max(rows, 1));
    cell_w = @intCast(@max(cw, 1));
    cell_h = @intCast(@max(ch, 1));

    ensureTerminal() catch |err| {
        std.debug.print("pinwin: {s}\n", .{@errorName(err)});
        std.process.exit(1);
    };
    _ = c.ghostty_terminal_resize(term, grid_cols, grid_rows, cell_w, cell_h);

    var size = std.mem.zeroes(c.GhosttyMouseEncoderSize);
    size.size = @sizeOf(c.GhosttyMouseEncoderSize);
    size.screen_width = @as(u32, grid_cols) * cell_w;
    size.screen_height = @as(u32, grid_rows) * cell_h;
    size.cell_width = cell_w;
    size.cell_height = cell_h;
    c.ghostty_mouse_encoder_setopt(mouse_encoder, c.GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &size);
}

/// Called by glue.c with bytes read from the PTY.
export fn pinwin_pty_data(data: [*c]const u8, len: usize) void {
    if (term == null) return;
    c.ghostty_terminal_vt_write(term, data, len);
    c.glue_queue_draw();
}

// ---- rendering ------------------------------------------------------------

/// Starts a frame. Every draw callback draws a complete frame: GTK asks for a
/// redraw for its own reasons too (focus, exposure, resize), and a frame that
/// skipped the cells it thinks are unchanged would leave those areas blank,
/// because the callback paints the panel's background first.
export fn pinwin_frame_begin() i32 {
    if (term == null) return 0;
    if (c.ghostty_render_state_update(render_state, term) != c.GHOSTTY_SUCCESS) return 0;

    var colors = std.mem.zeroes(c.GhosttyRenderStateColors);
    colors.size = @sizeOf(c.GhosttyRenderStateColors);
    if (c.ghostty_render_state_get(render_state, c.GHOSTTY_RENDER_STATE_DATA_COLORS, &colors) == c.GHOSTTY_SUCCESS) {
        default_bg = colors.background;
        default_fg = colors.foreground;
    }

    _ = c.ghostty_render_state_get(render_state, c.GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, @ptrCast(&row_iter));

    frame_cursor = std.mem.zeroes(c.PinwinCursor);
    var cursor = std.mem.zeroes(c.GhosttyRenderStateCursor);
    cursor.size = @sizeOf(c.GhosttyRenderStateCursor);
    if (c.ghostty_render_state_get(render_state, c.GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor) == c.GHOSTTY_SUCCESS and
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
    _ = c.ghostty_render_state_get(render_state, c.GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, @ptrCast(&row_iter));
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
            if (c.ghostty_render_state_row_cells_next(cells)) {
                fillCell(out);
                cell_x += 1;
                return true;
            }
            in_row = false;
        }

        if (!c.ghostty_render_state_row_iterator_next(row_iter)) return false;
        // The whole grid is walked every frame (see design D5): the draw
        // callback repaints the panel from scratch, so a row that is not
        // visited would be left blank.
        var viewport_y: i32 = 0;
        if (c.ghostty_render_state_row_get(row_iter, c.GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y, &viewport_y) != c.GHOSTTY_SUCCESS)
            continue;
        cell_y = viewport_y;
        if (c.ghostty_render_state_row_get(row_iter, c.GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, @ptrCast(&cells)) != c.GHOSTTY_SUCCESS)
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
    _ = c.ghostty_render_state_row_cells_get(cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN, &grapheme_len);
    if (grapheme_len > 0) {
        var codepoints: [8]u32 = undefined;
        const count = @min(grapheme_len, codepoints.len);
        _ = c.ghostty_render_state_row_cells_get(cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF, &codepoints);
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
    if (c.ghostty_render_state_row_cells_get(cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &raw) == c.GHOSTTY_SUCCESS)
        _ = c.ghostty_cell_get(raw, c.GHOSTTY_CELL_DATA_WIDE, &wide);
    out.*.wide = @intCast(wide);

    var style = std.mem.zeroes(c.GhosttyStyle);
    style.size = @sizeOf(c.GhosttyStyle);
    _ = c.ghostty_render_state_row_cells_get(cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style);
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
    var has_fg = c.ghostty_render_state_row_cells_get(cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &fg) == c.GHOSTTY_SUCCESS;
    var has_bg = c.ghostty_render_state_row_cells_get(cells, c.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &bg) == c.GHOSTTY_SUCCESS;

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
        const got = c.ghostty_terminal_get(term, c.GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, @ptrCast(&graphics));
        if (debug_enabled) std.debug.print("img: graphics get={} handle={}\n", .{ got, graphics != null });
        if (got != c.GHOSTTY_SUCCESS) return 0;
        if (graphics == null) return 0;
        const it = c.ghostty_kitty_graphics_get(graphics, c.GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR, @ptrCast(&placement_iter));
        if (debug_enabled) std.debug.print("img: iterator get={} handle={}\n", .{ it, placement_iter != null });
        if (it != c.GHOSTTY_SUCCESS) return 0;
    }
    if (placement_iter == null) return 0;

    while (c.ghostty_kitty_graphics_placement_next(placement_iter)) {
        var image_id: u32 = 0;
        var z: i32 = 0;
        var is_virtual = false;
        if (c.ghostty_kitty_graphics_placement_get(placement_iter, c.GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID, &image_id) != c.GHOSTTY_SUCCESS)
            continue;
        _ = c.ghostty_kitty_graphics_placement_get(placement_iter, c.GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z, &z);
        _ = c.ghostty_kitty_graphics_placement_get(placement_iter, c.GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL, &is_virtual);

        if (debug_enabled) std.debug.print("img: placement id={} virtual={} z={}\n", .{ image_id, is_virtual, z });
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
                if (debug_enabled) std.debug.print("img: no placeholder origin for {}\n", .{image_id});
                continue;
            };
            if (debug_enabled) std.debug.print("img: virtual origin row={} col={} size={}x{}\n", .{ origin.row, origin.col, image_w, image_h });
            out.*.x = origin.col * @as(i32, @intCast(cell_w));
            out.*.y = origin.row * @as(i32, @intCast(cell_h));
            out.*.w = @intCast(image_w);
            out.*.h = @intCast(image_h);
            out.*.sw = @intCast(image_w);
            out.*.sh = @intCast(image_h);
            return 1;
        }

        var info = std.mem.zeroes(c.GhosttyKittyGraphicsPlacementRenderInfo);
        info.size = @sizeOf(c.GhosttyKittyGraphicsPlacementRenderInfo);
        if (c.ghostty_kitty_graphics_placement_render_info(placement_iter, image, term, &info) != c.GHOSTTY_SUCCESS)
            continue;
        if (!info.viewport_visible) continue;

        out.*.x = info.viewport_col * @as(i32, @intCast(cell_w));
        out.*.y = info.viewport_row * @as(i32, @intCast(cell_h));
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
    _ = c.ghostty_render_state_clean(render_state);
}

// ---- input ----------------------------------------------------------------

/// Writes an encoded sequence to the PTY. `written` becomes the required size
/// when `GhosttyResult` is GHOSTTY_OUT_OF_SPACE, so the caller retries once
/// with an exact-size buffer instead of dropping the event.
fn writeEncoded(comptime encode: fn ([*c]u8, usize, *usize) c.GhosttyResult, buf: []u8) void {
    var written: usize = 0;
    var result = encode(buf.ptr, buf.len, &written);
    if (result == c.GHOSTTY_OUT_OF_SPACE) {
        const exact = allocator.alloc(u8, written) catch return;
        defer allocator.free(exact);
        result = encode(exact.ptr, exact.len, &written);
        if (result == c.GHOSTTY_SUCCESS and written > 0) c.glue_pty_write(exact.ptr, written);
        return;
    }
    if (result == c.GHOSTTY_SUCCESS and written > 0) c.glue_pty_write(buf.ptr, written);
}

fn encodeKey(out: [*c]u8, len: usize, written: *usize) c.GhosttyResult {
    return c.ghostty_key_encoder_encode(key_encoder, key_event, out, len, written);
}

fn encodeMouse(out: [*c]u8, len: usize, written: *usize) c.GhosttyResult {
    return c.ghostty_mouse_encoder_encode(mouse_encoder, mouse_event, out, len, written);
}

export fn pinwin_key(action: i32, keyval: i32, keycode: i32, mods: u32, consumed_mods: u32, is_modifier: i32) void {
    if (term == null) return;

    const key_action: c.GhosttyKeyAction = switch (action) {
        c.PINWIN_KEY_RELEASE => c.GHOSTTY_KEY_ACTION_RELEASE,
        c.PINWIN_KEY_REPEAT => c.GHOSTTY_KEY_ACTION_REPEAT,
        else => c.GHOSTTY_KEY_ACTION_PRESS,
    };

    const keycode_u: u32 = @intCast(@max(keycode, 0));
    const keyval_u: u32 = @intCast(@max(keyval, 0));
    const key = physicalKey(keycode_u, keyval_u, is_modifier != 0);

    c.ghostty_key_event_set_action(key_event, key_action);
    c.ghostty_key_event_set_key(key_event, key);
    c.ghostty_key_event_set_mods(key_event, @intCast(mods));
    c.ghostty_key_event_set_consumed_mods(key_event, @intCast(consumed_mods));
    c.ghostty_key_event_set_unshifted_codepoint(key_event, c.glue_keycode_unshifted_codepoint(keycode_u));

    var text: [8]u8 = undefined;
    var text_len: usize = 0;
    const codepoint = c.glue_keyval_unicode(keyval_u);
    if (key != c.GHOSTTY_KEY_UNIDENTIFIED and key_action != c.GHOSTTY_KEY_ACTION_RELEASE and
        codepoint >= 0x20 and codepoint <= 0x10ffff)
    {
        text_len = std.unicode.utf8Encode(@intCast(codepoint), &text) catch 0;
    }
    c.ghostty_key_event_set_utf8(key_event, &text, text_len);

    c.ghostty_key_encoder_setopt_from_terminal(key_encoder, term);
    var buf: [256]u8 = undefined;
    if (debug_enabled) {
        var kitty: u8 = 0;
        _ = c.ghostty_terminal_get(term, c.GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, @ptrCast(&kitty));
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
    if (term == null) return;

    c.ghostty_mouse_encoder_setopt_from_terminal(mouse_encoder, term);
    var size = std.mem.zeroes(c.GhosttyMouseEncoderSize);
    size.size = @sizeOf(c.GhosttyMouseEncoderSize);
    size.screen_width = @as(u32, grid_cols) * cell_w;
    size.screen_height = @as(u32, grid_rows) * cell_h;
    size.cell_width = cell_w;
    size.cell_height = cell_h;
    c.ghostty_mouse_encoder_setopt(mouse_encoder, c.GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &size);
    var pressed = any_button_pressed;
    c.ghostty_mouse_encoder_setopt(mouse_encoder, c.GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED, &pressed);

    c.ghostty_mouse_event_set_action(mouse_event, action);
    if (button == c.PINWIN_MOUSE_UNKNOWN) {
        c.ghostty_mouse_event_clear_button(mouse_event);
    } else {
        c.ghostty_mouse_event_set_button(mouse_event, @intCast(button));
    }
    c.ghostty_mouse_event_set_mods(mouse_event, @intCast(mods));
    var position = std.mem.zeroes(c.GhosttyMousePosition);
    position.x = @floatCast(x);
    position.y = @floatCast(y);
    c.ghostty_mouse_event_set_position(mouse_event, position);

    var buf: [64]u8 = undefined;
    writeEncoded(encodeMouse, &buf);
}

export fn pinwin_mouse(action: i32, x: f64, y: f64, button: i32, mods: u32) void {
    if (term == null) return;
    const mouse_action: c.GhosttyMouseAction = switch (action) {
        c.PINWIN_MOUSE_RELEASE => c.GHOSTTY_MOUSE_ACTION_RELEASE,
        c.PINWIN_MOUSE_MOTION => c.GHOSTTY_MOUSE_ACTION_MOTION,
        else => c.GHOSTTY_MOUSE_ACTION_PRESS,
    };
    if (debug_enabled)
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
    if (term == null) return;

    if (debug_enabled)
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
    if (term == null) return;

    const now_focused = gained != 0;
    if (now_focused == focus_gained) return;
    focus_gained = now_focused;

    var config = std.mem.zeroes(c.GhosttyTerminalModeConfig);
    config.mode = MODE_FOCUS_EVENT;
    if (c.ghostty_terminal_get(term, c.GHOSTTY_TERMINAL_DATA_MODE, &config) != c.GHOSTTY_SUCCESS) return;
    if (debug_enabled)
        std.debug.print("focus gained={} mode1004={}\n", .{ gained != 0, config.value });
    if (!config.value) return;

    var buf: [8]u8 = undefined;
    var written: usize = 0;
    const event: c.GhosttyFocusEvent = if (gained != 0) c.GHOSTTY_FOCUS_GAINED else c.GHOSTTY_FOCUS_LOST;
    if (c.ghostty_focus_encode(event, &buf, buf.len, &written) == c.GHOSTTY_SUCCESS and written > 0)
        c.glue_pty_write(&buf, written);
}
