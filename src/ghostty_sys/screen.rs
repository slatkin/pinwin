//! Packed cell access (`ghostty/vt/screen.h`).

use std::os::raw::{c_int, c_void};

use super::GhosttyResult;

/// A packed cell (`GhosttyCell`, screen.h).
pub type GhosttyCell = u64;

/// How many columns a cell occupies (`GhosttyCellWide`, screen.h).
pub type GhosttyCellWide = c_int;

pub const GHOSTTY_CELL_WIDE_NARROW: GhosttyCellWide = 0;
pub const GHOSTTY_CELL_WIDE_WIDE: GhosttyCellWide = 1;
pub const GHOSTTY_CELL_WIDE_SPACER_TAIL: GhosttyCellWide = 2;
pub const GHOSTTY_CELL_WIDE_SPACER_HEAD: GhosttyCellWide = 3;

/// A cell query id (`GhosttyCellData`, screen.h).
pub type GhosttyCellData = c_int;

pub const GHOSTTY_CELL_DATA_WIDE: GhosttyCellData = 3;

unsafe extern "C" {
    /// Query one field of a packed cell; `out`'s type depends on `data`
    /// (screen.h).
    pub fn ghostty_cell_get(
        cell: GhosttyCell,
        data: GhosttyCellData,
        out: *mut c_void,
    ) -> GhosttyResult;
}
