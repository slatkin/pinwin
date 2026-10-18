//! The panel window's viewport (replace-terminal-with-app-host D5, D8): the
//! proxy-owned `wp_viewport` that carries the composed crop on the panel
//! surface, and the app viewports the proxy keeps local.
//!
//! A GTK3 app draws its drop shadow into its buffer and reports where the
//! real window sits with `xdg_surface.set_window_geometry`; a compositor
//! draws a layer surface's buffer at the buffer's own size, so without a
//! crop the shadow is a gap at the docked edge and an overhang at the far
//! one (D5). The proxy therefore owns the panel surface's viewport: its
//! source is the window geometry and its destination the geometry's own
//! size, so the window — not the buffer — fills the panel, one pixel to one
//! pixel.
//!
//! D8's one-viewport rule: a surface has at most one `wp_viewport`, and a
//! second is the fatal `viewport_exists` error. The proxy therefore creates
//! its own only on a surface that has no upstream one, and every app
//! viewport stays local: on the panel surface its source and destination are
//! composed into the proxy's values ([`super::compose`]), on any other
//! surface they are mirrored upstream unchanged, and a viewport created
//! before the window's role is known waits, because that surface may be the
//! panel's.
//!
//! The values are double-buffered viewport state: they apply with the app's
//! next commit, so the proxy sends no commit of its own here (D7). The
//! buffer sizes the composition is clamped to are [`super::buffers`]. A
//! surface may have only one `wp_fractional_scale_v1` too, so the proxy
//! watches the app's object for the panel surface's preferred scale and
//! never creates one of its own.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use wl_proxy::fixed::Fixed;
use wl_proxy::object::ObjectCoreApi;
use wl_proxy::protocols::fractional_scale_v1::wp_fractional_scale_manager_v1::{
    WpFractionalScaleManagerV1, WpFractionalScaleManagerV1Handler,
};
use wl_proxy::protocols::fractional_scale_v1::wp_fractional_scale_v1::{
    WpFractionalScaleV1, WpFractionalScaleV1Handler,
};
use wl_proxy::protocols::viewporter::wp_viewport::{WpViewport, WpViewportHandler};
use wl_proxy::protocols::viewporter::wp_viewporter::{WpViewporter, WpViewporterHandler};
use wl_proxy::protocols::wayland::wl_surface::WlSurface;

use super::buffers::PanelBuffers;
use super::compose::{AppViewport, Compose, Composition, Fraction, UNSET_FIXED, compose};
use super::output::Globals;
use super::routing::Routing;
use crate::geometry::Geometry;

/// The viewport values the proxy puts on the panel surface (D5, D8): the
/// source rectangle in the app's buffer coordinates — `None` draws the whole
/// buffer — and the destination it is drawn at, `None` when the crop is
/// removed. Built only by [`StaticCrop::composed`] and
/// [`StaticCrop::cleared`] (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StaticCrop {
    source: Option<[i32; 4]>,
    destination: Option<(i32, i32)>,
}

impl StaticCrop {
    /// The crop a composition decided (D8).
    fn composed(composition: Composition) -> Self {
        Self {
            source: composition.source(),
            destination: Some(composition.destination()),
        }
    }

    /// The removal of the crop: the app's own viewport went away, so the
    /// whole buffer draws at its own size again.
    fn cleared() -> Self {
        Self {
            source: None,
            destination: None,
        }
    }

    /// The source rectangle, in the viewporter's coordinates.
    fn source(self) -> Option<[i32; 4]> {
        self.source
    }

    /// The destination size, in surface-local pixels.
    fn destination(self) -> Option<(i32, i32)> {
        self.destination
    }
}

/// Applies the panel window's steady-state crop (D5, D8): the window
/// geometry the app reported, the app's own viewport mapping and the buffer
/// it attached, composed into the proxy's viewport. Called when the app sets
/// a window geometry and when the panel window takes the slot.
pub(super) fn apply_steady_crop(routing: &Routing, geometry: Option<[i32; 4]>) {
    routing.globals().viewport().set_window(geometry);
    settle_app_viewports(routing);
    recompose(routing);
}

