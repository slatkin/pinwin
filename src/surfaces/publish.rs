//! The publish verdict: the pure half of the layout publish, validated
//! before anything touches a surface. Split out so
//! `src/panel/handshake.rs` and the panel thread (`src/panel/wayland_side/`)
//! can use it without depending on any surface module. The animate
//! decision lives with the panel thread's tween driver
//! (`wayland_side::tween::should_animate`), its only caller.

/// The outcome of publishing a layout. The panel's callers map these onto
/// `PinwinError`: `NotLive` is `NotRunning` and `InvalidLayout` is
/// `InvalidLayout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    /// The layout is applied and the grid follows.
    Applied,
    /// The panel has no live metrics yet, or is already torn down.
    NotLive,
    /// The layout the live monitor refuses.
    InvalidLayout,
}
