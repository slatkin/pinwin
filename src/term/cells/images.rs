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
//!
//! Like the cells, the placements are captured as plain data: the image
//! pass's first call in a frame walks the placement iterator once, resolves
//! every visible placement against the terminal, and stores the resulting
//! [`Image`]s on the frame; later calls hand them out one at a time. The
//! pixel pointer outlives the walk the same way it always did: it points at
//! the terminal's own storage, valid until the next mutating terminal call.

use libghostty_vt as vt;

use super::{FrameState, Image};
use crate::term::Handles;

/// A placeholder cell carries only the low 24 bits of the image id in its
/// foreground colour; the high byte travels in the third diacritic.
pub(super) const PLACEHOLDER_ID_MASK: u32 = 0xFF_FFFF;

/// Origins are keyed by the masked image id and capped so a frame cannot grow
/// without bound; beyond the cap new entries are dropped. The bound has to
/// clear a real grid's worth of distinct images (a media browser shows more
/// than a handful of covers at once), so it is generous; the map itself is a
/// dozen bytes per entry.
const MAX_PLACEHOLDERS: usize = 64;

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

/// One device pixel size in logical pixels (device-pixel-image-size D2):
/// `device * 120 / scale_120`, rounded half up in exact integer arithmetic —
/// the inverse of the size report's `device_cell`. At scale 1
/// (`scale_120 == 120`) it is the identity. A zero note (a broken
/// compositor's preferred scale) is treated as scale 1, the note's initial
/// value, so the image pass cannot panic on a division by zero.
#[must_use]
fn logical_from_device(device: u32, scale_120: u32) -> u32 {
    if scale_120 == 0 {
        return device;
    }
    // Half up: floor((device*120/scale_120) + 1/2) computed as
    // (2*num + den) / (2*den); the product cannot overflow `u64`.
    let numerator = u64::from(device) * 120;
    let denominator = u64::from(scale_120);
    u32::try_from((2 * numerator + denominator) / (2 * denominator)).unwrap_or(u32::MAX)
}

/// A virtual placement is drawn at its placeholder origin, at the image's own
/// pixel size divided by the scale (device-pixel-image-size D2): one image
/// pixel is one device pixel. The source rectangle stays in image pixels
/// (D4).
pub(super) fn virtual_rect(
    origin: PlaceholderOrigin,
    cell_w: u32,
    cell_h: u32,
    image_w: u32,
    image_h: u32,
    scale_120: u32,
) -> Rect {
    Rect {
        x: origin.col * cell_w.cast_signed(),
        y: origin.row * cell_h.cast_signed(),
        w: logical_from_device(image_w, scale_120).cast_signed(),
        h: logical_from_device(image_h, scale_120).cast_signed(),
        sx: 0,
        sy: 0,
        sw: image_w.cast_signed(),
        sh: image_h.cast_signed(),
    }
}

/// A placement's destination size. A placement that gives neither a column
/// nor a row count is sized from its own pixels — device pixels, so they
/// divide by the scale (device-pixel-image-size D3). A placement that gives
/// either count is sized from the logical cell already: ghostty resolves
/// `pixel_width`/`pixel_height` from it, and dividing again would shrink
/// the image.
#[must_use]
fn destination_size(
    pixel_width: u32,
    pixel_height: u32,
    columns: u32,
    rows: u32,
    scale_120: u32,
) -> (i32, i32) {
    if columns == 0 && rows == 0 {
        (
            logical_from_device(pixel_width, scale_120).cast_signed(),
            logical_from_device(pixel_height, scale_120).cast_signed(),
        )
    } else {
        (pixel_width.cast_signed(), pixel_height.cast_signed())
    }
}

