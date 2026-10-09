//! The terminal core (port-to-rust D3): owns the pinned libghostty-vt
//! terminal and the effect callbacks that `src/main.zig` used to keep in
//! process-global variables, now through the `libghostty-vt` crate's safe
//! API (adopt-libghostty-rs A5).
//!
//! A single [`Terminal`] owns the crate's terminal, render state, iterators
//! and encoders, so the `term` sub-units (`cells`, `keys`, `input`) reach
//! them through the handle instead of globals. The type is `!Send`/`!Sync`
//! (D2 item 8, D4): it lives on the panel thread only.
//!
//! The crate's `extern "C"` trampolines do not catch panics, so a panic in a
//! callback closure would abort the host (adopt-libghostty-rs A3). Every
//! closure body below runs through the shared [`crate::guard`] helper with
//! the terminal's poison latch, and a panic latches the flag so later calls
//! become no-ops.
//!
//! Callbacks share state through `Rc<Shared>` captures instead of a userdata
//! pointer (A5). The pty write sink and the PNG decoder are injected behind
//! the [`PtySink`] and [`PngDecoder`] traits, so nothing here depends on the
//! windowing layer (D3).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use libghostty_vt as vt;

use crate::guard::Poisoned;

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

/// The grid identity the size-report callback answers from: the clamped
/// dimensions `push_size` last accepted.
#[derive(Clone, Copy, Debug)]
struct GridMetrics {
    cols: u16,
    rows: u16,
    cell_w: u32,
    cell_h: u32,
}

impl GridMetrics {
    const DEFAULT: Self = Self {
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        cell_w: 1,
        cell_h: 1,
    };
}

/// Everything the effect callbacks and the terminal share. The closures the
/// crate's terminal calls back into capture an [`Rc`] to this, as do the
/// `Terminal` and the PNG forwarder (adopt-libghostty-rs A5).
struct Shared {
    poisoned: Poisoned,
    /// The pty write sink, behind a `RefCell` because the `on_pty_write`
    /// callback and `push_focus` both write through it.
    sink: RefCell<Box<dyn PtySink>>,
    /// The PNG decoder, behind a `RefCell` because the process-global decode
    /// forwarder is the only caller (adopt-libghostty-rs A4).
    decoder: RefCell<Box<dyn PngDecoder>>,
    /// The grid metrics the size-report callback answers from; `push_size`
    /// publishes here.
    metrics: Cell<GridMetrics>,
    /// The resolved scale in 1/120 units (device-pixel-cell-reports D3):
    /// `size_report` scales the logical cell by it; 120 is scale 1.
    scale_120: Rc<Cell<u32>>,
    /// Test-only panic injection: the named callback's body panics before
    /// doing anything, so the D5 guard tests can prove a panic is contained.
    #[cfg(test)]
    fail_point: Cell<Option<&'static str>>,
}

/// The crate handles a [`Terminal`] owns. Creation is all-or-nothing: every
/// type frees itself when dropped, and Rust's own drop order frees the leaf
/// handles before the terminal they borrow from.
struct Handles {
    terminal: vt::Terminal<'static, 'static>,
    render_state: vt::RenderState<'static>,
    row_iterator: vt::render::RowIterator<'static>,
    cell_iterator: vt::render::CellIterator<'static>,
    placement_iterator: vt::kitty::graphics::PlacementIterator<'static>,
    key_encoder: vt::key::Encoder<'static>,
    key_event: vt::key::Event<'static>,
    mouse_encoder: vt::mouse::Encoder<'static>,
    mouse_event: vt::mouse::Event<'static>,
}

