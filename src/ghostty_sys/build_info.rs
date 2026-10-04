//! Build-info queries (`ghostty/vt/build_info.h`).

use std::os::raw::{c_int, c_void};

use super::GhosttyResult;

/// A build-info query id (`GhosttyBuildInfo`, build_info.h).
pub type GhosttyBuildInfo = c_int;

pub const GHOSTTY_BUILD_INFO_SIMD: GhosttyBuildInfo = 1;

unsafe extern "C" {
    /// Query compile-time build configuration; `out`'s type depends on `data`
    /// (build_info.h).
    pub fn ghostty_build_info(data: GhosttyBuildInfo, out: *mut c_void) -> GhosttyResult;
}