/// Writes the values the panel's inputs decide now into the proxy's viewport
/// (D8): the source and destination of the composed crop. A change is only
/// sent when it differs from what the proxy wrote last, and nothing is
/// created while the buffer size is unknown, because a source the proxy
/// cannot bound is the fatal `out_of_buffer`.
pub(super) fn recompose(routing: &Routing) {
    let Some((surface, _, _)) = routing.panel_window() else {
        return;
    };
    let surface_id = surface.unique_id();
    let globals = routing.globals();
    let state = globals.viewport();
    let geometry = routing.geometry();
    let window = state.window();
    let has_app_viewport = routing.panel_has_app_viewport();
    let crop = if window.is_some() || has_app_viewport || routing.tween_surface().is_some() {
        let surface = surface_extent(
            window,
            routing.tween_surface().unwrap_or_else(|| geometry.size()),
        );
        let fraction = routing
            .tween_fraction()
            .unwrap_or_else(|| steady_fraction(geometry, surface));
        let Some(composition) = compose(&Compose {
            edge: geometry.edge(),
            // D8: the running tween's visible fraction; a change that is not
            // animated shows the panel's part of the surface.
            fraction,
            surface,
            origin: window.map_or((0, 0), |[x, y, _, _]| (x, y)),
            app: state.app(surface_id),
            preferred_scale: state.preferred_scale(surface_id),
            buffer: state.buffers().panel_size(),
            buffer_scale: state.buffers().panel_scale(),
        }) else {
            return;
        };
        StaticCrop::composed(composition)
    } else if routing.has_proxy_viewport() {
        // Nothing is left to compose: the app destroyed its own viewport and
        // reported no window geometry, so the proxy removes its crop (D8).
        StaticCrop::cleared()
    } else {
        return;
    };
    if state.written(surface_id) == Some(crop) {
        return;
    }
    if crop.source().is_none() && !routing.has_proxy_viewport() {
        // Nothing to write yet: the buffer size is unknown, and a viewport
        // created for it would only draw the whole buffer.
        return;
    }
    if globals.viewporter().is_none() || globals.has_app_viewport(surface_id) {
        // There is no viewport to write, or the app's own already maps this
        // surface (D8). Nothing is recorded as written, so a later pass can
        // try again.
        return;
    }
    state.set_written(surface_id, crop);
    routing.send_viewport_crop(&crop);
}

/// The app surface's extent in surface-local pixels (D8): its window
/// geometry's size when it reports one, the fallback otherwise.
fn surface_extent(window: Option<[i32; 4]>, fallback: (u32, u32)) -> (u32, u32) {
    match window {
        Some([_, _, width, height]) => (
            u32::try_from(width).unwrap_or(1).max(1),
            u32::try_from(height).unwrap_or(1).max(1),
        ),
        None => (fallback.0.max(1), fallback.1.max(1)),
    }
}

/// The visible fraction of a change that is not animated (D8): the whole app
/// surface when it fits the panel, and the panel's length from the docked
/// edge when the app's surface is still wider — the frames after a shrink's
/// last frame, before the app commits its smaller buffer. The same rule the
/// tween's final frame lands on, so an animated change and a snap end alike.
fn steady_fraction(geometry: Geometry, surface: (u32, u32)) -> Fraction {
    let across = if geometry.edge().across_is_width() {
        surface.0
    } else {
        surface.1
    };
    Fraction::of(geometry.across(), across).unwrap_or_else(Fraction::whole)
}

/// What the proxy does with an app viewport now (D8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kept {
    /// Composed into the proxy's own viewport on the panel surface.
    Adopted,
    /// Forwarded to the compositor on a viewport of its own surface.
    Mirrored,
    /// Waiting for the panel surface to be known: this surface may be it,
    /// and mirroring now would put a second viewport on the panel surface.
    Deferred,
}

