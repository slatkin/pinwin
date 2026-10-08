//! The render state's dirty data (`replace-gtk-with-wayland` D5):
//! what [`crate::render::frame_gate`] reads to decide what a frame must
//! redraw. The pinned libghostty-vt tracks dirtiness on two independent
//! layers — a global state (`GHOSTTY_RENDER_STATE_DIRTY_FALSE`, `PARTIAL`,
//! `FULL`, read with `GHOSTTY_RENDER_STATE_DATA_DIRTY`) and a per-row dirty
//! flag (`GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY`, read on the row iterator).
//! `ghostty_render_state_update` only updates both, it never unsets them;
//! `ghostty_render_state_clean` — [`Terminal::frame_end`] — unsets both
//! after a consumed frame, so no per-row setter is needed.
//!
//! Both layers are captured once per frame, in [`Terminal::frame_begin`]
//! ([`capture_dirty`], [`capture_images`]), and exposed through the small
//! read-only API at the bottom. The walk that captures the per-row flags
//! leaves the row iterator at the end of the grid, so it re-fetches the
//! iterator afterwards — the same re-fetch the glyph pass's rewind does —
//! and the cell passes start from the first row.
//!
//! The row filter ([`Terminal::frame_walk_rows`], [`row_visible`]) is the
//! partial repaint's other half: every cell pass shares the frame walk, so
//! filtering the walk clips them all to the repainted rows at once.
//!
//! GTK-free (`replace-gtk-with-wayland` D10); the tests in this module run
//! without a display.

use std::ptr;

use super::{FrameState, Handles, Terminal};
use crate::ghostty_sys::GHOSTTY_SUCCESS;
use crate::ghostty_sys::kitty::{
    GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
    GhosttyKittyGraphics, ghostty_kitty_graphics_get, ghostty_kitty_graphics_placement_get,
    ghostty_kitty_graphics_placement_next,
};
use crate::ghostty_sys::render::{
    GHOSTTY_RENDER_STATE_DATA_DIRTY, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
    GHOSTTY_RENDER_STATE_DIRTY_FALSE, GHOSTTY_RENDER_STATE_DIRTY_PARTIAL,
    GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y,
    GhosttyRenderState, GhosttyRenderStateDirty, GhosttyRenderStateRowIterator,
    ghostty_render_state_get, ghostty_render_state_row_get, ghostty_render_state_row_iterator_next,
};
use crate::ghostty_sys::terminal::{GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, ghostty_terminal_get};

