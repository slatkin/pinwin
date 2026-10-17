//! The panel's focus ring (replace-terminal-with-app-host row 9.1, D10): four
//! desynchronized `wl_subsurface`s of the panel window's surface, one per
//! side, each an opaque `wl_shm` bar of the configured colour and width with
//! an empty input region, so a click on the ring reaches the app.
//!
//! The ring shows while the panel holds keyboard focus and goes away when it
//! does not (spec "Accent appears while focused"); a hidden panel shows no
//! ring (spec "Hide"), and a ring setting of `off` builds no objects at all
//! (spec "Accent disabled"). The keyboard focus is the app's own
//! `wl_keyboard.enter` and `leave` for the panel surface, watched on the seat
//! the app binds.
//!
//! The bars follow the size the compositor configures the panel window with,
//! which is the size the app draws at. A bar position is double-buffered
//! state of the parent surface, so a position change rides the commit
//! coordinator (D7); showing and hiding only commit the ring's own
//! desynchronized surfaces (D10), and a bar that does not fit a degenerate
//! panel is empty and draws nothing. The ring objects are created with the
//! first configure and destroyed with the panel window; a re-dock keeps them,
//! because the app's surface, their parent, survives it.
//!
//! The bars ([`bars`]) and the visibility state ([`RingState`]) are pure, so
//! their tests run without a compositor (`port-to-rust` D10).

use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::io::{self, Write as _};
use std::os::fd::FromRawFd;
use std::rc::{Rc, Weak};

use wl_proxy::object::{ObjectCoreApi, ObjectUtils};
use wl_proxy::protocols::wayland::wl_buffer::WlBuffer;
use wl_proxy::protocols::wayland::wl_compositor::WlCompositor;
use wl_proxy::protocols::wayland::wl_keyboard::{WlKeyboard, WlKeyboardHandler};
use wl_proxy::protocols::wayland::wl_seat::{WlSeat, WlSeatHandler};
use wl_proxy::protocols::wayland::wl_shm::{WlShm, WlShmFormat};
use wl_proxy::protocols::wayland::wl_subcompositor::WlSubcompositor;
use wl_proxy::protocols::wayland::wl_subsurface::WlSubsurface;
use wl_proxy::protocols::wayland::wl_surface::WlSurface;

use super::commit::PanelSurfaceCommits;
use super::routing::Routing;
use crate::config::{FocusRing, Rgb};

/// The bytes of one ARGB8888 pixel, the ring buffer's format.
const BYTES_PER_PIXEL: u32 = 4;

/// The name the ring's `wl_shm` files carry; it is not a visible name.
const SHM_NAME: &str = "pinwin-ring";

/// One bar of the focus ring in the panel surface's coordinates (D10): the
/// top, right, bottom and left sides, without overlapping corners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Bar {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl Bar {
    /// Whether the bar draws nothing: a panel smaller than the ring width
    /// leaves a side no room.
    pub(super) const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// The four bars of a ring of `width` logical pixels around a panel of `size`
/// (D10, spec "Accent appears while focused"): top, right, bottom, left, each
/// covering its whole side but not the corners, which belong to the
/// perpendicular bars.
///
/// A bar is clipped to the panel, so a size smaller than twice the ring
/// width still gives four disjoint rectangles: the top and bottom bars share
/// the height, and a side with no room in between is empty.
pub(super) fn bars(size: (u32, u32), width: u32) -> [Bar; 4] {
    let (panel_width, panel_height) = size;
    let top = width.min(panel_height);
    let bottom = width.min(panel_height - top);
    let bottom_y = panel_height - bottom;
    let left = width.min(panel_width);
    let right = width.min(panel_width - left);
    let sides = panel_height - top - bottom;
    [
        Bar {
            x: 0,
            y: 0,
            width: panel_width,
            height: top,
        },
        Bar {
            x: (panel_width - right).cast_signed(),
            y: top.cast_signed(),
            width: right,
            height: sides,
        },
        Bar {
            x: 0,
            y: bottom_y.cast_signed(),
            width: panel_width,
            height: bottom,
        },
        Bar {
            x: 0,
            y: top.cast_signed(),
            width: left,
            height: sides,
        },
    ]
}

/// The panel's keyboard-focus and hidden state and whether a ring is set
/// (spec "Accent appears while focused", "Accent disabled"): the ring is
/// visible only while it is set, the panel holds the keyboard and the panel
/// is not hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RingState {
    enabled: bool,
    focused: bool,
    hidden: bool,
}

impl RingState {
    /// The state of a panel whose ring setting and hidden flag are known.
    pub(super) const fn new(enabled: bool, hidden: bool) -> Self {
        Self {
            enabled,
            focused: false,
            hidden,
        }
    }

