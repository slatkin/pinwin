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
        input::ghostty_key_encoder_new as *const (),
        input::ghostty_key_encoder_setopt_from_terminal as *const (),
        input::ghostty_key_event_new as *const (),
        input::ghostty_key_event_set_action as *const (),
        input::ghostty_key_event_set_consumed_mods as *const (),
        input::ghostty_key_event_set_key as *const (),
        input::ghostty_key_event_set_mods as *const (),
        input::ghostty_key_event_set_unshifted_codepoint as *const (),
        input::ghostty_key_event_set_utf8 as *const (),
        input::ghostty_mouse_encoder_encode as *const (),
        input::ghostty_mouse_encoder_new as *const (),
        input::ghostty_mouse_encoder_setopt as *const (),
        input::ghostty_mouse_encoder_setopt_from_terminal as *const (),
        input::ghostty_mouse_event_clear_button as *const (),
        input::ghostty_mouse_event_new as *const (),
        input::ghostty_mouse_event_set_action as *const (),
        input::ghostty_mouse_event_set_button as *const (),
        input::ghostty_mouse_event_set_mods as *const (),
        input::ghostty_mouse_event_set_position as *const (),
        kitty::ghostty_kitty_graphics_get as *const (),
        kitty::ghostty_kitty_graphics_image as *const (),
        kitty::ghostty_kitty_graphics_image_get_multi as *const (),
        kitty::ghostty_kitty_graphics_placement_get as *const (),
        kitty::ghostty_kitty_graphics_placement_iterator_new as *const (),
        kitty::ghostty_kitty_graphics_placement_next as *const (),
        kitty::ghostty_kitty_graphics_placement_render_info as *const (),
        render::ghostty_render_state_clean as *const (),
        render::ghostty_render_state_get as *const (),
        render::ghostty_render_state_new as *const (),
        render::ghostty_render_state_row_cells_get as *const (),
        render::ghostty_render_state_row_cells_new as *const (),
        render::ghostty_render_state_row_cells_next as *const (),
        render::ghostty_render_state_row_get as *const (),
        render::ghostty_render_state_row_iterator_new as *const (),
        render::ghostty_render_state_row_iterator_next as *const (),
        render::ghostty_render_state_update as *const (),
        screen::ghostty_cell_get as *const (),
        sys::ghostty_sys_set as *const (),
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

    /// The two data paths the rejected `libghostty-vt` crate could not reach
    /// against the pin (D2): the row's viewport Y position and the combined
    /// render-state colours.
    #[test]
    fn d2_gap_constants_match_the_pin() {
        assert_eq!(render::GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y, 6);
        assert_eq!(render::GHOSTTY_RENDER_STATE_DATA_COLORS, 19);
    }

    /// Spot-check key ids against the pin's `key/event.h` ordering.
    #[test]
    fn key_ids_match_the_pin() {
        assert_eq!(input::GHOSTTY_KEY_UNIDENTIFIED, 0);
        assert_eq!(input::GHOSTTY_KEY_A, 20);
        assert_eq!(input::GHOSTTY_KEY_ESCAPE, 120);
        assert_eq!(input::GHOSTTY_KEY_F25, 145);
        assert_eq!(input::GHOSTTY_KEY_COPY, 173);
        assert_eq!(input::GHOSTTY_KEY_CUT, 174);
        assert_eq!(input::GHOSTTY_KEY_PASTE, 175);
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
            assert_eq!(actual, expected, "{name} size");
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
            assert_eq!(actual, expected, "{name} offset");
        }
    }

    /// Compile and run the generated C probe against the pinned headers and
    /// return its `SIZE:`/`OFF:` lines as a map.
    fn probe_output() -> HashMap<String, usize> {
        let out_dir = PathBuf::from(env!("OUT_DIR")).join("ghostty-layout-probe");
        std::fs::create_dir_all(&out_dir).expect("create probe dir");
        let source = out_dir.join("layout_probe.c");
        std::fs::write(&source, LAYOUT_PROBE_C).expect("write probe source");
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
                    value.parse::<usize>().expect("probe value is a number"),
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
  return 0;
}
";
}
