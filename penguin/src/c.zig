//! The single C translation unit penguin's Zig code sees.
//!
//! `ghostty/vt.h` is libghostty-vt's C API. `penguin.h` is penguin's own
//! GTK-free interface to glue.c: Zig's translate-c cannot consume the GTK4
//! headers, so no GTK header is ever translated (see penguin.h).
pub const c = @cImport({
    @cInclude("ghostty/vt.h");
    @cInclude("penguin.h");
});