    /// Whether the ring should be on the panel now.
    pub(super) const fn is_visible(self) -> bool {
        self.enabled && self.focused && !self.hidden
    }

    /// Takes a fresh ring setting and the panel's hidden state; hiding gives
    /// up the focus, so a later show cannot resurrect a stale one. Returns
    /// whether the visibility changed.
    pub(super) fn sync(&mut self, enabled: bool, hidden: bool) -> bool {
        let before = self.is_visible();
        self.enabled = enabled;
        self.hidden = hidden;
        if hidden || !enabled {
            self.focused = false;
        }
        before != self.is_visible()
    }

    /// A keyboard enter or leave for the panel surface; returns whether the
    /// visibility changed. Focus cannot latch while the ring is disabled or
    /// the panel hidden.
    pub(super) fn focus(&mut self, focused: bool) -> bool {
        let before = self.is_visible();
        self.focused = focused && self.enabled && !self.hidden;
        before != self.is_visible()
    }
}

/// The panel window ended (D10).
pub(super) fn window_destroyed(routing: &Routing) {
    routing.globals().ring().window_destroyed(routing);
}

/// The per-panel focus ring (D10): the ring objects once the panel's size is
/// known, the compositor-configured size their bars follow, and the
/// visibility state. It lives in [`Globals`](super::output::Globals), which
/// the routing owns for the panel's life, so a re-dock keeps the ring and the
/// window's end destroys it.
pub(super) struct RingSlot {
    ring: RefCell<Option<Rc<Ring>>>,
    size: Cell<Option<(u32, u32)>>,
    state: Cell<RingState>,
}

impl RingSlot {
    pub(super) fn new() -> Self {
        Self {
            ring: RefCell::new(None),
            size: Cell::new(None),
            state: Cell::new(RingState::default()),
        }
    }

    /// The compositor configured the panel window with `size` (D10): the
    /// first configure creates the ring and every size change moves its bars.
    /// A bar position is parent-surface state, so the change goes through the
    /// commit coordinator (D7).
    pub(super) fn on_configure(&self, routing: &Routing, size: (u32, u32)) {
        self.size.set(Some(size));
        let ring = if let Some(ring) = self.ring.borrow().clone() {
            ring
        } else {
            let Some(ring) = self.ensure(routing) else {
                return;
            };
            // The app commits after the configure that mapped the window, so
            // the new sub-surfaces join the parent with that commit; the
            // ring starts hidden because no keyboard focus exists yet.
            ring
        };
        if ring.size() == size {
            return;
        }
        let Some((surface, _, _)) = routing.panel_window() else {
            return;
        };
        let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>() else {
            return;
        };
        commits.request(Box::new(move || ring.resize(size)));
    }

    /// A keyboard enter or leave for the panel surface (D10, spec "Accent
    /// appears while focused"): the ring shows while the panel has the
    /// keyboard and hides when it goes away.
    pub(super) fn on_focus(&self, routing: &Routing, focused: bool) {
        if !self.update(routing, Some(focused)) {
            return;
        }
        self.paint(routing);
    }

    /// The panel's hidden state changed (spec "Hide"): a hidden panel shows
    /// no ring, and its focus is given up.
    pub(super) fn on_hidden(&self, routing: &Routing) {
        if !self.update(routing, None) {
            return;
        }
        self.paint(routing);
    }

    /// The panel window ended (D10): the ring objects go with it, and the
    /// next window starts without a stale focus.
    pub(super) fn window_destroyed(&self, routing: &Routing) {
        if let Some(ring) = self.ring.borrow_mut().take() {
            ring.destroy();
        }
        self.state
            .set(RingState::new(enabled(routing), routing.hidden.is_hidden()));
    }

    /// Syncs the state with the routing and, when asked, one focus change.
    /// Returns whether the ring's visibility changed.
    fn update(&self, routing: &Routing, focused: Option<bool>) -> bool {
        let mut state = self.state.get();
        let mut changed = state.sync(enabled(routing), routing.hidden.is_hidden());
        if let Some(focused) = focused {
            changed |= state.focus(focused);
        }
        self.state.set(state);
        changed
    }

