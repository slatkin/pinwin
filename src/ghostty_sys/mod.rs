//! Hand-written FFI declarations for the pinned libghostty-vt (port-to-rust
//! D2: the FFI crate was rejected, so pinwin owns these declarations).
//!
//! This module is the only place where pinwin talks to C. It is bound to the
//! one ghostty commit `build.rs` fetches
//! (`3a3047f6b62a791fd8b12d9f07a85b3d2160370b`); every constant, struct layout
//! and function signature below is transcribed from that commit's generated
//! headers under `OUT_DIR/ghostty/include/ghostty/vt/`. The layout test in
//! this module compiles a C probe against those same headers at test time and
//! compares it against the Rust `#[repr(C)]` layouts, so a pin change that
//! shifts an ABI fact fails the test instead of corrupting memory.
//!
//! Conventions:
//!
//! * C enums are backed by `int` on the targets pinwin builds for (types.h's
//!   own `GHOSTTY_ENUM_TYPED` note: "the Zig side backs all C enums with
//!   `c_int`"). Each enum is therefore a type alias for the matching Rust
//!   integer plus `GHOSTTY_*` constants, never a Rust `enum`: C may hand back
//!   a value that is not one of the declared constants, and a Rust `enum`
//!   with an invalid discriminant is undefined behaviour.
//! * Opaque handles are `#[repr(transparent)]` newtypes over `*mut c_void`
//!   (or `*const c_void` for the borrowed kitty image handle) so their size
//!   and calling convention are exactly the C pointer's.
//! * Sized structs are `#[repr(C)]` with the `size` field first, matching the
//!   `GHOSTTY_INIT_SIZED` convention: callers zero the struct and set `size`
//!   to `size_of::<T>()` before handing it to the library.
//!
//! ## Panics must never cross the FFI boundary (D5)
//!
//! `Cargo.toml` sets `panic = "unwind"` in every profile, and pinwin wraps
//! host-facing work in `catch_unwind`. An `extern "C" fn` that lets a panic
//! escape is undefined behaviour ("panic in a function that cannot unwind"),
//! and on this platform aborts the process instead. Every callback declared
//! here (`GhosttyTerminalWritePtyFn`, `GhosttyTerminalSizeFn`,
//! `GhosttyTerminalDeviceAttributesFn`, `GhosttySysDecodePngFn`) is therefore
//! called from C into pinwin code that must be panic-free; the Rust
//! trampolines that install `catch_unwind` guards around these callbacks are
//! pinwin's own and arrive with the `term` unit that uses them. No callback
//! declared in this module may unwind.

pub mod build_info;
pub mod input;
pub mod kitty;
pub mod render;
pub mod screen;
pub mod style;
pub mod sys;
pub mod terminal;

use std::os::raw::c_uchar;
use std::os::raw::{c_int, c_void};

/// Result codes for libghostty-vt operations (`GhosttyResult`, types.h).
pub type GhosttyResult = c_int;

pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;
pub const GHOSTTY_IO_ERROR: GhosttyResult = -5;
pub const GHOSTTY_LIMIT_EXCEEDED: GhosttyResult = -6;
pub const GHOSTTY_REJECTED: GhosttyResult = -7;

/// A terminal mode: the low 15 bits are the mode number, bit 15 is the ANSI
/// (DEC vs ANSI) flag (`GhosttyMode`, modes.h). The inline
/// `ghostty_mode_new`/`ghostty_mode_value`/`ghostty_mode_ansi` helpers are
/// reproduced here; `input` needs `ghostty_mode_new(1004, false)`.
pub type GhosttyMode = u16;

/// Build a `GhosttyMode` from a mode number and whether it is an ANSI mode.
pub const fn ghostty_mode_new(value: u16, ansi: bool) -> GhosttyMode {
    (value & 0x7fff) | ((ansi as u16) << 15)
}

/// The mode number of a `GhosttyMode`, without the ANSI bit.
pub const fn ghostty_mode_value(mode: GhosttyMode) -> u16 {
    mode & 0x7fff
}

/// Whether a `GhosttyMode` is an ANSI mode.
pub const fn ghostty_mode_ansi(mode: GhosttyMode) -> bool {
    (mode >> 15) != 0
}

/// A custom memory allocator (`GhosttyAllocator`, allocator.h). A NULL pointer
/// passed to any function that takes `*const GhosttyAllocator` selects the
/// library's default allocator.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyAllocator {
    /// Opaque context passed to every vtable call.
    pub ctx: *mut c_void,
    /// The allocator's function table.
    pub vtable: *const GhosttyAllocatorVtable,
}

/// The Zig-style allocator vtable (`GhosttyAllocatorVtable`, allocator.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyAllocatorVtable {
    pub alloc: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            len: usize,
            alignment: u8,
            ret_addr: usize,
        ) -> *mut c_void,
    >,
    pub resize: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            memory: *mut c_void,
            memory_len: usize,
            alignment: u8,
            new_len: usize,
            ret_addr: usize,
        ) -> bool,
    >,
    pub remap: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            memory: *mut c_void,
            memory_len: usize,
            alignment: u8,
            new_len: usize,
            ret_addr: usize,
        ) -> *mut c_void,
    >,
    pub free: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            memory: *mut c_void,
            memory_len: usize,
            alignment: u8,
            ret_addr: usize,
        ),
    >,
}

