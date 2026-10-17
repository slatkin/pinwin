//! The panel window's viewport crop (replace-terminal-with-app-host D5, D8):
//! the proxy-owned `wp_viewport` that crops a client-side title bar and
//! shadow out of the panel's buffer.
//!
//! A GTK3 app draws its drop shadow into its buffer and reports where the
//! real window sits with `xdg_surface.set_window_geometry`. A compositor
//! draws a layer surface's buffer at the buffer's own size, so without a
//! crop the shadow is a gap at the docked edge and an overhang at the far
//! one (D5). The proxy owns the panel surface's viewport: its source is the
//! window geometry and its destination the geometry's own size, so the
//! window — not the buffer — fills the panel, one logical pixel to one
//! logical pixel.
//!
//! The values are double-buffered viewport state: they apply with the app's
//! next commit, so the proxy sends no commit of its own here (the commit
//! coordinator is row 8.1, D7).
//!
//! D8's one-viewport rule: a surface has at most one `wp_viewport`, and a
//! second is the fatal `viewport_exists` error. So the proxy creates its own
//! only when the surface has none. An app that made its own viewport on the
//! panel surface (foot and browsers do, before the toplevel) keeps it: the
//! proxy mirrors its values upstream unchanged, so the app's own mapping —
//! fractional scale included — survives. Composing that mapping into the
//! crop waits for the size-animation row (10.x), which owns the buffer-size
//! tracking the composition needs to stay inside `out_of_buffer` bounds;
//! until then mirrored apps are not cropped, exactly as they would render
//! without pinwin.
//!
//! [`static_crop`] and [`CropDecision`] are pure (`port-to-rust` D10), so
//! their matrix is unit tested without a compositor.

use std::rc::{Rc, Weak};

use wl_proxy::fixed::Fixed;
use wl_proxy::object::ObjectCoreApi;
use wl_proxy::protocols::viewporter::wp_viewport::{WpViewport, WpViewportHandler};
use wl_proxy::protocols::viewporter::wp_viewporter::{WpViewporter, WpViewporterHandler};
use wl_proxy::protocols::wayland::wl_surface::WlSurface;

use super::routing::Routing;

/// What the proxy does about the panel window's viewport (D8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CropDecision {
    /// The app put its own `wp_viewport` on the panel surface: the proxy
    /// must not create a second, and until row 10.x composes with the app's
    /// mapping the app's values stand. No crop.
    DeferToAppViewport,
    /// No app viewport: the proxy owns the crop, built from the window
    /// geometry.
    OwnCrop(StaticCrop),
    /// Nothing to apply: the app reported no window geometry, so the whole
    /// buffer already draws at the surface's size.
    NoCrop,
}

/// The steady-state viewport decision (D5, D8): a pure function of whether
/// the app reported a window geometry and whether the panel surface already
/// carries an app viewport.
pub(super) fn crop_decision(
    geometry: Option<[i32; 4]>,
    app_viewport_present: bool,
) -> CropDecision {
    if app_viewport_present {
        return CropDecision::DeferToAppViewport;
    }
    match geometry.map(static_crop) {
        Some(Some(crop)) => CropDecision::OwnCrop(crop),
        _ => CropDecision::NoCrop,
    }
}

/// The viewport values the proxy puts on the panel surface for the steady
/// state (D5): the source rectangle — the app's window geometry, in the
/// coordinates the viewporter reads — and the destination it is drawn at,
/// the geometry's own size so the crop is 1:1 and only what lies outside
/// the geometry is cut. Built only by [`static_crop`] (`port-to-rust` D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StaticCrop {
    source: [i32; 4],
    destination: (i32, i32),
}

impl StaticCrop {
    /// The source rectangle: the window geometry, clamped to the buffer's
    /// origin.
    #[must_use]
    pub const fn source(&self) -> [i32; 4] {
        self.source
    }

    /// The destination: the geometry's own size, so the crop draws 1:1.
    #[must_use]
    pub const fn destination(&self) -> (i32, i32) {
        self.destination
    }
}

