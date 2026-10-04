//! libghostty effect callbacks (port-to-rust D3): the `extern "C"`
//! trampolines libghostty calls back into, moved out of `mod.rs` so it stays
//! under the module size cap. The creation path ([`init_ghostty`]) installs
//! the process-global sys hooks and the per-terminal effect callbacks; every
//! trampoline catches unwinds (D5), latching a poisoned flag so later calls
//! become no-ops instead of unwinding into C.

use std::os::raw::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::slice;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{CallbackContext, Handles, KITTY_STORAGE_LIMIT};
use crate::ghostty_sys::sys::{
    GHOSTTY_SYS_OPT_DECODE_PNG, GHOSTTY_SYS_OPT_USERDATA, GhosttySysImage, ghostty_sys_set,
};
use crate::ghostty_sys::terminal::{
    GhosttyDeviceAttributes, GhosttySizeReportSize, GhosttyTerminal, ghostty_terminal_set,
};
use crate::ghostty_sys::{GHOSTTY_SUCCESS, GhosttyAllocator, ghostty_alloc};

/// Install the process-global libghostty hooks before the terminal exists.
/// libghostty wants these set once at startup; libghostty's own userdata slot
/// is process-global, so when several terminals are built in one process the
/// last one wins (the real application has exactly one).
fn install_sys_hooks(userdata: *mut c_void) {
    static SYS_HOOK_LOCK: Mutex<()> = Mutex::new(());
    let _guard = SYS_HOOK_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // SAFETY: `ghostty_sys_set` takes the option's value directly (sys.zig
    // casts it to the option's `InType`): the userdata pointer itself and the
    // decode function pointer.
    unsafe {
        let _ = ghostty_sys_set(GHOSTTY_SYS_OPT_USERDATA, userdata);
        let _ = ghostty_sys_set(GHOSTTY_SYS_OPT_DECODE_PNG, decode_png as *const c_void);
    }
}

/// The real creation path: install the global hooks, create the handles,
/// attach the effect callbacks and the kitty storage limit.
pub(super) fn init_ghostty(userdata: *mut c_void, cols: u16, rows: u16) -> Result<Handles, ()> {
    install_sys_hooks(userdata);
    let handles = Handles::create(cols, rows)?;
    let terminal = handles.terminal.expect("create built a terminal");

    // SAFETY: the terminal is live. `ghostty_terminal_set` takes the userdata
    // pointer directly and each callback as a function pointer (terminal.zig
    // casts the value to the option's `InType`); the storage limit is the one
    // option that takes a pointer to a value.
    unsafe {
        if ghostty_terminal_set(
            terminal,
            crate::ghostty_sys::terminal::GHOSTTY_TERMINAL_OPT_USERDATA,
            userdata,
        ) != GHOSTTY_SUCCESS
        {
            return Err(());
        }

        if ghostty_terminal_set(
            terminal,
            crate::ghostty_sys::terminal::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
            write_pty as *const c_void,
        ) != GHOSTTY_SUCCESS
        {
            return Err(());
        }

        if ghostty_terminal_set(
            terminal,
            crate::ghostty_sys::terminal::GHOSTTY_TERMINAL_OPT_SIZE,
            size_report as *const c_void,
        ) != GHOSTTY_SUCCESS
        {
            return Err(());
        }

        if ghostty_terminal_set(
            terminal,
            crate::ghostty_sys::terminal::GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
            device_attributes as *const c_void,
        ) != GHOSTTY_SUCCESS
        {
            return Err(());
        }

        let storage_limit: u64 = KITTY_STORAGE_LIMIT;
        if ghostty_terminal_set(
            terminal,
            crate::ghostty_sys::terminal::GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
            (&storage_limit as *const u64).cast(),
        ) != GHOSTTY_SUCCESS
        {
            return Err(());
        }
    }

    Ok(handles)
}

/// The minimum D5 guard: run `body`, latching `poisoned` and returning `None`
/// when it panics. Row 4.2 grows this into the shared helper applied to every
/// other boundary.
fn guarded<T>(poisoned: &AtomicBool, body: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => Some(value),
        Err(_) => {
            poisoned.store(true, Ordering::Relaxed);
            None
        }
    }
}

