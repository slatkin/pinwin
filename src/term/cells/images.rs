//! Kitty graphics image iteration for the frame protocol, ported from the
//! image half of `src/cells.zig` (`pinwin_image_next`, the placeholder origin
//! map) and the `PinwinImage` struct in `src/pinwin.h`.
//!
//! A unicode-placeholder placement (kitty's virtual placement for a run of
//! U+10EEEE cells) has no viewport position of its own: it is drawn where its
//! placeholder cells are, at the image's own pixel size. The placeholder cell
//! carries only the low 24 bits of the image id in its foreground colour; the
//! high byte travels in the third diacritic, which is the placement id. Origins
//! are therefore keyed by those low 24 bits, and two placements of the same
//! image share the first one's origin (design D4 of `port-to-rust`).

use std::mem;
use std::ptr;

use crate::ghostty_sys::GHOSTTY_SUCCESS;
use crate::ghostty_sys::kitty::{
    GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
    GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL, GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z,
    GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR, GHOSTTY_KITTY_IMAGE_DATA_GENERATION,
    GHOSTTY_KITTY_IMAGE_DATA_HEIGHT, GHOSTTY_KITTY_IMAGE_DATA_WIDTH, GhosttyKittyGraphics,
    GhosttyKittyGraphicsImageData, GhosttyKittyGraphicsPlacementRenderInfo,
    ghostty_kitty_graphics_get, ghostty_kitty_graphics_image,
    ghostty_kitty_graphics_image_get_multi, ghostty_kitty_graphics_placement_get,
    ghostty_kitty_graphics_placement_next, ghostty_kitty_graphics_placement_render_info,
};
use crate::ghostty_sys::terminal::{GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, ghostty_terminal_get};
use crate::term::Handles;

use super::{FrameState, Image};

/// A placeholder cell carries only the low 24 bits of the image id in its
/// foreground colour; the high byte travels in the third diacritic.
pub(super) const PLACEHOLDER_ID_MASK: u32 = 0xFF_FFFF;

/// Origins are keyed by the masked image id and capped so a frame cannot grow
/// without bound; beyond the cap the oldest-kept entries win.
const MAX_PLACEHOLDERS: usize = 8;

/// Where a virtual placement's placeholder cells first appeared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct PlaceholderOrigin {
    pub(super) image_id: u32,
    pub(super) row: i32,
    pub(super) col: i32,
}

/// The per-frame placeholder origin map.
#[derive(Default)]
pub(super) struct PlaceholderMap {
    origins: Vec<PlaceholderOrigin>,
}

impl PlaceholderMap {
    /// Forget the previous frame's origins.
    pub(super) fn clear(&mut self) {
        self.origins.clear();
    }

    /// Record that a placeholder cell for `image_id` was seen at `(row, col)`,
    /// keeping the top-left-most origin for that image.
    pub(super) fn note(&mut self, image_id: u32, row: i32, col: i32) {
        let key = image_id & PLACEHOLDER_ID_MASK;
        for origin in &mut self.origins {
            if origin.image_id != key {
                continue;
            }
            if row < origin.row || (row == origin.row && col < origin.col) {
                origin.row = row;
                origin.col = col;
            }
            return;
        }
        if self.origins.len() == MAX_PLACEHOLDERS {
            return;
        }
        self.origins.push(PlaceholderOrigin {
            image_id: key,
            row,
            col,
        });
    }

    /// The origin recorded for `image_id`, matched on its low 24 bits.
    pub(super) fn origin(&self, image_id: u32) -> Option<PlaceholderOrigin> {
        let key = image_id & PLACEHOLDER_ID_MASK;
        self.origins.iter().copied().find(|o| o.image_id == key)
    }
}

/// The destination and source rectangles derived for one placement.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Rect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) w: i32,
    pub(super) h: i32,
    pub(super) sx: i32,
    pub(super) sy: i32,
    pub(super) sw: i32,
    pub(super) sh: i32,
}

/// A virtual placement is drawn at its placeholder origin, at the image's own
/// pixel size (the program sizes it to that cell box).
pub(super) fn virtual_rect(
    origin: PlaceholderOrigin,
    cell_w: u32,
    cell_h: u32,
    image_w: u32,
    image_h: u32,
) -> Rect {
    Rect {
        x: origin.col * cell_w as i32,
        y: origin.row * cell_h as i32,
        w: image_w as i32,
        h: image_h as i32,
        sx: 0,
        sy: 0,
        sw: image_w as i32,
        sh: image_h as i32,
    }
}

