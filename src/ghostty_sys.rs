//! Hand-written FFI declarations for the pinned libghostty-vt (port-to-rust
//! D2: the FFI crate was rejected, so pinwin owns these declarations).
//!
//! This module is the only place where pinwin talks to C. It is bound to the
//! one ghostty commit `build.rs` fetches
//! (`3a3047f6b62a791fd8b12d9f07a85b3d2160370b`); every constant, struct layout
//! and function signature below is transcribed from that commit's generated
//! headers under `OUT_DIR/ghostty/include/ghostty/vt/`. The layout test in
//! this module compiles a C probe against those same headers at test time and
//! compares it against the Rust `#[repr(C)]` layouts, so a pin change that
//! shifts an ABI fact fails the test instead of corrupting memory.
//!
//! Conventions:
//!
//! * C enums are backed by `int` on the targets pinwin builds for (types.h's
//!   own `GHOSTTY_ENUM_TYPED` note: "the Zig side backs all C enums with
//!   `c_int`"). Each enum is therefore a type alias for the matching Rust
//!   integer plus `GHOSTTY_*` constants, never a Rust `enum`: C may hand back
//!   a value that is not one of the declared constants, and a Rust `enum`
//!   with an invalid discriminant is undefined behaviour.
//! * Opaque handles are `#[repr(transparent)]` newtypes over `*mut c_void`
//!   (or `*const c_void` for the borrowed kitty image handle) so their size
//!   and calling convention are exactly the C pointer's.
//! * Sized structs are `#[repr(C)]` with the `size` field first, matching the
//!   `GHOSTTY_INIT_SIZED` convention: callers zero the struct and set `size`
//!   to `size_of::<T>()` before handing it to the library.
//!
//! ## Panics must never cross the FFI boundary (D5)
//!
//! `Cargo.toml` sets `panic = "unwind"` in every profile, and pinwin wraps
//! host-facing work in `catch_unwind`. An `extern "C" fn` that lets a panic
//! escape is undefined behaviour ("panic in a function that cannot unwind"),
//! and on this platform aborts the process instead. Every callback declared
//! here (`GhosttyTerminalWritePtyFn`, `GhosttyTerminalSizeFn`,
//! `GhosttyTerminalDeviceAttributesFn`, `GhosttySysDecodePngFn`) is therefore
//! called from C into pinwin code that must be panic-free; the Rust
//! trampolines that install `catch_unwind` guards around these callbacks are
//! pinwin's own and arrive with the `term` unit that uses them. No callback
//! declared in this module may unwind.

pub mod build_info;
pub mod input;
pub mod kitty;
pub mod render;
pub mod screen;
pub mod style;
pub mod sys;
pub mod terminal;

use std::os::raw::c_uchar;
use std::os::raw::{c_int, c_void};

/// Result codes for libghostty-vt operations (`GhosttyResult`, types.h).
pub type GhosttyResult = c_int;

pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;
pub const GHOSTTY_IO_ERROR: GhosttyResult = -5;
pub const GHOSTTY_LIMIT_EXCEEDED: GhosttyResult = -6;
pub const GHOSTTY_REJECTED: GhosttyResult = -7;

/// A terminal mode: the low 15 bits are the mode number, bit 15 is the ANSI
/// (DEC vs ANSI) flag (`GhosttyMode`, modes.h). The inline
/// `ghostty_mode_new`/`ghostty_mode_value`/`ghostty_mode_ansi` helpers are
/// reproduced here; `input` needs `ghostty_mode_new(1004, false)`.
pub type GhosttyMode = u16;

/// Build a `GhosttyMode` from a mode number and whether it is an ANSI mode.
#[must_use]
pub const fn ghostty_mode_new(value: u16, ansi: bool) -> GhosttyMode {
    (value & 0x7fff) | ((ansi as u16) << 15)
}

/// The mode number of a `GhosttyMode`, without the ANSI bit.
#[must_use]
pub const fn ghostty_mode_value(mode: GhosttyMode) -> u16 {
    mode & 0x7fff
}

/// Whether a `GhosttyMode` is an ANSI mode.
#[must_use]
pub const fn ghostty_mode_ansi(mode: GhosttyMode) -> bool {
    (mode >> 15) != 0
}

/// A custom memory allocator (`GhosttyAllocator`, allocator.h). A NULL pointer
/// passed to any function that takes `*const GhosttyAllocator` selects the
/// library's default allocator.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyAllocator {
    /// Opaque context passed to every vtable call.
    pub ctx: *mut c_void,
    /// The allocator's function table.
    pub vtable: *const GhosttyAllocatorVtable,
}

/// The Zig-style allocator vtable (`GhosttyAllocatorVtable`, allocator.h).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyAllocatorVtable {
    pub alloc: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            len: usize,
            alignment: u8,
            ret_addr: usize,
        ) -> *mut c_void,
    >,
    pub resize: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            memory: *mut c_void,
            memory_len: usize,
            alignment: u8,
            new_len: usize,
            ret_addr: usize,
        ) -> bool,
    >,
    pub remap: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            memory: *mut c_void,
            memory_len: usize,
            alignment: u8,
            new_len: usize,
            ret_addr: usize,
        ) -> *mut c_void,
    >,
    pub free: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            memory: *mut c_void,
            memory_len: usize,
            alignment: u8,
            ret_addr: usize,
        ),
    >,
}

unsafe extern "C" {
    /// Allocate `len` bytes with `allocator`, or the default allocator when it
    /// is NULL (allocator.h).
    pub fn ghostty_alloc(allocator: *const GhosttyAllocator, len: usize) -> *mut c_uchar;
}

#[cfg(test)]
mod tests;
