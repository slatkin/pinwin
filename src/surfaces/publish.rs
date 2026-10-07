//! The publish verdict: the pure half of the former GTK
//! [`super`] publish (`glue_publish_layout`'s return codes). Split out so
//! `src/panel/handshake.rs` and the panel thread (`src/panel/wayland_side/`)
//! can use it without depending on any surface module (row 8.1). The animate
//! decision lives with the panel thread's tween driver
//! (`wayland_side::tween::should_animate`), the only caller since the switch.

/// The outcome of publishing a layout (`glue_publish_layout`'s return codes).
/// The `panel` row (4.1) maps these onto `PinwinError`: `NotLive` is
/// `NotRunning` and `InvalidLayout` is `InvalidLayout`
/// (`pinwin_api.c`'s `apply_on_gtk_thread`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    /// The layout is applied and the grid follows (`PINWIN_GEOM_OK`).
    Applied,
    /// The panel has no live metrics yet, or is already torn down
    /// (`GLUE_NOT_LIVE`).
    NotLive,
    /// The layout the live monitor refuses (`PINWIN_GEOM_ERR_METRICS`).
    InvalidLayout,
}
