//! The node emitter's kitty image pass (gsk-render-nodes task 3.1): the
//! node form of [`ImageCache::draw`], the same placement iteration emitting
//! each placement's [`gdk::MemoryTexture`] as a texture node. Split from
//! `nodes.rs` as its own sibling (like [`node_sprites`](super::node_sprites)
//! and [`node_cursor`](super::node_cursor)) so the cell pass stays in
//! budget; the cache's keys and eviction live in [`super::images`], shared
//! with the cairo painter's `draw`.
//!
//! The geometry mirrors `ImageCache::draw`'s cairo maths exactly (see
//! [`super::images::draw_placement`]): the placement's destination
//! rectangle is the clip, the source rectangle is scaled into it by
//! `w/sw`/`h/sh`, and the whole image is appended at the source offset in
//! the scaled space. GSK clips are hard, like cairo's; the texture node's
//! filtering differs from the cairo pattern's only when a placement is
//! resampled, which the parity tolerance states (task 3.1).

use gtk4::gdk;
use gtk4::graphene;
use gtk4::prelude::SnapshotExt as _;

use super::images::ImageCache;
use crate::term::Terminal;
use crate::term::cells::Image;

impl ImageCache {
    /// Emit the frame's kitty image placements as texture nodes into
    /// `snapshot` (gsk-render-nodes task 3.1), the node form of
    /// [`ImageCache::draw`]: the same placement iteration, the same
    /// skip-empty-rect guard, and the same frame bookkeeping and eviction
    /// so the two painters keep one cache. The frame must be open.
    pub(crate) fn draw_nodes(&mut self, snapshot: &gtk4::Snapshot, terminal: &mut Terminal) {
        self.begin_frame();
        while let Some(img) = terminal.image_next() {
            let Some(texture) = self.texture_for(&img) else {
                continue;
            };
            if img.sw <= 0 || img.sh <= 0 || img.w <= 0 || img.h <= 0 {
                continue;
            }
            emit_placement(snapshot, &texture, &img);
        }
        self.evict();
    }
}

/// One placement as a texture node, the node form of
/// [`super::images::draw_placement`]'s cairo maths: clip to the
/// destination rect, scale by `w/sw`/`h/sh`, append the texture at the
/// source offset in the scaled space — cairo's `set_source_surface` offset
/// is in the scaled user space, and the texture's bounds are the image's
/// own pixel size, so the two agree point for point.
// Approved per-instance (#13): GSK/graphene take f32; the image geometry
// is screen-bounded.
#[allow(
    clippy::cast_possible_truncation,
    reason = "approved #13: GSK/graphene take f32, screen-bounded pixels"
)]
pub(super) fn emit_placement(snapshot: &gtk4::Snapshot, texture: &gdk::MemoryTexture, img: &Image) {
    snapshot.save();
    snapshot.push_clip(&graphene::Rect::new(
        f64::from(img.x) as f32,
        f64::from(img.y) as f32,
        f64::from(img.w) as f32,
        f64::from(img.h) as f32,
    ));
    snapshot.scale(
        (f64::from(img.w) / f64::from(img.sw)) as f32,
        (f64::from(img.h) / f64::from(img.sh)) as f32,
    );
    snapshot.append_texture(
        texture,
        &graphene::Rect::new(
            f64::from(img.x - img.sx) as f32,
            f64::from(img.y - img.sy) as f32,
            f64::from(img.image_w) as f32,
            f64::from(img.image_h) as f32,
        ),
    );
    snapshot.pop();
    snapshot.restore();
}

