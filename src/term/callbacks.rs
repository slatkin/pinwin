//! libghostty effect callbacks (port-to-rust D3): the closures the
//! `libghostty-vt` crate's terminal calls back into, moved out of the `term`
//! module root (`term.rs`) so it stays under the module size cap. The
//! creation path ([`init_ghostty`]) installs the process-wide PNG decode
//! forwarder (A4) and the per-terminal effect callbacks, sharing state
//! through [`Rc<Shared>`] captures instead of a userdata pointer (A5). The
//! crate's trampolines do not catch panics, so every closure body runs
//! through the D5 guard, latching the poisoned flag so later calls become
//! no-ops instead of aborting the host (A3).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use libghostty_vt as vt;
use libghostty_vt::alloc::{Allocator, Bytes};
use libghostty_vt::kitty::graphics::{self, DecodePng, DecodedImage};
use libghostty_vt::terminal::{
    ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType,
    PrimaryDeviceAttributes, SecondaryDeviceAttributes, SizeReportSize, TertiaryDeviceAttributes,
};

use super::{Handles, KITTY_STORAGE_LIMIT, Shared};
use crate::guard::{guard, guard_default};

thread_local! {
    /// Live terminal contexts on this thread, oldest first. The PNG decode
    /// hook is process-global and receives no terminal argument, so
    /// [`PngForwarder::decode_png`] consults this registry. libghostty calls
    /// it synchronously from the thread driving the terminal (D4 keeps a
    /// terminal on one thread), and `Terminal` removes its context before it
    /// is freed, so a decode can never observe a freed context.
    static DECODE_CONTEXTS: RefCell<Vec<Rc<Shared>>> = const { RefCell::new(Vec::new()) };

    /// Whether this thread already installed the decode forwarder. The hook
    /// crate stores its decoder per thread, so the installation is once per
    /// thread, not once per process.
    static DECODER_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Record a terminal's shared state as a decode target on this thread.
pub(super) fn register_decode_context(shared: &Rc<Shared>) {
    DECODE_CONTEXTS.with(|contexts| contexts.borrow_mut().push(Rc::clone(shared)));
}

/// Stop routing decodes at a context before its `Terminal` frees it.
pub(super) fn unregister_decode_context(shared: &Rc<Shared>) {
    DECODE_CONTEXTS.with(|contexts| {
        contexts
            .borrow_mut()
            .retain(|each| !Rc::ptr_eq(each, shared));
    });
}

/// The context most recently registered on this thread, if any.
fn current_decode_context() -> Option<Rc<Shared>> {
    DECODE_CONTEXTS.with(|contexts| contexts.borrow().last().cloned())
}

/// The process-wide PNG decode forwarder (adopt-libghostty-rs A4): the crate
/// installs one global decoder, and this one routes every decode at the live
/// terminal's [`PngDecoder`] through the per-thread registry.
struct PngForwarder;

impl DecodePng for PngForwarder {
    fn decode_png<'alloc>(
        &mut self,
        alloc: &'alloc Allocator<'_>,
        data: &[u8],
    ) -> Option<DecodedImage<'alloc>> {
        let shared = current_decode_context()?;
        guard_default(&shared.poisoned, None, || {
            let image = shared.decoder.borrow_mut().decode_png(data)?;
            // Derive the RGBA length from the reported dimensions, as
            // `src/main.zig` did, and reject a decoder whose buffer disagrees
            // so the buffer handed to libghostty always matches the
            // dimensions it receives.
            let width = usize::try_from(image.width).ok()?;
            let height = usize::try_from(image.height).ok()?;
            let len = width.checked_mul(height)?.checked_mul(4)?;
            if image.rgba.len() != len {
                return None;
            }
            // libghostty frees the buffer with the allocator it passed in, so
            // the output must come from that allocator (A4).
            let mut bytes = Bytes::new_with_alloc(alloc, len).ok()?;
            bytes.copy_from_slice(&image.rgba);
            Some(DecodedImage {
                width: image.width,
                height: image.height,
                data: bytes,
            })
        })
    }
}