    /// Applies the state to the ring: shows it, hides it, or builds it late
    /// when a global arrived after the first configure.
    fn paint(&self, routing: &Routing) {
        let visible = self.state.get().is_visible();
        let mut created = false;
        if visible && self.ring.borrow().is_none() {
            created = self.ensure(routing).is_some();
        }
        let Some(ring) = self.ring.borrow().clone() else {
            return;
        };
        if !created {
            if visible {
                ring.show();
            } else {
                ring.hide();
            }
            return;
        }
        // A ring built after the configure sent its positions and buffers
        // already, but its sub-surfaces join the parent only with a parent
        // commit (D10). The focus is what made the ring appear, so the
        // coordinator places that commit (D7) instead of waiting for a
        // commit the app may not send.
        let Some((surface, _, _)) = routing.panel_window() else {
            return;
        };
        let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>() else {
            return;
        };
        commits.request(Box::new(move || ring.show()));
    }

    /// Creates the ring for the configured size, when the ring is set, the
    /// size is known and the proxy has the globals the ring is built from.
    fn ensure(&self, routing: &Routing) -> Option<Rc<Ring>> {
        let FocusRing::On { color, width } = routing.settings().focus_ring() else {
            return None;
        };
        let size = self.size.get()?;
        let compositor = routing.globals().compositor()?;
        let shm = routing.globals().shm()?;
        let subcompositor = routing.globals().subcompositor()?;
        let (parent, _, _) = routing.panel_window()?;
        let ring = Ring::create(
            &compositor,
            &shm,
            &subcompositor,
            &parent,
            size,
            color,
            width.get(),
        );
        *self.ring.borrow_mut() = Some(Rc::clone(&ring));
        Some(ring)
    }
}

/// The panel's hidden state changed (spec "Hide"): a hidden panel shows no
/// ring. The caller has already stored the new state in the routing.
pub(super) fn on_panel_hidden(routing: &Routing) {
    routing.globals().ring().on_hidden(routing);
}

/// Whether the panel's settings carry a ring at all (spec "Accent
/// disabled"): `off` builds no objects.
fn enabled(routing: &Routing) -> bool {
    matches!(routing.settings().focus_ring(), FocusRing::On { .. })
}

/// The panel window's `wl_seat` handler (D10): it puts the focus watch on
/// every keyboard the app asks the seat for and forwards the request
/// unchanged, so the app's own keyboard objects keep working.
pub(super) struct ClientWlSeat {
    routing: Rc<Routing>,
    /// The seat this handler answers, kept weak so the handler does not keep
    /// its own object alive (the routing's handler convention); the request
    /// it forwards goes through it.
    seat: Weak<WlSeat>,
}

impl ClientWlSeat {
    pub(super) fn new(routing: &Rc<Routing>, seat: &Rc<WlSeat>) -> Self {
        Self {
            routing: Rc::clone(routing),
            seat: Rc::downgrade(seat),
        }
    }
}

impl WlSeatHandler for ClientWlSeat {
    fn handle_get_keyboard(&mut self, _slf: &Rc<WlSeat>, id: &Rc<WlKeyboard>) {
        id.set_handler(PanelKeyboard {
            routing: Rc::clone(&self.routing),
            keyboard: Rc::downgrade(id),
        });
        if let Some(seat) = self.seat.upgrade() {
            WlSeatHandler::handle_get_keyboard(&mut ForwardSeat, &seat, id);
        }
    }
}

/// The generated default `wl_seat` handler (D10): what the seat does without
/// the focus watch, which is to forward the app's requests to the
/// compositor.
struct ForwardSeat;

impl WlSeatHandler for ForwardSeat {}

/// One app `wl_keyboard` (D10): it reports the keyboard focus of the panel
/// surface to the ring and forwards the events to the app, as the generated
/// default handler would.
struct PanelKeyboard {
    routing: Rc<Routing>,
    /// The keyboard this handler answers, kept weak so the handler does not
    /// keep its own object alive; the event it forwards goes through it.
    keyboard: Weak<WlKeyboard>,
}

impl PanelKeyboard {
    /// Reports `focused` for `surface`, when it is the panel window: a
    /// keyboard focus on a passed-through window does not touch the ring.
    fn report(&self, surface: &WlSurface, focused: bool) {
        if !self.routing.is_panel_surface(surface.unique_id()) {
            return;
        }
        self.routing
            .globals()
            .ring()
            .on_focus(&self.routing, focused);
    }
}

impl WlKeyboardHandler for PanelKeyboard {
    fn handle_enter(
        &mut self,
        _slf: &Rc<WlKeyboard>,
        serial: u32,
        surface: &Rc<WlSurface>,
        keys: &[u8],
    ) {
        self.report(surface, true);
        let Some(keyboard) = self.keyboard.upgrade() else {
            return;
        };
        WlKeyboardHandler::handle_enter(&mut ForwardKeyboard, &keyboard, serial, surface, keys);
    }