/// The plan for a new app viewport (D8): the panel surface's own is adopted,
/// a surface that cannot be the panel's is mirrored, and while the slot is
/// still empty the decision waits.
fn plan(routing: &Routing, surface_id: u64) -> Kept {
    if routing.is_panel_surface(surface_id) {
        Kept::Adopted
    } else if routing.has_window() {
        Kept::Mirrored
    } else {
        Kept::Deferred
    }
}

/// One app `wp_viewport` the proxy kept local (D8). The objects are held
/// weak, as every handler here holds its own object: a strong reference
/// would close the cycle through the handler's routing.
struct AppEntry {
    viewport: Weak<WpViewport>,
    surface: Weak<WlSurface>,
    values: AppViewport,
    kept: Kept,
}

/// The inputs of the panel surface's viewport (D8): the app's own viewports
/// by surface, the compositor's preferred scale, the window geometry the app
/// reported, the buffer sizes the crop is clamped to, and what the proxy
/// wrote last.
pub(super) struct ViewportState {
    app: RefCell<HashMap<u64, AppEntry>>,
    preferred: RefCell<HashMap<u64, u32>>,
    window: Cell<Option<[i32; 4]>>,
    written: Cell<Option<(u64, StaticCrop)>>,
    buffers: PanelBuffers,
}

impl ViewportState {
    /// No app viewport, no window geometry and nothing written yet.
    pub(super) fn new() -> Self {
        Self {
            app: RefCell::new(HashMap::new()),
            preferred: RefCell::new(HashMap::new()),
            window: Cell::new(None),
            written: Cell::new(None),
            buffers: PanelBuffers::new(),
        }
    }

    /// The buffer sizes the crop is clamped to (D8).
    pub(super) fn buffers(&self) -> &PanelBuffers {
        &self.buffers
    }

    /// Records the window geometry the app reported on the panel surface
    /// (D5).
    fn set_window(&self, window: Option<[i32; 4]>) {
        self.window.set(window);
    }

    /// The window geometry the app reported on the panel surface (D5).
    pub(super) fn window(&self) -> Option<[i32; 4]> {
        self.window.get()
    }

    /// The app's own viewport values on the surface named `surface_id` (D8).
    pub(super) fn app(&self, surface_id: u64) -> AppViewport {
        self.app
            .borrow()
            .get(&surface_id)
            .map_or_else(AppViewport::none, |entry| entry.values)
    }

    /// The app's own viewport object on the surface, when it kept one.
    fn app_viewport(&self, surface_id: u64) -> Option<Rc<WpViewport>> {
        self.app.borrow().get(&surface_id)?.viewport.upgrade()
    }

    /// How the proxy keeps the surface's app viewport (D8).
    fn kept(&self, surface_id: u64) -> Option<Kept> {
        self.app.borrow().get(&surface_id).map(|entry| entry.kept)
    }

    /// Records a viewport the app just created (D8).
    fn add(&self, surface_id: u64, viewport: &Rc<WpViewport>, surface: &Rc<WlSurface>, kept: Kept) {
        self.app.borrow_mut().insert(
            surface_id,
            AppEntry {
                viewport: Rc::downgrade(viewport),
                surface: Rc::downgrade(surface),
                values: AppViewport::none(),
                kept,
            },
        );
    }

    /// Records the source the app set, `None` for the unset form (D8).
    fn set_source(&self, surface_id: u64, source: Option<[Fixed; 4]>) {
        if let Some(entry) = self.app.borrow_mut().get_mut(&surface_id) {
            entry.values = entry.values.with_source(source);
        }
    }

    /// Records the destination the app set, `None` for the unset form (D8).
    fn set_destination(&self, surface_id: u64, destination: Option<(i32, i32)>) {
        if let Some(entry) = self.app.borrow_mut().get_mut(&surface_id) {
            entry.values = entry.values.with_destination(destination);
        }
    }

    /// Forgets a viewport the app destroyed, and says how the proxy kept it
    /// (D8).
    fn remove(&self, surface_id: u64) -> Option<Kept> {
        self.app
            .borrow_mut()
            .remove(&surface_id)
            .map(|entry| entry.kept)
    }