/// Install the decode forwarder exactly once per thread. libghostty wants the
/// hook set before any kitty graphics traffic, which starts after the first
/// terminal exists.
fn install_png_decoder() {
    DECODER_INSTALLED.with(|installed| {
        if !installed.get() {
            installed.set(true);
            // A second installation would replace, not stack, so the result
            // only reports an allocator failure in the hook crate.
            let _ = graphics::set_png_decoder(Some(Box::new(PngForwarder)));
        }
    });
}

/// The real creation path: install the decode forwarder, create the handles,
/// attach the effect callbacks and the kitty storage limit. Every step frees
/// what it built when a later one fails.
pub(super) fn init_ghostty(cols: u16, rows: u16, shared: &Rc<Shared>) -> Result<Handles, ()> {
    install_png_decoder();

    let mut terminal = vt_terminal(cols, rows)?;
    register_callbacks(&mut terminal, shared)?;
    or_init_failure(terminal.set_kitty_image_storage_limit(KITTY_STORAGE_LIMIT))?;

    Ok(Handles {
        render_state: or_init_failure(vt::RenderState::new())?,
        row_iterator: or_init_failure(vt::render::RowIterator::new())?,
        cell_iterator: or_init_failure(vt::render::CellIterator::new())?,
        placement_iterator: or_init_failure(vt::kitty::graphics::PlacementIterator::new())?,
        key_encoder: or_init_failure(vt::key::Encoder::new())?,
        key_event: or_init_failure(vt::key::Event::new())?,
        mouse_encoder: or_init_failure(vt::mouse::Encoder::new())?,
        mouse_event: or_init_failure(vt::mouse::Event::new())?,
        terminal,
    })
}

/// Collapse any crate error into pinwin's sticky init failure. The crate's
/// error is not recoverable here (the allocation that failed is gone), the
/// previous grid stays, and `push_size` reports false; nothing can log a
/// more useful detail than the crate's own `Display` already would.
fn or_init_failure<T, E>(result: Result<T, E>) -> Result<T, ()> {
    match result {
        Ok(value) => Ok(value),
        Err(_) => Err(()),
    }
}

/// Create the crate's terminal; a failure maps to the sticky init failure
/// (`push_size` reports false and keeps the previous grid).
fn vt_terminal(cols: u16, rows: u16) -> Result<vt::Terminal<'static, 'static>, ()> {
    or_init_failure(vt::Terminal::new(cols, rows))
}

/// Attach the effect callbacks to the terminal. Each closure captures an
/// [`Rc`] clone of the shared state; a failed registration aborts creation.
fn register_callbacks(
    terminal: &mut vt::Terminal<'static, 'static>,
    shared: &Rc<Shared>,
) -> Result<(), ()> {
    let pty_shared = Rc::clone(shared);
    or_init_failure(terminal.on_pty_write(move |_, data| {
        let _ = guard(&pty_shared.poisoned, || {
            pty_shared.sink.borrow_mut().write_pty(data);
        });
    }))?;

    let size_shared = Rc::clone(shared);
    or_init_failure(terminal.on_size(move |_| size_report(&size_shared)))?;

    let attributes_shared = Rc::clone(shared);
    or_init_failure(terminal.on_device_attributes(move |_| device_attributes(&attributes_shared)))?;
    Ok(())
}

/// One logical cell dimension in device pixels (device-pixel-cell-reports
/// D1/D5): `logical * units_120 / 120`, rounded half up in exact integer
/// arithmetic — the same rule as the buffers' `scale_dimension`, so a
/// reported size never disagrees with a drawn one by rounding-path alone.
/// Saturates instead of overflowing: a cell that size has no drawable grid.
fn device_cell(logical: u32, units_120: u32) -> u32 {
    let scaled = u64::from(logical) * u64::from(units_120) + 60;
    u32::try_from(scaled / 120).unwrap_or(u32::MAX)
}