    fn handle_leave(&mut self, _slf: &Rc<WlKeyboard>, serial: u32, surface: &Rc<WlSurface>) {
        self.report(surface, false);
        let Some(keyboard) = self.keyboard.upgrade() else {
            return;
        };
        WlKeyboardHandler::handle_leave(&mut ForwardKeyboard, &keyboard, serial, surface);
    }
}

/// The generated default `wl_keyboard` handler (D10): the focus watch's
/// forwarding path for the events it does not change.
struct ForwardKeyboard;

impl WlKeyboardHandler for ForwardKeyboard {}

/// The panel window's ring (D10): four bars, each one `wl_subsurface` of the
/// app's panel surface with its own desynchronized surface and opaque
/// buffer.
struct Ring {
    bars: [BarObject; 4],
    /// The ring width in logical pixels, which the setting fixes.
    width: u32,
    color: Rgb,
    shm: Rc<WlShm>,
    size: Cell<(u32, u32)>,
    visible: Cell<bool>,
}

impl Ring {
    /// Creates the four bar surfaces of a ring around a panel of `size`.
    /// Positions are parent-surface state and apply with the app's next
    /// commit; the first buffer attach is the show that follows the first
    /// keyboard focus.
    fn create(
        compositor: &Rc<WlCompositor>,
        shm: &Rc<WlShm>,
        subcompositor: &Rc<WlSubcompositor>,
        parent: &Rc<WlSurface>,
        size: (u32, u32),
        color: Rgb,
        width: u32,
    ) -> Rc<Self> {
        let bars = bars(size, width)
            .map(|bar| BarObject::create(compositor, subcompositor, shm, parent, bar, color));
        Rc::new(Self {
            bars,
            width,
            color,
            shm: Rc::clone(shm),
            size: Cell::new(size),
            visible: Cell::new(false),
        })
    }

    /// The size the bars are laid out for.
    fn size(&self) -> (u32, u32) {
        self.size.get()
    }

    /// Lays the bars out for `size` and replaces the buffers whose bar
    /// changed shape (D10). Runs inside a commit-coordinator change (D7), so
    /// the new positions apply with the parent's next commit while the
    /// desynchronized bars commit their own surfaces. The ring stays as
    /// visible as it was when the change runs, not when it was requested.
    fn resize(&self, size: (u32, u32)) {
        for (object, bar) in self.bars.iter().zip(bars(size, self.width)) {
            object.resize(&self.shm, bar, self.color);
        }
        self.size.set(size);
        if self.visible.get() {
            self.show();
        } else {
            self.hide();
        }
    }

    /// Shows the ring: each non-empty bar attaches its buffer and commits its
    /// own desynchronized surface, so no commit on the app's surface is
    /// needed (D10).
    fn show(&self) {
        self.visible.set(true);
        for bar in &self.bars {
            bar.attach();
        }
    }

    /// Hides the ring: each bar attaches a null buffer and commits its own
    /// surface (D10).
    fn hide(&self) {
        self.visible.set(false);
        for bar in &self.bars {
            bar.detach();
        }
    }

    /// Destroys the ring objects with the panel window (D10).
    fn destroy(&self) {
        for bar in &self.bars {
            bar.destroy();
        }
    }
}

/// One bar's protocol objects (D10): the proxy-created `wl_surface` with its
/// empty input region, the desynchronized `wl_subsurface` of the panel
/// surface, and the opaque buffer of the bar's current shape.
struct BarObject {
    surface: Rc<WlSurface>,
    subsurface: Rc<WlSubsurface>,
    buffer: RefCell<Option<Rc<WlBuffer>>>,
    size: Cell<(u32, u32)>,
    attached: Cell<bool>,
}

impl BarObject {
    /// Creates the bar's surface, puts it in the parent's subsurface stack
    /// above the app, gives it an empty input region and, when the bar has a
    /// shape, its buffer. A failed buffer leaves the bar invisible instead of
    /// ending the panel.
    fn create(
        compositor: &Rc<WlCompositor>,
        subcompositor: &Rc<WlSubcompositor>,
        shm: &Rc<WlShm>,
        parent: &Rc<WlSurface>,
        bar: Bar,
        color: Rgb,
    ) -> Self {
        let surface = compositor.new_send_create_surface();
        surface.set_forward_to_client(false);
        // D10: an empty input region, so a click on the ring reaches the app.
        let region = compositor.new_send_create_region();
        surface.send_set_input_region(Some(&region));
        region.send_destroy();
        let subsurface = subcompositor.new_send_get_subsurface(&surface, parent);
        subsurface.set_forward_to_client(false);
        // D10: showing or hiding the ring needs no commit on the app's
        // surface, and the bar's own commit applies its state.
        subsurface.send_set_desync();
        let object = Self {
            surface,
            subsurface,
            buffer: RefCell::new(None),
            size: Cell::new((bar.width, bar.height)),
            attached: Cell::new(false),
        };
        object.subsurface.send_set_position(bar.x, bar.y);
        object.replace_buffer(shm, bar, color);
        object
    }

