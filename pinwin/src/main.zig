//! pinwin — a terminal docked at the left edge of one monitor via
//! wlr-layer-shell, running one command (mbv by default).
//!
//! Layout: `main.zig` owns the terminal (libghostty-vt), its setup and the
//! render/input state the other Zig files drive; `cells.zig` runs the frame
//! protocol and `input.zig` the input encoders; `glue.c` and its siblings own
//! GTK4, gtk4-layer-shell, Pango, cairo, GdkPixbuf and the PTY. `pinwin.h` is
//! the whole interface between them and the only C header besides
//! `ghostty/vt.h` that this file imports: Zig's translate-c cannot consume the
//! GTK4 headers (see pinwin.h).
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

const DEFAULT_COLS: u32 = 40;
const DEFAULT_GUTTER: u32 = 0;
const DEFAULT_KEYBOARD = "on-demand";

/// Ghostty's own default for `image-storage-limit`, and what mbv's posters fit in.
const KITTY_STORAGE_LIMIT: u64 = 320 * 1024 * 1024;

pub const allocator = std.heap.c_allocator;

// ---- shared terminal state (cells.zig and input.zig read these) -----------

pub var term: c.GhosttyTerminal = null;
pub var render_state: c.GhosttyRenderState = null;
pub var row_iter: c.GhosttyRenderStateRowIterator = null;
pub var cells: c.GhosttyRenderStateRowCells = null;
pub var key_encoder: c.GhosttyKeyEncoder = null;
pub var key_event: c.GhosttyKeyEvent = null;
pub var mouse_encoder: c.GhosttyMouseEncoder = null;
pub var mouse_event: c.GhosttyMouseEvent = null;
pub var placement_iter: c.GhosttyKittyGraphicsPlacementIterator = null;

pub var grid_cols: u16 = DEFAULT_COLS;
pub var grid_rows: u16 = 24;
pub var cell_w: u32 = 1;
pub var cell_h: u32 = 1;

/// PINWIN_DEBUG=1 routes libghostty-vt's own log through stderr.
pub var debug_enabled = false;

// The frame and input halves are driven from C through pinwin.h; importing
// them here pulls their exported functions into the build.
comptime {
    _ = @import("cells.zig");
    _ = @import("input.zig");
}

pub fn main(init: std.process.Init.Minimal) void {
    const cols = envSetting(init.environ, "COLS", DEFAULT_COLS, true);
    const gutter = envSetting(init.environ, "GUTTER", DEFAULT_GUTTER, false);
    const keyboard = keyboardMode(init.environ);

    const command = commandArgv(init.environ, init.args) catch |err| {
        std.debug.print("pinwin: {s}\n", .{@errorName(err)});
        std.process.exit(1);
    };

    debug_enabled = std.process.Environ.getPosix(init.environ, "PINWIN_DEBUG") != null;

    // Process-global, and must be installed before the terminal exists.
    _ = c.ghostty_sys_set(c.GHOSTTY_SYS_OPT_DECODE_PNG, @ptrCast(&decodePng));
    if (debug_enabled) _ = c.ghostty_sys_set(c.GHOSTTY_SYS_OPT_LOG, @ptrCast(&c.ghostty_sys_log_stderr));

    if (c.glue_init(@intCast(cols), @intCast(gutter), keyboard, @intFromBool(command.no_tray)) == 0) std.process.exit(1);
    c.glue_start(@ptrCast(command.argv.ptr));
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

/// The command to run in the panel, NUL-terminated for execvp, plus whether
/// the user passed `--no-tray` (design D2): pinwin's own options precede the
/// command and `--` ends them; an unknown leading `--...` argument is an
/// error naming it. With no command, `$SHELL` runs (or `/bin/sh` when `SHELL`
/// is unset or empty).
const Command = struct { argv: [:null]?[*:0]const u8, no_tray: bool };

fn commandArgv(environ: std.process.Environ, args: std.process.Args) !Command {
    var list: std.ArrayList(?[*:0]const u8) = .empty;
    var iterator = std.process.Args.iterate(args);
    _ = iterator.skip(); // argv[0], pinwin itself
    var no_tray = false;
    var options_done = false;
    while (iterator.next()) |arg| {
        if (!options_done) {
            if (std.mem.eql(u8, arg, "--no-tray")) {
                no_tray = true;
                continue;
            }
            if (std.mem.eql(u8, arg, "--")) {
                options_done = true;
                continue;
            }
            if (std.mem.startsWith(u8, arg, "--")) {
                std.debug.print("pinwin: unknown option '{s}'\n", .{arg});
                std.process.exit(2);
            }
            // The first non-option argument starts the command; everything
            // after it, option-looking or not, belongs to the command.
            options_done = true;
        }
        try list.append(allocator, arg.ptr);
    }
    if (list.items.len == 0) {
        const shell = std.process.Environ.getPosix(environ, "SHELL") orelse "/bin/sh";
        try list.append(allocator, if (shell.len > 0) shell.ptr else "/bin/sh");
    }
    try list.append(allocator, null);
    return .{ .argv = list.items[0 .. list.items.len - 1 :null], .no_tray = no_tray };
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

/// Called by the glue when the grid changes.
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

/// Called by the glue with bytes read from the PTY.
export fn pinwin_pty_data(data: [*c]const u8, len: usize) void {
    if (term == null) return;
    c.ghostty_terminal_vt_write(term, data, len);
    c.glue_queue_draw();
}
