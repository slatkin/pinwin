//! The panel thread's renderer (row 8.1, `replace-gtk-with-wayland` D5):
//! the one owner of everything a frame draws with — one [`TextPass`], one
//! [`ImagePass`], one [`FrameGate`] and a persistent [`Canvas`] at the
//! current frame's device size — plus the scale, theme, accent and focus
//! state the frames and the tween's wide draw read. There is exactly one
//! renderer per panel thread, exactly as there is one text pass and one
//! image pass; the tween path borrows it through the
//! [`TweenRender`](super::tween_draw::TweenRender) bundle and the live
//! frames draw through the renderer's frame entry.
//!
//! The renderer holds no Wayland object and no display: it is testable
//! like the painter (`replace-gtk-with-wayland` D10), with the terminal
//! and the frame as parameters. The pool buffers, the present step and
//! the frame callback wiring are the later row 8.1 dispatches; this
//! module is `pub` so that dispatch can switch the panel to it (a
//! `pub(crate)` entry with no caller is dead code under `-D warnings`,
//! and no lint suppression is permitted).
//!
//! Panics never cross back into calloop or the compositor (D5): the
//! renderer's bodies are plain field operations and painter calls whose
//! failure paths return rather than panic, and the thread runs them
//! under the shared guard.
//!
//! This commit carries the font setup half: the one font load and cell
//! measurement the thread performs at start (D3). The renderer itself
//! joins in the next commit, with the tween bundle's rewiring.

mod font_setup;
#[cfg(test)]
mod tests;

pub use font_setup::{FontSetup, FontSetupError};
