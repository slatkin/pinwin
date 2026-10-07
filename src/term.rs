//! The terminal core (port-to-rust D3): owns the pinned libghostty-vt handles
//! and the effect callbacks that `src/main.zig` used to keep in process-global
//! variables.
//!
//! `main.zig` exposed `term`, `render_state`, the row/placement iterators, the
//! key/mouse encoders and events, and the grid metrics as `pub var` globals
//! that `cells.zig` and `input.zig` read. Here a single [`Terminal`] owns
//! them, so the next `term` sub-units (`cells`, `keys`, `input`) take the
//! handles through accessors instead of reaching for globals. The type is
//! `!Send`/`!Sync` (D2 item 8, D4): it lives on the panel thread only.
//!
//! Panics must never cross back into C (D5). Every trampoline below runs its
//! body through the shared [`crate::guard`] helper, and a panic latches a
//! shared poisoned flag; later calls become no-ops.
//!
//! The pty write sink and the PNG decoder are injected behind the [`PtySink`]
//! and [`PngDecoder`] traits, so nothing here depends on the windowing
//! layer (D3).

use std::cell::Cell;
use std::os::raw::c_void;
use std::ptr;
use std::rc::Rc;

use crate::guard::Poisoned;

use crate::ghostty_sys::input::{
    GHOSTTY_MOUSE_ENCODER_OPT_SIZE, GhosttyKeyEncoder, GhosttyKeyEvent, GhosttyMouseEncoder,
    GhosttyMouseEncoderSize, GhosttyMouseEvent, ghostty_key_encoder_free, ghostty_key_encoder_new,
    ghostty_key_event_free, ghostty_key_event_new, ghostty_mouse_encoder_free,
    ghostty_mouse_encoder_new, ghostty_mouse_encoder_setopt, ghostty_mouse_event_free,
    ghostty_mouse_event_new,
};
use crate::ghostty_sys::kitty::{
    GhosttyKittyGraphicsPlacementIterator, ghostty_kitty_graphics_placement_iterator_free,
    ghostty_kitty_graphics_placement_iterator_new,
};
use crate::ghostty_sys::render::{
    GhosttyRenderState, GhosttyRenderStateRowCells, GhosttyRenderStateRowIterator,
    ghostty_render_state_free, ghostty_render_state_new, ghostty_render_state_row_cells_free,
    ghostty_render_state_row_cells_new, ghostty_render_state_row_iterator_free,
    ghostty_render_state_row_iterator_new,
};
use crate::ghostty_sys::terminal::{
    GhosttyTerminal, GhosttyTerminalData, GhosttyTerminalOption, ghostty_terminal_free,
    ghostty_terminal_get, ghostty_terminal_new, ghostty_terminal_resize, ghostty_terminal_set,
    ghostty_terminal_vt_write,
};
use crate::ghostty_sys::{GHOSTTY_REJECTED, GHOSTTY_SUCCESS, GhosttyResult};

mod callbacks;
pub mod cells;
pub mod input;
pub mod keys;

/// Ghostty's own default for `image-storage-limit`, and headroom for
/// image-heavy hosts (`src/main.zig`).
const KITTY_STORAGE_LIMIT: u64 = 320 * 1024 * 1024;

/// Replay cap for pty bytes that arrived before the terminal existed. Startup
/// traffic is a handful of queries and a first paint, far below this; a child
/// that floods the pty before the terminal can lose the overflow
/// (`src/main.zig`).
const EARLY_PTY_CAP: usize = 1 << 20;

const DEFAULT_COLS: u16 = 40;
const DEFAULT_ROWS: u16 = 24;

/// Where terminal-initiated pty writes go. The host owns the real fd; the
/// library only needs to hand it bytes.
pub trait PtySink: 'static {
    /// Write response bytes back to the pty.
    fn write_pty(&mut self, data: &[u8]);
}

