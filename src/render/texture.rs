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