/// The steady-state crop from the app's window geometry `[x, y, width,
/// height]` (D5): the geometry itself as the source, drawn 1:1 at its own
/// size. The viewporter refuses a negative source origin (`bad_value`), and
/// a client-side drop shadow reports exactly that (GTK3 draws the window at
/// a negative offset inside its buffer), so the rectangle is clamped to the
/// buffer's origin. `None` when nothing usable remains — the caller then
/// unsets the source so the whole buffer draws instead of risking
/// `out_of_buffer`.
pub(super) fn static_crop(geometry: [i32; 4]) -> Option<StaticCrop> {
    let [x, y, width, height] = geometry;
    let left = x.max(0);
    let top = y.max(0);
    let right = x.saturating_add(width);
    let bottom = y.saturating_add(height);
    if right <= left || bottom <= top {
        return None;
    }
    Some(StaticCrop {
        source: [left, top, right - left, bottom - top],
        destination: (right - left, bottom - top),
    })
}

/// Applies the steady-state crop to the panel surface through the routing
/// (D5, D8): creates the proxy's viewport on first use and writes the
/// source and destination. Called when the app sets a window geometry and
/// when the slot is taken with a geometry already recorded.
pub(super) fn apply_steady_crop(routing: &Routing, geometry: Option<[i32; 4]>) {
    match crop_decision(geometry, routing.panel_has_app_viewport()) {
        CropDecision::NoCrop | CropDecision::DeferToAppViewport => {}
        CropDecision::OwnCrop(crop) => routing.send_viewport_crop(&crop),
    }
}

/// The app's `wp_viewporter` handler (D8): every viewport the app creates is
/// recorded and mirrored upstream — the values pass through unchanged — and
/// on the panel surface a second viewport is kept local instead of
/// forwarded, because a second would raise the fatal `viewport_exists`
/// error.
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
        let is_panel = self.routing.is_panel_surface(surface_id);
        if is_panel && self.routing.has_proxy_viewport() {
            // The proxy owns the panel surface's viewport (D8): the app's
            // request stays local, or forwarding it would kill the
            // connection with `viewport_exists`.
            id.set_handler(ClientWpViewport {
                viewport: Rc::downgrade(id),
                routing: Rc::clone(&self.routing),
                surface_id,
                mirror: false,
            });
            return;
        }
        if is_panel {
            self.routing.set_panel_has_app_viewport(true);
        }
        self.routing.globals().add_app_viewport(surface_id);
        id.set_handler(ClientWpViewport {
            viewport: Rc::downgrade(id),
            routing: Rc::clone(&self.routing),
            surface_id,
            mirror: true,
        });
        if let Some(viewporter) = self.viewporter.upgrade() {
            viewporter.send_get_viewport(id, surface);
        }
    }
}

/// The app's `wp_viewport` (D8): a mirrored one forwards its values
/// upstream, a swallowed one on the panel surface keeps them local. Every
/// other event keeps the default forward path.
pub(super) struct ClientWpViewport {
    /// The viewport this handler answers, kept weak so the handler does not
    /// keep its own object alive (the routing's handler convention).
    viewport: Weak<WpViewport>,
    routing: Rc<Routing>,
    surface_id: u64,
    mirror: bool,
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
        if !self.mirror {
            return;
        }
        if let Some(viewport) = self.viewport.upgrade() {
            viewport.send_set_source(x, y, width, height);
        }
    }

    fn handle_set_destination(&mut self, _slf: &Rc<WpViewport>, width: i32, height: i32) {
        if !self.mirror {
            return;
        }
        if let Some(viewport) = self.viewport.upgrade() {
            viewport.send_set_destination(width, height);
        }
    }

    fn handle_destroy(&mut self, _slf: &Rc<WpViewport>) {
        self.routing.globals().remove_app_viewport(self.surface_id);
        if self.mirror && self.routing.is_panel_surface(self.surface_id) {
            // The app gave up its own mapping: the proxy can crop again on
            // the next window geometry.
            self.routing.set_panel_has_app_viewport(false);
        }
        if let Some(viewport) = self.viewport.upgrade() {
            viewport.send_destroy();
        }
    }
}

