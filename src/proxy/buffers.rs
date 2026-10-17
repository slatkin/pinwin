//! The panel surface's buffer sizes (replace-terminal-with-app-host row
//! 10.1, D8): the bounds the viewport crop is clamped to.
//!
//! A `wp_viewport` source rectangle past the buffer it crops is the fatal
//! `out_of_buffer` error, so the proxy has to know how large the buffer it
//! crops is. Wayland carries a buffer's pixel size only in the request that
//! creates it — `wl_shm_pool.create_buffer`, or a
//! `zwp_linux_buffer_params_v1` `create`/`create_immed` — so the proxy
//! watches the app's `wl_shm` pools and dmabuf params, records each buffer's
//! size when it is created, and forgets it when the app destroys the buffer.
//! A buffer the proxy did not see created has no size, and the composition
//! then draws the whole buffer instead of guessing (D8).
//!
//! The surface's own scale is tracked here too, because the viewporter's
//! source rectangle is in the coordinates after the buffer scale: the crop
//! bound is the buffer's size divided by it. The panel surface's attached
//! buffer and scale are recorded by the commit coordinator
//! ([`super::commit`]), which sees the app's `attach` and
//! `set_buffer_scale`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{self, Write as _};
use std::os::fd::{FromRawFd, OwnedFd};
use std::rc::{Rc, Weak};

use wl_proxy::object::ObjectCoreApi;
use wl_proxy::protocols::linux_dmabuf_v1::zwp_linux_buffer_params_v1::{
    ZwpLinuxBufferParamsV1, ZwpLinuxBufferParamsV1Flags, ZwpLinuxBufferParamsV1Handler,
};
use wl_proxy::protocols::linux_dmabuf_v1::zwp_linux_dmabuf_v1::{
    ZwpLinuxDmabufV1, ZwpLinuxDmabufV1Handler,
};
use wl_proxy::protocols::wayland::wl_buffer::{WlBuffer, WlBufferHandler};
use wl_proxy::protocols::wayland::wl_shm::{WlShm, WlShmFormat, WlShmHandler};
use wl_proxy::protocols::wayland::wl_shm_pool::{WlShmPool, WlShmPoolHandler};

use super::output::Globals;

