//! Texture conversion for cairo image surfaces (poc-gsk-texture-grid task
//! 1.1): wrapping a finished surface's pixels as a [`gdk::MemoryTexture`] so
//! a snapshot can upload it and render it as a texture node instead of
//! re-rasterising the surface through Cairo every frame. The stage-1 grid
//! cache that first used this went away with gsk-render-nodes row 4.2; the
//! per-image textures in [`super::images`] are the remaining user.

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
pub(super) fn surface_texture(surface: &mut cairo::ImageSurface) -> Option<gdk::MemoryTexture> {
    let (width, height, stride) = (surface.width(), surface.height(), surface.stride());
    if width <= 0 || height <= 0 {
        return None;
    }
    surface.flush();
    let data = surface.data().ok()?;
    let bytes = gtk4::glib::Bytes::from(&*data);
    Some(gdk::MemoryTexture::new(
        width,
        height,
        CACHE_MEMORY_FORMAT,
        &bytes,
        stride.cast_unsigned() as usize,
    ))
}