/// A decoded PNG: RGBA pixels plus their dimensions.
#[derive(Debug)]
pub struct DecodedPng {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes kitty graphics PNG data. The panel thread's `png` module supplies
/// the implementation; tests can supply a stub.
pub trait PngDecoder: 'static {
    /// Decode `data` into RGBA pixels, or `None` to reject the image.
    fn decode_png(&mut self, data: &[u8]) -> Option<DecodedPng>;
}

/// Everything the effect callbacks and the library share for one terminal.
/// Its address is the `userdata` handed to libghostty, so it must live in a
/// `Box` for as long as the terminal does.
struct CallbackContext {
    sink: Box<dyn PtySink>,
    decoder: Box<dyn PngDecoder>,
    queue_draw: Box<dyn Fn()>,
    poisoned: Poisoned,
    cols: u16,
    rows: u16,
    cell_w: u32,
    cell_h: u32,
    /// The resolved scale in 1/120 units (device-pixel-cell-reports D3):
    /// `size_report` scales the logical cell by it; 120 is scale 1.
    scale_120: Rc<Cell<u32>>,
}

/// The owned ghostty handles. Each field is `Some` once the handle exists, so
/// a partially failed creation frees exactly what it built in `Drop`.
#[derive(Default)]
struct Handles {
    terminal: Option<GhosttyTerminal>,
    render_state: Option<GhosttyRenderState>,
    row_iterator: Option<GhosttyRenderStateRowIterator>,
    row_cells: Option<GhosttyRenderStateRowCells>,
    placement_iterator: Option<GhosttyKittyGraphicsPlacementIterator>,
    key_encoder: Option<GhosttyKeyEncoder>,
    key_event: Option<GhosttyKeyEvent>,
    mouse_encoder: Option<GhosttyMouseEncoder>,
    mouse_event: Option<GhosttyMouseEvent>,
}

impl Handles {
    /// Create every handle, or fail and free whatever was created. The
    /// terminal must outlive the handles that borrow from it.
    fn create(cols: u16, rows: u16) -> Result<Self, ()> {
        let mut handles = Handles::default();
        // SAFETY: every out pointer is a valid, writable `Option`-free local
        // of the right handle type; NULL selects the default allocator.
        unsafe {
            let mut terminal = GhosttyTerminal(ptr::null_mut());
            if ghostty_terminal_new(ptr::null(), &raw mut terminal, cols, rows) != GHOSTTY_SUCCESS {
                return Err(());
            }
            handles.terminal = Some(terminal);

            let mut render_state = GhosttyRenderState(ptr::null_mut());
            if ghostty_render_state_new(ptr::null(), &raw mut render_state) != GHOSTTY_SUCCESS {
                return Err(());
            }
            handles.render_state = Some(render_state);

            let mut row_iterator = GhosttyRenderStateRowIterator(ptr::null_mut());
            if ghostty_render_state_row_iterator_new(ptr::null(), &raw mut row_iterator)
                != GHOSTTY_SUCCESS
            {
                return Err(());
            }
            handles.row_iterator = Some(row_iterator);

            let mut row_cells = GhosttyRenderStateRowCells(ptr::null_mut());
            if ghostty_render_state_row_cells_new(ptr::null(), &raw mut row_cells)
                != GHOSTTY_SUCCESS
            {
                return Err(());
            }
            handles.row_cells = Some(row_cells);

            let mut placement_iterator = GhosttyKittyGraphicsPlacementIterator(ptr::null_mut());
            if ghostty_kitty_graphics_placement_iterator_new(
                ptr::null(),
                &raw mut placement_iterator,
            ) != GHOSTTY_SUCCESS
            {
                return Err(());
            }
            handles.placement_iterator = Some(placement_iterator);

            let mut key_encoder = GhosttyKeyEncoder(ptr::null_mut());
            if ghostty_key_encoder_new(ptr::null(), &raw mut key_encoder) != GHOSTTY_SUCCESS {
                return Err(());
            }
            handles.key_encoder = Some(key_encoder);

            let mut key_event = GhosttyKeyEvent(ptr::null_mut());
            if ghostty_key_event_new(ptr::null(), &raw mut key_event) != GHOSTTY_SUCCESS {
                return Err(());
            }
            handles.key_event = Some(key_event);

            let mut mouse_encoder = GhosttyMouseEncoder(ptr::null_mut());
            if ghostty_mouse_encoder_new(ptr::null(), &raw mut mouse_encoder) != GHOSTTY_SUCCESS {
                return Err(());
            }
            handles.mouse_encoder = Some(mouse_encoder);

            let mut mouse_event = GhosttyMouseEvent(ptr::null_mut());
            if ghostty_mouse_event_new(ptr::null(), &raw mut mouse_event) != GHOSTTY_SUCCESS {
                return Err(());
            }
            handles.mouse_event = Some(mouse_event);
        };
        Ok(handles)
    }
}