/// The size-report callback: answer `CSI 14/16/18 t` with the live grid in
/// device pixels (device-pixel-cell-reports D1/D3, D5 guard).
fn size_report(shared: &Rc<Shared>) -> Option<SizeReportSize> {
    guard_default(&shared.poisoned, None, || {
        fail_point(shared, "size");
        // Device pixels: the logical cell times the panel thread's scale
        // note, rounded (D1/D3) — rows and columns answer unchanged.
        let metrics = shared.metrics.get();
        let units_120 = shared.scale_120.get();
        Some(SizeReportSize {
            rows: metrics.rows,
            columns: metrics.cols,
            cell_width: device_cell(metrics.cell_w, units_120),
            cell_height: device_cell(metrics.cell_h, units_120),
        })
    })
}

/// The device-attributes callback: reply with Ghostty's own DA1/DA2/DA3
/// values (D5 guard).
fn device_attributes(shared: &Rc<Shared>) -> Option<DeviceAttributes> {
    guard_default(&shared.poisoned, None, || {
        fail_point(shared, "device_attributes");
        Some(DeviceAttributes {
            primary: PrimaryDeviceAttributes::new(
                ConformanceLevel(62), // level 2, like Ghostty
                &[
                    DeviceAttributeFeature(22), // ansi color
                    DeviceAttributeFeature(52), // clipboard
                ],
            ),
            secondary: SecondaryDeviceAttributes {
                device_type: DeviceType(1),
                firmware_version: 10,
                rom_cartridge: 0,
            },
            tertiary: TertiaryDeviceAttributes { unit_id: 0 },
        })
    })
}

/// The test-only panic injection (D10): the named callback's body panics at
/// its first step, so the guard tests prove a panic is contained.
#[cfg(test)]
fn fail_point(shared: &Rc<Shared>, name: &'static str) {
    assert_ne!(shared.fail_point.get(), Some(name), "fail point: {name}");
}