    /// The viewports still waiting for the panel surface, with the values
    /// they already set (D8).
    fn deferred(&self) -> Vec<(u64, Rc<WpViewport>, Rc<WlSurface>, AppViewport)> {
        let mut waiting: Vec<u64> = self
            .app
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.kept == Kept::Deferred)
            .map(|(surface_id, _)| *surface_id)
            .collect();
        waiting.sort_unstable();
        waiting
            .into_iter()
            .filter_map(|surface_id| {
                let (viewport, surface, values) = self.deferred_entry(surface_id)?;
                Some((surface_id, viewport, surface, values))
            })
            .collect()
    }

    /// One viewport still waiting for the panel surface, with the values it
    /// already set (D8).
    fn deferred_entry(
        &self,
        surface_id: u64,
    ) -> Option<(Rc<WpViewport>, Rc<WlSurface>, AppViewport)> {
        let app = self.app.borrow();
        let entry = app.get(&surface_id)?;
        if entry.kept != Kept::Deferred {
            return None;
        }
        Some((
            entry.viewport.upgrade()?,
            entry.surface.upgrade()?,
            entry.values,
        ))
    }

    /// Records that the surface's viewport is the proxy's to compose (D8).
    fn adopt(&self, surface_id: u64) {
        if let Some(entry) = self.app.borrow_mut().get_mut(&surface_id) {
            entry.kept = Kept::Adopted;
        }
    }

    /// Records that the surface's viewport is mirrored upstream (D8).
    fn mirror(&self, surface_id: u64) {
        if let Some(entry) = self.app.borrow_mut().get_mut(&surface_id) {
            entry.kept = Kept::Mirrored;
        }
    }

    /// Records the compositor's preferred scale for the surface (D8).
    fn set_preferred_scale(&self, surface_id: u64, scale: u32) {
        self.preferred.borrow_mut().insert(surface_id, scale);
    }

    /// The compositor's preferred scale for the surface, in 1/120ths (D8).
    fn preferred_scale(&self, surface_id: u64) -> Option<u32> {
        self.preferred.borrow().get(&surface_id).copied()
    }

    /// The crop the proxy last wrote on the surface's viewport (D8).
    fn written(&self, surface_id: u64) -> Option<StaticCrop> {
        match self.written.get() {
            Some((written_surface, crop)) if written_surface == surface_id => Some(crop),
            _ => None,
        }
    }

    /// Records the crop the proxy just wrote (D8).
    fn set_written(&self, surface_id: u64, crop: StaticCrop) {
        self.written.set(Some((surface_id, crop)));
    }
}

/// Settles the app viewports created before the panel surface was known
/// (D8): the panel surface's own is adopted and composed into the proxy's
/// crop, and every other one is mirrored upstream with the values it already
/// set.
fn settle_app_viewports(routing: &Routing) {
    let Some((panel_surface, _, _)) = routing.panel_window() else {
        return;
    };
    let panel_id = panel_surface.unique_id();
    let globals = routing.globals();
    for (surface_id, viewport, surface, values) in globals.viewport().deferred() {
        if surface_id == panel_id {
            globals.viewport().adopt(surface_id);
            routing.set_panel_has_app_viewport(true);
            continue;
        }
        let Some(viewporter) = globals.viewporter() else {
            continue;
        };
        mirror_deferred(
            &globals,
            &viewporter,
            surface_id,
            &viewport,
            &surface,
            values,
        );
    }
}

/// Settles the deferred app viewport of a surface that was forwarded instead
/// of docked (D8): a passed-through window is not the panel's, so the
/// viewport it created before the window mapping is mirrored now, rather
/// than waiting for a slot the app may never take.
pub(super) fn settle_forwarded(routing: &Routing, surface_id: u64) {
    let globals = routing.globals();
    let Some(viewporter) = globals.viewporter() else {
        return;
    };
    let Some((viewport, surface, values)) = globals.viewport().deferred_entry(surface_id) else {
        return;
    };
    mirror_deferred(
        &globals,
        &viewporter,
        surface_id,
        &viewport,
        &surface,
        values,
    );
}