impl Drop for Handles {
    fn drop(&mut self) {
        // SAFETY: each handle was created by the matching `*_new` and is freed
        // exactly once, leaf objects before the terminal they borrow from.
        unsafe {
            if let Some(event) = self.mouse_event {
                ghostty_mouse_event_free(event);
            }
            if let Some(event) = self.key_event {
                ghostty_key_event_free(event);
            }
            if let Some(encoder) = self.mouse_encoder {
                ghostty_mouse_encoder_free(encoder);
            }
            if let Some(encoder) = self.key_encoder {
                ghostty_key_encoder_free(encoder);
            }
            if let Some(iterator) = self.placement_iterator {
                ghostty_kitty_graphics_placement_iterator_free(iterator);
            }
            if let Some(cells) = self.row_cells {
                ghostty_render_state_row_cells_free(cells);
            }
            if let Some(iterator) = self.row_iterator {
                ghostty_render_state_row_iterator_free(iterator);
            }
            if let Some(state) = self.render_state {
                ghostty_render_state_free(state);
            }
            if let Some(terminal) = self.terminal {
                ghostty_terminal_free(terminal);
            }
        }
    }
}

/// The creation step, injectable so a test can force init failure (D10).
type InitFn = dyn FnMut(*mut c_void, u16, u16) -> Result<Handles, ()>;

/// A libghostty-vt terminal and everything it owns. `!Send`/`!Sync`: it may
/// only be used from the thread that created it (D4).
pub struct Terminal {
    handles: Option<Handles>,
    ctx: Box<CallbackContext>,
    init: Option<Box<InitFn>>,
    /// Sticky: once init fails, later calls report failure instead of
    /// re-entering creation, whose partial handles would be unusable
    /// (`src/main.zig`).
    init_failed: bool,
    /// PTY bytes that arrived before the terminal existed, replayed once it
    /// does (`src/main.zig`).
    early_pty_data: Vec<u8>,
    /// Scroll/focus/button state for the input encoders (D3).
    input_state: input::InputState,
    /// Per-frame state for the cell/glyph protocol (port-to-rust D3).
    frame: cells::FrameState,
}

/// The grid identity and lifecycle of a [`Terminal`]: the handles, sinks
/// and decoder are intentionally absent (a `dyn PtySink` carries no `Debug`).
impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("initialized", &self.handles.is_some())
            .field("cols", &self.ctx.cols)
            .field("rows", &self.ctx.rows)
            .field("cell_w", &self.ctx.cell_w)
            .field("cell_h", &self.ctx.cell_h)
            .field("init_failed", &self.init_failed)
            .finish_non_exhaustive()
    }
}

impl Terminal {
    /// Build a terminal that lazily creates its handles on the first
    /// [`Terminal::push_size`]. `poisoned` is the owner's shared D5 latch: a
    /// panic in an effect-callback trampoline latches it so the rest of the
    /// panel's glue code stops too.
    pub fn new(
        poisoned: Poisoned,
        sink: impl PtySink + 'static,
        decoder: impl PngDecoder + 'static,
        queue_draw: impl Fn() + 'static,
    ) -> Self {
        Self::with_scale_note(poisoned, sink, decoder, queue_draw, Rc::new(Cell::new(120)))
    }

