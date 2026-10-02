//! The single C translation unit pinwin's Zig code sees.
//!
//! `ghostty/vt.h` is libghostty-vt's C API. `pinwin.h` is pinwin's own
//! GTK-free interface to glue.c: Zig's translate-c cannot consume the GTK4
//! headers, so no GTK header is ever translated (see pinwin.h).
pub const c = @cImport({
    @cInclude("ghostty/vt.h");
    @cInclude("pinwin.h");
});
