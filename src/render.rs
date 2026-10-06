//! The render row (port-to-rust D3): how pinwin draws a frame. Since
//! replace-gtk-with-wayland D5/D6 the painter is toolkit-free: the grid
//! painter draws the theme background, the cell backgrounds, the underline
//! and strikethrough bands, the sprite ranges, the text, the cursor and the
//! kitty images into a [`canvas::Canvas`], the tiny-skia pixmap the panel
//! thread fills its shared-memory buffer from.
//!
//! A canvas is an in-memory pixmap, so every painter module's tests render
//! without a display (`port-to-rust` D10).

pub(crate) use snap::OutputScale;

mod accent;
mod bands;
mod bg;
/// The canvas the grid painter draws a frame into (row 4.3): a tiny-skia
/// pixmap with the channel swap done at fill time.
pub mod canvas;
/// The cell metrics from the font, computed with swash (row 4.6).
pub mod cell_metrics;
pub mod cursor;
/// The font module (row 4.4): fontconfig resolves the Ghostty family to font
/// files and caches a fallback face per code point
/// (replace-gtk-with-wayland D6/D11).
pub mod font;
/// The frame gate (row 4.8): what a frame must redraw, from the render
/// state's dirty data and the inputs it does not cover.
pub mod frame_gate;
/// The geometry seam between the snapped logical grid and the device-pixel
/// canvas: the cast seam, the painter's cell metrics and the frame input
/// (D5).
pub mod geom;
/// The glyph module (row 4.5): swash rasterizes one glyph id of one face at
/// one device size, with hinting, colour and style synthesis, and caches the
/// results (replace-gtk-with-wayland D6/D11).
pub mod glyph;
/// The kitty image pass (row 4.7): the frame's kitty placements decoded
/// with the `png` crate, scaled to their placement sizes and cached, and
/// drawn into the canvas above the cursor (replace-gtk-with-wayland D5).
pub mod image_pass;
/// The Nerd Font glyph constraints over swash geometry (row 4.5): the
/// per-glyph placement transform (replace-gtk-with-wayland D6).
pub mod nerd;
/// The grid painter for the canvas (row 4.3): the theme background, the
/// cell backgrounds, the underline and strikethrough bands, the cursor
/// shapes and the focus accent.
pub mod painter;
/// The `png`-crate kitty PNG decoder (row 4.7): the
/// [`crate::term::PngDecoder`] implementation the panel hands to the
/// terminal from row 8 on.
pub mod png;
/// The shaper (row 4.5): swash shapes one cell's grapheme cluster into
/// glyph ids and pen positions, with the face chosen from the family's
/// styles and the per-code-point fallback, cached
/// (replace-gtk-with-wayland D6/D11).
pub mod shape;
mod snap;
mod sprite;
pub mod text_pass;
