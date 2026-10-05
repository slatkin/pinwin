//! The kitty-graphics side of the render row (port-to-rust D3): the cairo
//! surfaces and [`gdk::MemoryTexture`]s built from the terminal's image
//! placements and their per-frame cache, plus the gdk-pixbuf PNG decoder the
//! terminal calls back into. Ported from `src/images.c` (design D4 there).
//!
//! The placements arrive through [`Terminal::image_next`]; their `pixels`
//! pointer is ghostty-owned and only valid for the synchronous read that
//! copies it into a cairo surface.

use std::slice;

use gdk_pixbuf::prelude::*;

use gtk4::gdk;

use super::texture;
use crate::term::cells::Image;
use crate::term::{DecodedPng, PngDecoder, Terminal};

/// One cairo surface and one memory texture per kitty image, rebuilt when the
/// image is retransmitted and dropped as soon as a frame draws no placement
/// for it, so a long session cannot accumulate entries (`struct image_cache`).
/// The texture wraps the same pixels as the surface (gsk-render-nodes task
/// 3.1): the node emitter appends the texture, the cairo painter blits the
/// surface.
#[derive(Default)]
pub(crate) struct ImageCache {
    entries: Vec<ImageEntry>,
    frame: u64,
}

struct ImageEntry {
    image_id: u32,
    generation: i64,
    surface: cairo::ImageSurface,
    texture: gdk::MemoryTexture,
    last_frame: u64,
}

/// Build an ARGB32 surface from a placement's straight RGBA pixels,
/// premultiplying on the way in (`image_surface`). Returns `None` when the
/// image has no usable pixels.
fn image_surface(img: &Image) -> Option<cairo::ImageSurface> {
    let (w, h) = (img.image_w, img.image_h);
    if w <= 0 || h <= 0 || img.pixels.is_null() {
        return None;
    }

    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    let stride = surface.stride();

    // SAFETY: `img.pixels` is ghostty-owned storage of `image_w * image_h`
    // RGBA8 pixels, valid for this synchronous read (the placement iterator
    // hands it out between `image_next` and the next call or `frame_end`).
    let src = unsafe { slice::from_raw_parts(img.pixels, w as usize * h as usize * 4) };

    {
        let mut data = surface.data().ok()?;
        for y in 0..h as usize {
            let src_row = &src[y * w as usize * 4..][..w as usize * 4];
            let dst_row = &mut data[y * stride as usize..][..w as usize * 4];
            for x in 0..w as usize {
                let s = &src_row[x * 4..x * 4 + 4];
                let (r, g, b, a) = (
                    u32::from(s[0]),
                    u32::from(s[1]),
                    u32::from(s[2]),
                    u32::from(s[3]),
                );
                // ARGB32 is premultiplied on little-endian: bytes B, G, R, A.
                let pr = ((r * a + 127) / 255) as u8;
                let pg = ((g * a + 127) / 255) as u8;
                let pb = ((b * a + 127) / 255) as u8;
                dst_row[x * 4] = pb;
                dst_row[x * 4 + 1] = pg;
                dst_row[x * 4 + 2] = pr;
                dst_row[x * 4 + 3] = a as u8;
            }
        }
    }
    surface.mark_dirty();

    Some(surface)
}

/// Build a placement's cairo surface and memory texture from its pixels —
/// the texture wraps the surface's premultiplied bytes, so both painters
/// composite the same pixels (gsk-render-nodes task 3.1). Returns `None`
/// when the image has no usable pixels.
fn image_assets(img: &Image) -> Option<(cairo::ImageSurface, gdk::MemoryTexture)> {
    let mut surface = image_surface(img)?;
    let texture = texture::surface_texture(&mut surface)?;
    Some((surface, texture))
}

