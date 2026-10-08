//! The surfaces' pure core: the held-gap
//! decisions in the `gap` submodule — the validation the panel thread
//! applies against its own Wayland layer surfaces before it touches them
//! (`replace-gtk-with-wayland` D3).
//!
//! Validation itself lives with the layout types ([`crate::layout`]): a
//! monitor with degenerate metrics has no `CellSize`/`OutputSize` to
//! validate with at all, so it maps to the invalid-layout verdict
//! (port-to-rust D6), as `glue.c` `PINWIN_GEOM_ERR_METRICS` did.

pub(crate) mod gap;