    /// Build a terminal sharing the panel thread's scale note
    /// (device-pixel-cell-reports D3): `size_report` reads it at reply time.
    pub fn with_scale_note(
        poisoned: Poisoned,
        sink: impl PtySink + 'static,
        decoder: impl PngDecoder + 'static,
        queue_draw: impl Fn() + 'static,
        scale_120: Rc<Cell<u32>>,
    ) -> Self {
        Self::with_init(
            poisoned,
            Box::new(sink),
            Box::new(decoder),
            Box::new(queue_draw),
            Box::new(callbacks::init_ghostty),
            scale_120,
        )
    }

    fn with_init(
        poisoned: Poisoned,
        sink: Box<dyn PtySink>,
        decoder: Box<dyn PngDecoder>,
        queue_draw: Box<dyn Fn()>,
        init: Box<InitFn>,
        scale_120: Rc<Cell<u32>>,
    ) -> Self {
        Terminal {
            handles: None,
            ctx: Box::new(CallbackContext {
                sink,
                decoder,
                queue_draw,
                poisoned,
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
                cell_w: 1,
                cell_h: 1,
                scale_120,
            }),
            init: Some(init),
            init_failed: false,
            early_pty_data: Vec::new(),
            input_state: input::InputState::default(),
            frame: cells::FrameState::default(),
        }
    }

    /// Whether a callback panicked and latched the terminal poisoned (D5).
    #[must_use]
    pub fn poisoned(&self) -> bool {
        self.ctx.poisoned.is_poisoned()
    }

    /// Whether the handles exist.
    #[must_use]
    pub fn initialized(&self) -> bool {
        self.handles.is_some()
    }

    /// The grid width in columns.
    #[must_use]
    pub fn cols(&self) -> u16 {
        self.ctx.cols
    }

    /// The grid height in rows.
    #[must_use]
    pub fn rows(&self) -> u16 {
        self.ctx.rows
    }

    /// The cell width in pixels.
    #[must_use]
    pub fn cell_w(&self) -> u32 {
        self.ctx.cell_w
    }

    /// The cell height in pixels.
    #[must_use]
    pub fn cell_h(&self) -> u32 {
        self.ctx.cell_h
    }

    /// The shared scale note `size_report` reads, in 1/120 units
    /// (device-pixel-cell-reports D3).
    #[must_use]
    pub fn scale_note(&self) -> Rc<Cell<u32>> {
        Rc::clone(&self.ctx.scale_120)
    }

    /// The terminal handle, once created.
    #[must_use]
    pub fn terminal(&self) -> Option<GhosttyTerminal> {
        self.handles.as_ref().and_then(|handles| handles.terminal)
    }

    /// The key encoder handle, once created.
    #[must_use]
    pub fn key_encoder(&self) -> Option<GhosttyKeyEncoder> {
        self.handles
            .as_ref()
            .and_then(|handles| handles.key_encoder)
    }

    /// The key event handle, once created.
    #[must_use]
    pub fn key_event(&self) -> Option<GhosttyKeyEvent> {
        self.handles.as_ref().and_then(|handles| handles.key_event)
    }

    /// The mouse encoder handle, once created.
    #[must_use]
    pub fn mouse_encoder(&self) -> Option<GhosttyMouseEncoder> {
        self.handles
            .as_ref()
            .and_then(|handles| handles.mouse_encoder)
    }

    /// The mouse event handle, once created.
    #[must_use]
    pub fn mouse_event(&self) -> Option<GhosttyMouseEvent> {
        self.handles
            .as_ref()
            .and_then(|handles| handles.mouse_event)
    }