impl ImageCache {
    /// The entry for this frame's placement: the cached one when the image
    /// is unchanged, a rebuilt one when it was retransmitted, or a new one
    /// (`image_surface` in `images.c`). Both painters' passes go through
    /// here, so the keys and the eviction stay shared.
    fn entry_for(&mut self, img: &Image) -> Option<&mut ImageEntry> {
        let frame = self.frame;
        if let Some(pos) = self.entries.iter().position(|e| e.image_id == img.image_id) {
            let e = &mut self.entries[pos];
            if e.generation == img.generation {
                e.last_frame = frame;
                return Some(e);
            }
            // Retransmitted: replace below.
            if let Some((surface, texture)) = image_assets(img) {
                e.surface = surface;
                e.texture = texture;
                e.generation = img.generation;
                e.last_frame = frame;
                return Some(e);
            }
            return None;
        }

        let (surface, texture) = image_assets(img)?;
        self.entries.push(ImageEntry {
            image_id: img.image_id,
            generation: img.generation,
            surface,
            texture,
            last_frame: frame,
        });
        self.entries.last_mut()
    }

    /// The surface for this frame's placement (`image_surface` in
    /// `images.c`).
    pub(crate) fn surface_for(&mut self, img: &Image) -> Option<cairo::ImageSurface> {
        self.entry_for(img).map(|e| e.surface.clone())
    }

    /// The texture for this frame's placement (gsk-render-nodes task 3.1),
    /// wrapping the same pixels as [`Self::surface_for`] returns.
    pub(crate) fn texture_for(&mut self, img: &Image) -> Option<gdk::MemoryTexture> {
        self.entry_for(img).map(|e| e.texture.clone())
    }

    /// Start one frame of placement drawing — the counter bump `draw` and
    /// `draw_nodes` share, which eviction keys on (`draw`'s `frame += 1`).
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Drop every entry no placement touched this frame (`image_cache_evict`).
    pub(crate) fn evict(&mut self) {
        let frame = self.frame;
        self.entries.retain(|e| e.last_frame == frame);
    }

    /// How many entries the cache holds (the sibling node emitter's tests
    /// assert the eviction contract through it).
    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Draw the frame's kitty image placements (`draw_images`).
    pub(crate) fn draw(&mut self, cr: &cairo::Context, terminal: &mut Terminal) {
        self.begin_frame();

        while let Some(img) = terminal.image_next() {
            let Some(surface) = self.surface_for(&img) else {
                continue;
            };
            if img.sw <= 0 || img.sh <= 0 || img.w <= 0 || img.h <= 0 {
                continue;
            }
            draw_placement(cr, &surface, &img);
        }
        self.evict();
    }
}

/// Draw one placement's surface with [`ImageCache::draw`]'s geometry: clip
/// to the destination rectangle, scale the source rectangle into it and
/// paint at the source offset. Split from `draw` so the node emitter's
/// parity tests drive the cairo painter's exact per-placement maths
/// (gsk-render-nodes task 3.1). `img.sw`/`sh` must be positive.
pub(crate) fn draw_placement(cr: &cairo::Context, surface: &cairo::ImageSurface, img: &Image) {
    let _ = cr.save();
    cr.rectangle(
        f64::from(img.x),
        f64::from(img.y),
        f64::from(img.w),
        f64::from(img.h),
    );
    cr.clip();
    cr.scale(
        f64::from(img.w) / f64::from(img.sw),
        f64::from(img.h) / f64::from(img.sh),
    );
    let _ = cr.set_source_surface(
        surface,
        f64::from(img.x - img.sx),
        f64::from(img.y - img.sy),
    );
    let _ = cr.paint();
    let _ = cr.restore();
}

/// The gdk-pixbuf PNG decoder (`glue_decode_png`): the [`PngDecoder`] the
/// panel hands to the terminal, replacing the C hook libghostty-vt called
/// back into. Straight (non-premultiplied) RGBA out, like `DecodedPng`.
pub struct PixbufDecoder;

