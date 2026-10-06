//! Plain data types for the frame protocol (port-to-rust D3), ported from
//! `PinwinCell`, `PinwinCursor`, `PinwinImage` and the `PINWIN_*`/`WIDE_*`
//! constants in `src/pinwin.h`. No GTK, GDK or cairo type appears here.

use std::ptr;

use crate::ghostty_sys::render::{
    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK,
    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW,
    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE, GhosttyRenderStateCursorVisualStyle,
};
use crate::ghostty_sys::screen::{
    GHOSTTY_CELL_WIDE_SPACER_TAIL, GHOSTTY_CELL_WIDE_WIDE, GhosttyCellWide,
};
use crate::ghostty_sys::style::GhosttyColorRgb;

/// The number of bytes a cell's UTF-8 text may hold (`PinwinCell.text`).
pub const CELL_TEXT_CAP: usize = 32;

/// How many columns a cell occupies (`GHOSTTY_CELL_WIDE_*`). Only the three
/// values the frame carries are named; anything else (an internal spacer head)
/// is treated as narrow, matching how the C renderer reads the field.
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Wide {
    #[default]
    Narrow = 0,
    Wide = 1,
    SpacerTail = 2,
}

impl Wide {
    /// A glyph that occupies one column.
    pub const NARROW: Wide = Wide::Narrow;
    /// A glyph that occupies two columns.
    pub const WIDE: Wide = Wide::Wide;
    /// The second column of a wide glyph; it must not be drawn.
    pub const SPACER_TAIL: Wide = Wide::SpacerTail;

    pub(super) fn from_raw(raw: GhosttyCellWide) -> Wide {
        match raw {
            GHOSTTY_CELL_WIDE_WIDE => Wide::Wide,
            GHOSTTY_CELL_WIDE_SPACER_TAIL => Wide::SpacerTail,
            _ => Wide::Narrow,
        }
    }
}

/// The shape drawn for the cursor (`PINWIN_CURSOR_*` /
/// `GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_*`).
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorStyle {
    #[default]
    Bar = 0,
    Block = 1,
    Underline = 2,
    BlockHollow = 3,
}

impl CursorStyle {
    /// A vertical bar.
    pub const BAR: CursorStyle = CursorStyle::Bar;
    /// A filled block.
    pub const BLOCK: CursorStyle = CursorStyle::Block;
    /// An underline.
    pub const UNDERLINE: CursorStyle = CursorStyle::Underline;
    /// An outlined block.
    pub const BLOCK_HOLLOW: CursorStyle = CursorStyle::BlockHollow;

    pub(super) fn from_raw(raw: GhosttyRenderStateCursorVisualStyle) -> CursorStyle {
        match raw {
            GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => CursorStyle::Block,
            GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => CursorStyle::Underline,
            GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => CursorStyle::BlockHollow,
            _ => CursorStyle::Bar,
        }
    }
}

/// A cell's style bits (`PinwinCell.flags` / `PINWIN_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StyleFlags(u32);

impl StyleFlags {
    /// Bold text.
    pub const BOLD: StyleFlags = StyleFlags(1 << 0);
    /// Italic text.
    pub const ITALIC: StyleFlags = StyleFlags(1 << 1);
    /// Swapped foreground and background.
    pub const INVERSE: StyleFlags = StyleFlags(1 << 2);
    /// Dimmed text.
    pub const FAINT: StyleFlags = StyleFlags(1 << 3);
    /// Text not drawn at all.
    pub const INVISIBLE: StyleFlags = StyleFlags(1 << 4);
    /// A horizontal line through the text.
    pub const STRIKETHROUGH: StyleFlags = StyleFlags(1 << 5);
    /// An underline.
    pub const UNDERLINE: StyleFlags = StyleFlags(1 << 6);

