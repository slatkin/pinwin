//! The reserve surface (replace-terminal-with-app-host D9): the second,
//! transparent layer surface that carries the panel's exclusive zone.
//!
//! The panel surface itself claims no zone — it sets an exclusive zone of -1
//! (the spike's `Layout::PANEL_ZONE`) — so the compositor's tiling engine
//! listens to this surface alone. The strip it reserves is the [`HeldGap`]:
//! a pushing panel reserves `near + across + far` from its first frame, a
//! covering start reserves nothing until the first pushing apply moves the
//! gap. The surface is created and destroyed with the panel window (D9), so
//! the reservation lives and dies with the panel's own lifecycle.
//!
//! Ported in structure from the spike's `src/reserve.rs`, which followed
//! wl-paper's `apps/wl-paper/src/paper.rs`
//! (<https://github.com/mahkoh/wl-proxy>, GPL-3.0, the licence of this
//! crate); the strip and zone math is pinwin's own [`crate::geometry`].

use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::io::{self, Write as _};
use std::os::fd::FromRawFd;
use std::rc::{Rc, Weak};

use wl_proxy::object::ObjectCoreApi;
use wl_proxy::protocols::wayland::wl_buffer::WlBuffer;
use wl_proxy::protocols::wayland::wl_compositor::WlCompositor;
use wl_proxy::protocols::wayland::wl_shm::{WlShm, WlShmFormat};
use wl_proxy::protocols::wayland::wl_surface::WlSurface;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::{
    ZwlrLayerShellV1, ZwlrLayerShellV1Layer,
};
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::{
    ZwlrLayerSurfaceV1, ZwlrLayerSurfaceV1Handler, ZwlrLayerSurfaceV1KeyboardInteractivity,
};

use super::mapping::anchor_of;
use crate::geometry::HeldGap;

/// The layer-shell namespace of the reserve. Compositors show it in their
/// logs and rules; it is not a visible name.
const RESERVE_NAMESPACE: &str = "pinwin-reserve";

/// The reserve's buffer is 1x1: the compositor scales it to the strip, and
/// a transparent pixel stays transparent at any size.
const BUFFER_SIDE: i32 = 1;

/// The reserve buffer's byte size: one ARGB8888 pixel, four bytes.
const BUFFER_BYTES: i32 = BUFFER_SIDE * 4;

/// A transparent ARGB8888 pixel: alpha 0, red 0, green 0, blue 0.
const TRANSPARENT_PIXEL: [u8; 4] = [0, 0, 0, 0];

/// The proxy-created reserve layer surface (D9). It is always transparent,
/// never takes input, and carries the exclusive zone the panel surface does
/// not claim.
pub(super) struct Reserve {
    surface: Rc<WlSurface>,
    layer_surface: Rc<ZwlrLayerSurfaceV1>,
    shm: Rc<WlShm>,
    /// The 1x1 transparent buffer, attached on the first configure.
    buffer: RefCell<Option<Rc<WlBuffer>>>,
    /// Whether the initial configure has been acked and the buffer attached.
    mapped: Cell<bool>,
}

impl Reserve {
    /// Creates the reserve surface and commits it without a buffer, which is
    /// what the layer-shell protocol requires before the first configure.
    /// Its size, zone and anchors come from the held gap the panel starts
    /// with — a pushing panel reserves its strip from the first frame, a
    /// covering start reserves nothing (zero size, zero zone). The buffer
    /// attach itself waits for the first configure, where a failure only
    /// leaves the strip unmapped.
    pub(super) fn create(
        compositor: &Rc<WlCompositor>,
        shm: &Rc<WlShm>,
        layer_shell: &Rc<ZwlrLayerShellV1>,
        gap: HeldGap,
        layer: ZwlrLayerShellV1Layer,
    ) -> Rc<Self> {
        let surface = compositor.new_send_create_surface();
        surface.set_forward_to_client(false);
        // An empty input region: clicks on the strip go to whatever is below.
        let region = compositor.new_send_create_region();
        surface.send_set_input_region(Some(&region));
        region.send_destroy();
        let layer_surface =
            layer_shell.new_send_get_layer_surface(&surface, None, layer, RESERVE_NAMESPACE);
        let reserve = Rc::new(Self {
            surface: Rc::clone(&surface),
            layer_surface: Rc::clone(&layer_surface),
            shm: Rc::clone(shm),
            buffer: RefCell::new(None),
            mapped: Cell::new(false),
        });
        layer_surface.set_handler(ProxyReserveLayerSurface {
            reserve: Rc::downgrade(&reserve),
        });
        let (width, height) = gap.size();
        layer_surface.send_set_size(width, height);
        layer_surface.send_set_anchor(anchor_of(gap.anchors()));
        // The strip starts at the output edge: no margins (spike reserve).
        layer_surface.send_set_margin(0, 0, 0, 0);
        layer_surface.send_set_exclusive_zone(gap.zone());
        // The reserve never takes keyboard focus, whatever the panel asks for.
        layer_surface
            .send_set_keyboard_interactivity(ZwlrLayerSurfaceV1KeyboardInteractivity::NONE);
        surface.send_commit();
        reserve
    }