/// A libghostty-vt terminal and everything it owns. `!Send`/`!Sync`: it may
/// only be used from the thread that created it (D4).
pub struct Terminal {
    /// The frame state, dropped before the handles so a stored snapshot can
    /// never outlive the render state it borrows (see `cells::FrameState`).
    frame: cells::FrameState,
    handles: Option<Handles>,
    shared: Rc<Shared>,
    /// The panel's repaint latch, called after terminal output (D3).
    queue_draw: Box<dyn Fn()>,
    /// The creation step, injectable so a test can force init failure (D10).
    #[cfg(test)]
    init: Option<Box<dyn FnMut() -> Result<(), ()>>>,
    /// Sticky: once handle creation fails, later calls report failure instead
    /// of re-entering creation, as `src/main.zig` did.
    init_failed: bool,
    /// PTY bytes that arrived before the terminal existed, replayed once it
    /// does (`src/main.zig`).
    early_pty_data: Vec<u8>,
    /// Scroll/focus/button state for the input encoders (D3).
    input_state: input::InputState,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let metrics = self.shared.metrics.get();
        f.debug_struct("Terminal")
            .field("initialized", &self.handles.is_some())
            .field("cols", &metrics.cols)
            .field("rows", &metrics.rows)
            .field("cell_w", &metrics.cell_w)
            .field("cell_h", &metrics.cell_h)
            .field("init_failed", &self.init_failed)
            .finish_non_exhaustive()
    }
}