/// The viewport values the proxy already owns, kept for reuse and cleared
/// with the panel window. Lives on [`super::routing::Routing`]; the helper
/// here keeps the double-buffered write in one place.
pub(super) fn write_crop(viewport: &WpViewport, crop: Option<&StaticCrop>) {
    match crop {
        Some(crop) => {
            let [x, y, width, height] = crop.source();
            viewport.send_set_source(
                Fixed::from_f64_lossy(f64::from(x)),
                Fixed::from_f64_lossy(f64::from(y)),
                Fixed::from_f64_lossy(f64::from(width)),
                Fixed::from_f64_lossy(f64::from(height)),
            );
            let (destination_width, destination_height) = crop.destination();
            viewport.send_set_destination(destination_width, destination_height);
        }
        None => {
            // -1 in every field unsets the source: the whole buffer is
            // drawn, which is always inside the buffer (viewporter.xml).
            viewport.send_set_source(
                Fixed::from_f64_lossy(-1.0),
                Fixed::from_f64_lossy(-1.0),
                Fixed::from_f64_lossy(-1.0),
                Fixed::from_f64_lossy(-1.0),
            );
        }
    }
}

// The registry-side state the viewport handlers share (the set of surfaces
// carrying app viewports) lives in [`super::output`]; the panel surface the
// crop writes through is reached through [`super::routing::Routing`].

#[cfg(test)]
mod tests {
    use super::{CropDecision, crop_decision, static_crop};

    /// The GTK3 CSD case (D5): the geometry is the source and is drawn 1:1
    /// at its own size, so the shadow outside it is cropped away.
    #[test]
    fn the_window_geometry_becomes_the_source_and_destination() {
        let crop = static_crop([5, 7, 640, 480]).expect("a usable geometry");
        assert_eq!(crop.source(), [5, 7, 640, 480]);
        assert_eq!(crop.destination(), (640, 480));
    }

    /// A negative geometry origin, which a client-side drop shadow produces,
    /// clamps to the buffer's origin: the source stays legal (`bad_value`
    /// fires on a negative x or y) and keeps the window's far edge.
    #[test]
    fn a_negative_origin_clamps_to_the_buffer_origin() {
        let crop = static_crop([-20, -30, 640, 480]).expect("a usable geometry");
        assert_eq!(crop.source(), [0, 0, 620, 450]);
        assert_eq!(crop.destination(), (620, 450));
    }

    /// A geometry that collapses — zero size, or an origin whose offset
    /// swallows the whole rectangle — gives no crop: the caller unsets the
    /// source and the whole buffer draws, which is always inside it.
    #[test]
    fn a_collapsed_geometry_gives_no_crop() {
        assert!(static_crop([0, 0, 0, 0]).is_none());
        assert!(static_crop([-20, -30, 10, 10]).is_none());
        assert!(static_crop([10, 10, -5, 100]).is_none());
        assert!(static_crop([10, 10, 100, -5]).is_none());
    }

    /// Origins that saturate past `i32` cannot wrap into a valid crop.
    #[test]
    fn extreme_origins_give_no_crop() {
        assert!(static_crop([i32::MAX, i32::MAX, i32::MAX, i32::MAX]).is_none());
        assert!(static_crop([i32::MIN, i32::MIN, 1, 1]).is_none());
    }

    /// The decision matrix (D8): an app viewport on the panel surface defers
    /// to the app's mapping, a geometry without one builds the proxy's crop,
    /// and no geometry means nothing to apply.
    #[test]
    fn the_decision_follows_the_viewport_and_the_geometry() {
        let geometry = [5, 7, 640, 480];

        assert_eq!(
            crop_decision(Some(geometry), true),
            CropDecision::DeferToAppViewport,
            "an app viewport owns the mapping (D8)"
        );
        assert_eq!(
            crop_decision(None, true),
            CropDecision::DeferToAppViewport,
            "the app viewport defers the crop whatever the geometry is"
        );

        let CropDecision::OwnCrop(crop) = crop_decision(Some(geometry), false) else {
            panic!("a geometry without an app viewport crops");
        };
        assert_eq!(crop.source(), geometry);

        assert_eq!(
            crop_decision(None, false),
            CropDecision::NoCrop,
            "no geometry, nothing to crop yet"
        );
        let CropDecision::NoCrop = crop_decision(Some([0, 0, 0, 0]), false) else {
            panic!("a collapsed geometry is NoCrop");
        };
    }
}