/// Mirrors one waiting app viewport upstream (D8): the viewport object the
/// app created on its own surface, with the values it already set.
fn mirror_deferred(
    globals: &Rc<Globals>,
    viewporter: &Rc<WpViewporter>,
    surface_id: u64,
    viewport: &Rc<WpViewport>,
    surface: &Rc<WlSurface>,
    values: AppViewport,
) {
    viewporter.send_get_viewport(viewport, surface);
    globals.add_app_viewport(surface_id);
    globals.viewport().mirror(surface_id);
    write_app_values(viewport, values);
}

/// Mirrors the app's own values onto its upstream viewport (D8).
fn write_app_values(viewport: &WpViewport, values: AppViewport) {
    if let Some([x, y, width, height]) = values.source() {
        viewport.send_set_source(x, y, width, height);
    }
    if let Some((width, height)) = values.destination() {
        viewport.send_set_destination(width, height);
    }
}

/// The app's `wp_viewporter` handler (D8): every viewport the app creates is
/// kept local. On the panel surface the values are composed into the proxy's
/// crop, on any other surface they are mirrored onto a viewport of that
/// surface, and while the slot is empty they wait for the panel surface to
/// be known.
pub(super) struct ClientWpViewporter {
    /// The viewporter this handler answers, kept weak so the handler does
    /// not keep its own object alive.
    viewporter: Weak<WpViewporter>,
    routing: Rc<Routing>,
}

impl ClientWpViewporter {
    /// The handler of the app's `wp_viewporter` bind.
    pub(super) fn new(viewporter: &Rc<WpViewporter>, routing: &Rc<Routing>) -> Self {
        Self {
            viewporter: Rc::downgrade(viewporter),
            routing: Rc::clone(routing),
        }
    }
}

impl WpViewporterHandler for ClientWpViewporter {
    fn handle_get_viewport(
        &mut self,
        _slf: &Rc<WpViewporter>,
        id: &Rc<WpViewport>,
        surface: &Rc<WlSurface>,
    ) {
        let surface_id = surface.unique_id();
        let kept = plan(&self.routing, surface_id);
        self.routing
            .globals()
            .viewport()
            .add(surface_id, id, surface, kept);
        id.set_handler(ClientWpViewport {
            viewport: Rc::downgrade(id),
            routing: Rc::clone(&self.routing),
            surface_id,
        });
        match kept {
            Kept::Adopted => {
                // D8: never forwarded — the proxy's own viewport is this
                // surface's one, and a second is the fatal `viewport_exists`.
                self.routing.set_panel_has_app_viewport(true);
            }
            Kept::Mirrored => {
                if let Some(viewporter) = self.viewporter.upgrade() {
                    viewporter.send_get_viewport(id, surface);
                }
                self.routing.globals().add_app_viewport(surface_id);
            }
            Kept::Deferred => {}
        }
    }
}

/// The app's own `wp_viewport` (D8): its values are kept local and either
/// composed into the proxy's crop or mirrored upstream, never forwarded as
/// they are.
pub(super) struct ClientWpViewport {
    /// The viewport this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    viewport: Weak<WpViewport>,
    routing: Rc<Routing>,
    surface_id: u64,
}

impl WpViewportHandler for ClientWpViewport {
    fn handle_set_source(
        &mut self,
        _slf: &Rc<WpViewport>,
        x: Fixed,
        y: Fixed,
        width: Fixed,
        height: Fixed,
    ) {
        let routing = Rc::clone(&self.routing);
        let surface_id = self.surface_id;
        let globals = routing.globals();
        let state = globals.viewport();
        let unset = [x, y, width, height]
            .iter()
            .all(|value| value.to_wire() == UNSET_FIXED);
        state.set_source(surface_id, (!unset).then_some([x, y, width, height]));
        super::commit::mark_app_viewport_dirty(&routing, surface_id);
        match state.kept(surface_id) {
            Some(Kept::Adopted) => recompose(&routing),
            Some(Kept::Mirrored) => {
                if let Some(viewport) = state.app_viewport(surface_id) {
                    viewport.send_set_source(x, y, width, height);
                }
            }
            Some(Kept::Deferred) | None => {}
        }
    }