/// A viewport placement is drawn at its resolved viewport column/row and
/// pixel size, with the resolved source rectangle. `columns`/`rows` are the
/// placement's own counts (0 = not given); only an image-sized placement's
/// destination divides by the scale (device-pixel-image-size D3).
pub(super) fn viewport_rect(
    info: &vt::kitty::graphics::PlacementRenderInfo,
    cell_w: u32,
    cell_h: u32,
    scale_120: u32,
    columns: u32,
    rows: u32,
) -> Rect {
    let (w, h) = destination_size(
        info.pixel_width,
        info.pixel_height,
        columns,
        rows,
        scale_120,
    );
    Rect {
        x: info.viewport_col * cell_w.cast_signed(),
        y: info.viewport_row * cell_h.cast_signed(),
        w,
        h,
        sx: info.source_x.cast_signed(),
        sy: info.source_y.cast_signed(),
        sw: info.source_width.cast_signed(),
        sh: info.source_height.cast_signed(),
    }
}

/// The next image placement of the frame, or `None` at the end. `scale_120`
/// is the terminal's shared scale note (device-pixel-image-size D1), the
/// same note `size_report` answers from. The first call in a frame captures
/// every placement; later calls hand the capture out.
pub(super) fn image_next(
    frame: &mut FrameState,
    handles: &mut Handles,
    cell_w: u32,
    cell_h: u32,
    scale_120: u32,
) -> Option<Image> {
    if !frame.flags.open {
        return None;
    }
    if !frame.flags.images_started {
        frame.flags.images_started = true;
        capture_images(frame, handles, cell_w, cell_h, scale_120);
    }
    if frame.images.is_empty() {
        return None;
    }
    // Hand the images out in capture order, one per call, like the old
    // streaming walk did.
    Some(frame.images.remove(0))
}

/// Walk the placement iterator once and resolve every drawable placement
/// against the terminal into the frame's capture. A placement that cannot be
/// resolved (a missing image, pending pixels, an off-viewport rectangle) is
/// skipped, as the old walk skipped it.
fn capture_images(
    frame: &mut FrameState,
    handles: &mut Handles,
    cell_w: u32,
    cell_h: u32,
    scale_120: u32,
) {
    let Ok(graphics) = handles.terminal.kitty_graphics() else {
        return;
    };
    let Ok(mut placements) = handles.placement_iterator.update(&graphics) else {
        return;
    };

    while let Some(placement) = placements.next() {
        let (Ok(image_id), Ok(z), Ok(is_virtual)) =
            (placement.image_id(), placement.z(), placement.is_virtual())
        else {
            continue;
        };
        let Some(image) = graphics.image(image_id) else {
            continue;
        };
        let (Ok(image_w), Ok(image_h), Ok(generation)) =
            (image.width(), image.height(), image.generation())
        else {
            continue;
        };
        if image_w == 0 || image_h == 0 {
            continue;
        }
        let Ok(Some(pixels)) = image.data() else {
            continue;
        };
        // The painter reads `image_w * image_h * 4` bytes through the
        // pointer; an image whose stored buffer is smaller is not drawable.
        let needed = usize::try_from(image_w)
            .ok()
            .and_then(|w| usize::try_from(image_h).ok().and_then(|h| w.checked_mul(h)))
            .and_then(|pixels| pixels.checked_mul(4));
        let Some(needed) = needed else { continue };
        if pixels.len() < needed {
            continue;
        }

        let mut out = Image {
            image_id,
            generation: generation.cast_signed(),
            z,
            image_w: image_w.cast_signed(),
            image_h: image_h.cast_signed(),
            pixels: pixels.as_ptr(),
            ..Image::default()
        };

        if is_virtual {
            let Some(origin) = frame.placeholders.origin(image_id) else {
                continue;
            };
            let rect = virtual_rect(origin, cell_w, cell_h, image_w, image_h, scale_120);
            out.apply_rect(rect);
            frame.images.push(out);
            continue;
        }

        let Ok(info) = placement.placement_render_info(&image, &handles.terminal) else {
            continue;
        };
        if !info.viewport_visible {
            continue;
        }
        let (columns, rows) = (
            placement.columns().unwrap_or(0),
            placement.rows().unwrap_or(0),
        );
        out.apply_rect(viewport_rect(
            &info, cell_w, cell_h, scale_120, columns, rows,
        ));
        frame.images.push(out);
    }
}

/// The image id the captured mbv 0.22.5 bytes carry (see
/// [`mbv_replay_bytes`]).
#[cfg(test)]
pub(crate) const IMAGE_ID: u32 = 536_687_111;