    /// Takes the bar's new shape: a position (parent-surface state) and,
    /// when the bar changed size, a new buffer. A bar that becomes empty
    /// detaches first, so no old buffer outlives its shape.
    fn resize(&self, shm: &Rc<WlShm>, bar: Bar, color: Rgb) {
        self.subsurface.send_set_position(bar.x, bar.y);
        if self.size.get() == (bar.width, bar.height) {
            return;
        }
        self.detach();
        self.destroy_buffer();
        self.size.set((bar.width, bar.height));
        self.replace_buffer(shm, bar, color);
    }

    /// Creates the bar's opaque buffer for `bar`'s shape, unless the bar is
    /// empty.
    fn replace_buffer(&self, shm: &Rc<WlShm>, bar: Bar, color: Rgb) {
        if bar.is_empty() {
            return;
        }
        match solid_buffer(shm, bar.width, bar.height, color) {
            Ok(buffer) => *self.buffer.borrow_mut() = Some(buffer),
            Err(error) => eprintln!("pinwin: could not create the focus ring's buffer: {error}"),
        }
    }

    /// Attaches the bar's buffer and commits the bar's own surface (D10).
    fn attach(&self) {
        let Some(buffer) = self.buffer.borrow().clone() else {
            return;
        };
        let (width, height) = self.size.get();
        self.surface.send_attach(Some(&buffer), 0, 0);
        self.surface
            .send_damage_buffer(0, 0, width.cast_signed(), height.cast_signed());
        self.surface.send_commit();
        self.attached.set(true);
    }

    /// Attaches a null buffer and commits the bar's own surface (D10).
    fn detach(&self) {
        if !self.attached.replace(false) {
            return;
        }
        self.surface.send_attach(None, 0, 0);
        self.surface.send_commit();
    }

    /// Destroys the bar's objects with the panel window (D10).
    fn destroy(&self) {
        self.subsurface.send_destroy();
        self.surface.send_destroy();
        self.destroy_buffer();
    }

    fn destroy_buffer(&self) {
        if let Some(buffer) = self.buffer.borrow_mut().take() {
            buffer.send_destroy();
        }
    }
}

/// An opaque ARGB8888 buffer of one colour, `width` by `height` logical
/// pixels, in a `wl_shm` pool the proxy owns (D10).
fn solid_buffer(shm: &Rc<WlShm>, width: u32, height: u32, color: Rgb) -> io::Result<Rc<WlBuffer>> {
    let stride = width
        .checked_mul(BYTES_PER_PIXEL)
        .ok_or_else(|| io::Error::other("the ring bar is too wide"))?;
    let bytes = stride
        .checked_mul(height)
        .ok_or_else(|| io::Error::other("the ring bar is too large"))?;
    let length = usize::try_from(bytes).map_err(io::Error::other)?;
    let name = CString::new(SHM_NAME).map_err(io::Error::other)?;
    // SAFETY: `memfd_create` takes a NUL-terminated name and flags and reads
    // no other pointer; a failure is a negative return, not a panic.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor the call above returned and nothing
    // else owns, so this `OwnedFd` is its only owner.
    let mut file = std::fs::File::from(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
    let [red, green, blue] = color.channels();
    let pixel = [blue, green, red, u8::MAX];
    let mut data = vec![0_u8; length];
    for chunk in data.chunks_exact_mut(usize::try_from(BYTES_PER_PIXEL).unwrap_or(4)) {
        chunk.copy_from_slice(&pixel);
    }
    file.write_all(&data)?;
    let pool = shm.new_send_create_pool(&Rc::new(file.into()), bytes.cast_signed());
    let buffer = pool.new_send_create_buffer(
        0,
        width.cast_signed(),
        height.cast_signed(),
        stride.cast_signed(),
        WlShmFormat::ARGB8888,
    );
    // The ring's buffer belongs to the proxy, so its release has no client
    // to go to.
    buffer.set_forward_to_client(false);
    pool.send_destroy();
    Ok(buffer)
}

// The path is explicit because `src/proxy.rs` includes this file with
// `#[path]`, so a child module searches in `src/proxy/` rather than beside
// this file (as in `src/daemon/events.rs`).
#[cfg(test)]
#[path = "ring/tests.rs"]
mod tests;