    /// Applies a held gap to the live surface: the strip's size and the
    /// exclusive zone move together, and the proxy commits its own surface
    /// so the compositor applies both at once.
    pub(super) fn apply_gap(&self, gap: HeldGap) {
        let (width, height) = gap.size();
        self.layer_surface.send_set_size(width, height);
        self.layer_surface.send_set_exclusive_zone(gap.zone());
        self.surface.send_commit();
    }

    /// Destroys the reserve with the panel window it was created for (D9):
    /// the zone goes away with the surface.
    pub(super) fn destroy(&self) {
        self.layer_surface.send_destroy();
        self.surface.send_destroy();
    }

    /// Acks a configure and, on the first one, attaches the transparent
    /// buffer and maps the surface.
    fn configure(&self, serial: u32) {
        self.layer_surface.send_ack_configure(serial);
        if self.mapped.get() {
            return;
        }
        self.mapped.set(true);
        let buffer = match self.transparent_buffer() {
            Ok(buffer) => buffer,
            Err(error) => {
                // The strip stays unmapped: with no buffer the surface shows
                // nothing, and the zone it already carries still holds.
                eprintln!("pinwin: could not create the reserve buffer: {error}");
                return;
            }
        };
        *self.buffer.borrow_mut() = Some(Rc::clone(&buffer));
        self.surface.send_attach(Some(&buffer), 0, 0);
        self.surface
            .send_damage_buffer(0, 0, BUFFER_SIDE, BUFFER_SIDE);
        self.surface.send_commit();
    }

    /// The transparent buffer, created on first use: a memfd holding one
    /// transparent ARGB8888 pixel, wrapped in a `wl_shm` pool.
    fn transparent_buffer(&self) -> io::Result<Rc<WlBuffer>> {
        let name = CString::new("pinwin-reserve").map_err(io::Error::other)?;
        // SAFETY: `memfd_create` takes a NUL-terminated name and flags and
        // reads no other pointer; a failure is a negative return, not a
        // panic.
        let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh descriptor the call above returned and
        // nothing else owns, so this `OwnedFd` is its only owner.
        let mut file = std::fs::File::from(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
        file.write_all(&TRANSPARENT_PIXEL)?;
        let pool = self
            .shm
            .new_send_create_pool(&Rc::new(file.into()), BUFFER_BYTES);
        let buffer = pool.new_send_create_buffer(
            0,
            BUFFER_SIDE,
            BUFFER_SIDE,
            BUFFER_BYTES,
            WlShmFormat::ARGB8888,
        );
        // The reserve's buffer belongs to the proxy, so its release has no
        // client to go to.
        buffer.set_forward_to_client(false);
        pool.send_destroy();
        Ok(buffer)
    }
}

/// The proxy's reserve layer surface handler: it acks configures and maps
/// the surface on the first one. `closed` is a compositor-side teardown
/// (for example the output going away); row 6.5 owns recreating surfaces on
/// a new output, so until then the strip is simply unreserved.
struct ProxyReserveLayerSurface {
    /// The reserve this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    reserve: Weak<Reserve>,
}

impl ZwlrLayerSurfaceV1Handler for ProxyReserveLayerSurface {
    fn handle_configure(
        &mut self,
        _slf: &Rc<ZwlrLayerSurfaceV1>,
        serial: u32,
        _width: u32,
        _height: u32,
    ) {
        if let Some(reserve) = self.reserve.upgrade() {
            reserve.configure(serial);
        }
    }

    fn handle_closed(&mut self, _slf: &Rc<ZwlrLayerSurfaceV1>) {
        eprintln!("pinwin: the compositor closed a reserve surface");
    }
}
