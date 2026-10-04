//! pinwin library (port-to-rust).
//!
//! This is the crate skeleton from task 2.1/2.2. The `Panel` API (D4, D6),
//! the real `ghostty_sys` declarations (D2) and the ported modules arrive in
//! tasks 3 and 4; everything below is scaffolding.

use std::os::raw::{c_int, c_void};

/// A datum that [`ghostty_build_info`] can write to its out pointer.
#[cfg(test)]
const GHOSTTY_BUILD_INFO_SIMD: c_int = 1;

// libghostty-vt build-info query, declared only as the link smoke for the
// archive that `build.rs` builds from the pinned commit (port-to-rust D2).
// Task 3.3 replaces this with the full hand-written `ghostty_sys` module.
#[allow(dead_code)]
unsafe extern "C" {
    fn ghostty_build_info(data: c_int, out: *mut c_void) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves `build.rs` linked the pinned archive: the symbol lives in
    /// libghostty-vt and nowhere else in the dependency graph.
    #[test]
    fn pinned_ghostty_archive_links() {
        let mut simd: u8 = 0;
        // SAFETY: ghostty_build_info writes a C `bool` through the out pointer
        // for GHOSTTY_BUILD_INFO_SIMD, and `simd` is valid for that write.
        let result =
            unsafe { ghostty_build_info(GHOSTTY_BUILD_INFO_SIMD, (&mut simd as *mut u8).cast()) };
        assert_eq!(result, 0, "ghostty_build_info returned {result}");
    }
}
