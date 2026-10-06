//! The panel's drawing area as a `DrawingArea` subclass. The GSK snapshot
//! override went away with the node painter (replace-gtk-with-wayland D11):
//! the area now draws only through the draw function `Surfaces::build` sets.
//! The named subclass stays until the GTK path itself goes (row 8.1).

use gtk4::glib;
use gtk4::subclass::prelude::*;

glib::wrapper! {
    pub struct GridArea(ObjectSubclass<GridAreaImp>) @extends gtk4::DrawingArea, gtk4::Widget, @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

#[derive(Default, Debug)]
pub struct GridAreaImp;

#[glib::object_subclass]
impl ObjectSubclass for GridAreaImp {
    const NAME: &'static str = "PinwinGridArea";
    type Type = GridArea;
    type ParentType = gtk4::DrawingArea;
}

impl ObjectImpl for GridAreaImp {}

impl WidgetImpl for GridAreaImp {}

impl DrawingAreaImpl for GridAreaImp {}

impl Default for GridArea {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl GridArea {
    /// The panel's drawing area; it draws through the draw function
    /// `Surfaces::build` sets.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}
