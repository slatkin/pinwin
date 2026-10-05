//! Process-global system hooks (`ghostty/vt/sys.h`).

use std::os::raw::{c_int, c_void};

use super::GhosttyAllocator;

/// A decoded PNG image handed back to the library (`GhosttySysImage`,
/// sys.h). `data` is `width * height * 4` RGBA bytes allocated with
/// `ghostty_alloc`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttySysImage {
    pub width: u32,
    pub height: u32,
    pub data: *mut u8,
    pub data_len: usize,
}

/// A sys option id (`GhosttySysOption`, sys.h).
pub type GhosttySysOption = c_int;

pub const GHOSTTY_SYS_OPT_USERDATA: GhosttySysOption = 0;
pub const GHOSTTY_SYS_OPT_DECODE_PNG: GhosttySysOption = 1;

/// The `GHOSTTY_SYS_OPT_DECODE_PNG` callback (sys.h). `out.data` must be
/// allocated with `ghostty_alloc` using the passed allocator.
pub type GhosttySysDecodePngFn = Option<
    unsafe extern "C" fn(
        userdata: *mut c_void,
        allocator: *const GhosttyAllocator,
        data: *const u8,
        data_len: usize,
        out: *mut GhosttySysImage,
    ) -> bool,
>;

unsafe extern "C" {
    /// Install a process-global hook; `value`'s type depends on `option`
    /// (sys.h).
    pub fn ghostty_sys_set(option: GhosttySysOption, value: *const c_void) -> super::GhosttyResult;
}