/// The captured mbv 0.22.5 image transmission (see the `tests` module):
/// a chunked `a=T` transmission with raw `f=32` pixels and `U=1`
/// (unicode placeholders), then the placeholder cell the program prints —
/// its foreground colour carries the image id's low 24 bits. Shared with
/// the render painters' tests.
#[cfg(test)]
pub(crate) fn mbv_replay_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    // 8x16 RGBA pixels (one 8x16 cell), chunked like mbv's `m=1`/`m=0`.
    bytes.extend_from_slice(b"\x1b_Gi=536687111,a=T,U=1,f=32,t=d,s=8,v=16,q=2,m=1;");
    bytes.extend_from_slice(b"AACA/yAAgP9AAID/YACA/4AAgP+gAID/wACA/+AAgP8AEID/IBCA/0AQgP9gEID/gBCA/6AQgP/AEID/4BCA/wAggP8gIID/QCCA/2AggP+AIID/oCCA/8AggP/gIID/ADCA/yAwgP9AMID/YDCA/4AwgP+gMID/wDCA/+AwgP8AQID/IECA/0BAgP9gQID/gECA/6BAgP/AQID/4ECA/wBQgP8gUID/QFCA/2BQgP+AUID/oFCA/8BQgP/gUID/AGCA/yBggP9AYID/YGCA/4BggP+gYID/wGCA/+BggP8AcID/IHCA/0BwgP9gcID/gHCA/6BwgP/AcID/4HCA/wCA");
    bytes.extend_from_slice(b"\x1b_Gq=2,m=0;gP8ggID/QICA/2CAgP+AgID/oICA/8CAgP/ggID/AJCA/yCQgP9AkID/YJCA/4CQgP+gkID/wJCA/+CQgP8AoID/IKCA/0CggP9goID/gKCA/6CggP/AoID/4KCA/wCwgP8gsID/QLCA/2CwgP+AsID/oLCA/8CwgP/gsID/AMCA/yDAgP9AwID/YMCA/4DAgP+gwID/wMCA/+DAgP8A0ID/INCA/0DQgP9g0ID/gNCA/6DQgP/A0ID/4NCA/wDggP8g4ID/QOCA/2DggP+A4ID/oOCA/8DggP/g4ID/APCA/yDwgP9A8ID/YPCA/4DwgP+g8ID/wPCA/+DwgP8=\x1b\\");
    // The placeholder cell the program prints for the image: the cell's
    // foreground colour carries the image id's low 24 bits (the captured
    // mbv stream sets `38;2;253;50;7` = 0xFD3207 well before the run, and
    // SGR state persists across the APCs).
    bytes.extend_from_slice(b"\x1b[3;5H\x1b[38;2;253;50;7m");
    bytes.extend_from_slice("\u{10EEEE}\u{305}\u{305}\u{484}".as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Poisoned;
    use crate::term::{DecodedPng, PngDecoder, PtySink, Terminal};

    /// Rejects every image; these tests do not exercise PNG decoding.
    struct NoDecoder;

    impl PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
            None
        }
    }

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
    fn placeholder_map_caps_at_max_placeholders() {
        let mut map = PlaceholderMap::default();
        for id in 0..70u32 {
            map.note(id, 0, 0);
        }
        assert_eq!(map.origins.len(), MAX_PLACEHOLDERS);
        assert!(map.origin(63).is_some());
        assert!(map.origin(64).is_none());
    }

    #[test]
    fn device_pixels_convert_to_logical_pixels_at_the_scale() {
        // Scale 1 (the note's 120) is the identity.
        assert_eq!(logical_from_device(576, 120), 576);
        assert_eq!(logical_from_device(324, 120), 324);
        assert_eq!(logical_from_device(0, 120), 0);

        // Scale 1.8 (216): the proposal's 576x324 thumbnail becomes 320x180.
        assert_eq!(logical_from_device(576, 216), 320);
        assert_eq!(logical_from_device(324, 216), 180);

        // Rounded half up: 2.5 rounds to 3 at scale 1.2 (144).
        assert_eq!(logical_from_device(3, 144), 3);
    }

    #[test]
    fn virtual_placement_uses_the_origin_cell_box() {
        let origin = PlaceholderOrigin {
            image_id: 1,
            row: 2,
            col: 3,
        };
        let rect = virtual_rect(origin, 8, 16, 32, 48, 120);
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
    fn virtual_placement_divides_the_destination_at_a_fractional_scale() {
        let origin = PlaceholderOrigin {
            image_id: 1,
            row: 0,
            col: 0,
        };
        let rect = virtual_rect(origin, 8, 16, 576, 324, 216);
        assert_eq!(
            (rect.w, rect.h),
            (320, 180),
            "one image pixel, one device pixel"
        );
        assert_eq!(
            (rect.sx, rect.sy, rect.sw, rect.sh),
            (0, 0, 576, 324),
            "the source rect stays in image pixels (D4)"
        );
    }

    #[test]
    fn viewport_placement_uses_resolved_geometry() {
        let info = vt::kitty::graphics::PlacementRenderInfo {
            size: std::mem::size_of::<vt::kitty::graphics::PlacementRenderInfo>(),
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
        let rect = viewport_rect(&info, 8, 16, 120, 0, 0);
        assert_eq!(rect.x, 32);
        assert_eq!(rect.y, 16);
        assert_eq!((rect.w, rect.h), (40, 20));
        assert_eq!((rect.sx, rect.sy, rect.sw, rect.sh), (2, 3, 40, 20));
    }

    #[test]
    fn only_an_image_sized_viewport_placement_divides_the_resolved_size() {
        let info = vt::kitty::graphics::PlacementRenderInfo {
            size: std::mem::size_of::<vt::kitty::graphics::PlacementRenderInfo>(),
            pixel_width: 576,
            pixel_height: 324,
            grid_cols: 72,
            grid_rows: 20,
            viewport_col: 0,
            viewport_row: 0,
            viewport_visible: true,
            source_x: 0,
            source_y: 0,
            source_width: 576,
            source_height: 324,
        };

        // Neither count given: the destination is image-sized device pixels,
        // so it divides by the scale; the source rect stays in image pixels.
        let rect = viewport_rect(&info, 8, 16, 216, 0, 0);
        assert_eq!((rect.w, rect.h), (320, 180));
        assert_eq!((rect.sw, rect.sh), (576, 324));

        // One count given: ghostty derives the other from the logical cell,
        // so the resolved size is already logical and stays.
        let rect = viewport_rect(&info, 8, 16, 216, 10, 0);
        assert_eq!((rect.w, rect.h), (576, 324));

        // Both given: the size is the cells', unchanged.
        let rect = viewport_rect(&info, 8, 16, 216, 10, 5);
        assert_eq!((rect.w, rect.h), (576, 324));
    }

    struct ReplaySink;

    impl PtySink for ReplaySink {
        fn write_pty(&mut self, _data: &[u8]) {}
    }

    fn replay_terminal() -> Terminal {
        let mut terminal = Terminal::new(Poisoned::new(), ReplaySink, NoDecoder, || {});
        assert!(terminal.push_size(40, 24, 8, 16));
        terminal
    }

    fn collect(terminal: &mut Terminal) {
        while terminal.cell_next().is_some() {}
    }

    /// The passes share one frame: the cell pass and the image pass run
    /// over the same frame, so the placeholder origins the cell pass
    /// recorded are still there when the virtual placement is resolved.
    #[test]
    fn replayed_mbv_image_is_placed_when_the_passes_share_a_frame() {
        let mut terminal = replay_terminal();
        terminal.push_pty_data(&mbv_replay_bytes());

        assert!(terminal.frame_begin());
        collect(&mut terminal);
        let img = terminal
            .image_next()
            .expect("the raw U=1 image is stored and placed in the same frame");
        terminal.frame_end();

        assert_eq!(img.image_id, IMAGE_ID);
        assert_eq!((img.image_w, img.image_h), (8, 16), "raw pixels kept");
        assert_eq!(
            (img.x, img.y, img.w, img.h),
            (4 * 8, 2 * 16, 8, 16),
            "drawn at the placeholder origin at the image's own size"
        );
    }
}