    /// Set a terminal option (a thin wrapper over `ghostty_terminal_set`),
    /// returning [`GHOSTTY_REJECTED`] before the terminal exists.
    ///
    /// # Safety
    /// `value` must point to a value of the type `option` expects, valid for
    /// the duration of the call.
    #[must_use]
    pub unsafe fn set_option(
        &self,
        option: GhosttyTerminalOption,
        value: *const c_void,
    ) -> GhosttyResult {
        match self.terminal() {
            // SAFETY: the caller upholds the option/value contract.
            Some(terminal) => unsafe { ghostty_terminal_set(terminal, option, value) },
            None => GHOSTTY_REJECTED,
        }
    }

    /// Query terminal state (a thin wrapper over `ghostty_terminal_get`),
    /// returning [`GHOSTTY_REJECTED`] before the terminal exists.
    ///
    /// # Safety
    /// `out` must point to storage of the type `data` returns, valid for the
    /// duration of the call.
    pub unsafe fn get_option(&self, data: GhosttyTerminalData, out: *mut c_void) -> GhosttyResult {
        match self.terminal() {
            // SAFETY: the caller upholds the data/out contract.
            Some(terminal) => unsafe { ghostty_terminal_get(terminal, data, out) },
            None => GHOSTTY_REJECTED,
        }
    }

    /// Apply a new grid size. Clamps each dimension to at least 1 (as
    /// `src/main.zig` did), creates the terminal on the first successful
    /// call, resizes it and syncs the mouse encoder's pixel size. Returns
    /// false when the terminal could not be created; the previous grid stays
    /// in effect and every later call keeps failing (`src/main.zig`).
    ///
    /// # Panics
    /// If handle creation reported success without installing handles, which
    /// cannot happen: `ensure_init` installs them on every `Ok` path.
    pub fn push_size(&mut self, cols: i32, rows: i32, cell_w: i32, cell_h: i32) -> bool {
        if self.init_failed {
            return false;
        }

        let previous = (
            self.ctx.cols,
            self.ctx.rows,
            self.ctx.cell_w,
            self.ctx.cell_h,
        );
        self.ctx.cols = clamp_u16(cols);
        self.ctx.rows = clamp_u16(rows);
        self.ctx.cell_w = clamp_u32(cell_w);
        self.ctx.cell_h = clamp_u32(cell_h);

        if !self.ensure_init() {
            (
                self.ctx.cols,
                self.ctx.rows,
                self.ctx.cell_w,
                self.ctx.cell_h,
            ) = previous;
            return false;
        }

        let handles = self.handles.as_ref().expect("ensure_init created handles");
        // SAFETY: the handles are live and the grid values are the ones just
        // passed to creation; resize only reads them.
        unsafe {
            ghostty_terminal_resize(
                handles.terminal.expect("terminal created"),
                self.ctx.cols,
                self.ctx.rows,
                self.ctx.cell_w,
                self.ctx.cell_h,
            );
        };

        let mut size = GhosttyMouseEncoderSize {
            size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
            screen_width: u32::from(self.ctx.cols) * self.ctx.cell_w,
            screen_height: u32::from(self.ctx.rows) * self.ctx.cell_h,
            cell_width: self.ctx.cell_w,
            cell_height: self.ctx.cell_h,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        // SAFETY: the mouse encoder is live and `size` is a sized struct of
        // the type `GHOSTTY_MOUSE_ENCODER_OPT_SIZE` expects.
        unsafe {
            ghostty_mouse_encoder_setopt(
                handles.mouse_encoder.expect("mouse encoder created"),
                GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&raw mut size).cast(),
            );
        };
        true
    }