#[cfg(not(test))]
fn fail_point(shared: &Rc<Shared>, name: &'static str) {
    let _ = (shared, name);
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_void;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::guard::Poisoned;
    use crate::term::{DecodedPng, PngDecoder, PtySink, Terminal};

    /// Records writes so the size replies can be asserted.
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
    }

    impl PtySink for RecordingSink {
        fn write_pty(&mut self, data: &[u8]) {
            self.writes
                .lock()
                .expect("sink lock")
                .extend_from_slice(data);
        }
    }

    /// Rejects every image; the size-report unit does not decode PNGs.
    struct NoDecoder;

    impl PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
            None
        }
    }

    /// Drain the replies the terminal wrote so far.
    fn drain(writes: &Mutex<Vec<u8>>) -> Vec<u8> {
        std::mem::take(&mut *writes.lock().expect("sink lock"))
    }

    /// The size report answers device pixels (device-pixel-cell-reports
    /// D1/D3): the logical cell times the shared scale note, rounded half
    /// up in exact 1/120 arithmetic — while rows and columns answer
    /// unchanged.
    #[test]
    fn size_report_answers_the_note_scaled_cell_and_the_live_grid() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(
            Poisoned::new(),
            RecordingSink {
                writes: Arc::clone(&writes),
            },
            NoDecoder,
            || {},
        );
        assert!(terminal.push_size(40, 24, 9, 20));
        assert_eq!(terminal.scale_note().get(), 120, "scale 1 by default");

        terminal.push_pty_data(b"\x1b[16t");
        assert_eq!(
            drain(&writes),
            b"\x1b[6;20;9t",
            "scale 1 answers the logical cell"
        );

        terminal.scale_note().set(216);
        terminal.push_pty_data(b"\x1b[16t");
        assert_eq!(
            drain(&writes),
            b"\x1b[6;36;16t",
            "scale 1.8 answers device pixels"
        );

        terminal.push_pty_data(b"\x1b[18t");
        assert_eq!(
            drain(&writes),
            b"\x1b[8;24;40t",
            "the grid answers unchanged"
        );
    }

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

    /// The decode hook is process-global and libghostty's only decode entry.
    /// Building a second terminal and dropping it must not leave that hook
    /// pointing at freed state: the earlier terminal still answers a decode.
    #[test]
    fn dropping_a_newer_terminal_leaves_the_older_decode_context_live() {
        let older_calls = Arc::new(AtomicUsize::new(0));
        let newer_calls = Arc::new(AtomicUsize::new(0));

        let mut older = Terminal::new(
            Poisoned::new(),
            NullSink,
            OneShotDecoder {
                calls: Arc::clone(&older_calls),
                image: Some(one_pixel()),
            },
            || {},
        );
        let mut newer = Terminal::new(
            Poisoned::new(),
            NullSink,
            OneShotDecoder {
                calls: Arc::clone(&newer_calls),
                image: Some(one_pixel()),
            },
            || {},
        );
        assert!(older.push_size(40, 24, 8, 16));
        assert!(newer.push_size(40, 24, 8, 16));

        drop(newer);

        let mut out = vt::ffi::SysImage {
            width: 0,
            height: 0,
            data: ptr::null_mut(),
            data_len: 0,
        };
        // SAFETY: `out` is writable and `data` is readable; the forwarder
        // resolves the live context itself.
        let ok = unsafe { decode_into(ptr::null_mut(), b"png", &raw mut out) };
        assert!(ok, "the surviving terminal answered the decode");
        assert_eq!((out.width, out.height), (1, 1));
        assert_eq!(older_calls.load(Ordering::Relaxed), 1);
        assert_eq!(newer_calls.load(Ordering::Relaxed), 0);
    }

    /// A decoder that yields a scripted sequence of images.
    struct ScriptedDecoder {
        calls: Arc<AtomicUsize>,
        images: Vec<Option<DecodedPng>>,
    }

    impl PngDecoder for ScriptedDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
            let index = self.calls.fetch_add(1, Ordering::Relaxed);
            self.images.get_mut(index).and_then(Option::take)
        }
    }

    /// The decoded buffer must match the reported dimensions; a padded buffer
    /// is rejected and an exact one is accepted.
    #[test]
    fn decode_png_rejects_a_buffer_that_disagrees_with_the_dimensions() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut terminal = Terminal::new(
            Poisoned::new(),
            NullSink,
            ScriptedDecoder {
                calls: Arc::clone(&calls),
                images: vec![
                    // 2x2 needs 16 bytes; this buffer is padded to 17.
                    Some(DecodedPng {
                        width: 2,
                        height: 2,
                        rgba: vec![0; 17],
                    }),
                    // The exact size is accepted.
                    Some(DecodedPng {
                        width: 2,
                        height: 2,
                        rgba: vec![0; 16],
                    }),
                ],
            },
            || {},
        );
        assert!(terminal.push_size(40, 24, 8, 16));

        let mut rejected = vt::ffi::SysImage {
            width: 0,
            height: 0,
            data: ptr::null_mut(),
            data_len: 0,
        };
        // SAFETY: `rejected` is writable and `data` is readable; the
        // forwarder resolves the live context itself.
        let ok = unsafe { decode_into(ptr::null_mut(), b"png", &raw mut rejected) };
        assert!(!ok, "a padded buffer is rejected");

        let mut accepted = vt::ffi::SysImage {
            width: 0,
            height: 0,
            data: ptr::null_mut(),
            data_len: 0,
        };
        // SAFETY: `accepted` is writable and `data` is readable; the
        // forwarder resolves the live context itself.
        let ok = unsafe { decode_into(ptr::null_mut(), b"png", &raw mut accepted) };
        assert!(ok, "an exact buffer is accepted");
        assert_eq!(
            (accepted.width, accepted.height, accepted.data_len),
            (2, 2, 16)
        );
    }

    /// Drive the forwarder the way libghostty does: through its `DecodePng`
    /// impl with the allocator libghostty would pass, filling a raw sys
    /// image. The old trampoline shape, kept so the dimension checks stay
    /// covered end to end.
    ///
    /// # Safety
    /// `out` must be writable; `data` must be readable for `data.len()`.
    unsafe fn decode_into(
        _userdata: *mut c_void,
        data: &[u8],
        out: *mut vt::ffi::SysImage,
    ) -> bool {
        let mut forwarder = PngForwarder;
        let Some(mut image) = forwarder.decode_png(&Allocator::GLOBAL, data) else {
            return false;
        };
        // SAFETY: `out` is writable per the caller's contract and the image
        // buffer outlives the write.
        unsafe {
            (*out).width = image.width;
            (*out).height = image.height;
            (*out).data = image.data.as_mut_ptr();
            (*out).data_len = image.data.len();
        };
        true
    }

    /// A panicking size-report callback must not unwind into C: the panic is
    /// caught, the terminal is poisoned and later size queries answer
    /// nothing (adopt-libghostty-rs A3).
    #[test]
    fn panicking_size_report_poisons_and_does_not_abort() {
        let (sink, writes) = RecordingSink::pair();
        let mut terminal = Terminal::new(Poisoned::new(), sink, NoDecoder, || {});
        assert!(terminal.push_size(40, 24, 8, 16));
        assert!(!terminal.poisoned());

        terminal.fail_point().set(Some("size"));
        terminal.push_pty_data(b"\x1b[16t");
        assert!(terminal.poisoned(), "the panic latched the flag");
        assert!(
            drain(&writes).is_empty(),
            "the panicking reply wrote nothing"
        );

        // Later calls do nothing: the guard short-circuits while latched.
        terminal.fail_point().set(None);
        terminal.push_pty_data(b"\x1b[16t");
        assert!(
            drain(&writes).is_empty(),
            "a poisoned terminal answers no size query"
        );
        assert!(terminal.poisoned());
    }

    /// A panicking device-attributes callback is contained the same way.
    #[test]
    fn panicking_device_attributes_poisons_and_does_not_abort() {
        let (sink, writes) = RecordingSink::pair();
        let mut terminal = Terminal::new(Poisoned::new(), sink, NoDecoder, || {});
        assert!(terminal.push_size(40, 24, 8, 16));
        assert!(!terminal.poisoned());

        terminal.fail_point().set(Some("device_attributes"));
        terminal.push_pty_data(b"\x1b[c");
        assert!(terminal.poisoned(), "the panic latched the flag");
        assert!(
            drain(&writes).is_empty(),
            "the panicking reply wrote nothing"
        );

        // Later calls do nothing: the guard short-circuits while latched.
        terminal.fail_point().set(None);
        terminal.push_pty_data(b"\x1b[c");
        assert!(
            drain(&writes).is_empty(),
            "a poisoned terminal answers no attributes query"
        );
        assert!(terminal.poisoned());
    }

    /// The forwarder's own panic containment: a decoder that panics leaves
    /// the decode rejected and the terminal poisoned, without aborting.
    #[test]
    fn panicking_png_decoder_poisons_and_rejects_the_image() {
        struct PanickingDecoder;

        impl PngDecoder for PanickingDecoder {
            fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
                panic!("decoder panicked");
            }
        }

        let mut terminal = Terminal::new(Poisoned::new(), NullSink, PanickingDecoder, || {});
        assert!(terminal.push_size(40, 24, 8, 16));
        assert!(!terminal.poisoned());

        // Feed a kitty PNG transmission: the decode runs inside the
        // forwarder's guard.
        terminal.push_pty_data(b"\x1b_Ga=T,f=100,i=1,q=2;AAAA\x1b\\");
        assert!(terminal.poisoned(), "the decode panic latched the flag");

        // Later calls do nothing (task 4.2): the poisoned forwarder rejects
        // the decode instead of panicking into the crate a second time.
        let mut rejected = vt::ffi::SysImage {
            width: 0,
            height: 0,
            data: ptr::null_mut(),
            data_len: 0,
        };
        // SAFETY: `rejected` is writable and `data` is readable; the
        // forwarder resolves the live context itself.
        let ok = unsafe { decode_into(ptr::null_mut(), b"AAAA", &raw mut rejected) };
        assert!(!ok, "a poisoned terminal decodes nothing");
    }
}
