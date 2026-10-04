//! The tween cache's texture conversion (poc-gsk-texture-grid task 1.1):
//! wrapping the finished cairo cache surface as a [`gdk::MemoryTexture`] so
//! the snapshot path can upload the grid once per tween and render each frame
//! as a transform, instead of re-rasterising the surface through Cairo every
//! frame.

use gtk4::gdk;

/// The memory format cairo's `ARgb32` bytes are laid out in on this platform:
/// premultiplied 8-bit channels in B, G, R, A byte order on little-endian and
/// R, G, B, A on big-endian.
#[cfg(target_endian = "little")]
const CACHE_MEMORY_FORMAT: gdk::MemoryFormat = gdk::MemoryFormat::B8g8r8a8Premultiplied;
#[cfg(target_endian = "big")]
const CACHE_MEMORY_FORMAT: gdk::MemoryFormat = gdk::MemoryFormat::A8r8g8b8Premultiplied;

/// Wrap a finished image surface's pixels in a [`gdk::MemoryTexture`], or
/// `None` when the pixels cannot be read or the surface is empty. cairo's
/// `ARgb32` is premultiplied like the memory texture formats, so the copy is a
/// straight upload with no re-encode.
pub(super) fn grid_cache_texture(surface: &mut cairo::ImageSurface) -> Option<gdk::MemoryTexture> {
    let (width, height, stride) = (surface.width(), surface.height(), surface.stride());
    if width <= 0 || height <= 0 {
        return None;
    }
    surface.flush();
    let data = surface.data().ok()?;
    let bytes = gtk4::glib::Bytes::from(&data[..]);
    Some(gdk::MemoryTexture::new(
        width,
        height,
        CACHE_MEMORY_FORMAT,
        &bytes,
        stride as usize,
    ))
}

#[cfg(test)]
mod tests {
    use gtk4::prelude::TextureExt as _;

    use super::super::font_lock;
    use super::super::tests::{state, terminal};

    fn surface(width: i32, height: i32) -> cairo::ImageSurface {
        cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).unwrap()
    }

    /// The cache and its [`gdk::MemoryTexture`] wrapper match the device-scaled
    /// surface (poc-gsk-texture-grid task 1.1): the cache is `cols * cell_w` by
    /// `height` in logical pixels, ceiling to whole device pixels at the
    /// target's device scale, the texture is exactly those device pixels, and
    /// the exposed logical size divides them back out.
    #[test]
    fn the_grid_cache_texture_matches_the_device_scaled_surface() {
        let _font = font_lock::guard();
        for scale in [1.0, 1.5] {
            let mut state = state();
            let mut terminal = terminal();
            let drawn = surface(64, 64);
            drawn.set_device_scale(scale, scale);
            let cr = cairo::Context::new(&drawn).unwrap();

            let cache = state.grid_cache_ensure(&cr, &mut terminal, 64).unwrap();
            let cols_w = i32::from(terminal.cols()) * state.cell_w();
            assert_eq!(cache.width(), (f64::from(cols_w) * scale).ceil() as i32);
            assert_eq!(cache.height(), (64.0 * scale).ceil() as i32);
            let texture = state
                .grid_cache_texture()
                .unwrap_or_else(|| panic!("no texture at scale {scale}"));
            assert_eq!(texture.width(), cache.width());
            assert_eq!(texture.height(), cache.height());
            let (logical_w, logical_h) = state.grid_cache_logical_size().unwrap();
            assert_eq!(logical_w, f64::from(cache.width()) / scale);
            assert_eq!(logical_h, f64::from(cache.height()) / scale);
        }
    }

    #[test]
    fn tween_cache_is_keyed_by_columns_and_height() {
        let _font = font_lock::guard();
        let mut state = state();
        let mut terminal = terminal();
        let drawn = surface(64, 64);
        let cr = cairo::Context::new(&drawn).unwrap();

        let first = state.grid_cache_ensure(&cr, &mut terminal, 64).unwrap();
        let again = state.grid_cache_ensure(&cr, &mut terminal, 64).unwrap();
        assert_eq!(
            first.to_raw_none(),
            again.to_raw_none(),
            "same key reuses the cache"
        );

        let other_height = state.grid_cache_ensure(&cr, &mut terminal, 32).unwrap();
        drop(cr);
        assert_ne!(
            first.to_raw_none(),
            other_height.to_raw_none(),
            "a new height rebuilds the cache"
        );

        state.drop_grid_cache();
        assert!(state.grid_cache.is_none());
        assert!(state.grid_cache_texture().is_none());
        assert!(state.grid_cache_logical_size().is_none());
    }
}
