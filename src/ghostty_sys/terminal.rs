//! Terminal state and its effect callbacks (`ghostty/vt/terminal.h`).

use std::os::raw::{c_int, c_void};

use super::{GhosttyAllocator, GhosttyMode, GhosttyResult};

/// Opaque handle to a terminal instance (`GhosttyTerminal`, terminal.h).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminal(pub *mut c_void);

/// A terminal option id (`GhosttyTerminalOption`, terminal.h).
pub type GhosttyTerminalOption = c_int;

pub const GHOSTTY_TERMINAL_OPT_USERDATA: GhosttyTerminalOption = 0;
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: GhosttyTerminalOption = 1;
pub const GHOSTTY_TERMINAL_OPT_SIZE: GhosttyTerminalOption = 6;
pub const GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES: GhosttyTerminalOption = 8;
pub const GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT: GhosttyTerminalOption = 15;
pub const GHOSTTY_TERMINAL_OPT_MODE: GhosttyTerminalOption = 34;

/// A terminal query id (`GhosttyTerminalData`, terminal.h).
pub type GhosttyTerminalData = c_int;

pub const GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS: GhosttyTerminalData = 8;
pub const GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS: GhosttyTerminalData = 30;
pub const GHOSTTY_TERMINAL_DATA_MODE: GhosttyTerminalData = 37;

/// A mode and its boolean value (`GhosttyTerminalModeConfig`, terminal.h).
/// Initialize `mode`, then pass the struct to `ghostty_terminal_get` with
/// [`GHOSTTY_TERMINAL_DATA_MODE`] to read `value`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalModeConfig {
    pub mode: GhosttyMode,
    pub value: bool,
}

/// A terminal size report (`GhosttySizeReportSize`, size_report.h). The
/// `GHOSTTY_TERMINAL_OPT_SIZE` callback fills one.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttySizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

/// Primary device attributes (`GhosttyDeviceAttributesPrimary`, device.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyDeviceAttributesPrimary {
    pub conformance_level: u16,
    pub features: [u16; 64],
    pub num_features: usize,
}

/// Secondary device attributes (`GhosttyDeviceAttributesSecondary`, device.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyDeviceAttributesSecondary {
    pub device_type: u16,
    pub firmware_version: u16,
    pub rom_cartridge: u16,
}

/// Tertiary device attributes (`GhosttyDeviceAttributesTertiary`, device.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyDeviceAttributesTertiary {
    pub unit_id: u32,
}

/// Device attributes (`GhosttyDeviceAttributes`, device.h). The
/// `GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES` callback fills one.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyDeviceAttributes {
    pub primary: GhosttyDeviceAttributesPrimary,
    pub secondary: GhosttyDeviceAttributesSecondary,
    pub tertiary: GhosttyDeviceAttributesTertiary,
}

/// The `GHOSTTY_TERMINAL_OPT_WRITE_PTY` callback: response bytes for the pty
/// (terminal.h). The data is only valid for the call.
pub type GhosttyTerminalWritePtyFn = Option<
    unsafe extern "C" fn(
        terminal: GhosttyTerminal,
        userdata: *mut c_void,
        data: *const u8,
        len: usize,
    ),
>;

/// The `GHOSTTY_TERMINAL_OPT_SIZE` callback: answer `CSI 14/16/18 t`. Return
/// false to ignore the query (terminal.h).
pub type GhosttyTerminalSizeFn = Option<
    unsafe extern "C" fn(
        terminal: GhosttyTerminal,
        userdata: *mut c_void,
        out_size: *mut GhosttySizeReportSize,
    ) -> bool,
>;

/// The `GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES` callback: answer `CSI c`,
/// `CSI > c` or `CSI = c`. Return false to ignore the query (terminal.h).
pub type GhosttyTerminalDeviceAttributesFn = Option<
    unsafe extern "C" fn(
        terminal: GhosttyTerminal,
        userdata: *mut c_void,
        out_attrs: *mut GhosttyDeviceAttributes,
    ) -> bool,
>;

unsafe extern "C" {
    /// Create a terminal (`cols` x `rows`) with `allocator`, or the default
    /// allocator when it is NULL (terminal.h).
    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;

    /// Resize the terminal and record the cell pixel size (terminal.h).
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;

    /// Configure a terminal option; `value`'s type depends on `option`
    /// (terminal.h).
    pub fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: GhosttyTerminalOption,
        value: *const c_void,
    ) -> GhosttyResult;

    /// Feed bytes to the terminal parser (terminal.h).
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);

    /// Query terminal state; `out`'s type depends on `data` (terminal.h).
    pub fn ghostty_terminal_get(
        terminal: GhosttyTerminal,
        data: GhosttyTerminalData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Free a terminal created with `ghostty_terminal_new` (terminal.h).
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
}