unsafe extern "C" {
    /// Allocate `len` bytes with `allocator`, or the default allocator when it
    /// is NULL (allocator.h).
    pub fn ghostty_alloc(allocator: *const GhosttyAllocator, len: usize) -> *mut c_uchar;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::mem::{offset_of, size_of};
    use std::path::PathBuf;
    use std::process::Command;

    /// Every `extern` declared in this module and its submodules. Taking each
    /// function's address forces the linker to resolve it in the pinned
    /// archive, so this test fails if the pin lacks a symbol pinwin uses.
    #[allow(clippy::type_complexity)]
    const LINKED_SYMBOLS: &[*const ()] = &[
        ghostty_alloc as *const (),
        build_info::ghostty_build_info as *const (),
        input::ghostty_focus_encode as *const (),
        input::ghostty_key_encoder_encode as *const (),
        input::ghostty_key_encoder_free as *const (),
        input::ghostty_key_encoder_new as *const (),
        input::ghostty_key_encoder_setopt_from_terminal as *const (),
        input::ghostty_key_event_free as *const (),
        input::ghostty_key_event_new as *const (),
        input::ghostty_key_event_set_action as *const (),
        input::ghostty_key_event_set_consumed_mods as *const (),
        input::ghostty_key_event_set_key as *const (),
        input::ghostty_key_event_set_mods as *const (),
        input::ghostty_key_event_set_unshifted_codepoint as *const (),
        input::ghostty_key_event_set_utf8 as *const (),
        input::ghostty_mouse_encoder_encode as *const (),
        input::ghostty_mouse_encoder_free as *const (),
        input::ghostty_mouse_encoder_new as *const (),
        input::ghostty_mouse_encoder_setopt as *const (),
        input::ghostty_mouse_encoder_setopt_from_terminal as *const (),
        input::ghostty_mouse_event_clear_button as *const (),
        input::ghostty_mouse_event_free as *const (),
        input::ghostty_mouse_event_new as *const (),
        input::ghostty_mouse_event_set_action as *const (),
        input::ghostty_mouse_event_set_button as *const (),
        input::ghostty_mouse_event_set_mods as *const (),
        input::ghostty_mouse_event_set_position as *const (),
        kitty::ghostty_kitty_graphics_get as *const (),
        kitty::ghostty_kitty_graphics_image as *const (),
        kitty::ghostty_kitty_graphics_image_get_multi as *const (),
        kitty::ghostty_kitty_graphics_placement_get as *const (),
        kitty::ghostty_kitty_graphics_placement_iterator_free as *const (),
        kitty::ghostty_kitty_graphics_placement_iterator_new as *const (),
        kitty::ghostty_kitty_graphics_placement_next as *const (),
        kitty::ghostty_kitty_graphics_placement_render_info as *const (),
        render::ghostty_render_state_clean as *const (),
        render::ghostty_render_state_free as *const (),
        render::ghostty_render_state_get as *const (),
        render::ghostty_render_state_new as *const (),
        render::ghostty_render_state_row_cells_free as *const (),
        render::ghostty_render_state_row_cells_get as *const (),
        render::ghostty_render_state_row_cells_new as *const (),
        render::ghostty_render_state_row_cells_next as *const (),
        render::ghostty_render_state_row_get as *const (),
        render::ghostty_render_state_row_iterator_free as *const (),
        render::ghostty_render_state_row_iterator_new as *const (),
        render::ghostty_render_state_row_iterator_next as *const (),
        render::ghostty_render_state_update as *const (),
        screen::ghostty_cell_get as *const (),
        sys::ghostty_sys_set as *const (),
        terminal::ghostty_terminal_free as *const (),
        terminal::ghostty_terminal_get as *const (),
        terminal::ghostty_terminal_new as *const (),
        terminal::ghostty_terminal_resize as *const (),
        terminal::ghostty_terminal_set as *const (),
        terminal::ghostty_terminal_vt_write as *const (),
    ];

    /// Proves `build.rs` linked the pinned archive: the symbol lives in
    /// libghostty-vt and nowhere else in the dependency graph, and every
    /// symbol this module declares resolves too.
    #[test]
    fn pinned_ghostty_archive_links() {
        let mut simd: u8 = 0;
        // SAFETY: ghostty_build_info writes a C `bool` through the out pointer
        // for GHOSTTY_BUILD_INFO_SIMD, and `simd` is valid for that write.
        let result = unsafe {
            build_info::ghostty_build_info(
                build_info::GHOSTTY_BUILD_INFO_SIMD,
                (&mut simd as *mut u8).cast(),
            )
        };
        assert_eq!(
            result, GHOSTTY_SUCCESS,
            "ghostty_build_info returned {result}"
        );
        std::hint::black_box(LINKED_SYMBOLS);
    }

    /// Every numeric id constant this module declares, paired with its name
    /// for the C probe. The probe reads the same identifier from the pinned
    /// headers, so a pin bump that renumbers an ABI id fails
    /// `id_constants_match_the_pin` instead of silently changing behaviour.
    ///
    /// `stringify!` keeps each entry's name and value in sync and the
    /// generated C lines are derived from this list, so every constant here
    /// is checked against the header.
    macro_rules! id_constants {
        ($($constant:path),* $(,)?) => {
            &[$( (stringify!($constant), $constant as i64) ),*]
        };
    }

    const ID_CONSTANTS: &[(&str, i64)] = id_constants![
        GHOSTTY_SUCCESS,
        GHOSTTY_OUT_OF_MEMORY,
        GHOSTTY_INVALID_VALUE,
        GHOSTTY_OUT_OF_SPACE,
        GHOSTTY_NO_VALUE,
        GHOSTTY_IO_ERROR,
        GHOSTTY_LIMIT_EXCEEDED,
        GHOSTTY_REJECTED,
        style::GHOSTTY_STYLE_COLOR_NONE,
        style::GHOSTTY_STYLE_COLOR_PALETTE,
        style::GHOSTTY_STYLE_COLOR_RGB,
        terminal::GHOSTTY_TERMINAL_OPT_USERDATA,
        terminal::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
        terminal::GHOSTTY_TERMINAL_OPT_SIZE,
        terminal::GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
        terminal::GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
        terminal::GHOSTTY_TERMINAL_OPT_MODE,
        terminal::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
        terminal::GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS,
        terminal::GHOSTTY_TERMINAL_DATA_MODE,
        render::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
        render::GHOSTTY_RENDER_STATE_DATA_CURSOR,
        render::GHOSTTY_RENDER_STATE_DATA_COLORS,
        render::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
        render::GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y,
        render::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
        render::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
        render::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
        render::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
        render::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
        render::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
        render::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR,
        render::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK,
        render::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE,
        render::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW,
        kitty::GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR,
        kitty::GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
        kitty::GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL,
        kitty::GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z,
        kitty::GHOSTTY_KITTY_IMAGE_DATA_WIDTH,
        kitty::GHOSTTY_KITTY_IMAGE_DATA_HEIGHT,
        kitty::GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR,
        kitty::GHOSTTY_KITTY_IMAGE_DATA_GENERATION,
        input::GHOSTTY_MODS_SHIFT,
        input::GHOSTTY_MODS_CTRL,
        input::GHOSTTY_MODS_ALT,
        input::GHOSTTY_MODS_SUPER,
        input::GHOSTTY_MODS_CAPS_LOCK,
        input::GHOSTTY_MODS_NUM_LOCK,
        input::GHOSTTY_MODS_SHIFT_SIDE,
        input::GHOSTTY_MODS_CTRL_SIDE,
        input::GHOSTTY_MODS_ALT_SIDE,
        input::GHOSTTY_MODS_SUPER_SIDE,
        input::GHOSTTY_KITTY_KEY_DISABLED,
        input::GHOSTTY_KITTY_KEY_DISAMBIGUATE,
        input::GHOSTTY_KITTY_KEY_REPORT_EVENTS,
        input::GHOSTTY_KITTY_KEY_REPORT_ALTERNATES,
        input::GHOSTTY_KITTY_KEY_REPORT_ALL,
        input::GHOSTTY_KITTY_KEY_REPORT_ASSOCIATED,
        input::GHOSTTY_KEY_ACTION_RELEASE,
        input::GHOSTTY_KEY_ACTION_PRESS,
        input::GHOSTTY_KEY_ACTION_REPEAT,
        input::GHOSTTY_KEY_UNIDENTIFIED,
        input::GHOSTTY_KEY_BACKQUOTE,
        input::GHOSTTY_KEY_BACKSLASH,
        input::GHOSTTY_KEY_BRACKET_LEFT,
        input::GHOSTTY_KEY_BRACKET_RIGHT,
        input::GHOSTTY_KEY_COMMA,
        input::GHOSTTY_KEY_DIGIT_0,
        input::GHOSTTY_KEY_DIGIT_1,
        input::GHOSTTY_KEY_DIGIT_2,
        input::GHOSTTY_KEY_DIGIT_3,
        input::GHOSTTY_KEY_DIGIT_4,
        input::GHOSTTY_KEY_DIGIT_5,
        input::GHOSTTY_KEY_DIGIT_6,
        input::GHOSTTY_KEY_DIGIT_7,
        input::GHOSTTY_KEY_DIGIT_8,
        input::GHOSTTY_KEY_DIGIT_9,
        input::GHOSTTY_KEY_EQUAL,
        input::GHOSTTY_KEY_INTL_BACKSLASH,
        input::GHOSTTY_KEY_INTL_RO,
        input::GHOSTTY_KEY_INTL_YEN,
        input::GHOSTTY_KEY_A,
        input::GHOSTTY_KEY_B,
        input::GHOSTTY_KEY_C,
        input::GHOSTTY_KEY_D,
        input::GHOSTTY_KEY_E,
        input::GHOSTTY_KEY_F,
        input::GHOSTTY_KEY_G,
        input::GHOSTTY_KEY_H,
        input::GHOSTTY_KEY_I,
        input::GHOSTTY_KEY_J,
        input::GHOSTTY_KEY_K,
        input::GHOSTTY_KEY_L,
        input::GHOSTTY_KEY_M,
        input::GHOSTTY_KEY_N,
        input::GHOSTTY_KEY_O,
        input::GHOSTTY_KEY_P,
        input::GHOSTTY_KEY_Q,
        input::GHOSTTY_KEY_R,
        input::GHOSTTY_KEY_S,
        input::GHOSTTY_KEY_T,
        input::GHOSTTY_KEY_U,
        input::GHOSTTY_KEY_V,
        input::GHOSTTY_KEY_W,
        input::GHOSTTY_KEY_X,
        input::GHOSTTY_KEY_Y,
        input::GHOSTTY_KEY_Z,
        input::GHOSTTY_KEY_MINUS,
        input::GHOSTTY_KEY_PERIOD,
        input::GHOSTTY_KEY_QUOTE,
        input::GHOSTTY_KEY_SEMICOLON,
        input::GHOSTTY_KEY_SLASH,
        input::GHOSTTY_KEY_ALT_LEFT,
        input::GHOSTTY_KEY_ALT_RIGHT,
        input::GHOSTTY_KEY_BACKSPACE,
        input::GHOSTTY_KEY_CAPS_LOCK,
        input::GHOSTTY_KEY_CONTEXT_MENU,
        input::GHOSTTY_KEY_CONTROL_LEFT,
        input::GHOSTTY_KEY_CONTROL_RIGHT,
        input::GHOSTTY_KEY_ENTER,
        input::GHOSTTY_KEY_META_LEFT,
        input::GHOSTTY_KEY_META_RIGHT,
        input::GHOSTTY_KEY_SHIFT_LEFT,
        input::GHOSTTY_KEY_SHIFT_RIGHT,
        input::GHOSTTY_KEY_SPACE,
        input::GHOSTTY_KEY_TAB,
        input::GHOSTTY_KEY_CONVERT,
        input::GHOSTTY_KEY_KANA_MODE,
        input::GHOSTTY_KEY_NON_CONVERT,
        input::GHOSTTY_KEY_DELETE,
        input::GHOSTTY_KEY_END,
        input::GHOSTTY_KEY_HELP,
        input::GHOSTTY_KEY_HOME,
        input::GHOSTTY_KEY_INSERT,
        input::GHOSTTY_KEY_PAGE_DOWN,
        input::GHOSTTY_KEY_PAGE_UP,
        input::GHOSTTY_KEY_ARROW_DOWN,
        input::GHOSTTY_KEY_ARROW_LEFT,
        input::GHOSTTY_KEY_ARROW_RIGHT,
        input::GHOSTTY_KEY_ARROW_UP,
        input::GHOSTTY_KEY_NUM_LOCK,
        input::GHOSTTY_KEY_NUMPAD_0,
        input::GHOSTTY_KEY_NUMPAD_1,
        input::GHOSTTY_KEY_NUMPAD_2,
        input::GHOSTTY_KEY_NUMPAD_3,
        input::GHOSTTY_KEY_NUMPAD_4,
        input::GHOSTTY_KEY_NUMPAD_5,
        input::GHOSTTY_KEY_NUMPAD_6,
        input::GHOSTTY_KEY_NUMPAD_7,
        input::GHOSTTY_KEY_NUMPAD_8,
        input::GHOSTTY_KEY_NUMPAD_9,
        input::GHOSTTY_KEY_NUMPAD_ADD,
        input::GHOSTTY_KEY_NUMPAD_BACKSPACE,
        input::GHOSTTY_KEY_NUMPAD_CLEAR,
        input::GHOSTTY_KEY_NUMPAD_CLEAR_ENTRY,
        input::GHOSTTY_KEY_NUMPAD_COMMA,
        input::GHOSTTY_KEY_NUMPAD_DECIMAL,
        input::GHOSTTY_KEY_NUMPAD_DIVIDE,
        input::GHOSTTY_KEY_NUMPAD_ENTER,
        input::GHOSTTY_KEY_NUMPAD_EQUAL,
        input::GHOSTTY_KEY_NUMPAD_MEMORY_ADD,
        input::GHOSTTY_KEY_NUMPAD_MEMORY_CLEAR,
        input::GHOSTTY_KEY_NUMPAD_MEMORY_RECALL,
        input::GHOSTTY_KEY_NUMPAD_MEMORY_STORE,
        input::GHOSTTY_KEY_NUMPAD_MEMORY_SUBTRACT,
        input::GHOSTTY_KEY_NUMPAD_MULTIPLY,
        input::GHOSTTY_KEY_NUMPAD_PAREN_LEFT,
        input::GHOSTTY_KEY_NUMPAD_PAREN_RIGHT,
        input::GHOSTTY_KEY_NUMPAD_SUBTRACT,
        input::GHOSTTY_KEY_NUMPAD_SEPARATOR,
        input::GHOSTTY_KEY_NUMPAD_UP,
        input::GHOSTTY_KEY_NUMPAD_DOWN,
        input::GHOSTTY_KEY_NUMPAD_RIGHT,
        input::GHOSTTY_KEY_NUMPAD_LEFT,
        input::GHOSTTY_KEY_NUMPAD_BEGIN,
        input::GHOSTTY_KEY_NUMPAD_HOME,
        input::GHOSTTY_KEY_NUMPAD_END,
        input::GHOSTTY_KEY_NUMPAD_INSERT,
        input::GHOSTTY_KEY_NUMPAD_DELETE,
        input::GHOSTTY_KEY_NUMPAD_PAGE_UP,
        input::GHOSTTY_KEY_NUMPAD_PAGE_DOWN,
        input::GHOSTTY_KEY_ESCAPE,
        input::GHOSTTY_KEY_F1,
        input::GHOSTTY_KEY_F2,
        input::GHOSTTY_KEY_F3,
        input::GHOSTTY_KEY_F4,
        input::GHOSTTY_KEY_F5,
        input::GHOSTTY_KEY_F6,
        input::GHOSTTY_KEY_F7,
        input::GHOSTTY_KEY_F8,
        input::GHOSTTY_KEY_F9,
        input::GHOSTTY_KEY_F10,
        input::GHOSTTY_KEY_F11,
        input::GHOSTTY_KEY_F12,
        input::GHOSTTY_KEY_F13,
        input::GHOSTTY_KEY_F14,
        input::GHOSTTY_KEY_F15,
        input::GHOSTTY_KEY_F16,
        input::GHOSTTY_KEY_F17,
        input::GHOSTTY_KEY_F18,
        input::GHOSTTY_KEY_F19,
        input::GHOSTTY_KEY_F20,
        input::GHOSTTY_KEY_F21,
        input::GHOSTTY_KEY_F22,
        input::GHOSTTY_KEY_F23,
        input::GHOSTTY_KEY_F24,
        input::GHOSTTY_KEY_F25,
        input::GHOSTTY_KEY_FN,
        input::GHOSTTY_KEY_FN_LOCK,
        input::GHOSTTY_KEY_PRINT_SCREEN,
        input::GHOSTTY_KEY_SCROLL_LOCK,
        input::GHOSTTY_KEY_PAUSE,
        input::GHOSTTY_KEY_BROWSER_BACK,
        input::GHOSTTY_KEY_BROWSER_FAVORITES,
        input::GHOSTTY_KEY_BROWSER_FORWARD,
        input::GHOSTTY_KEY_BROWSER_HOME,
        input::GHOSTTY_KEY_BROWSER_REFRESH,
        input::GHOSTTY_KEY_BROWSER_SEARCH,
        input::GHOSTTY_KEY_BROWSER_STOP,
        input::GHOSTTY_KEY_EJECT,
        input::GHOSTTY_KEY_LAUNCH_APP_1,
        input::GHOSTTY_KEY_LAUNCH_APP_2,
        input::GHOSTTY_KEY_LAUNCH_MAIL,
        input::GHOSTTY_KEY_MEDIA_PLAY_PAUSE,
        input::GHOSTTY_KEY_MEDIA_SELECT,
        input::GHOSTTY_KEY_MEDIA_STOP,
        input::GHOSTTY_KEY_MEDIA_TRACK_NEXT,
        input::GHOSTTY_KEY_MEDIA_TRACK_PREVIOUS,
        input::GHOSTTY_KEY_POWER,
        input::GHOSTTY_KEY_SLEEP,
        input::GHOSTTY_KEY_AUDIO_VOLUME_DOWN,
        input::GHOSTTY_KEY_AUDIO_VOLUME_MUTE,
        input::GHOSTTY_KEY_AUDIO_VOLUME_UP,
        input::GHOSTTY_KEY_WAKE_UP,
        input::GHOSTTY_KEY_COPY,
        input::GHOSTTY_KEY_CUT,
        input::GHOSTTY_KEY_PASTE,
        input::GHOSTTY_MOUSE_ACTION_PRESS,
        input::GHOSTTY_MOUSE_ACTION_RELEASE,
        input::GHOSTTY_MOUSE_ACTION_MOTION,
        input::GHOSTTY_MOUSE_BUTTON_UNKNOWN,
        input::GHOSTTY_MOUSE_BUTTON_LEFT,
        input::GHOSTTY_MOUSE_BUTTON_RIGHT,
        input::GHOSTTY_MOUSE_BUTTON_MIDDLE,
        input::GHOSTTY_MOUSE_BUTTON_FOUR,
        input::GHOSTTY_MOUSE_BUTTON_FIVE,
        input::GHOSTTY_MOUSE_BUTTON_SIX,
        input::GHOSTTY_MOUSE_BUTTON_SEVEN,
        input::GHOSTTY_MOUSE_BUTTON_EIGHT,
        input::GHOSTTY_MOUSE_BUTTON_NINE,
        input::GHOSTTY_MOUSE_BUTTON_TEN,
        input::GHOSTTY_MOUSE_BUTTON_ELEVEN,
        input::GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
        input::GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
        input::GHOSTTY_FOCUS_GAINED,
        input::GHOSTTY_FOCUS_LOST,
        input::GHOSTTY_MODE_FOCUS_EVENT,
        sys::GHOSTTY_SYS_OPT_DECODE_PNG,
        sys::GHOSTTY_SYS_OPT_USERDATA,
        screen::GHOSTTY_CELL_WIDE_NARROW,
        screen::GHOSTTY_CELL_WIDE_WIDE,
        screen::GHOSTTY_CELL_WIDE_SPACER_TAIL,
        screen::GHOSTTY_CELL_WIDE_SPACER_HEAD,
        screen::GHOSTTY_CELL_DATA_WIDE,
        build_info::GHOSTTY_BUILD_INFO_SIMD,
    ];

    /// Compare every declared id constant against the value the pinned
    /// headers assign to it: the two D2 data-path constants (row viewport Y,
    /// render-state colours) and the key/mouse ids that literal comparisons
    /// used to pin by hand.
    #[test]
    fn id_constants_match_the_pin() {
        let probe = probe_output();
        for (path, expected) in ID_CONSTANTS {
            let name = path.rsplit("::").next().expect("a name").trim();
            let actual = probe
                .get(&format!("VAL:{name}"))
                .copied()
                .unwrap_or_else(|| panic!("probe did not report {name}"));
            assert_eq!(actual, *expected, "{name} id");
        }
    }

    /// Compile a C probe against the pinned headers at test time, run it, and
    /// compare `sizeof`/`offsetof` for every `#[repr(C)]` type here. This is
    /// the stronger of the two options in the task: the layout is checked
    /// against the actual headers on every test run, not against a size
    /// recorded once.
    #[test]
    fn struct_layouts_match_the_pinned_headers() {
        let probe = probe_output();
        let sizes: [(&str, usize); 18] = [
            ("GhosttyAllocator", size_of::<GhosttyAllocator>()),
            (
                "GhosttyAllocatorVtable",
                size_of::<GhosttyAllocatorVtable>(),
            ),
            ("GhosttyColorRgb", size_of::<style::GhosttyColorRgb>()),
            (
                "GhosttyDeviceAttributes",
                size_of::<terminal::GhosttyDeviceAttributes>(),
            ),
            (
                "GhosttyDeviceAttributesPrimary",
                size_of::<terminal::GhosttyDeviceAttributesPrimary>(),
            ),
            (
                "GhosttyDeviceAttributesSecondary",
                size_of::<terminal::GhosttyDeviceAttributesSecondary>(),
            ),
            (
                "GhosttyDeviceAttributesTertiary",
                size_of::<terminal::GhosttyDeviceAttributesTertiary>(),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo",
                size_of::<kitty::GhosttyKittyGraphicsPlacementRenderInfo>(),
            ),
            (
                "GhosttyMouseEncoderSize",
                size_of::<input::GhosttyMouseEncoderSize>(),
            ),
            (
                "GhosttyMousePosition",
                size_of::<input::GhosttyMousePosition>(),
            ),
            (
                "GhosttyRenderStateColors",
                size_of::<render::GhosttyRenderStateColors>(),
            ),
            (
                "GhosttyRenderStateCursor",
                size_of::<render::GhosttyRenderStateCursor>(),
            ),
            (
                "GhosttySizeReportSize",
                size_of::<terminal::GhosttySizeReportSize>(),
            ),
            ("GhosttyStyle", size_of::<style::GhosttyStyle>()),
            ("GhosttyStyleColor", size_of::<style::GhosttyStyleColor>()),
            (
                "GhosttyStyleColorValue",
                size_of::<style::GhosttyStyleColorValue>(),
            ),
            ("GhosttySysImage", size_of::<sys::GhosttySysImage>()),
            (
                "GhosttyTerminalModeConfig",
                size_of::<terminal::GhosttyTerminalModeConfig>(),
            ),
        ];
        for (name, expected) in sizes {
            let actual = probe[&format!("SIZE:{name}")];
            assert_eq!(actual, expected as i64, "{name} size");
        }

        let offsets: [(&str, usize); 85] = [
            ("GhosttyAllocator.ctx", offset_of!(GhosttyAllocator, ctx)),
            (
                "GhosttyAllocator.vtable",
                offset_of!(GhosttyAllocator, vtable),
            ),
            (
                "GhosttyAllocatorVtable.alloc",
                offset_of!(GhosttyAllocatorVtable, alloc),
            ),
            (
                "GhosttyAllocatorVtable.resize",
                offset_of!(GhosttyAllocatorVtable, resize),
            ),
            (
                "GhosttyAllocatorVtable.remap",
                offset_of!(GhosttyAllocatorVtable, remap),
            ),
            (
                "GhosttyAllocatorVtable.free",
                offset_of!(GhosttyAllocatorVtable, free),
            ),
            ("GhosttyColorRgb.r", offset_of!(style::GhosttyColorRgb, r)),
            ("GhosttyColorRgb.g", offset_of!(style::GhosttyColorRgb, g)),
            ("GhosttyColorRgb.b", offset_of!(style::GhosttyColorRgb, b)),
            (
                "GhosttyDeviceAttributes.primary",
                offset_of!(terminal::GhosttyDeviceAttributes, primary),
            ),
            (
                "GhosttyDeviceAttributes.secondary",
                offset_of!(terminal::GhosttyDeviceAttributes, secondary),
            ),
            (
                "GhosttyDeviceAttributes.tertiary",
                offset_of!(terminal::GhosttyDeviceAttributes, tertiary),
            ),
            (
                "GhosttyDeviceAttributesPrimary.conformance_level",
                offset_of!(terminal::GhosttyDeviceAttributesPrimary, conformance_level),
            ),
            (
                "GhosttyDeviceAttributesPrimary.features",
                offset_of!(terminal::GhosttyDeviceAttributesPrimary, features),
            ),
            (
                "GhosttyDeviceAttributesPrimary.num_features",
                offset_of!(terminal::GhosttyDeviceAttributesPrimary, num_features),
            ),
            (
                "GhosttyDeviceAttributesSecondary.device_type",
                offset_of!(terminal::GhosttyDeviceAttributesSecondary, device_type),
            ),
            (
                "GhosttyDeviceAttributesSecondary.firmware_version",
                offset_of!(terminal::GhosttyDeviceAttributesSecondary, firmware_version),
            ),
            (
                "GhosttyDeviceAttributesSecondary.rom_cartridge",
                offset_of!(terminal::GhosttyDeviceAttributesSecondary, rom_cartridge),
            ),
            (
                "GhosttyDeviceAttributesTertiary.unit_id",
                offset_of!(terminal::GhosttyDeviceAttributesTertiary, unit_id),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.grid_cols",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, grid_cols),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.grid_rows",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, grid_rows),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.pixel_height",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, pixel_height),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.pixel_width",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, pixel_width),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.size",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, size),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.source_height",
                offset_of!(
                    kitty::GhosttyKittyGraphicsPlacementRenderInfo,
                    source_height
                ),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.source_width",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, source_width),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.source_x",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, source_x),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.source_y",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, source_y),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.viewport_col",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, viewport_col),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.viewport_row",
                offset_of!(kitty::GhosttyKittyGraphicsPlacementRenderInfo, viewport_row),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo.viewport_visible",
                offset_of!(
                    kitty::GhosttyKittyGraphicsPlacementRenderInfo,
                    viewport_visible
                ),
            ),
            (
                "GhosttyMouseEncoderSize.cell_height",
                offset_of!(input::GhosttyMouseEncoderSize, cell_height),
            ),
            (
                "GhosttyMouseEncoderSize.cell_width",
                offset_of!(input::GhosttyMouseEncoderSize, cell_width),
            ),
            (
                "GhosttyMouseEncoderSize.padding_bottom",
                offset_of!(input::GhosttyMouseEncoderSize, padding_bottom),
            ),
            (
                "GhosttyMouseEncoderSize.padding_left",
                offset_of!(input::GhosttyMouseEncoderSize, padding_left),
            ),
            (
                "GhosttyMouseEncoderSize.padding_right",
                offset_of!(input::GhosttyMouseEncoderSize, padding_right),
            ),
            (
                "GhosttyMouseEncoderSize.padding_top",
                offset_of!(input::GhosttyMouseEncoderSize, padding_top),
            ),
            (
                "GhosttyMouseEncoderSize.screen_height",
                offset_of!(input::GhosttyMouseEncoderSize, screen_height),
            ),
            (
                "GhosttyMouseEncoderSize.screen_width",
                offset_of!(input::GhosttyMouseEncoderSize, screen_width),
            ),
            (
                "GhosttyMouseEncoderSize.size",
                offset_of!(input::GhosttyMouseEncoderSize, size),
            ),
            (
                "GhosttyMousePosition.x",
                offset_of!(input::GhosttyMousePosition, x),
            ),
            (
                "GhosttyMousePosition.y",
                offset_of!(input::GhosttyMousePosition, y),
            ),
            (
                "GhosttyRenderStateColors.background",
                offset_of!(render::GhosttyRenderStateColors, background),
            ),
            (
                "GhosttyRenderStateColors.cursor",
                offset_of!(render::GhosttyRenderStateColors, cursor),
            ),
            (
                "GhosttyRenderStateColors.cursor_has_value",
                offset_of!(render::GhosttyRenderStateColors, cursor_has_value),
            ),
            (
                "GhosttyRenderStateColors.foreground",
                offset_of!(render::GhosttyRenderStateColors, foreground),
            ),
            (
                "GhosttyRenderStateColors.palette",
                offset_of!(render::GhosttyRenderStateColors, palette),
            ),
            (
                "GhosttyRenderStateColors.size",
                offset_of!(render::GhosttyRenderStateColors, size),
            ),
            (
                "GhosttyRenderStateCursor.blinking",
                offset_of!(render::GhosttyRenderStateCursor, blinking),
            ),
            (
                "GhosttyRenderStateCursor.password_input",
                offset_of!(render::GhosttyRenderStateCursor, password_input),
            ),
            (
                "GhosttyRenderStateCursor.size",
                offset_of!(render::GhosttyRenderStateCursor, size),
            ),
            (
                "GhosttyRenderStateCursor.viewport_has_value",
                offset_of!(render::GhosttyRenderStateCursor, viewport_has_value),
            ),
            (
                "GhosttyRenderStateCursor.viewport_x",
                offset_of!(render::GhosttyRenderStateCursor, viewport_x),
            ),
            (
                "GhosttyRenderStateCursor.viewport_y",
                offset_of!(render::GhosttyRenderStateCursor, viewport_y),
            ),
            (
                "GhosttyRenderStateCursor.visible",
                offset_of!(render::GhosttyRenderStateCursor, visible),
            ),
            (
                "GhosttyRenderStateCursor.visual_style",
                offset_of!(render::GhosttyRenderStateCursor, visual_style),
            ),
            (
                "GhosttyRenderStateCursor.wide_tail",
                offset_of!(render::GhosttyRenderStateCursor, wide_tail),
            ),
            (
                "GhosttySizeReportSize.cell_height",
                offset_of!(terminal::GhosttySizeReportSize, cell_height),
            ),
            (
                "GhosttySizeReportSize.cell_width",
                offset_of!(terminal::GhosttySizeReportSize, cell_width),
            ),
            (
                "GhosttySizeReportSize.columns",
                offset_of!(terminal::GhosttySizeReportSize, columns),
            ),
            (
                "GhosttySizeReportSize.rows",
                offset_of!(terminal::GhosttySizeReportSize, rows),
            ),
            (
                "GhosttyStyle.bg_color",
                offset_of!(style::GhosttyStyle, bg_color),
            ),
            ("GhosttyStyle.blink", offset_of!(style::GhosttyStyle, blink)),
            ("GhosttyStyle.bold", offset_of!(style::GhosttyStyle, bold)),
            ("GhosttyStyle.faint", offset_of!(style::GhosttyStyle, faint)),
            (
                "GhosttyStyle.fg_color",
                offset_of!(style::GhosttyStyle, fg_color),
            ),
            (
                "GhosttyStyle.inverse",
                offset_of!(style::GhosttyStyle, inverse),
            ),
            (
                "GhosttyStyle.invisible",
                offset_of!(style::GhosttyStyle, invisible),
            ),
            (
                "GhosttyStyle.italic",
                offset_of!(style::GhosttyStyle, italic),
            ),
            (
                "GhosttyStyle.overline",
                offset_of!(style::GhosttyStyle, overline),
            ),
            ("GhosttyStyle.size", offset_of!(style::GhosttyStyle, size)),
            (
                "GhosttyStyle.strikethrough",
                offset_of!(style::GhosttyStyle, strikethrough),
            ),
            (
                "GhosttyStyle.underline",
                offset_of!(style::GhosttyStyle, underline),
            ),
            (
                "GhosttyStyle.underline_color",
                offset_of!(style::GhosttyStyle, underline_color),
            ),
            (
                "GhosttyStyleColor.tag",
                offset_of!(style::GhosttyStyleColor, tag),
            ),
            (
                "GhosttyStyleColor.value",
                offset_of!(style::GhosttyStyleColor, value),
            ),
            (
                "GhosttyStyleColorValue._padding",
                offset_of!(style::GhosttyStyleColorValue, _padding),
            ),
            (
                "GhosttyStyleColorValue.palette",
                offset_of!(style::GhosttyStyleColorValue, palette),
            ),
            (
                "GhosttyStyleColorValue.rgb",
                offset_of!(style::GhosttyStyleColorValue, rgb),
            ),
            (
                "GhosttySysImage.data",
                offset_of!(sys::GhosttySysImage, data),
            ),
            (
                "GhosttySysImage.data_len",
                offset_of!(sys::GhosttySysImage, data_len),
            ),
            (
                "GhosttySysImage.height",
                offset_of!(sys::GhosttySysImage, height),
            ),
            (
                "GhosttySysImage.width",
                offset_of!(sys::GhosttySysImage, width),
            ),
            (
                "GhosttyTerminalModeConfig.mode",
                offset_of!(terminal::GhosttyTerminalModeConfig, mode),
            ),
            (
                "GhosttyTerminalModeConfig.value",
                offset_of!(terminal::GhosttyTerminalModeConfig, value),
            ),
        ];
        for (name, expected) in offsets {
            let actual = probe[&format!("OFF:{name}")];
            assert_eq!(actual, expected as i64, "{name} offset");
        }
    }

    /// Compile and run the generated C probe against the pinned headers once
    /// and return its `SIZE:`/`OFF:`/`VAL:` lines as a map. The result is
    /// cached so the tests can share one compilation and never race on the
    /// probe binary.
    fn probe_output() -> &'static HashMap<String, i64> {
        static PROBE: std::sync::OnceLock<HashMap<String, i64>> = std::sync::OnceLock::new();
        PROBE.get_or_init(build_probe_output)
    }

    fn build_probe_output() -> HashMap<String, i64> {
        let out_dir = PathBuf::from(env!("OUT_DIR")).join("ghostty-layout-probe");
        std::fs::create_dir_all(&out_dir).expect("create probe dir");
        let source = out_dir.join("layout_probe.c");
        let mut id_lines = String::new();
        for (path, _) in ID_CONSTANTS {
            let name = path.rsplit("::").next().expect("a name").trim();
            id_lines.push_str(&format!("  V({name});\n"));
        }
        let probe_c = LAYOUT_PROBE_C.replace("  //ID_CONSTANTS\n", &id_lines);
        std::fs::write(&source, probe_c).expect("write probe source");
        let binary = out_dir.join("layout_probe");

        let include = PathBuf::from(env!("OUT_DIR")).join("ghostty/include");
        let status = Command::new("cc")
            .arg("-DGHOSTTY_STATIC")
            .arg("-I")
            .arg(&include)
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .status()
            .expect("run the C compiler for the layout probe");
        assert!(status.success(), "cc failed to build the layout probe");

        let output = Command::new(&binary)
            .output()
            .expect("run the layout probe");
        assert!(output.status.success(), "the layout probe exited non-zero");
        String::from_utf8(output.stdout)
            .expect("probe output is UTF-8")
            .lines()
            .map(|line| {
                let (key, value) = line.split_once(' ').expect("probe line has a value");
                (
                    key.to_owned(),
                    value.parse::<i64>().expect("probe value is a number"),
                )
            })
            .collect()
    }

    /// `sizeof`/`offsetof` for every `#[repr(C)]` type in this module,
    /// generated from the pinned headers' field order.
    const LAYOUT_PROBE_C: &str = "\
#include <stddef.h>
#include <stdio.h>
#define S(t) printf(\"SIZE:%s %zu\\n\", #t, sizeof(t))
#define F(t, f) printf(\"OFF:%s.%s %zu\\n\", #t, #f, offsetof(t, f))
#define V(x) printf(\"VAL:%s %ld\\n\", #x, (long)(x))
#include <ghostty/vt.h>
int main(void) {
  S(GhosttyColorRgb);
  F(GhosttyColorRgb, r); F(GhosttyColorRgb, g); F(GhosttyColorRgb, b);
  S(GhosttyAllocatorVtable);
  F(GhosttyAllocatorVtable, alloc); F(GhosttyAllocatorVtable, resize);
  F(GhosttyAllocatorVtable, remap); F(GhosttyAllocatorVtable, free);
  S(GhosttyAllocator);
  F(GhosttyAllocator, ctx); F(GhosttyAllocator, vtable);
  S(GhosttyStyleColorValue);
  F(GhosttyStyleColorValue, palette); F(GhosttyStyleColorValue, rgb);
  F(GhosttyStyleColorValue, _padding);
  S(GhosttyStyleColor);
  F(GhosttyStyleColor, tag); F(GhosttyStyleColor, value);
  S(GhosttyStyle);
  F(GhosttyStyle, size); F(GhosttyStyle, fg_color); F(GhosttyStyle, bg_color);
  F(GhosttyStyle, underline_color); F(GhosttyStyle, bold);
  F(GhosttyStyle, italic); F(GhosttyStyle, faint); F(GhosttyStyle, blink);
  F(GhosttyStyle, inverse); F(GhosttyStyle, invisible);
  F(GhosttyStyle, strikethrough); F(GhosttyStyle, overline);
  F(GhosttyStyle, underline);
  S(GhosttySizeReportSize);
  F(GhosttySizeReportSize, rows); F(GhosttySizeReportSize, columns);
  F(GhosttySizeReportSize, cell_width); F(GhosttySizeReportSize, cell_height);
  S(GhosttyDeviceAttributesPrimary);
  F(GhosttyDeviceAttributesPrimary, conformance_level);
  F(GhosttyDeviceAttributesPrimary, features);
  F(GhosttyDeviceAttributesPrimary, num_features);
  S(GhosttyDeviceAttributesSecondary);
  F(GhosttyDeviceAttributesSecondary, device_type);
  F(GhosttyDeviceAttributesSecondary, firmware_version);
  F(GhosttyDeviceAttributesSecondary, rom_cartridge);
  S(GhosttyDeviceAttributesTertiary);
  F(GhosttyDeviceAttributesTertiary, unit_id);
  S(GhosttyDeviceAttributes);
  F(GhosttyDeviceAttributes, primary); F(GhosttyDeviceAttributes, secondary);
  F(GhosttyDeviceAttributes, tertiary);
  S(GhosttySysImage);
  F(GhosttySysImage, width); F(GhosttySysImage, height);
  F(GhosttySysImage, data); F(GhosttySysImage, data_len);
  S(GhosttyMousePosition);
  F(GhosttyMousePosition, x); F(GhosttyMousePosition, y);
  S(GhosttyMouseEncoderSize);
  F(GhosttyMouseEncoderSize, size); F(GhosttyMouseEncoderSize, screen_width);
  F(GhosttyMouseEncoderSize, screen_height);
  F(GhosttyMouseEncoderSize, cell_width);
  F(GhosttyMouseEncoderSize, cell_height);
  F(GhosttyMouseEncoderSize, padding_top);
  F(GhosttyMouseEncoderSize, padding_bottom);
  F(GhosttyMouseEncoderSize, padding_right);
  F(GhosttyMouseEncoderSize, padding_left);
  S(GhosttyRenderStateCursor);
  F(GhosttyRenderStateCursor, size);
  F(GhosttyRenderStateCursor, viewport_has_value);
  F(GhosttyRenderStateCursor, viewport_x);
  F(GhosttyRenderStateCursor, viewport_y);
  F(GhosttyRenderStateCursor, wide_tail);
  F(GhosttyRenderStateCursor, visible);
  F(GhosttyRenderStateCursor, blinking);
  F(GhosttyRenderStateCursor, password_input);
  F(GhosttyRenderStateCursor, visual_style);
  S(GhosttyRenderStateColors);
  F(GhosttyRenderStateColors, size); F(GhosttyRenderStateColors, background);
  F(GhosttyRenderStateColors, foreground); F(GhosttyRenderStateColors, cursor);
  F(GhosttyRenderStateColors, cursor_has_value);
  F(GhosttyRenderStateColors, palette);
  S(GhosttyKittyGraphicsPlacementRenderInfo);
  F(GhosttyKittyGraphicsPlacementRenderInfo, size);
  F(GhosttyKittyGraphicsPlacementRenderInfo, pixel_width);
  F(GhosttyKittyGraphicsPlacementRenderInfo, pixel_height);
  F(GhosttyKittyGraphicsPlacementRenderInfo, grid_cols);
  F(GhosttyKittyGraphicsPlacementRenderInfo, grid_rows);
  F(GhosttyKittyGraphicsPlacementRenderInfo, viewport_col);
  F(GhosttyKittyGraphicsPlacementRenderInfo, viewport_row);
  F(GhosttyKittyGraphicsPlacementRenderInfo, viewport_visible);
  F(GhosttyKittyGraphicsPlacementRenderInfo, source_x);
  F(GhosttyKittyGraphicsPlacementRenderInfo, source_y);
  F(GhosttyKittyGraphicsPlacementRenderInfo, source_width);
  F(GhosttyKittyGraphicsPlacementRenderInfo, source_height);
  S(GhosttyTerminalModeConfig);
  F(GhosttyTerminalModeConfig, mode); F(GhosttyTerminalModeConfig, value);
  //ID_CONSTANTS
  return 0;
}
";
}