impl PngDecoder for PixbufDecoder {
    fn decode_png(&mut self, data: &[u8]) -> Option<DecodedPng> {
        let loader = gdk_pixbuf::PixbufLoader::with_type("png").ok()?;
        loader.write(data).ok()?;
        loader.close().ok()?;
        let pixbuf = loader.pixbuf()?;
        let alpha = pixbuf.add_alpha(false, 0, 0, 0).ok()?;

        let w = alpha.width();
        let h = alpha.height();
        if w <= 0 || h <= 0 {
            return None;
        }
        let channels = alpha.n_channels();
        let stride = alpha.rowstride();

        // SAFETY: the pixbuf owns its pixel buffer; the read stays within the
        // borrow of `alpha`.
        let src = unsafe { alpha.pixels() };
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        for y in 0..h as usize {
            let row = &src[y * stride as usize..];
            for x in 0..w as usize {
                let s = &row[x * channels as usize..][..channels as usize];
                let d = &mut rgba[(y * w as usize + x) * 4..][..4];
                d[0] = s[0];
                d[1] = s[1];
                d[2] = s[2];
                d[3] = if channels == 4 { s[3] } else { 0xff };
            }
        }

        Some(DecodedPng {
            width: w as u32,
            height: h as u32,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn image(pixels: &'static [u8], w: i32, h: i32) -> Image {
        Image {
            image_w: w,
            image_h: h,
            pixels: pixels.as_ptr(),
            ..Image::default()
        }
    }

    #[test]
    fn argb32_pixels_are_premultiplied() {
        // Red at half alpha, and transparent black.
        const PIXELS: [u8; 8] = [255, 0, 0, 128, 10, 20, 30, 0];
        let mut surface = image_surface(&image(&PIXELS, 2, 1)).unwrap();
        surface.flush();
        let data = surface.data().unwrap();
        assert_eq!(data[0], 0, "b premultiplied to 0");
        assert_eq!(data[1], 0, "g premultiplied to 0");
        assert_eq!(data[2], 128, "r * a / 255 rounds to 128");
        assert_eq!(data[3], 128, "alpha kept");
        assert_eq!(&data[4..8], &[0, 0, 0, 0], "transparent stays transparent");
    }

    #[test]
    fn rejects_empty_and_null_images() {
        let mut null_img = image(&[], 2, 2);
        null_img.pixels = ptr::null();
        assert!(image_surface(&null_img).is_none());
        assert!(image_surface(&image(&[0, 0, 0, 0], 0, 1)).is_none());
        assert!(image_surface(&image(&[0, 0, 0, 0], 1, 0)).is_none());
    }

    #[test]
    fn cache_reuses_untouched_images_and_replaces_retransmitted_ones() {
        const A: [u8; 4] = [255, 0, 0, 255];
        const B: [u8; 4] = [0, 255, 0, 255];
        let mut cache = ImageCache::default();

        let img_a = Image {
            image_id: 7,
            generation: 1,
            image_w: 1,
            image_h: 1,
            pixels: A.as_ptr(),
            ..Image::default()
        };
        let first = cache.surface_for(&img_a).unwrap();
        let second = cache.surface_for(&img_a).unwrap();
        assert_eq!(
            first.to_raw_none(),
            second.to_raw_none(),
            "the same generation reuses the surface"
        );

        let img_b = Image {
            image_id: 7,
            generation: 2,
            image_w: 1,
            image_h: 1,
            pixels: B.as_ptr(),
            ..Image::default()
        };
        let replaced = cache.surface_for(&img_b).unwrap();
        assert_ne!(
            first.to_raw_none(),
            replaced.to_raw_none(),
            "a new generation rebuilds the surface"
        );
    }

    #[test]
    fn evict_drops_entries_no_frame_touched() {
        const A: [u8; 4] = [255, 0, 0, 255];
        let mut cache = ImageCache::default();
        let img = Image {
            image_id: 1,
            generation: 1,
            image_w: 1,
            image_h: 1,
            pixels: A.as_ptr(),
            ..Image::default()
        };

        cache.frame += 1;
        assert!(cache.surface_for(&img).is_some());
        cache.evict();
        assert_eq!(cache.entries.len(), 1, "touched this frame: kept");

        cache.frame += 1;
        cache.evict();
        assert!(cache.entries.is_empty(), "not touched this frame: dropped");
    }
}
