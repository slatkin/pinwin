//! The render state's dirty data (`replace-gtk-with-wayland` D5):
//! what [`crate::render::frame_gate`] reads to decide what a frame must
//! redraw. The libghostty-vt render state tracks dirtiness on two
//! independent layers — a global state (`Clean`, `Partial`, `Full`, read
//! from the frame's snapshot) and a per-row dirty flag (read on the row
//! iterator). An update only escalates both, it never unsets them;
//! `frame_end`'s snapshot-level clean unsets both after a consumed frame, so
//! no per-row setter is needed.
//!
//! Both layers are captured once per frame, in [`Terminal::frame_begin`],
//! and exposed through the small read-only API at the bottom. The capture
//! shares the cell walk's single pass over the grid (see the `cells` module
//! comment), so the dirty rows arrive in walk order with no extra row
//! iterator pass.
//!
//! The row filter ([`Terminal::frame_walk_rows`], [`row_visible`]) is the
//! partial repaint's other half: every cell pass shares the frame walk, so
//! filtering the walk clips them all to the repainted rows at once.
//!
//! GTK-free (`replace-gtk-with-wayland` D10); the tests in this module run
//! without a display.

use libghostty_vt as vt;

use super::{FrameState, Terminal};
use crate::term::Handles;

/// The frame's global dirty state, read from the render state at
/// `frame_begin` (the crate's `render::Dirty`): `Clean` frames draw and
/// commit nothing, `Partial` frames redraw the dirty rows, `Full` frames
/// redraw everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FrameDirty {
    /// Not dirty at all; rendering can be skipped.
    #[default]
    Clean,
    /// Some rows changed; the renderer can redraw incrementally.
    Partial,
    /// Global state changed; the renderer should redraw everything.
    Full,
}

impl FrameDirty {
    /// The frame form of the crate's dirty state; `from_raw` matches every
    /// declared variant, so the mapping is exact.
    pub(crate) fn from_raw(raw: vt::render::Dirty) -> FrameDirty {
        match raw {
            vt::render::Dirty::Clean => FrameDirty::Clean,
            vt::render::Dirty::Partial => FrameDirty::Partial,
            vt::render::Dirty::Full => FrameDirty::Full,
        }
    }
}

impl Terminal {
    /// The global dirty state the render state reported when this frame
    /// began: `Clean` frames draw and commit nothing, `Partial`
    /// frames redraw the dirty rows, `Full` frames redraw everything.
    /// `Clean` also when no frame is open — the painter only asks after a
    /// successful [`frame_begin`](Terminal::frame_begin).
    #[must_use]
    pub fn frame_dirty(&self) -> FrameDirty {
        if self.frame.flags.open {
            self.frame.dirty
        } else {
            FrameDirty::Clean
        }
    }

    /// The viewport rows whose dirty flag the render state reported when
    /// this frame began, in walk order. The flag is conservative:
    /// a row may be listed without its content changing, so redrawing a
    /// listed row is always correct. Empty unless the frame is open.
    #[must_use]
    pub fn frame_dirty_rows(&self) -> &[i32] {
        if self.frame.flags.open {
            &self.frame.dirty_rows
        } else {
            &[]
        }
    }

    /// Whether the render state carries any kitty placement at all (row
    /// 4.8). An image placement changing or being deleted does not
    /// necessarily mark any row dirty, so the frame gate treats every frame
    /// with placements as a full redraw. False unless the frame is open.
    #[must_use]
    pub fn frame_has_images(&self) -> bool {
        self.frame.flags.open && self.frame.has_images
    }

    /// Restrict the frame walk to `rows` for the rest of this frame (row
    /// 4.8): every cell pass shares the walk, so this clips them all to the
    /// partial repaint's rows at once. Cleared by the next
    /// [`frame_begin`](Terminal::frame_begin); a
    /// [`frame_rewind`](Terminal::frame_rewind) keeps it, so every pass of
    /// one frame walks the same rows.
    pub fn frame_walk_rows(&mut self, rows: &[i32]) {
        self.frame.row_filter = Some(rows.to_vec());
    }
}

/// Whether the frame walk visits the viewport row `y` under the partial
/// repaint's row filter. No filter visits everything.
pub(super) fn row_visible(frame: &FrameState, y: i32) -> bool {
    frame
        .row_filter
        .as_ref()
        .is_none_or(|rows| rows.contains(&y))
}

/// The frame's global dirty state, read off the snapshot the update just
/// produced. An unreadable dirty state is treated as a full redraw: the
/// conservative end, like every unknown the frame gate escalates.
pub(super) fn global_dirty(snapshot: &vt::render::Snapshot<'static, 'static>) -> FrameDirty {
    match snapshot.dirty() {
        Ok(dirty) => FrameDirty::from_raw(dirty),
        // An unreadable dirty state is a full redraw: the conservative end,
        // like every unknown the frame gate escalates.
        Err(_) => FrameDirty::Full,
    }
}

/// Capture whether the render state carries any kitty placement at all. The
/// peek runs before the cell pass, so a virtual placement's placeholder
/// origins are not recorded yet — presence alone is what the frame gate
/// escalates on, not resolvability. The image pass re-walks the placement
/// iterator on its own first call, which resets it, so leaving it advanced
/// here is harmless.
pub(super) fn capture_images(frame: &mut FrameState, handles: &mut Handles) {
    let Ok(graphics) = handles.terminal.kitty_graphics() else {
        return;
    };
    let Ok(mut placements) = handles.placement_iterator.update(&graphics) else {
        return;
    };
    if let Some(placement) = placements.next()
        && placement.image_id().is_ok()
    {
        frame.has_images = true;
    }
}

/// Feed the dirty capture a real terminal: text marks its row dirty, a
/// frame that draws and consumes it leaves the next frame clean, and the
/// row filter clips the walk to the rows it names.
#[cfg(test)]
mod tests;
