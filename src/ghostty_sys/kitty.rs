//! Kitty graphics placement iteration (`ghostty/vt/kitty_graphics.h`).

use std::os::raw::{c_int, c_void};

use super::terminal::GhosttyTerminal;
use super::{GhosttyAllocator, GhosttyResult};

/// Opaque handle to a kitty graphics image storage
/// (`GhosttyKittyGraphics`, kitty_graphics.h). Borrowed from the terminal.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct GhosttyKittyGraphics(pub *mut c_void);

/// Opaque handle to a kitty graphics image
/// (`GhosttyKittyGraphicsImage`, kitty_graphics.h). Borrowed from the storage.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct GhosttyKittyGraphicsImage(pub *const c_void);

/// Opaque handle to a kitty graphics placement iterator
/// (`GhosttyKittyGraphicsPlacementIterator`, kitty_graphics.h).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct GhosttyKittyGraphicsPlacementIterator(pub *mut c_void);

/// A graphics query id (`GhosttyKittyGraphicsData`, kitty_graphics.h).
pub type GhosttyKittyGraphicsData = c_int;

pub const GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR: GhosttyKittyGraphicsData = 1;

/// A placement query id (`GhosttyKittyGraphicsPlacementData`,
/// kitty_graphics.h).
pub type GhosttyKittyGraphicsPlacementData = c_int;

pub const GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID: GhosttyKittyGraphicsPlacementData = 1;
pub const GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL: GhosttyKittyGraphicsPlacementData = 3;
pub const GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z: GhosttyKittyGraphicsPlacementData = 12;

/// An image info id (`GhosttyKittyGraphicsImageData`, kitty_graphics.h).
pub type GhosttyKittyGraphicsImageData = c_int;

pub const GHOSTTY_KITTY_IMAGE_DATA_WIDTH: GhosttyKittyGraphicsImageData = 3;
pub const GHOSTTY_KITTY_IMAGE_DATA_HEIGHT: GhosttyKittyGraphicsImageData = 4;
pub const GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR: GhosttyKittyGraphicsImageData = 7;
pub const GHOSTTY_KITTY_IMAGE_DATA_GENERATION: GhosttyKittyGraphicsImageData = 9;

/// Resolved placement geometry for one frame
/// (`GhosttyKittyGraphicsPlacementRenderInfo`, kitty_graphics.h). A sized
/// struct: set `size` to
/// `size_of::<GhosttyKittyGraphicsPlacementRenderInfo>()` first.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyKittyGraphicsPlacementRenderInfo {
    pub size: usize,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub viewport_col: i32,
    pub viewport_row: i32,
    pub viewport_visible: bool,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
}

unsafe extern "C" {
    /// Query the graphics storage; `out`'s type depends on `data`
    /// (kitty_graphics.h).
    pub fn ghostty_kitty_graphics_get(
        graphics: GhosttyKittyGraphics,
        data: GhosttyKittyGraphicsData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Look up a borrowed image by id, or NULL when it is gone
    /// (kitty_graphics.h).
    pub fn ghostty_kitty_graphics_image(
        graphics: GhosttyKittyGraphics,
        image_id: u32,
    ) -> GhosttyKittyGraphicsImage;

    /// Query several image facts at once; `values[i]`'s type depends on
    /// `keys[i]` (kitty_graphics.h).
    pub fn ghostty_kitty_graphics_image_get_multi(
        image: GhosttyKittyGraphicsImage,
        count: usize,
        keys: *const GhosttyKittyGraphicsImageData,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> GhosttyResult;

    /// Create a placement iterator with `allocator`, or the default allocator
    /// when it is NULL (kitty_graphics.h).
    pub fn ghostty_kitty_graphics_placement_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyKittyGraphicsPlacementIterator,
    ) -> GhosttyResult;

    /// Advance the placement iterator; false at the end (kitty_graphics.h).
    pub fn ghostty_kitty_graphics_placement_next(
        iterator: GhosttyKittyGraphicsPlacementIterator,
    ) -> bool;

    /// Query the current placement; `out`'s type depends on `data`
    /// (kitty_graphics.h).
    pub fn ghostty_kitty_graphics_placement_get(
        iterator: GhosttyKittyGraphicsPlacementIterator,
        data: GhosttyKittyGraphicsPlacementData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Resolve the current placement's viewport geometry
    /// (kitty_graphics.h).
    pub fn ghostty_kitty_graphics_placement_render_info(
        iterator: GhosttyKittyGraphicsPlacementIterator,
        image: GhosttyKittyGraphicsImage,
        terminal: GhosttyTerminal,
        out_info: *mut GhosttyKittyGraphicsPlacementRenderInfo,
    ) -> GhosttyResult;

    /// Free a placement iterator created with
    /// `ghostty_kitty_graphics_placement_iterator_new` (kitty_graphics.h).
    pub fn ghostty_kitty_graphics_placement_iterator_free(
        iterator: GhosttyKittyGraphicsPlacementIterator,
    );
}
