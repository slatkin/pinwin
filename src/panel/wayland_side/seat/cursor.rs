//! The cursor-shape hook of the seat side (replace-gtk-with-wayland D8,
//! row 5.5): when the compositor offers `wp_cursor_shape_manager_v1`, the
//! panel pointer gets a shape device and each pointer enter sets the default
//! shape — the cursor the GDK path showed over the drawing area. Without the
//! global, the pointer cursor stays unset, the way a client that never sets
//! one leaves it.
//!
//! The device is created once per pointer, at the row 8.1 bind, and the
//! enter serial the shape request needs comes from the enter event itself.

use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};

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

    /// Set the default cursor shape for one pointer enter. The serial is the
    /// enter event's, the one the protocol requires; a request without a
    /// device does nothing.
    pub fn set_default(&self, serial: u32) {
        if let Some(device) = &self.0 {
            device.set_shape(serial, Shape::Default);
        }
    }
}