    /// The raw bits, for code that compares against a plain integer.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether no style bit is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every bit in `other` is set.
    #[must_use]
    pub const fn contains(self, other: StyleFlags) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for StyleFlags {
    type Output = StyleFlags;

    fn bitor(self, rhs: StyleFlags) -> StyleFlags {
        StyleFlags(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for StyleFlags {
    fn bitor_assign(&mut self, rhs: StyleFlags) {
        self.0 |= rhs.0;
    }
}

/// An 8-bit RGB colour.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl From<GhosttyColorRgb> for Rgb {
    fn from(color: GhosttyColorRgb) -> Rgb {
        Rgb {
            r: color.r,
            g: color.g,
            b: color.b,
        }
    }
}

/// One grapheme of the frame the render row should paint (`PinwinCell`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Cell {
    /// Viewport column.
    pub x: i32,
    /// Viewport row.
    pub y: i32,
    /// How many columns the cell occupies.
    pub wide: Wide,
    /// Cells this glyph may use when constrained (1 or 2).
    pub cw: i32,
    /// UTF-8 byte count of `text`; 0 means the cell has no glyph.
    pub len: usize,
    /// UTF-8 bytes of the grapheme, valid for `..len`.
    pub text: [u8; CELL_TEXT_CAP],
    /// Whether `fg` is explicit; otherwise the renderer uses its default.
    pub has_fg: bool,
    /// The cell's foreground colour.
    pub fg: Rgb,
    /// Whether `bg` is explicit; otherwise the renderer uses its default.
    pub has_bg: bool,
    /// The cell's background colour.
    pub bg: Rgb,
    /// The cell's style bits.
    pub flags: StyleFlags,
}

impl Cell {
    /// The glyph bytes, valid UTF-8.
    #[must_use]
    pub fn text_bytes(&self) -> &[u8] {
        &self.text[..self.len]
    }

    /// The glyph as a string, or `""` when it is empty or malformed.
    #[must_use]
    pub fn text_str(&self) -> &str {
        std::str::from_utf8(self.text_bytes()).unwrap_or("")
    }

    /// Replace the cell's text with `bytes`, truncating at the buffer cap.
    #[cfg(test)]
    pub(crate) fn set_text(&mut self, bytes: &[u8]) {
        let len = bytes.len().min(CELL_TEXT_CAP);
        self.text[..len].copy_from_slice(&bytes[..len]);
        self.len = len;
    }
}

/// The cursor's viewport position and shape (`PinwinCursor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// Whether the cursor is visible and has a viewport position.
    pub has_value: bool,
    /// Viewport column.
    pub x: i32,
    /// Viewport row.
    pub y: i32,
    /// The shape drawn for the cursor.
    pub style: CursorStyle,
    /// Whether the cursor sits on a wide glyph's tail cell.
    pub wide_tail: bool,
}

/// One kitty graphics placement of the frame (`PinwinImage`).
#[derive(Clone, Copy, Debug)]
pub struct Image {
    /// The image id.
    pub image_id: u32,
    /// The image generation, used to invalidate decoded surfaces.
    pub generation: i64,
    /// The placement's z order.
    pub z: i32,
    /// Destination left edge in device pixels, relative to the widget.
    pub x: i32,
    /// Destination top edge.
    pub y: i32,
    /// Destination width.
    pub w: i32,
    /// Destination height.
    pub h: i32,
    /// Source left edge in image pixels.
    pub sx: i32,
    /// Source top edge.
    pub sy: i32,
    /// Source width.
    pub sw: i32,
    /// Source height.
    pub sh: i32,
    /// The whole image's width in pixels.
    pub image_w: i32,
    /// The whole image's height in pixels.
    pub image_h: i32,
    /// Straight (non-premultiplied) RGBA8 pixels of the whole image.
    pub pixels: *const u8,
}

impl Default for Image {
    fn default() -> Image {
        Image {
            image_id: 0,
            generation: 0,
            z: 0,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            sx: 0,
            sy: 0,
            sw: 0,
            sh: 0,
            image_w: 0,
            image_h: 0,
            pixels: ptr::null(),
        }
    }
}

impl Image {
    pub(super) fn apply_rect(&mut self, rect: super::images::Rect) {
        self.x = rect.x;
        self.y = rect.y;
        self.w = rect.w;
        self.h = rect.h;
        self.sx = rect.sx;
        self.sy = rect.sy;
        self.sw = rect.sw;
        self.sh = rect.sh;
    }
}

/// The terminal's default colours (`pinwin_colors`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Colors {
    /// The default background.
    pub background: Rgb,
    /// The default foreground.
    pub foreground: Rgb,
}
