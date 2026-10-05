//! The panel's drawing area as a `DrawingArea` subclass (poc-gsk-texture-grid
//! tasks 2.1/2.2): the subclass overrides `snapshot` so every frame can be
//! presented as GSK nodes — a tween frame presents the cached grid node
//! translated to the dock offset, every other frame rebuilds the grid's
//! nodes (gsk-render-nodes row 4.1) — instead of the cairo draw func, which
//! stays as the fallback the `false` return chains to. Input
//! controllers, `connect_resize` and `set_draw_func` keep attaching to the
//! same widget, so nothing in the input or resize paths moves (task 2.2).
//!
//! The snapshot branch is reached through the [`GridSnapshotFn`] hook, which
//! the panel wires to the existing renderer, terminal and surfaces `Rc`
//! handles — no new globals (poc-gsk-texture-grid design). Panics never
//! cross back into GTK (D5): the snapshot body runs through the shared
//! [`crate::guard`] helper, and a poisoned or panicking frame falls back to
//! the parent snapshot, whose draw func no-ops under the same latch.

use std::cell::OnceCell;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use super::GridSnapshotFn;
use crate::guard::{Poisoned, guard_default};

glib::wrapper! {
    pub struct GridArea(ObjectSubclass<GridAreaImp>) @extends gtk4::DrawingArea, gtk4::Widget, @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

/// The subclass state (poc-gsk-texture-grid design): the grid snapshot
/// hook and the panel's shared D5 latch. Both are set once at
/// construction, before any snapshot can run.
#[derive(Default)]
pub struct GridAreaImp {
    state: OnceCell<(Rc<GridSnapshotFn>, Poisoned)>,
}

impl std::fmt::Debug for GridAreaImp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The hook closure has no `Debug` impl; the latch is set-once
        // construction state, not per-frame data.
        f.debug_struct("GridAreaImp").finish_non_exhaustive()
    }
}

#[glib::object_subclass]
impl ObjectSubclass for GridAreaImp {
    const NAME: &'static str = "PinwinGridArea";
    type Type = GridArea;
    type ParentType = gtk4::DrawingArea;
}

impl ObjectImpl for GridAreaImp {}

impl WidgetImpl for GridAreaImp {
    /// The GSK branch (task 2.1, gsk-render-nodes row 4.1): when the hook
    /// reports that it emitted the frame's nodes, this frame is done;
    /// otherwise chain to the parent snapshot, which runs the set draw
    /// func — the ordinary cairo draw path.
    fn snapshot(&self, snapshot: &gtk4::Snapshot) {
        let obj = self.obj();
        let emitted = self.state.get().is_some_and(|(hook, poisoned)| {
            guard_default(poisoned, false, || {
                hook(snapshot, obj.width(), obj.height())
            })
        });
        if !emitted {
            self.parent_snapshot(snapshot);
        }
    }
}

impl DrawingAreaImpl for GridAreaImp {}

impl GridArea {
    /// The panel's drawing area, taking the panel's shared D5 latch and the
    /// grid snapshot hook (task 2.2: constructed in `Surfaces::build` in
    /// place of `DrawingArea::new()`).
    pub fn new(poisoned: Poisoned, grid_snapshot: Rc<GridSnapshotFn>) -> Self {
        let area: Self = glib::Object::new();
        let _ = area.imp().state.set((grid_snapshot, poisoned));
        area
    }
}