impl Terminal {
    /// Build a terminal that creates its libghostty handles on the first
    /// [`Terminal::push_size`]. `poisoned` is the owner's shared D5 latch: a
    /// panic in an effect callback latches it so the rest of the panel's
    /// glue code stops too.
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
        Self::from_shared(
            poisoned,
            Box::new(sink),
            Box::new(decoder),
            Box::new(queue_draw),
            scale_120,
        )
    }

    fn from_shared(
        poisoned: Poisoned,
        sink: Box<dyn PtySink>,
        decoder: Box<dyn PngDecoder>,
        queue_draw: Box<dyn Fn()>,
        scale_120: Rc<Cell<u32>>,
    ) -> Self {
        let shared = Self::build_shared(poisoned, sink, decoder, scale_120);
        callbacks::register_decode_context(&shared);
        Terminal {
            frame: cells::FrameState::default(),
            handles: None,
            shared,
            queue_draw,
            #[cfg(test)]
            init: None,
            init_failed: false,
            early_pty_data: Vec::new(),
            input_state: input::InputState::default(),
        }
    }

    /// Whether a callback panicked and latched the terminal poisoned (D5).
    #[must_use]
    pub fn poisoned(&self) -> bool {
        self.shared.poisoned.is_poisoned()
    }

    /// Whether the handles exist.
    #[must_use]
    pub fn initialized(&self) -> bool {
        self.handles.is_some()
    }

    /// The grid width in columns.
    #[must_use]
    pub fn cols(&self) -> u16 {
        self.shared.metrics.get().cols
    }

    /// The grid height in rows.
    #[must_use]
    pub fn rows(&self) -> u16 {
        self.shared.metrics.get().rows
    }

    /// The cell width in pixels.
    #[must_use]
    pub fn cell_w(&self) -> u32 {
        self.shared.metrics.get().cell_w
    }

    /// The cell height in pixels.
    #[must_use]
    pub fn cell_h(&self) -> u32 {
        self.shared.metrics.get().cell_h
    }

    /// The shared scale note `size_report` reads, in 1/120 units
    /// (device-pixel-cell-reports D3).
    #[must_use]
    pub fn scale_note(&self) -> Rc<Cell<u32>> {
        Rc::clone(&self.shared.scale_120)
    }

    /// Apply a new grid size. Clamps each dimension to at least 1 (as
    /// `src/main.zig` did), creates the terminal on the first successful
    /// call, resizes it and syncs the mouse encoder's pixel size. Returns
    /// false when the terminal could not be created or resized; the previous
    /// grid stays in effect and a failed creation keeps failing
    /// (`src/main.zig`).
    ///
    /// # Panics
    /// If handle creation reported success without installing handles, which
    /// cannot happen: `ensure_init` installs them on every success path.
    pub fn push_size(&mut self, cols: i32, rows: i32, cell_w: i32, cell_h: i32) -> bool {
        if self.init_failed {
            return false;
        }

        let previous = self.shared.metrics.get();
        self.shared.metrics.set(GridMetrics {
            cols: clamp_u16(cols),
            rows: clamp_u16(rows),
            cell_w: clamp_u32(cell_w),
            cell_h: clamp_u32(cell_h),
        });

        if !self.ensure_init() {
            self.shared.metrics.set(previous);
            return false;
        }

        let handles = self.handles.as_mut().expect("ensure_init built handles");
        if handles
            .terminal
            .resize(
                self.shared.metrics.get().cols,
                self.shared.metrics.get().rows,
                self.shared.metrics.get().cell_w,
                self.shared.metrics.get().cell_h,
            )
            .is_err()
        {
            // The resize failed (an OOM during reflow): keep the previous
            // grid, as the spec requires for a failed allocation.
            self.shared.metrics.set(previous);
            return false;
        }

        handles.mouse_encoder.set_size(vt::mouse::EncoderSize {
            screen_width: u32::from(self.shared.metrics.get().cols)
                * self.shared.metrics.get().cell_w,
            screen_height: u32::from(self.shared.metrics.get().rows)
                * self.shared.metrics.get().cell_h,
            cell_width: self.shared.metrics.get().cell_w,
            cell_height: self.shared.metrics.get().cell_h,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        });
        true
    }

    /// Feed pty bytes to the terminal. Before the terminal exists the bytes
    /// are buffered up to `EARLY_PTY_CAP` and replayed once it does; after a
    /// sticky init failure they are dropped (`src/main.zig`).
    pub fn push_pty_data(&mut self, data: &[u8]) {
        if let Some(handles) = self.handles.as_mut() {
            handles.terminal.vt_write(data);
            (self.queue_draw)();
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
        #[cfg(test)]
        if let Some(mut init) = self.init.take()
            && init().is_err()
        {
            self.init_failed = true;
            return false;
        }
        let metrics = self.shared.metrics.get();
        if let Ok(handles) =
            callbacks::init_ghostty(metrics.cols, metrics.rows, &Rc::clone(&self.shared))
        {
            self.handles = Some(handles);
            self.replay_early_pty();
            return true;
        }
        self.init_failed = true;
        false
    }

    /// Replay pty bytes that arrived before the terminal existed, now that
    /// queries can be answered, then ask for a draw.
    fn replay_early_pty(&mut self) {
        if self.early_pty_data.is_empty() {
            return;
        }
        if let Some(handles) = self.handles.as_mut() {
            handles.terminal.vt_write(&self.early_pty_data);
            self.early_pty_data.clear();
            (self.queue_draw)();
        }
    }

    /// The test-only forced-init seam (D10).
    #[cfg(test)]
    fn with_init(
        poisoned: Poisoned,
        sink: Box<dyn PtySink>,
        decoder: Box<dyn PngDecoder>,
        queue_draw: Box<dyn Fn()>,
        init: Box<dyn FnMut() -> Result<(), ()>>,
    ) -> Self {
        let mut terminal =
            Self::from_shared(poisoned, sink, decoder, queue_draw, Rc::new(Cell::new(120)));
        terminal.init = Some(init);
        terminal
    }

    /// The test-only panic-injection switch: the named callback panics at
    /// its first step (see `callbacks::fail_point`).
    #[cfg(test)]
    pub(crate) fn fail_point(&self) -> &Cell<Option<&'static str>> {
        &self.shared.fail_point
    }

    fn build_shared(
        poisoned: Poisoned,
        sink: Box<dyn PtySink>,
        decoder: Box<dyn PngDecoder>,
        scale_120: Rc<Cell<u32>>,
    ) -> Rc<Shared> {
        Rc::new(Shared {
            poisoned,
            sink: RefCell::new(sink),
            decoder: RefCell::new(decoder),
            metrics: Cell::new(GridMetrics::DEFAULT),
            scale_120,
            #[cfg(test)]
            fail_point: Cell::new(None),
        })
    }
}

/// Stop routing decodes at this terminal's decoder before it is freed.
impl Drop for Terminal {
    fn drop(&mut self) {
        callbacks::unregister_decode_context(&self.shared);
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
            Box::new(move || {
                calls_for_init.fetch_add(1, Ordering::Relaxed);
                Err(())
            }),
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

    /// After init, a DA1 query reaches the sink through the `on_pty_write`
    /// callback.
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

    /// The optimise mode is the checked-in `.cargo/config.toml` override
    /// (adopt-libghostty-rs A2): never the Zig default, in the dev profile
    /// the tests run under or any other.
    #[test]
    fn the_archive_is_built_release_safe() {
        assert_eq!(
            vt::build_info::optimize_mode().expect("build info"),
            vt::build_info::OptimizeMode::ReleaseSafe,
        );
    }
}
