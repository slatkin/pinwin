//! libghostty effect callbacks (port-to-rust D3): the `extern "C"`
//! trampolines libghostty calls back into, moved out of `mod.rs` so it stays
//! under the module size cap. The creation path ([`init_ghostty`]) installs
//! the process-global sys hooks and the per-terminal effect callbacks; every
//! trampoline catches unwinds (D5), latching a poisoned flag so later calls
//! become no-ops instead of unwinding into C.

use std::cell::RefCell;
use std::os::raw::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::slice;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{CallbackContext, Handles, KITTY_STORAGE_LIMIT};
use crate::ghostty_sys::sys::{
    GHOSTTY_SYS_OPT_DECODE_PNG, GHOSTTY_SYS_OPT_USERDATA, GhosttySysImage, ghostty_sys_set,
};
use crate::ghostty_sys::terminal::{
    GhosttyDeviceAttributes, GhosttySizeReportSize, GhosttyTerminal, ghostty_terminal_set,
};
use crate::ghostty_sys::{GHOSTTY_SUCCESS, GhosttyAllocator, ghostty_alloc};

thread_local! {
    /// Live terminal contexts on this thread, oldest first. The sys PNG hook
    /// is process-global and receives no terminal argument, so [`decode_png`]
    /// consults this registry. libghostty calls it synchronously from the
    /// thread driving the terminal (D4 keeps a terminal on one thread), and
    /// `Terminal` removes its context before freeing it, so a decode can never
    /// observe a dangling pointer.
    static DECODE_CONTEXTS: RefCell<Vec<*mut CallbackContext>> =
        const { RefCell::new(Vec::new()) };
}

/// Record a terminal's context as a decode target on this thread.
pub(super) fn register_decode_context(ctx: *mut CallbackContext) {
    DECODE_CONTEXTS.with(|contexts| contexts.borrow_mut().push(ctx));
}

/// Stop routing decodes at a context before its `Terminal` frees it.
pub(super) fn unregister_decode_context(ctx: *mut CallbackContext) {
    DECODE_CONTEXTS.with(|contexts| contexts.borrow_mut().retain(|each| *each != ctx));
}

/// The context most recently registered on this thread, if any.
fn current_decode_context() -> Option<*mut CallbackContext> {
    DECODE_CONTEXTS.with(|contexts| contexts.borrow().last().copied())
}

/// A process-lifetime token installed as the libghostty sys userdata. It is a
/// `static`, so no `Terminal` drop can leave the process-global slot dangling;
/// the decode forwarder finds the live context through [`DECODE_CONTEXTS`]
/// rather than through this pointer.
static SYS_USERDATA: u8 = 0;

/// Install the process-global libghostty hooks exactly once. libghostty wants
/// these set once at startup and its userdata slot is process-global; the
/// token above outlives every terminal, so later terminals never repoint it.
fn install_sys_hooks() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let userdata = &SYS_USERDATA as *const u8 as *mut c_void;
        // SAFETY: `ghostty_sys_set` takes the option's value directly (sys.zig
        // casts it to the option's `InType`): the stable userdata token and
        // the decode function pointer.
        unsafe {
            let _ = ghostty_sys_set(GHOSTTY_SYS_OPT_USERDATA, userdata);
            let _ = ghostty_sys_set(GHOSTTY_SYS_OPT_DECODE_PNG, decode_png as *const c_void);
        }
    });
}

/// The real creation path: install the global hooks, create the handles,
/// attach the effect callbacks and the kitty storage limit.
pub(super) fn init_ghostty(userdata: *mut c_void, cols: u16, rows: u16) -> Result<Handles, ()> {
    install_sys_hooks();
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
/// Called by libghostty with a valid `allocator`, a readable `data`/`data_len`
/// pair and a writable `out`. The `userdata` argument is ignored: the forwarder
/// resolves the live terminal through the per-thread registry, so the
/// process-global sys slot never has to hold a droppable pointer.
unsafe extern "C" fn decode_png(
    _userdata: *mut c_void,
    allocator: *const GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut GhosttySysImage,
) -> bool {
    if out.is_null() {
        return false;
    }
    let Some(ctx) = current_decode_context() else {
        return false;
    };
    // SAFETY: the context stays registered on this thread until its `Terminal`
    // frees it, so it is live for the duration of this call.
    let ctx = unsafe { &mut *ctx };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use crate::term::{DecodedPng, PngDecoder, PtySink, Terminal};

    /// A sink with nowhere to write; the decode tests only exercise the PNG
    /// path.
    struct NullSink;

    impl PtySink for NullSink {
        fn write_pty(&mut self, _data: &[u8]) {}
    }

    /// Counts decodes and yields its image once.
    struct OneShotDecoder {
        calls: Arc<AtomicUsize>,
        image: Option<DecodedPng>,
    }

    impl PngDecoder for OneShotDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.image.take()
        }
    }

    fn one_pixel() -> DecodedPng {
        DecodedPng {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        }
    }

    /// The sys userdata is process-global and libghostty's only decode hook.
    /// Building a second terminal and dropping it must not leave that hook
    /// pointing at freed memory: the earlier terminal still answers a decode.
    #[test]
    fn dropping_a_newer_terminal_leaves_the_older_decode_context_live() {
        let older_calls = Arc::new(AtomicUsize::new(0));
        let newer_calls = Arc::new(AtomicUsize::new(0));

        let mut older = Terminal::new(
            NullSink,
            OneShotDecoder {
                calls: older_calls.clone(),
                image: Some(one_pixel()),
            },
            || {},
        );
        let mut newer = Terminal::new(
            NullSink,
            OneShotDecoder {
                calls: newer_calls.clone(),
                image: Some(one_pixel()),
            },
            || {},
        );
        assert!(older.push_size(40, 24, 8, 16));
        assert!(newer.push_size(40, 24, 8, 16));

        drop(newer);

        let mut out = GhosttySysImage {
            width: 0,
            height: 0,
            data: ptr::null_mut(),
            data_len: 0,
        };
        // SAFETY: `out` is writable and `data` is readable; the forwarder
        // resolves the live context itself.
        let ok = unsafe { decode_png(ptr::null_mut(), ptr::null(), b"png".as_ptr(), 3, &mut out) };
        assert!(ok, "the surviving terminal answered the decode");
        assert_eq!((out.width, out.height), (1, 1));
        assert_eq!(older_calls.load(Ordering::Relaxed), 1);
        assert_eq!(newer_calls.load(Ordering::Relaxed), 0);
    }
}