    /// Feed pty bytes to the terminal. Before the terminal exists the bytes
    /// are buffered up to `EARLY_PTY_CAP` and replayed once it does; after a
    /// sticky init failure they are dropped (`src/main.zig`).
    ///
    /// # Panics
    /// If the terminal handle is missing while other handles exist, which
    /// cannot happen: the terminal is created first and freed last.
    pub fn push_pty_data(&mut self, data: &[u8]) {
        if let Some(handles) = self.handles.as_ref() {
            // SAFETY: the terminal is live and `data` is a valid slice for
            // the call.
            unsafe {
                ghostty_terminal_vt_write(
                    handles.terminal.expect("terminal created"),
                    data.as_ptr(),
                    data.len(),
                );
            };
            (self.ctx.queue_draw)();
            return;
        }
        if self.init_failed {
            return;
        }
        let room = EARLY_PTY_CAP.saturating_sub(self.early_pty_data.len());
        if data.len() <= room {
            self.early_pty_data.extend_from_slice(data);
        }
    }

    /// Create the handles on first use. A failure is sticky: later calls do
    /// not retry and report failure.
    fn ensure_init(&mut self) -> bool {
        if self.handles.is_some() {
            return true;
        }
        if self.init_failed {
            return false;
        }
        let Some(mut init) = self.init.take() else {
            self.init_failed = true;
            return false;
        };
        let userdata = (&raw mut *self.ctx).cast::<c_void>();
        callbacks::register_decode_context(userdata.cast::<CallbackContext>());
        if let Ok(handles) = init(userdata, self.ctx.cols, self.ctx.rows) {
            self.handles = Some(handles);
            self.replay_early_pty();
            true
        } else {
            callbacks::unregister_decode_context(userdata.cast::<CallbackContext>());
            self.init_failed = true;
            false
        }
    }

    /// Replay pty bytes that arrived before the terminal existed, now that
    /// queries can be answered, then ask for a draw.
    fn replay_early_pty(&mut self) {
        if self.early_pty_data.is_empty() {
            return;
        }
        if let Some(handles) = self.handles.as_ref() {
            // SAFETY: the terminal is live and `early_pty_data` is valid for
            // the call.
            unsafe {
                ghostty_terminal_vt_write(
                    handles.terminal.expect("terminal created"),
                    self.early_pty_data.as_ptr(),
                    self.early_pty_data.len(),
                );
            };
            self.early_pty_data.clear();
            (self.ctx.queue_draw)();
        }
    }
}

/// Removing the context from the decode registry before the `Box` is freed
/// keeps the process-global sys hook from ever seeing a dangling pointer.
impl Drop for Terminal {
    fn drop(&mut self) {
        callbacks::unregister_decode_context(&raw mut *self.ctx);
    }
}

fn clamp_u16(value: i32) -> u16 {
    // The clamp keeps the value inside `u16` range, so the conversion below
    // cannot fail.
    u16::try_from(value.clamp(1, i32::from(u16::MAX))).expect("clamped to u16 range")
}

