//! The popups of the panel window (replace-terminal-with-app-host D5): the
//! recorded anchor rectangle of a positioner, the configure a layer popup
//! reports in the app's own coordinates, and the same offset applied when
//! the app repositions a popup.
//!
//! A popup whose parent is the panel window becomes a popup of the panel's
//! layer surface, and the compositor positions it against that surface, whose
//! origin is the panel's whole surface. The app asked for a position relative
//! to its window geometry, so the proxy moves the positioner's anchor
//! rectangle by that geometry when the popup is created — and again on every
//! `xdg_popup.reposition`, which carries a fresh positioner — and moves the
//! popup's configure back before the app sees it. Popups of passed-through
//! windows take none of this: they are forwarded unchanged.
//!
//! The positioner recording and the configure translation are copied from
//! wl-paper's `apps/wl-paper/src/paper.rs` `ClientXdgPositioner` and
//! `ClientXdgPopup` (<https://github.com/mahkoh/wl-proxy>, GPL-3.0, the
//! licence of this crate). The offset and the move back are pinwin's pure
//! helpers (`port-to-rust` D10), so their arithmetic — a geometry the app
//! never set, a negative CSD origin and an origin past `i32` — is unit tested
//! without a compositor. Both handlers keep their own object in a `Weak`, as
//! the routing's handlers do, so a used object is reached through the handler
//! and not through the trait's parameter name.

use std::rc::{Rc, Weak};

use wl_proxy::object::{Object, ObjectUtils};
use wl_proxy::protocols::xdg_shell::xdg_popup::{XdgPopup, XdgPopupHandler};
use wl_proxy::protocols::xdg_shell::xdg_positioner::{XdgPositioner, XdgPositionerHandler};

/// The anchor rectangle a popup of the panel window gets upstream (D5): the
/// app's anchor rectangle moved by the parent's window geometry, so the
/// compositor places the popup where the app asked relative to its window. A
/// parent whose geometry the app never set leaves the rectangle alone.
pub(super) fn offset_anchor_rect(anchor: [i32; 4], parent_geometry: Option<[i32; 4]>) -> [i32; 4] {
    let Some([gx, gy, ..]) = parent_geometry else {
        return anchor;
    };
    let [x, y, width, height] = anchor;
    [gx.saturating_add(x), gy.saturating_add(y), width, height]
}

/// The popup configure the app sees (D5): the compositor's configure for a
/// layer popup is relative to the panel surface, the app's own is relative to
/// its window geometry, so the geometry's origin comes off. This is the move
/// back that undoes [`offset_anchor_rect`] for the popup's own position.
pub(super) fn translate_configure(x: i32, y: i32, parent_geometry: [i32; 4]) -> (i32, i32) {
    let [gx, gy, ..] = parent_geometry;
    (x.saturating_sub(gx), y.saturating_sub(gy))
}

/// The anchor rectangle a repositioned layer popup gets upstream (D5): the
/// same parent-geometry offset [`offset_anchor_rect`] applies at creation,
/// because the compositor positions the repositioned popup against the
/// panel surface too. `None` when the app's positioner set no anchor
/// rectangle — there is nothing to rewrite and the reposition forwards
/// unchanged.
pub(super) fn reposition_offset(
    anchor: Option<[i32; 4]>,
    parent_geometry: [i32; 4],
) -> Option<[i32; 4]> {
    let anchor = anchor?;
    Some(offset_anchor_rect(anchor, Some(parent_geometry)))
}

/// The app's `xdg_positioner` (D5): it records the anchor rectangle the app
/// set, which the popup creation offsets by the parent's window geometry.
/// Copied from wl-paper's `paper.rs` `ClientXdgPositioner` (GPL-3.0); the
/// held-back `destroy` is pinwin's (D9).
pub(super) struct ClientXdgPositioner {
    /// The positioner this handler answers, kept weak so the handler does not
    /// keep its own object alive.
    positioner: Weak<XdgPositioner>,
    anchor_rect: Option<[i32; 4]>,
    /// Whether a popup that waits for the panel's first dock still needs this
    /// positioner (D9): the app's `destroy` waits for that popup.
    held: bool,
    /// Whether the app destroyed this positioner while the popup that names
    /// it waited (D9): the upstream object stays until the popup is created.
    destroyed: bool,
}