/// `GHOSTTY_TERMINAL_OPT_WRITE_PTY`: hand response bytes to the sink (D5 guard).
///
/// # Safety
/// Called by libghostty with a `userdata` previously set to a live
/// `CallbackContext` and a `data`/`len` pair that is valid for the call.
unsafe extern "C" fn write_pty(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() {
        return;
    }
    // SAFETY: the caller guarantees `userdata` points at the live context.
    let ctx = unsafe { &mut *(userdata as *mut CallbackContext) };
    if ctx.poisoned.load(Ordering::Relaxed) {
        return;
    }
    let poisoned = ctx.poisoned.clone();
    let _ = guarded(&poisoned, || {
        // SAFETY: the caller guarantees `data`/`len` describe a readable
        // region; an empty write never touches the pointer.
        let bytes = if data.is_null() || len == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(data, len) }
        };
        ctx.sink.write_pty(bytes);
    });
}

/// `GHOSTTY_TERMINAL_OPT_SIZE`: answer `CSI 14/16/18 t` with the live grid
/// (D5 guard).
///
/// # Safety
/// Same contract as [`write_pty`]; `out` must be writable.
unsafe extern "C" fn size_report(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut GhosttySizeReportSize,
) -> bool {
    if userdata.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: the caller guarantees `userdata` points at the live context.
    let ctx = unsafe { &*(userdata as *const CallbackContext) };
    if ctx.poisoned.load(Ordering::Relaxed) {
        return false;
    }
    let poisoned = ctx.poisoned.clone();
    guarded(&poisoned, || {
        // SAFETY: the caller guarantees `out` is writable.
        unsafe {
            (*out).rows = ctx.rows;
            (*out).columns = ctx.cols;
            (*out).cell_width = ctx.cell_w;
            (*out).cell_height = ctx.cell_h;
        }
        true
    })
    .unwrap_or(false)
}

/// `GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES`: reply with Ghostty's own DA1/DA2
/// values (D5 guard).
///
/// # Safety
/// Same contract as [`write_pty`]; `out` must be writable.
unsafe extern "C" fn device_attributes(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut GhosttyDeviceAttributes,
) -> bool {
    if userdata.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: the caller guarantees `userdata` points at the live context.
    let ctx = unsafe { &*(userdata as *const CallbackContext) };
    if ctx.poisoned.load(Ordering::Relaxed) {
        return false;
    }
    let poisoned = ctx.poisoned.clone();
    guarded(&poisoned, || {
        // SAFETY: the caller guarantees `out` is writable.
        unsafe {
            (*out).primary.conformance_level = 62; // level 2, like Ghostty
            (*out).primary.features[0] = 22; // ansi color
            (*out).primary.features[1] = 52; // clipboard
            (*out).primary.num_features = 2;
            (*out).secondary.device_type = 1;
            (*out).secondary.firmware_version = 10;
            (*out).secondary.rom_cartridge = 0;
            (*out).tertiary.unit_id = 0;
        }
        true
    })
    .unwrap_or(false)
}

/// `GHOSTTY_SYS_OPT_DECODE_PNG`: decode a kitty-graphics PNG into a
/// ghostty-allocated RGBA buffer (D5 guard).
///
/// # Safety
/// Called by libghostty with a `userdata` previously set to a live
/// `CallbackContext`, a valid `allocator`, a readable `data`/`data_len` pair
/// and a writable `out`.
unsafe extern "C" fn decode_png(
    userdata: *mut c_void,
    allocator: *const GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut GhosttySysImage,
) -> bool {
    if userdata.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: the caller guarantees `userdata` points at the live context.
    let ctx = unsafe { &mut *(userdata as *mut CallbackContext) };
    if ctx.poisoned.load(Ordering::Relaxed) {
        return false;
    }
    let poisoned = ctx.poisoned.clone();
    guarded(&poisoned, || {
        // SAFETY: the caller guarantees `data`/`data_len` describe a readable
        // region; an empty decode never touches the pointer.
        let bytes = if data.is_null() || data_len == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(data, data_len) }
        };
        let Some(image) = ctx.decoder.decode_png(bytes) else {
            return false;
        };
        let len = image.rgba.len();
        // SAFETY: `allocator` is the allocator libghostty handed us and `len`
        // is the buffer size.
        let buffer = unsafe { ghostty_alloc(allocator, len) };
        if buffer.is_null() {
            return false;
        }
        // SAFETY: `buffer` is `len` writable bytes and `image.rgba` is `len`
        // readable bytes; they cannot overlap.
        unsafe {
            ptr::copy_nonoverlapping(image.rgba.as_ptr(), buffer, len);
            (*out).width = image.width;
            (*out).height = image.height;
            (*out).data = buffer;
            (*out).data_len = len;
        }
        true
    })
    .unwrap_or(false)
}