    fn handle_set_destination(&mut self, _slf: &Rc<WpViewport>, width: i32, height: i32) {
        let routing = Rc::clone(&self.routing);
        let surface_id = self.surface_id;
        let globals = routing.globals();
        let state = globals.viewport();
        let unset = width == -1 && height == -1;
        state.set_destination(surface_id, (!unset).then_some((width, height)));
        super::commit::mark_app_viewport_dirty(&routing, surface_id);
        match state.kept(surface_id) {
            Some(Kept::Adopted) => recompose(&routing),
            Some(Kept::Mirrored) => {
                if let Some(viewport) = state.app_viewport(surface_id) {
                    viewport.send_set_destination(width, height);
                }
            }
            Some(Kept::Deferred) | None => {}
        }
    }

    fn handle_destroy(&mut self, _slf: &Rc<WpViewport>) {
        let routing = Rc::clone(&self.routing);
        let surface_id = self.surface_id;
        let Some(viewport) = self.viewport.upgrade() else {
            return;
        };
        let Some(kept) = routing.globals().viewport().remove(surface_id) else {
            return;
        };
        match kept {
            Kept::Mirrored => {
                routing.globals().remove_app_viewport(surface_id);
                viewport.send_destroy();
            }
            Kept::Adopted => {
                // The panel surface's mapping is the proxy's own again: the
                // composition drops the app's values (D8).
                routing.set_panel_has_app_viewport(false);
                recompose(&routing);
                viewport.delete_id();
            }
            Kept::Deferred => viewport.delete_id(),
        }
    }
}

/// The app's `wp_fractional_scale_manager_v1` bind (D8): a surface may have
/// only one `wp_fractional_scale_v1`, so the proxy watches the app's object
/// for the preferred scale instead of asking for one itself.
pub(super) struct ClientFractionalScaleManager {
    /// The manager this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    manager: Weak<WpFractionalScaleManagerV1>,
    routing: Rc<Routing>,
}

impl ClientFractionalScaleManager {
    /// The handler of the app's `wp_fractional_scale_manager_v1` bind.
    pub(super) fn new(manager: &Rc<WpFractionalScaleManagerV1>, routing: &Rc<Routing>) -> Self {
        Self {
            manager: Rc::downgrade(manager),
            routing: Rc::clone(routing),
        }
    }
}

impl WpFractionalScaleManagerV1Handler for ClientFractionalScaleManager {
    fn handle_get_fractional_scale(
        &mut self,
        _slf: &Rc<WpFractionalScaleManagerV1>,
        id: &Rc<WpFractionalScaleV1>,
        surface: &Rc<WlSurface>,
    ) {
        id.set_handler(ClientFractionalScale {
            scale: Rc::downgrade(id),
            routing: Rc::clone(&self.routing),
            surface_id: surface.unique_id(),
        });
        if let Some(manager) = self.manager.upgrade() {
            manager.send_get_fractional_scale(id, surface);
        }
    }
}

/// One app `wp_fractional_scale_v1` (D8): its preferred scale is what a
/// destination-only app viewport on the panel surface is cropped at.
pub(super) struct ClientFractionalScale {
    /// The fractional-scale object this handler answers, kept weak so the
    /// handler does not keep its own object alive.
    scale: Weak<WpFractionalScaleV1>,
    routing: Rc<Routing>,
    surface_id: u64,
}

impl WpFractionalScaleV1Handler for ClientFractionalScale {
    fn handle_preferred_scale(&mut self, _slf: &Rc<WpFractionalScaleV1>, scale: u32) {
        let routing = Rc::clone(&self.routing);
        routing
            .globals()
            .viewport()
            .set_preferred_scale(self.surface_id, scale);
        if routing.is_panel_surface(self.surface_id) {
            recompose(&routing);
        }
        if let Some(object) = self.scale.upgrade() {
            object.send_preferred_scale(scale);
        }
    }
}