impl ClientXdgPositioner {
    /// The handler of the positioner `positioner`, which has recorded no
    /// anchor rectangle yet.
    pub(super) fn new(positioner: &Rc<XdgPositioner>) -> Self {
        Self {
            positioner: Rc::downgrade(positioner),
            anchor_rect: None,
            held: false,
            destroyed: false,
        }
    }

    /// The anchor rectangle the app set, or `None` while it set none: the
    /// protocol's unset rectangle stays unset here, so the offset skips it.
    pub(super) fn anchor_rect(&self) -> Option<[i32; 4]> {
        self.anchor_rect
    }

    /// Holds the app's `destroy` until the popup that names this positioner
    /// exists (D9): a popup cannot be created from a positioner whose
    /// upstream object is gone.
    pub(super) fn hold_destroy(&mut self) {
        self.held = true;
    }

    /// Takes whether the app's `destroy` was held back (D9), so the caller
    /// finishes it once the popup exists.
    fn take_destroyed(&mut self) -> bool {
        std::mem::take(&mut self.destroyed)
    }
}

/// Holds a `destroy` the app may still send for `positioner` until the popup
/// it names exists (D9).
pub(super) fn hold_destroy(positioner: &Rc<XdgPositioner>) {
    if let Ok(mut handler) = positioner.try_get_handler_mut::<ClientXdgPositioner>() {
        handler.hold_destroy();
    }
}

/// Forwards a `destroy` the app asked for while the popup that names the
/// positioner waited for the panel's first dock (D9): the upstream object
/// outlived the request, so the popup could still be created from it, and it
/// goes now.
pub(super) fn finish_deferred_destroy(positioner: &Rc<XdgPositioner>) {
    let Ok(mut handler) = positioner.try_get_handler_mut::<ClientXdgPositioner>() else {
        return;
    };
    if !handler.take_destroyed() {
        return;
    }
    positioner.send_destroy();
    positioner.unset_handler();
}

impl XdgPositionerHandler for ClientXdgPositioner {
    /// A `destroy` of a positioner a waiting popup still needs goes out after
    /// the popup is created: a popup cannot be created from a positioner
    /// whose upstream object is gone (D9).
    fn handle_destroy(&mut self, _slf: &Rc<XdgPositioner>) {
        let Some(positioner) = self.positioner.upgrade() else {
            return;
        };
        if self.held {
            self.destroyed = true;
            return;
        }
        positioner.send_destroy();
    }

    fn handle_set_anchor_rect(
        &mut self,
        _slf: &Rc<XdgPositioner>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
        self.anchor_rect = Some([x, y, width, height]);
        if let Some(positioner) = self.positioner.upgrade() {
            positioner.send_set_anchor_rect(x, y, width, height);
        }
    }
}

/// The app's `xdg_popup` of the panel's layer surface (D5): the compositor's
/// configure is relative to the panel surface, the app's own is relative to
/// its window geometry. Copied from wl-paper's `paper.rs` `ClientXdgPopup`
/// (GPL-3.0), with the arithmetic in [`translate_configure`]. Every other
/// event keeps the default forward path.
pub(super) struct ClientXdgPopup {
    /// The popup this handler answers, kept weak so the handler does not keep
    /// its own object alive.
    popup: Weak<XdgPopup>,
    /// The panel window's geometry, `[x, y, width, height]`, whose origin the
    /// configure is moved by.
    parent_geometry: [i32; 4],
}

impl ClientXdgPopup {
    /// The popup handler of the layer popup `popup`, whose parent's window
    /// geometry is `parent_geometry` (D5).
    pub(super) fn new(popup: &Rc<XdgPopup>, parent_geometry: [i32; 4]) -> Self {
        Self {
            popup: Rc::downgrade(popup),
            parent_geometry,
        }
    }
}

impl XdgPopupHandler for ClientXdgPopup {
    fn handle_configure(&mut self, _slf: &Rc<XdgPopup>, x: i32, y: i32, width: i32, height: i32) {
        let (x, y) = translate_configure(x, y, self.parent_geometry);
        if let Some(popup) = self.popup.upgrade() {
            popup.send_configure(x, y, width, height);
        }
    }

