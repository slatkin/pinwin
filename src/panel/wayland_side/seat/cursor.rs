//! The cursor-shape hook of the seat side (replace-gtk-with-wayland D8):
//! when the compositor offers `wp_cursor_shape_manager_v1`, the
//! panel pointer gets a shape device and each pointer enter sets the default
//! shape. Without the global, the pointer cursor stays unset, the way a
//! client that never sets one leaves it.
//!
//! The device is created once per pointer, when the pointer capability
//! arrives, and the enter serial the shape request needs comes from the
//! enter event itself.

use smithay_client_toolkit::globals::GlobalData;
use smithay_client_toolkit::reexports::client::globals::GlobalList;
use smithay_client_toolkit::reexports::client::protocol::wl_pointer::WlPointer;
use smithay_client_toolkit::reexports::client::{Dispatch, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;

/// The panel pointer's cursor-shape state: the device when the compositor
/// offered the manager global, nothing when it did not.
#[derive(Debug, Default)]
pub struct CursorShape(Option<WpCursorShapeDeviceV1>);

impl CursorShape {
    /// No cursor-shape device: every shape request is a no-op.
    #[must_use]
    pub fn absent() -> Self {
        CursorShape(None)
    }

    /// Bind the manager global and create the pointer's shape device, when
    /// the compositor offers `wp_cursor_shape_manager_v1` (versions 1 to 2,
    /// the range the toolkit binds). Without the global — or when the bind
    /// fails — the cursor stays unset, the spec's rule for a compositor
    /// without the protocol.
    ///
    /// The `Dispatch` bounds are what the panel's dispatch state already
    /// carries: the toolkit's `GlobalData` handles both proxies' (empty)
    /// event sets through its `Dispatch2` impls, which the dispatch state's
    /// blanket `Dispatch` impl picks up.
    #[must_use]
    pub fn bind<D>(globals: &GlobalList, qh: &QueueHandle<D>, pointer: &WlPointer) -> Self
    where
        D: Dispatch<WpCursorShapeManagerV1, GlobalData>
            + Dispatch<WpCursorShapeDeviceV1, GlobalData>
            + 'static,
    {
        let offered = globals.contents().with_list(|list| {
            list.iter()
                .any(|global| global.interface == WpCursorShapeManagerV1::interface().name)
        });
        if !offered {
            return CursorShape::absent();
        }
        let Ok(manager) =
            globals.bind::<WpCursorShapeManagerV1, D, GlobalData>(qh, 1..=2, GlobalData)
        else {
            return CursorShape::absent();
        };
        CursorShape(Some(manager.get_pointer(pointer, qh, GlobalData)))
    }

    /// Set the default cursor shape for one pointer enter. The serial is the
    /// enter event's, the one the protocol requires; a request without a
    /// device does nothing.
    pub fn set_default(&self, serial: u32) {
        if let Some(device) = &self.0 {
            device.set_shape(serial, Shape::Default);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absent device's shape request is a no-op: a compositor without
    /// the global leaves the pointer cursor unset, and nothing panics.
    #[test]
    fn an_absent_device_sets_nothing() {
        CursorShape::absent().set_default(7);
    }
}