/// The viewport values the proxy already owns, kept for reuse and cleared
/// with the panel window. Lives on [`super::routing::Routing`]; the helper
/// here keeps the double-buffered write in one place.
pub(super) fn write_crop(viewport: &WpViewport, crop: Option<&StaticCrop>) {
    let (source, destination) =
        crop.map_or((None, None), |crop| (crop.source(), crop.destination()));
    match source {
        Some([x, y, width, height]) => viewport.send_set_source(
            Fixed::from_f64_lossy(f64::from(x)),
            Fixed::from_f64_lossy(f64::from(y)),
            Fixed::from_f64_lossy(f64::from(width)),
            Fixed::from_f64_lossy(f64::from(height)),
        ),
        None => unset_source(viewport),
    }
    match destination {
        Some((width, height)) => viewport.send_set_destination(width, height),
        None => viewport.send_set_destination(-1, -1),
    }
}

/// Unsets the viewport's source rectangle: `-1` in every field draws the
/// whole buffer, which is always inside it (`wp_viewport`).
fn unset_source(viewport: &WpViewport) {
    viewport.send_set_source(
        Fixed::from_f64_lossy(-1.0),
        Fixed::from_f64_lossy(-1.0),
        Fixed::from_f64_lossy(-1.0),
        Fixed::from_f64_lossy(-1.0),
    );
}

// The panel surface the crop writes through is reached through
// [`super::routing::Routing`]; the values it carries are composed in
// [`super::compose`] and bounded by [`super::buffers`].

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use wl_proxy::baseline::Baseline;
    use wl_proxy::object::ObjectCoreApi;
    use wl_proxy::protocols::viewporter::wp_viewport::WpViewport;
    use wl_proxy::protocols::viewporter::wp_viewporter::{WpViewporter, WpViewporterHandler};
    use wl_proxy::protocols::wayland::wl_surface::WlSurface;
    use wl_proxy::state::State;

    use super::{AppViewport, ClientWpViewporter, Kept, mirror_deferred};
    use crate::config::PanelSettings;
    use crate::proxy::routing::Routing;

    /// A display-free state: the test mints proxy objects without a
    /// compositor (`port-to-rust` D10).
    fn state() -> Rc<State> {
        State::builder(Baseline::V6)
            .without_server()
            .build()
            .expect("a client-side state needs no compositor")
    }

    /// A viewport the app created before its window was mapped is settled
    /// when the surface passes through instead of docking (D8): it would
    /// otherwise wait for a panel slot the window can never take, and a
    /// fractional-scale app would draw at the wrong size.
    #[test]
    fn a_forwarded_surfaces_waiting_viewport_is_mirrored() {
        let state = state();
        let routing = Routing::new(PanelSettings::built_in());
        let viewporter = state.create_object::<WpViewporter>(1);
        let viewport = state.create_object::<WpViewport>(1);
        let surface = state.create_object::<WlSurface>(6);
        let surface_id = surface.unique_id();

        let mut app_viewporter = ClientWpViewporter::new(&viewporter, &routing);
        WpViewporterHandler::handle_get_viewport(
            &mut app_viewporter,
            &viewporter,
            &viewport,
            &surface,
        );
        assert_eq!(
            routing.globals().viewport().kept(surface_id),
            Some(Kept::Deferred),
            "a viewport made before the window mapping waits"
        );

        let globals = routing.globals();
        mirror_deferred(
            &globals,
            &viewporter,
            surface_id,
            &viewport,
            &surface,
            AppViewport::none(),
        );
        assert_eq!(
            routing.globals().viewport().kept(surface_id),
            Some(Kept::Mirrored),
            "a forwarded surface's viewport is no longer the panel's"
        );
        assert!(
            routing.globals().has_app_viewport(surface_id),
            "the proxy's own viewport keeps off that surface (D8)"
        );
    }
}