    fn handle_reposition(
        &mut self,
        _slf: &Rc<XdgPopup>,
        positioner: &Rc<XdgPositioner>,
        token: u32,
    ) {
        // A repositioned layer popup places against the panel surface, so
        // its fresh positioner gets the same parent-geometry offset the
        // popup's creation got (D5) — and the upstream positioner keeps the
        // app's rectangle afterwards, as at creation.
        let anchor = positioner
            .try_get_handler_ref::<ClientXdgPositioner>()
            .ok()
            .and_then(|handler| handler.anchor_rect());
        let Some(offset) = reposition_offset(anchor, self.parent_geometry) else {
            if let Some(popup) = self.popup.upgrade() {
                popup.send_reposition(positioner, token);
            }
            return;
        };
        let [x, y, width, height] = offset;
        positioner.send_set_anchor_rect(x, y, width, height);
        if let Some(popup) = self.popup.upgrade() {
            popup.send_reposition(positioner, token);
        }
        if let Some([ax, ay, aw, ah]) = anchor {
            positioner.send_set_anchor_rect(ax, ay, aw, ah);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{offset_anchor_rect, reposition_offset, translate_configure};

    /// A geometry the app never set leaves the anchor rectangle alone, so the
    /// popup is created where the app asked relative to the whole surface.
    #[test]
    fn no_geometry_leaves_the_anchor_rectangle_alone() {
        let anchor = [10, 20, 300, 40];
        assert_eq!(offset_anchor_rect(anchor, None), anchor);
    }

    /// The anchor rectangle moves by the parent's window geometry origin and
    /// keeps its own size (D5, spec "Context menu": the menu shows at the
    /// point the app asked for).
    #[test]
    fn the_anchor_rectangle_moves_by_the_geometry_origin() {
        assert_eq!(
            offset_anchor_rect([10, 20, 300, 40], Some([5, 7, 640, 480])),
            [15, 27, 300, 40]
        );
    }

    /// A negative geometry origin, which a client-side drop shadow produces,
    /// moves the rectangle the same way: the geometry is added, not clamped.
    #[test]
    fn a_negative_geometry_origin_moves_the_rectangle_back() {
        assert_eq!(
            offset_anchor_rect([10, 10, 100, 100], Some([-20, -30, 640, 480])),
            [-10, -20, 100, 100]
        );
    }

    /// An origin past `i32` saturates instead of wrapping, so a hostile
    /// client cannot wrap the rectangle it hands the compositor.
    #[test]
    fn an_origin_past_i32_saturates() {
        assert_eq!(
            offset_anchor_rect([i32::MAX, i32::MIN, 1, 1], Some([5, -5, 1, 1])),
            [i32::MAX, i32::MIN, 1, 1]
        );
    }

    /// The configure the app sees is the compositor's configure moved back by
    /// the same origin, so offset and move back undo each other.
    #[test]
    fn the_configure_move_back_undoes_the_offset() {
        let geometry = [5, 7, 640, 480];
        let anchor = [13, 29, 300, 40];
        let [x, y, ..] = offset_anchor_rect(anchor, Some(geometry));
        assert_eq!(translate_configure(x, y, geometry), (13, 29));
    }

    /// The configure's own arithmetic saturates too, so a compositor position
    /// past `i32` cannot wrap into the app's coordinates.
    #[test]
    fn the_configure_move_back_saturates() {
        assert_eq!(
            translate_configure(i32::MAX, i32::MIN, [5, -5, 1, 1]),
            (i32::MAX - 5, i32::MIN + 5)
        );
    }

    /// A repositioned layer popup offsets its fresh positioner by the same
    /// parent geometry the creation offset used (D5), so a re-opened menu
    /// tracks the pointer against the window, not the surface.
    #[test]
    fn the_reposition_offset_matches_the_creation_offset() {
        let geometry = [5, 7, 640, 480];
        let anchor = [13, 29, 300, 40];
        assert_eq!(
            reposition_offset(Some(anchor), geometry),
            Some(offset_anchor_rect(anchor, Some(geometry))),
            "reposition and creation move the rectangle identically"
        );
    }

    /// A positioner whose anchor rectangle the app never set forwards its
    /// reposition unchanged — nothing to rewrite (D5).
    #[test]
    fn a_reposition_without_an_anchor_rectangle_is_not_rewritten() {
        assert_eq!(reposition_offset(None, [5, 7, 640, 480]), None);
    }
}