impl super::DrawState {
    /// Emit the frame's kitty image placements as texture nodes
    /// (gsk-render-nodes task 3.1), the node pass corresponding to
    /// [`render_grid`](super::DrawState::render_grid)'s trailing
    /// `images.draw`. Its own frame cycle, like
    /// [`emit_cells`](super::DrawState::emit_cells)'s; `false` — no live
    /// frame, or no metrics yet — asks the caller to draw the cairo path
    /// instead.
    ///
    /// The image pass reopens the frame with
    /// [`Terminal::frame_begin_images`](crate::term::Terminal::frame_begin_images)
    /// rather than a plain `frame_begin`: the cell pass that ran just before
    /// recorded the unicode-placeholder origins, and a fresh frame would
    /// clear them and silently drop every U=1 image — which is how the
    /// whole mbv grid went blank.
    pub fn emit_images(&mut self, snapshot: &gtk4::Snapshot, terminal: &mut Terminal) -> bool {
        if self.cell_metrics.cell_h <= 0 {
            return false;
        }
        if !terminal.frame_begin_images() {
            return false;
        }
        self.images.draw_nodes(snapshot, terminal);
        terminal.frame_end();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::images::draw_placement;
    use super::*;
    use crate::render::parity;
    use crate::render::tests::terminal;

    /// An 8x6 RGBA image with an opaque red quadrant, a half-alpha green
    /// quadrant, an opaque blue quadrant and a transparent quadrant, so
    /// parity covers alpha compositing as well as colour.
    fn test_pixels() -> Vec<u8> {
        let mut px = vec![0u8; 8 * 6 * 4];
        for y in 0..6usize {
            for x in 0..8usize {
                let i = (y * 8 + x) * 4;
                match (x / 4, y / 3) {
                    (0, 0) => {
                        px[i] = 255;
                        px[i + 3] = 255;
                    }
                    (1, 0) => {
                        px[i + 1] = 255;
                        px[i + 3] = 128;
                    }
                    (0, 1) => {
                        px[i + 2] = 255;
                        px[i + 3] = 255;
                    }
                    _ => {}
                }
            }
        }
        px
    }

    /// An `Image` over [`test_pixels`]: `dest` is the placement's
    /// `(x, y, w, h)` rect, `src` its `(sx, sy, sw, sh)` source rect.
    fn placement(dest: (i32, i32, i32, i32), src: (i32, i32, i32, i32)) -> Image {
        let px = Box::leak(test_pixels().into_boxed_slice());
        let (x, y, w, h) = dest;
        let (sx, sy, sw, sh) = src;
        Image {
            image_id: 1,
            generation: 1,
            image_w: 8,
            image_h: 6,
            x,
            y,
            w,
            h,
            sx,
            sy,
            sw,
            sh,
            pixels: px.as_ptr(),
            ..Image::default()
        }
    }

    /// The assets for one placement, fetched through the real cache path
    /// ([`ImageCache::surface_for`] and [`ImageCache::texture_for`]) with
    /// the frame bumped so the entry survives: both painters draw from the
    /// one cache, which is the invariant task 3.1 pins.
    fn cache_assets(img: &Image) -> (cairo::ImageSurface, gtk4::gdk::MemoryTexture) {
        let mut cache = ImageCache::default();
        let surface = cache.surface_for(img).expect("usable test pixels");
        let texture = cache.texture_for(img).expect("texture beside the surface");
        (surface, texture)
    }

    /// The cairo oracle for one placement: [`draw_placement`] — the exact
    /// per-placement maths `ImageCache::draw` runs — into a backed surface
    /// at `scale`.
    fn cairo_placement(img: &Image, scale: f64) -> cairo::ImageSurface {
        let (surface, _) = cache_assets(img);
        let out = parity::backed_surface(24, 16, scale, parity::BACKDROP);
        let cr = cairo::Context::new(&out).expect("context");
        draw_placement(&cr, &surface, img);
        out
    }

    /// The node side for one placement: [`emit_placement`] into a snapshot,
    /// drawn to a backed surface at `scale`.
    fn node_placement(img: &Image, scale: f64) -> cairo::ImageSurface {
        let (_, texture) = cache_assets(img);
        let snapshot = parity::snapshot();
        emit_placement(&snapshot, &texture, img);
        let node = snapshot.to_node().expect("snapshot produced a node");
        let surface = parity::backed_surface(24, 16, scale, parity::BACKDROP);
        parity::draw_node(&node, &surface);
        surface
    }

    #[test]
    fn a_natural_size_placement_matches_the_cairo_painter_exactly() {
        let img = placement((3, 2, 8, 6), (0, 0, 8, 6));
        let mut cairo_side = cairo_placement(&img, 1.0);
        let mut node_side = node_placement(&img, 1.0);
        parity::assert_exact(&mut cairo_side, &mut node_side, "natural size at scale 1");
    }

    /// A magnified placement whose destination rectangle sits a filter
    /// kernel's width inside the drawn image, so the clip crops the resampled
    /// edge pixels: the scale/clip geometry must match the cairo painter up
    /// to a rounding step (the measured delta is 1). (A resampled
    /// placement's own edge pixels are the one larger, stated difference —
    /// see [`a_scaled_sub_rect_placement_matches_the_cairo_painter_within_tolerance`].)
    #[test]
    fn a_magnified_placement_matches_the_cairo_painter_within_the_clip() {
        let img = placement((2, 2, 6, 6), (0, 0, 8, 6));
        let mut cairo_side = cairo_placement(&img, 1.0);
        let mut node_side = node_placement(&img, 1.0);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            FILTER_TOLERANCE,
            "magnified interior at scale 1",
        );
    }

