//! Colors and cell styles (`ghostty/vt/color.h`, `style.h`).

use std::os::raw::c_int;

/// An 8-bit RGB color (`GhosttyColorRgb`, color.h).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// An index into the 256-entry palette (`GhosttyColorPaletteIndex`, color.h).
pub type GhosttyColorPaletteIndex = u8;

/// A style id (`GhosttyStyleId`, style.h).
pub type GhosttyStyleId = u16;

/// The discriminant of a [`GhosttyStyleColor`] (`GhosttyStyleColorTag`,
/// style.h).
pub type GhosttyStyleColorTag = c_int;

pub const GHOSTTY_STYLE_COLOR_NONE: GhosttyStyleColorTag = 0;
pub const GHOSTTY_STYLE_COLOR_PALETTE: GhosttyStyleColorTag = 1;
pub const GHOSTTY_STYLE_COLOR_RGB: GhosttyStyleColorTag = 2;

/// The value of a [`GhosttyStyleColor`] (`GhosttyStyleColorValue`, style.h).
/// The active arm is named by the enclosing `tag`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    pub palette: GhosttyColorPaletteIndex,
    pub rgb: GhosttyColorRgb,
    pub _padding: u64,
}

/// The raw bits of a [`GhosttyStyleColorValue`]: every arm is an integer
/// type where all bit patterns are valid, so the widest arm always reports
/// a well-formed value.
impl std::fmt::Debug for GhosttyStyleColorValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SAFETY: the union is 8 bytes and every arm is an integer type
        // where all bit patterns are valid, so reinterpreting the raw bits
        // as u64 cannot produce an invalid value; the tag that names the
        // live arm lives outside this union.
        let raw: u64 = unsafe { std::mem::transmute_copy(self) };
        f.debug_struct("GhosttyStyleColorValue")
            .field("raw", &raw)
            .finish()
    }
}

/// A tagged style color (`GhosttyStyleColor`, style.h).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyStyleColor {
    pub tag: GhosttyStyleColorTag,
    pub value: GhosttyStyleColorValue,
}

/// A cell style (`GhosttyStyle`, style.h). A sized struct: set `size` to
/// `size_of::<GhosttyStyle>()` before passing it to the library.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyStyle {
    pub size: usize,
    pub fg_color: GhosttyStyleColor,
    pub bg_color: GhosttyStyleColor,
    pub underline_color: GhosttyStyleColor,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: c_int,
}