/// A fresh close-on-exec memfd named `name`, the backing store of a
/// proxy-owned `wl_shm` pool (D4, D9, D10): the reserve strip and the focus
/// ring both build one, so the descriptor creation lives here once.
///
/// # Errors
///
/// The error of `memfd_create`, such as running out of file descriptors.
pub(super) fn memfd(name: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: `memfd_create` takes a NUL-terminated name and flags and reads
    // no other pointer; a failure is a negative return, not a panic.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor the call above returned and nothing
    // else owns, so this `OwnedFd` is its only owner.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// An opaque `wl_shm` ARGB8888 buffer holding `pixels`, `width` by `height`
/// with `stride` bytes a row, in a memfd pool named `name` (D4, D9, D10). The
/// buffer belongs to the proxy, so its release has no client to go to.
///
/// # Errors
///
/// The error of `memfd_create` or of writing the pixels.
pub(super) fn argb_buffer(
    shm: &WlShm,
    name: &str,
    pixels: &[u8],
    (width, height, stride): (i32, i32, i32),
) -> io::Result<Rc<WlBuffer>> {
    let name = CString::new(name).map_err(io::Error::other)?;
    let mut file = std::fs::File::from(memfd(&name)?);
    file.write_all(pixels)?;
    let size = i32::try_from(pixels.len()).map_err(io::Error::other)?;
    let pool = shm.new_send_create_pool(&Rc::new(file.into()), size);
    let buffer = pool.new_send_create_buffer(0, width, height, stride, WlShmFormat::ARGB8888);
    buffer.set_forward_to_client(false);
    pool.send_destroy();
    Ok(buffer)
}

/// The buffer sizes one panel knows (D8): every buffer the app created
/// through `wl_shm` or `zwp_linux_dmabuf_v1`, the buffer the panel surface
/// attached last, and that surface's buffer scale.
pub(super) struct PanelBuffers {
    sizes: RefCell<HashMap<u64, (u32, u32)>>,
    panel_buffer: Cell<Option<u64>>,
    panel_scale: Cell<u32>,
}

impl PanelBuffers {
    /// No buffer seen yet, and the default scale of one.
    pub(super) fn new() -> Self {
        Self {
            sizes: RefCell::new(HashMap::new()),
            panel_buffer: Cell::new(None),
            panel_scale: Cell::new(1),
        }
    }

    /// Records the pixel size of a buffer the app just created (D8). A size
    /// the protocol does not allow is not a buffer, so it is not recorded:
    /// the composition then draws the whole buffer.
    pub(super) fn record(&self, buffer: &Rc<WlBuffer>, width: i32, height: i32) {
        let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        self.sizes
            .borrow_mut()
            .insert(buffer.unique_id(), (width, height));
    }

    /// Forgets a buffer the app destroyed, so a long-lived app's buffer churn
    /// does not grow the map. A buffer the panel surface attached calls this
    /// through the release handler too: that attach replaces the tracked
    /// handler (D6 amendment 3), and its entry must still leave (D8).
    pub(super) fn forget(&self, buffer: u64) {
        self.sizes.borrow_mut().remove(&buffer);
    }

    /// Records the buffer the panel surface attached (D8), or none when the
    /// app detached.
    pub(super) fn set_panel_buffer(&self, buffer: Option<u64>) {
        self.panel_buffer.set(buffer);
    }

    /// Records the panel surface's `wl_surface.set_buffer_scale` (D8): the
    /// viewporter's source rectangle is in the coordinates after it.
    pub(super) fn set_panel_scale(&self, scale: i32) {
        self.panel_scale
            .set(u32::try_from(scale).unwrap_or(1).max(1));
    }

    /// The pixel size of the buffer the panel surface attached last (D8):
    /// `None` while the app attached none, or when the proxy did not see it
    /// created.
    pub(super) fn panel_size(&self) -> Option<(u32, u32)> {
        let buffer = self.panel_buffer.get()?;
        self.sizes.borrow().get(&buffer).copied()
    }

    /// The panel surface's buffer scale, at least one.
    pub(super) fn panel_scale(&self) -> u32 {
        self.panel_scale.get()
    }
}

/// The app's `wl_shm` bind (D8): every pool the app creates is watched, so a
/// buffer's pixel size is known when the app attaches it.
pub(super) struct ClientWlShm {
    /// The `wl_shm` this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    shm: Weak<WlShm>,
    globals: Rc<Globals>,
}

impl ClientWlShm {
    /// The handler of the app's `wl_shm` bind.
    pub(super) fn new(shm: &Rc<WlShm>, globals: &Rc<Globals>) -> Self {
        Self {
            shm: Rc::downgrade(shm),
            globals: Rc::clone(globals),
        }
    }
}

impl WlShmHandler for ClientWlShm {
    fn handle_create_pool(
        &mut self,
        _slf: &Rc<WlShm>,
        id: &Rc<WlShmPool>,
        fd: &Rc<OwnedFd>,
        size: i32,
    ) {
        id.set_handler(ClientWlShmPool {
            pool: Rc::downgrade(id),
            globals: Rc::clone(&self.globals),
        });
        if let Some(shm) = self.shm.upgrade() {
            shm.send_create_pool(id, fd, size);
        }
    }
}

/// One app `wl_shm_pool` (D8): every buffer it creates carries its pixel
/// size.
struct ClientWlShmPool {
    /// The pool this handler answers, kept weak so the handler does not keep
    /// its own object alive.
    pool: Weak<WlShmPool>,
    globals: Rc<Globals>,
}

impl WlShmPoolHandler for ClientWlShmPool {
    fn handle_create_buffer(
        &mut self,
        _slf: &Rc<WlShmPool>,
        id: &Rc<WlBuffer>,
        offset: i32,
        width: i32,
        height: i32,
        stride: i32,
        format: WlShmFormat,
    ) {
        let buffers = self.globals.viewport().buffers();
        buffers.record(id, width, height);
        id.set_handler(TrackedBuffer {
            buffer: Rc::downgrade(id),
            globals: Rc::clone(&self.globals),
        });
        if let Some(pool) = self.pool.upgrade() {
            pool.send_create_buffer(id, offset, width, height, stride, format);
        }
    }
}

/// The app's `zwp_linux_dmabuf_v1` bind (D8): every params object it creates
/// carries the size of the buffer it makes.
pub(super) struct ClientDmabuf {
    /// The `zwp_linux_dmabuf_v1` this handler answers, kept weak so the
    /// handler does not keep its own object alive.
    dmabuf: Weak<ZwpLinuxDmabufV1>,
    globals: Rc<Globals>,
}

impl ClientDmabuf {
    /// The handler of the app's `zwp_linux_dmabuf_v1` bind.
    pub(super) fn new(dmabuf: &Rc<ZwpLinuxDmabufV1>, globals: &Rc<Globals>) -> Self {
        Self {
            dmabuf: Rc::downgrade(dmabuf),
            globals: Rc::clone(globals),
        }
    }
}

impl ZwpLinuxDmabufV1Handler for ClientDmabuf {
    fn handle_create_params(
        &mut self,
        _slf: &Rc<ZwpLinuxDmabufV1>,
        params_id: &Rc<ZwpLinuxBufferParamsV1>,
    ) {
        params_id.set_handler(ClientDmabufParams {
            params: Rc::downgrade(params_id),
            globals: Rc::clone(&self.globals),
            pending: Cell::new(None),
        });
        if let Some(dmabuf) = self.dmabuf.upgrade() {
            dmabuf.send_create_params(params_id);
        }
    }
}

/// One `zwp_linux_buffer_params_v1` of the app (D8). A `create` names the
/// buffer only in the compositor's `created` event, so its size waits there;
/// `create_immed` names the buffer itself.
struct ClientDmabufParams {
    /// The params object this handler answers, kept weak so the handler does
    /// not keep its own object alive.
    params: Weak<ZwpLinuxBufferParamsV1>,
    globals: Rc<Globals>,
    /// The size of a `create` request, until its `created` event names the
    /// buffer.
    pending: Cell<Option<(i32, i32)>>,
}

impl ZwpLinuxBufferParamsV1Handler for ClientDmabufParams {
    fn handle_create(
        &mut self,
        _slf: &Rc<ZwpLinuxBufferParamsV1>,
        width: i32,
        height: i32,
        format: u32,
        flags: ZwpLinuxBufferParamsV1Flags,
    ) {
        self.pending.set(Some((width, height)));
        if let Some(params) = self.params.upgrade() {
            params.send_create(width, height, format, flags);
        }
    }

    fn handle_create_immed(
        &mut self,
        _slf: &Rc<ZwpLinuxBufferParamsV1>,
        buffer_id: &Rc<WlBuffer>,
        width: i32,
        height: i32,
        format: u32,
        flags: ZwpLinuxBufferParamsV1Flags,
    ) {
        self.track(buffer_id, width, height);
        if let Some(params) = self.params.upgrade() {
            params.send_create_immed(buffer_id, width, height, format, flags);
        }
    }

    fn handle_created(&mut self, _slf: &Rc<ZwpLinuxBufferParamsV1>, buffer: &Rc<WlBuffer>) {
        if let Some((width, height)) = self.pending.take() {
            self.track(buffer, width, height);
        }
        if let Some(params) = self.params.upgrade() {
            params.send_created(buffer);
        }
    }
}

impl ClientDmabufParams {
    /// Records one dmabuf buffer's size and watches its destroy (D8).
    fn track(&self, buffer: &Rc<WlBuffer>, width: i32, height: i32) {
        self.globals
            .viewport()
            .buffers()
            .record(buffer, width, height);
        buffer.set_handler(TrackedBuffer {
            buffer: Rc::downgrade(buffer),
            globals: Rc::clone(&self.globals),
        });
    }
}

/// One app `wl_buffer` the proxy knows the size of (D8): its destroy frees
/// the entry. A buffer the panel surface attaches takes the release handler
/// instead ([`super::hide::BufferRelease`]), which frees the entry the same
/// way.
struct TrackedBuffer {
    /// The buffer this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    buffer: Weak<WlBuffer>,
    globals: Rc<Globals>,
}

impl WlBufferHandler for TrackedBuffer {
    fn handle_destroy(&mut self, _slf: &Rc<WlBuffer>) {
        let Some(buffer) = self.buffer.upgrade() else {
            return;
        };
        self.globals.viewport().buffers().forget(buffer.unique_id());
        forward_destroy(&buffer);
    }
}

/// Forwards a `wl_buffer.destroy` exactly as the generated default handler
/// does (D8): every proxy buffer handler forwards it after its own
/// bookkeeping.
pub(super) fn forward_destroy(buffer: &Rc<WlBuffer>) {
    ForwardBuffer.handle_destroy(buffer);
}

/// The `wl_buffer` requests without the size bookkeeping: what the proxy
/// forwards when it has watched the buffer.
struct ForwardBuffer;

impl WlBufferHandler for ForwardBuffer {}

#[cfg(test)]
mod tests {
    use super::PanelBuffers;
    use wl_proxy::baseline::Baseline;
    use wl_proxy::object::ObjectCoreApi;
    use wl_proxy::protocols::wayland::wl_buffer::WlBuffer;
    use wl_proxy::state::State;

    /// Two buffers recorded, looked up and forgotten (D8): the crop reads the
    /// attached buffer's size and a destroyed buffer's entry leaves.
    #[test]
    fn a_buffer_size_is_known_until_the_buffer_is_destroyed() {
        let state = object_state();
        let first = test_buffer(&state);
        let second = test_buffer(&state);
        let buffers = PanelBuffers::new();
        buffers.record(&first, 640, 480);
        buffers.record(&second, 0, 480);
        buffers.record(&second, 640, 0);
        buffers.set_panel_buffer(Some(first.unique_id()));

        assert_eq!(buffers.panel_size(), Some((640, 480)));

        buffers.forget(first.unique_id());
        assert_eq!(buffers.panel_size(), None, "a destroyed buffer is gone");
    }

    /// A detached surface has no buffer size, and its scale is at least one
    /// (D8): the composition reads both.
    #[test]
    fn a_surface_with_no_buffer_has_no_size() {
        let buffers = PanelBuffers::new();
        assert_eq!(buffers.panel_size(), None);
        assert_eq!(buffers.panel_scale(), 1);
        buffers.set_panel_scale(0);
        assert_eq!(buffers.panel_scale(), 1, "a zero scale is the default one");
        buffers.set_panel_scale(2);
        assert_eq!(buffers.panel_scale(), 2);
    }

    /// A client-side state the buffer-size tests mint their objects in: no
    /// server is attached, so nothing is sent and no compositor is needed
    /// (`port-to-rust` D10).
    fn object_state() -> std::rc::Rc<State> {
        State::builder(Baseline::V6)
            .without_server()
            .build()
            .expect("a client-side state needs no compositor")
    }

    /// A `wl_buffer` object that is never sent anywhere, for the size map.
    fn test_buffer(state: &std::rc::Rc<State>) -> std::rc::Rc<WlBuffer> {
        state.create_object::<WlBuffer>(1)
    }
}