    /// A minified placement whose destination rectangle crops the image on
    /// every side: the clip must cut the drawn texture exactly where the
    /// cairo path's clip does, and the resampled interior matches within a
    /// filter-step bound (the measured delta is 1).
    const FILTER_TOLERANCE: u8 = 8;

    #[test]
    fn a_cropped_minified_placement_matches_the_cairo_painter_within_tolerance() {
        let img = placement((1, 1, 4, 3), (0, 0, 8, 6));
        let mut cairo_side = cairo_placement(&img, 1.0);
        let mut node_side = node_placement(&img, 1.0);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            FILTER_TOLERANCE,
            "cropped minified at scale 1",
        );
    }
    /// A sub-rectangle of the image drawn scaled, with a non-zero origin:
    /// exercises the source-offset maths (the whole image is appended at
    /// `x - sx` in the scaled space, and the clip crops everything outside
    /// the destination rect) against the same numbers the cairo painter
    /// uses.
    ///
    /// The stated tolerance covers the one real difference between the
    /// painters (task 3.1's filter caveat): the placement's own edge pixels,
    /// where the cairo pattern's default extend (none) fades the resampling
    /// kernel's overhang into transparency and the node's texture pads it
    /// (`EXTEND_PAD`). The geometry is identical — the magnified-interior
    /// test above pins that — and the measured delta from the extend fade
    /// is 94 against this bound of half the full channel distance.
    #[test]
    fn a_scaled_sub_rect_placement_matches_the_cairo_painter_within_tolerance() {
        let img = placement((4, 1, 8, 6), (2, 1, 4, 3));
        let mut cairo_side = cairo_placement(&img, 1.0);
        let mut node_side = node_placement(&img, 1.0);
        parity::assert_within(
            &mut cairo_side,
            &mut node_side,
            parity::BLEND_TOLERANCE,
            "scaled sub-rect at scale 1",
        );
    }

    /// `emit_images` reopens the frame keeping the placeholder origins the
    /// cell pass recorded, so a unicode-placeholder image (U=1) emitted the
    /// way mbv does is placed and cached. A plain `frame_begin` here cleared
    /// the origins and silently dropped every such image — the mbv bug.
    #[test]
    fn emit_images_keeps_the_cell_pass_placeholder_origins() {
        let _font = crate::render::font_lock::guard();
        let mut terminal = crate::render::tests::terminal();
        terminal.push_pty_data(&crate::term::cells::mbv_replay_bytes());

        let snapshot = parity::snapshot();
        let mut state = super::super::tests::state();
        assert!(state.emit_cell_backgrounds(&snapshot, &mut terminal, 4 * 16));
        assert!(state.emit_cells(&snapshot, &mut terminal));
        assert!(state.emit_images(&snapshot, &mut terminal));
        assert_eq!(
            state.images.entry_count(),
            1,
            "the U=1 placement produced a cached texture"
        );
    }

    /// `emit_images` runs the cache's frame bookkeeping: an entry fetched
    /// in an earlier frame is dropped when a frame draws no placements —
    /// the same eviction contract `ImageCache::draw` gives the cairo
    /// painter (the keep case is pinned by `images`' own tests).
    #[test]
    fn emit_images_evicts_entries_like_draw() {
        let img = placement((0, 0, 8, 6), (0, 0, 8, 6));
        let mut cache = ImageCache::default();
        assert!(cache.texture_for(&img).is_some(), "entry fetched");

        let mut terminal = terminal();
        assert!(terminal.frame_begin());
        let snapshot = parity::snapshot();
        let mut state = super::super::tests::state();
        state.images = cache;
        assert!(state.emit_images(&snapshot, &mut terminal));
        terminal.frame_end();
        assert_eq!(
            state.images.entry_count(),
            0,
            "no placements this frame: evicted"
        );
    }
}