/// A viewport placement is drawn at its resolved viewport column/row and pixel
/// size, with the resolved source rectangle.
pub(super) fn viewport_rect(
    info: &GhosttyKittyGraphicsPlacementRenderInfo,
    cell_w: u32,
    cell_h: u32,
) -> Rect {
    Rect {
        x: info.viewport_col * cell_w as i32,
        y: info.viewport_row * cell_h as i32,
        w: info.pixel_width as i32,
        h: info.pixel_height as i32,
        sx: info.source_x as i32,
        sy: info.source_y as i32,
        sw: info.source_width as i32,
        sh: info.source_height as i32,
    }
}

/// The next image placement of the frame, or `None` at the end.
pub(super) fn image_next(
    frame: &mut FrameState,
    handles: &mut Handles,
    cell_w: u32,
    cell_h: u32,
) -> Option<Image> {
    if !frame.open {
        return None;
    }

    if !frame.images_started {
        frame.images_started = true;
        let terminal = handles.terminal.expect("image_next with a live terminal");
        let mut graphics = GhosttyKittyGraphics(ptr::null_mut());
        // SAFETY: the terminal is live and `graphics` is writable storage of
        // the type `GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS` returns.
        let got = unsafe {
            ghostty_terminal_get(
                terminal,
                GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS,
                (&mut graphics as *mut GhosttyKittyGraphics).cast(),
            )
        };
        if got != GHOSTTY_SUCCESS || graphics.0.is_null() {
            return None;
        }
        frame.graphics = Some(graphics);
        // SAFETY: `graphics` is live and the placement iterator is writable
        // storage created by `ghostty_kitty_graphics_placement_iterator_new`.
        let mut placement_iterator = handles
            .placement_iterator
            .expect("image_next with a placement iterator");
        let iterator = unsafe {
            ghostty_kitty_graphics_get(
                graphics,
                GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR,
                ptr::from_mut(&mut placement_iterator).cast(),
            )
        };
        if iterator != GHOSTTY_SUCCESS {
            return None;
        }
        handles.placement_iterator = Some(placement_iterator);
    }

    let graphics = frame.graphics?;
    let iterator = handles.placement_iterator.expect("image_next iterator");
    if iterator.0.is_null() {
        return None;
    }

    // SAFETY: `iterator` was initialized from the live graphics storage above.
    while unsafe { ghostty_kitty_graphics_placement_next(iterator) } {
        let mut image_id: u32 = 0;
        let mut z: i32 = 0;
        let mut is_virtual = false;
        // SAFETY: each out pointer is writable storage of the type its data
        // selector names; the placement iterator is live.
        let have_id = unsafe {
            ghostty_kitty_graphics_placement_get(
                iterator,
                GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
                (&mut image_id as *mut u32).cast(),
            )
        } == GHOSTTY_SUCCESS;
        if !have_id {
            continue;
        }
        unsafe {
            ghostty_kitty_graphics_placement_get(
                iterator,
                GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z,
                (&mut z as *mut i32).cast(),
            );
            ghostty_kitty_graphics_placement_get(
                iterator,
                GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL,
                (&mut is_virtual as *mut bool).cast(),
            );
        }

        // SAFETY: `graphics` is live and `image_id` was read from it.
        let image = unsafe { ghostty_kitty_graphics_image(graphics, image_id) };
        if image.0.is_null() {
            continue;
        }

        let mut pixels: *const u8 = ptr::null();
        let mut image_w: u32 = 0;
        let mut image_h: u32 = 0;
        let mut generation: u64 = 0;
        let kinds: [GhosttyKittyGraphicsImageData; 4] = [
            GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR,
            GHOSTTY_KITTY_IMAGE_DATA_WIDTH,
            GHOSTTY_KITTY_IMAGE_DATA_HEIGHT,
            GHOSTTY_KITTY_IMAGE_DATA_GENERATION,
        ];
        let mut values: [*mut std::os::raw::c_void; 4] = [
            (&mut pixels as *mut *const u8).cast(),
            (&mut image_w as *mut u32).cast(),
            (&mut image_h as *mut u32).cast(),
            (&mut generation as *mut u64).cast(),
        ];
        // SAFETY: `image` is live; `kinds`/`values` are parallel arrays of
        // matching length and each value points at writable storage.
        let ok = unsafe {
            ghostty_kitty_graphics_image_get_multi(
                image,
                kinds.len(),
                kinds.as_ptr(),
                values.as_mut_ptr(),
                ptr::null_mut(),
            )
        } == GHOSTTY_SUCCESS;
        if !ok || pixels.is_null() || image_w == 0 || image_h == 0 {
            continue;
        }

        let mut out = Image {
            image_id,
            generation: generation as i64,
            z,
            image_w: image_w as i32,
            image_h: image_h as i32,
            pixels,
            ..Image::default()
        };

        if is_virtual {
            let Some(origin) = frame.placeholders.origin(image_id) else {
                continue;
            };
            let rect = virtual_rect(origin, cell_w, cell_h, image_w, image_h);
            out.apply_rect(rect);
            return Some(out);
        }

        // SAFETY: `info` is a sized struct of the type the placement expects.
        let mut info = unsafe { mem::zeroed::<GhosttyKittyGraphicsPlacementRenderInfo>() };
        info.size = mem::size_of::<GhosttyKittyGraphicsPlacementRenderInfo>();
        // SAFETY: `iterator`, `image` and the terminal are live, and `info` is
        // a sized struct of the expected type.
        let resolved = unsafe {
            ghostty_kitty_graphics_placement_render_info(
                iterator,
                image,
                handles.terminal.expect("image_next with a live terminal"),
                (&mut info as *mut GhosttyKittyGraphicsPlacementRenderInfo).cast(),
            )
        } == GHOSTTY_SUCCESS;
        if !resolved || !info.viewport_visible {
            continue;
        }
        out.apply_rect(viewport_rect(&info, cell_w, cell_h));
        return Some(out);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_ids_are_masked_and_merge_top_left() {
        let mut map = PlaceholderMap::default();
        // The same low-24-bit id with different placement ids shares one entry.
        map.note(0x0100_0001, 5, 3);
        map.note(0x0200_0001, 2, 7);
        let origin = map.origin(0x0300_0001).expect("masked lookup");
        assert_eq!(origin.image_id, 0x0000_0001);
        assert_eq!((origin.row, origin.col), (2, 7), "top-left-most wins");

        // A later, lower-left origin also wins.
        map.note(0x0000_0001, 2, 4);
        assert_eq!(map.origin(0x0000_0001).unwrap().col, 4);

        // A different id is a separate entry.
        map.note(0x0000_0002, 0, 0);
        assert_eq!(map.origin(0x0000_0002).unwrap().image_id, 0x0000_0002);
        assert_eq!(map.origins.len(), 2);

        // The high byte is discarded, so any placement of the image matches.
        assert!(map.origin(0xAB00_0001).is_some());
    }

    #[test]
    fn placeholder_map_caps_at_eight_origins() {
        let mut map = PlaceholderMap::default();
        for id in 0..12u32 {
            map.note(id, 0, 0);
        }
        assert_eq!(map.origins.len(), MAX_PLACEHOLDERS);
        assert!(map.origin(7).is_some());
        assert!(map.origin(8).is_none());
    }

    #[test]
    fn virtual_placement_uses_the_origin_cell_box() {
        let origin = PlaceholderOrigin {
            image_id: 1,
            row: 2,
            col: 3,
        };
        let rect = virtual_rect(origin, 8, 16, 32, 48);
        assert_eq!(
            rect,
            Rect {
                x: 24,
                y: 32,
                w: 32,
                h: 48,
                sx: 0,
                sy: 0,
                sw: 32,
                sh: 48,
            }
        );
    }

    #[test]
    fn viewport_placement_uses_resolved_geometry() {
        let info = GhosttyKittyGraphicsPlacementRenderInfo {
            size: mem::size_of::<GhosttyKittyGraphicsPlacementRenderInfo>(),
            pixel_width: 40,
            pixel_height: 20,
            grid_cols: 5,
            grid_rows: 2,
            viewport_col: 4,
            viewport_row: 1,
            viewport_visible: true,
            source_x: 2,
            source_y: 3,
            source_width: 40,
            source_height: 20,
        };
        let rect = viewport_rect(&info, 8, 16);
        assert_eq!(rect.x, 32);
        assert_eq!(rect.y, 16);
        assert_eq!((rect.w, rect.h), (40, 20));
        assert_eq!((rect.sx, rect.sy, rect.sw, rect.sh), (2, 3, 40, 20));
    }
}