fn clamp_u32(value: i32) -> u32 {
    // `max(1)` keeps the value non-negative, so the conversion below cannot
    // fail.
    u32::try_from(value.max(1)).expect("non-negative, fits in u32")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Records every pty write so a test can assert the response bytes.
    struct RecordingSink {
        writes: Arc<Mutex<Vec<u8>>>,
    }

    impl RecordingSink {
        fn pair() -> (Self, Arc<Mutex<Vec<u8>>>) {
            let writes = Arc::new(Mutex::new(Vec::new()));
            (
                RecordingSink {
                    writes: Arc::clone(&writes),
                },
                writes,
            )
        }

        fn new() -> Self {
            RecordingSink::pair().0
        }
    }

    impl PtySink for RecordingSink {
        fn write_pty(&mut self, data: &[u8]) {
            self.writes
                .lock()
                .expect("sink lock")
                .extend_from_slice(data);
        }
    }

    /// Rejects every image; the `term` unit does not exercise PNG decoding.
    struct NoDecoder;

    impl PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
            None
        }
    }

    /// A sink that panics, to prove a callback panic is contained.
    struct PanickingSink;

    impl PtySink for PanickingSink {
        fn write_pty(&mut self, _data: &[u8]) {
            panic!("sink panicked");
        }
    }

    /// Pty data that arrives before the terminal exists is replayed once the
    /// terminal is created, and the DA1 reply goes back through the sink:
    /// `CSI c` answers `CSI ? 62 ; 22 ; 52 c` (Ghostty's own features). The
    /// replay also signals a draw.
    #[test]
    fn pty_data_before_init_is_replayed() {
        let (sink, writes) = RecordingSink::pair();
        let draws = Arc::new(AtomicUsize::new(0));
        let draws_for_push = Arc::clone(&draws);
        let mut terminal =
            Terminal::new(crate::guard::Poisoned::new(), sink, NoDecoder, move || {
                draws_for_push.fetch_add(1, Ordering::Relaxed);
            });

        terminal.push_pty_data(b"\x1b[c");
        assert!(!terminal.initialized());
        assert!(writes.lock().expect("sink lock").is_empty());

        assert!(terminal.push_size(40, 24, 8, 16));
        assert!(terminal.initialized());
        assert_eq!(
            writes.lock().expect("sink lock").clone(),
            b"\x1b[?62;22;52c"
        );
        assert_eq!(draws.load(Ordering::Relaxed), 1);
    }

    /// A terminal created with a failing initializer latches the failure and
    /// never retries; the previous grid stays in effect.
    #[test]
    fn init_failure_is_sticky() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_init = Arc::clone(&calls);
        let mut terminal = Terminal::with_init(
            Poisoned::new(),
            Box::new(RecordingSink::new()),
            Box::new(NoDecoder),
            Box::new(|| {}),
            Box::new(move |_, _, _| {
                calls_for_init.fetch_add(1, Ordering::Relaxed);
                Err(())
            }),
            Rc::new(Cell::new(120)),
        );

        assert!(!terminal.push_size(80, 24, 9, 18));
        assert!(!terminal.push_size(80, 24, 9, 18));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(!terminal.initialized());
        assert_eq!(
            (
                terminal.cols(),
                terminal.rows(),
                terminal.cell_w(),
                terminal.cell_h()
            ),
            (DEFAULT_COLS, DEFAULT_ROWS, 1, 1)
        );
    }

    /// Sizes are clamped to at least 1 on every dimension.
    #[test]
    fn size_clamps_to_at_least_one() {
        let mut terminal = Terminal::new(
            crate::guard::Poisoned::new(),
            RecordingSink::new(),
            NoDecoder,
            || {},
        );
        assert!(terminal.push_size(0, -3, 0, -7));
        assert_eq!(
            (
                terminal.cols(),
                terminal.rows(),
                terminal.cell_w(),
                terminal.cell_h()
            ),
            (1, 1, 1, 1)
        );
        assert!(terminal.push_size(80, 24, 9, 18));
        assert_eq!(
            (
                terminal.cols(),
                terminal.rows(),
                terminal.cell_w(),
                terminal.cell_h()
            ),
            (80, 24, 9, 18)
        );
    }

    /// After init, a DA1 query reaches the sink through the `write_pty`
    /// trampoline.
    #[test]
    fn write_pty_trampoline_delivers_bytes() {
        let (sink, writes) = RecordingSink::pair();
        let mut terminal = Terminal::new(crate::guard::Poisoned::new(), sink, NoDecoder, || {});
        assert!(terminal.push_size(40, 24, 8, 16));
        terminal.push_pty_data(b"\x1b[c");
        assert_eq!(
            writes.lock().expect("sink lock").clone(),
            b"\x1b[?62;22;52c"
        );
    }

    /// A panicking sink must not unwind into C: the panic is caught, the
    /// terminal is poisoned and the process keeps running.
    #[test]
    fn panicking_sink_poisons_and_does_not_abort() {
        let mut terminal = Terminal::new(
            crate::guard::Poisoned::new(),
            PanickingSink,
            NoDecoder,
            || {},
        );
        assert!(terminal.push_size(40, 24, 8, 16));
        assert!(!terminal.poisoned());
        terminal.push_pty_data(b"\x1b[c");
        assert!(terminal.poisoned());
    }
}