/// The frame's global dirty state, read from the render state at
/// `frame_begin` (`GHOSTTY_RENDER_STATE_DIRTY_*`, render.h): `Clean` frames
/// draw and commit nothing, `Partial` frames redraw the dirty rows, `Full`
/// frames redraw everything.
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
    /// The Rust form of the raw `GhosttyRenderStateDirty` value. A value
    /// that is none of the declared constants reads as `Full` — the
    /// conservative end, like every unknown the frame gate escalates.
    pub(crate) fn from_raw(raw: GhosttyRenderStateDirty) -> FrameDirty {
        match raw {
            GHOSTTY_RENDER_STATE_DIRTY_FALSE => FrameDirty::Clean,
            GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => FrameDirty::Partial,
            // The catch-all covers `FULL` and every undeclared value.
            _ => FrameDirty::Full,
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

/// Capture the render state's dirty data for this frame: the
/// global state and the per-row dirty flags, walked off the row iterator.
/// `update` only updates the dirty state — it never unsets it — so what is
/// captured here is everything that changed since the last
/// [`end`](crate::ghostty_sys::render::ghostty_render_state_clean). The
/// walk leaves the row iterator at the end of the grid, so it re-fetches
/// the iterator afterwards, the same re-fetch the glyph pass's rewind
/// does, and the cell passes start from the first row.
pub(super) fn capture_dirty(
    frame: &mut FrameState,
    render_state: GhosttyRenderState,
    row_iterator: &mut GhosttyRenderStateRowIterator,
) {
    let mut dirty: GhosttyRenderStateDirty = 0;
    // SAFETY: `dirty` is writable storage of the type `DATA_DIRTY` returns
    // and the render state is live.
    frame.dirty = if unsafe {
        ghostty_render_state_get(
            render_state,
            GHOSTTY_RENDER_STATE_DATA_DIRTY,
            ptr::from_mut(&mut dirty).cast(),
        )
    } == GHOSTTY_SUCCESS
    {
        FrameDirty::from_raw(dirty)
    } else {
        // An unreadable dirty state is treated as a full redraw: the
        // conservative end, like every unknown the frame gate escalates.
        FrameDirty::Full
    };

    frame.dirty_rows.clear();
    loop {
        // SAFETY: the row iterator was filled from the live render state.
        if !unsafe { ghostty_render_state_row_iterator_next(*row_iterator) } {
            break;
        }
        let mut viewport_y: i32 = 0;
        // SAFETY: `viewport_y` is writable storage of the expected type and
        // the row iterator is live.
        if unsafe {
            ghostty_render_state_row_get(
                *row_iterator,
                GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y,
                ptr::from_mut(&mut viewport_y).cast(),
            )
        } != GHOSTTY_SUCCESS
        {
            continue;
        }
        let mut dirty = false;
        // SAFETY: `dirty` is writable storage of the expected type and the
        // row iterator is live.
        if unsafe {
            ghostty_render_state_row_get(
                *row_iterator,
                GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                ptr::from_mut(&mut dirty).cast(),
            )
        } == GHOSTTY_SUCCESS
            && dirty
        {
            frame.dirty_rows.push(viewport_y);
        }
    }

    // Re-fetch the iterator so the cell passes start from the first row.
    // SAFETY: the render state is live and the iterator is writable storage
    // of the type the selector returns.
    let _ = unsafe {
        ghostty_render_state_get(
            render_state,
            GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            ptr::from_mut(row_iterator).cast(),
        )
    };
}

/// Capture whether the render state carries any kitty placement at all. The walk stops at the first placement with an image id; the
/// image pass re-fetches the placement iterator through `begin_image_pass`,
/// which resets it, so leaving it advanced here is harmless. The peek runs
/// before the cell pass, so a virtual placement's placeholder origins are
/// not recorded yet — presence alone is what the frame gate escalates on,
/// not resolvability.
pub(super) fn capture_images(frame: &mut FrameState, handles: &mut Handles) {
    let Some(terminal) = handles.terminal else {
        return;
    };
    let mut graphics = GhosttyKittyGraphics(ptr::null_mut());
    // SAFETY: the terminal is live and `graphics` is writable storage of
    // the type `GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS` returns.
    let got = unsafe {
        ghostty_terminal_get(
            terminal,
            GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS,
            (&raw mut graphics).cast(),
        )
    };
    if got != GHOSTTY_SUCCESS || graphics.0.is_null() {
        return;
    }
    // No placement iterator means the image pass can draw no images
    // either, so reporting "no images" here stays consistent with what
    // the frame will actually draw.
    let Some(mut iterator) = handles.placement_iterator else {
        return;
    };
    // SAFETY: `graphics` is live and the iterator is writable storage
    // created by `ghostty_kitty_graphics_placement_iterator_new`.
    if unsafe {
        ghostty_kitty_graphics_get(
            graphics,
            GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR,
            ptr::from_mut(&mut iterator).cast(),
        )
    } != GHOSTTY_SUCCESS
    {
        return;
    }
    // SAFETY: the iterator was initialized from the live graphics storage
    // above.
    while unsafe { ghostty_kitty_graphics_placement_next(iterator) } {
        let mut image_id: u32 = 0;
        // SAFETY: `image_id` is writable storage of the type the selector
        // returns and the iterator is live.
        if unsafe {
            ghostty_kitty_graphics_placement_get(
                iterator,
                GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
                (&raw mut image_id).cast(),
            )
        } == GHOSTTY_SUCCESS
        {
            frame.has_images = true;
            return;
        }
    }
}

/// Feed the dirty capture a real terminal: text marks its row dirty, a
/// frame that draws and consumes it leaves the next frame clean, and the
/// row filter clips the walk to the rows it names.
#[cfg(test)]
mod tests;
