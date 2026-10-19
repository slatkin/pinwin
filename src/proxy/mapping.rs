//! Settings-to-protocol mappers for the panel's layer surfaces
//! (replace-terminal-with-app-host D9): the [`PanelSettings`] values the
//! panel and its reserve surface are built from, translated into the
//! `zwlr_layer_shell_v1` arguments the compositor expects.
//!
//! Shared by the window routing ([`super::routing`]) and the reserve surface
//! ([`super::reserve`]), which must anchor to the same edge and length so the
//! strip and the panel line up on one monitor. The strip's layer is its own,
//! fixed below every other reservation.

use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::ZwlrLayerShellV1Layer;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::{
    ZwlrLayerSurfaceV1Anchor, ZwlrLayerSurfaceV1KeyboardInteractivity,
};

use crate::config::{Keyboard, Layer};
use crate::geometry::{Anchors, Edge};

/// The protocol anchor bits of a panel's anchors (D9).
pub(super) const fn anchor_of(anchors: Anchors) -> ZwlrLayerSurfaceV1Anchor {
    let mut anchor = ZwlrLayerSurfaceV1Anchor::empty();
    if anchors.contains(Edge::Left) {
        anchor = anchor.union(ZwlrLayerSurfaceV1Anchor::LEFT);
    }
    if anchors.contains(Edge::Right) {
        anchor = anchor.union(ZwlrLayerSurfaceV1Anchor::RIGHT);
    }
    if anchors.contains(Edge::Top) {
        anchor = anchor.union(ZwlrLayerSurfaceV1Anchor::TOP);
    }
    if anchors.contains(Edge::Bottom) {
        anchor = anchor.union(ZwlrLayerSurfaceV1Anchor::BOTTOM);
    }
    anchor
}

/// The protocol layer of a panel's layer setting.
pub(super) const fn layer_of(layer: Layer) -> ZwlrLayerShellV1Layer {
    match layer {
        Layer::Background => ZwlrLayerShellV1Layer::BACKGROUND,
        Layer::Bottom => ZwlrLayerShellV1Layer::BOTTOM,
        Layer::Top => ZwlrLayerShellV1Layer::TOP,
        Layer::Overlay => ZwlrLayerShellV1Layer::OVERLAY,
    }
}

/// The protocol keyboard mode of a panel's keyboard setting.
pub(super) const fn keyboard_of(keyboard: Keyboard) -> ZwlrLayerSurfaceV1KeyboardInteractivity {
    match keyboard {
        Keyboard::None => ZwlrLayerSurfaceV1KeyboardInteractivity::NONE,
        Keyboard::OnDemand => ZwlrLayerSurfaceV1KeyboardInteractivity::ON_DEMAND,
        Keyboard::Exclusive => ZwlrLayerSurfaceV1KeyboardInteractivity::EXCLUSIVE,
    }
}
