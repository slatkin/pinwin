//! The grid frame's GSK snapshot branch (poc-gsk-texture-grid task 2.1,
//! extended by gsk-render-nodes row 4.1): every `GridArea::snapshot` frame is
//! presented as GSK nodes. While a width tween runs, the whole grid — theme
//! background, cell backgrounds, cells, cursor and kitty images — is built
//! once into a retained `gsk::RenderNode` on the tween's first frame and
//! appended translated to the docked edge for the rest of the tween: each
//! frame's CPU work is a transform, with no raster and no upload, the glyphs
//! staying in GSK's GPU atlas. Every non-tween draw rebuilds the grid's nodes
//! straight into the widget's snapshot, as the Cairo path redrew each frame.
//!
//! The branch mirrors [`DrawState::draw`]'s two arms: background first, then
//! the grid (cached and translated while animating), then the focus accent —
//! appended after the translate is popped, in the window coordinates
//! `draw_inner`'s accent uses.
//!
//! Fallback: when node emission is not possible (no fonts yet, no live
//! frame), the method reports `false` and the widget chains to the parent
//! snapshot, which runs the ordinary cairo draw path — still the parity
//! oracle and the `GSK_RENDERER=cairo` fallback. On the tween arm that path
//! keeps the stage-1 texture cache (`grid_cache_ensure`) working, so a tween
//! that cannot emit nodes falls back to the texture per tween. A panic in a
//! snapshot body is caught by the hook's D5 guard and the frame falls back
//! the same way; a poisoned latch draws nothing on either path.
//!
//! The widget half that calls it lives in [`crate::surfaces::area`]; the hook
//! body — which reads the tween state and the terminal — is wired in
//! [`crate::panel::gtk_side`].

use gtk4::graphene;
use gtk4::gsk;
use gtk4::prelude::SnapshotExt as _;

use super::DrawState;
use super::nodes::rgba;
use crate::term::Terminal;

/// A fresh `gtk4::Snapshot` for a grid build. `glib::Object::new` instead of
/// `Snapshot::new`: the convenience constructor asserts GTK initialisation,
/// and the display-free tests build snapshots too (gsk-render-nodes task
/// 1.1's finding).
pub(super) fn new_snapshot() -> gtk4::Snapshot {
    gtk4::glib::Object::new()
}

impl DrawState {
    /// One frame as GSK nodes (gsk-render-nodes row 4.1): the theme
    /// background, the grid — the retained node translated to the dock offset
    /// while a width tween runs, rebuilt into `snapshot` on every non-tween
    /// draw — and the focus accent as a cairo node. Reports whether the nodes
    /// were emitted; `false` — node emission impossible, see the module docs
    /// — asks the widget to chain to the parent snapshot, which runs the
    /// ordinary cairo draw path.
    ///
    /// `draw_offset` is the tween's docked-edge offset in pixels
    /// (`glue_anim_draw_offset`); zero when not animating
    /// (`Anim::draw_offset`), so the non-tween arm needs no translate.
    pub fn snapshot_grid(
        &mut self,
        snapshot: &gtk4::Snapshot,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        draw_offset: i32,
        animating: bool,
    ) -> bool {
        let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);

        if animating {
            let Some(node) = self.grid_node_ensure(terminal, width, height) else {
                return false;
            };
            // The background fills the whole widget, as draw_inner's fill
            // does; the cached node covers only its build-time bounds, and a
            // tween resizes the widget every frame.
            snapshot.append_color(&rgba(&self.theme_background), &bounds);

            // The cached grid at the docked edge: translate, node, pop.
            snapshot.save();
            snapshot.translate(&graphene::Point::new(draw_offset as f32, 0.0));
            snapshot.append_node(&node);
            snapshot.restore();
        } else if !self.emit_grid(snapshot, terminal, width, height) {
            return false;
        }

        // The focus accent keeps its cairo routine, appended after the grid —
        // translated or not — in the window coordinates draw_inner's accent
        // uses.
        let cr = snapshot.append_cairo(&bounds);
        self.draw_focus_accent(&cr, width, height);
        drop(cr);
        true
    }

    /// The cached grid node for this tween (`grid_node_ensure`), built on the
    /// tween's first frame. Keyed by column count and height, exactly like
    /// the cairo texture cache: the terminal grid is resized only when the
    /// tween ends (the surfaces' deferred grid resize), so the node starts
    /// correct, and content that changes while the tween runs shows when the
    /// tween ends and the non-tween path resumes — a 200 ms stale window is
    /// invisible next to the cost it avoids. Dropped when a tween stops
    /// ([`Self::drop_grid_cache`]) and when the cell metrics change. `None`
    /// — node emission impossible — falls back to the cairo draw path.
    fn grid_node_ensure(
        &mut self,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
    ) -> Option<gsk::RenderNode> {
        let cols = i32::from(terminal.cols());
        if let Some(node) = &self.grid_node
            && self.grid_node_cols == cols
            && self.grid_node_height == height
        {
            return Some(node.clone());
        }
        let node = self.build_grid_node(terminal, width, height)?;
        self.grid_node = Some(node.clone());
        self.grid_node_cols = cols;
        self.grid_node_height = height;
        Some(node)
    }

    /// Build the retained grid node (gsk-render-nodes row 4.1): the whole
    /// grid emitted into a fresh snapshot and finished with `to_node` — the
    /// node-cache twin of `grid_cache_ensure`'s surface render. `None` —
    /// emission impossible — falls back to the cairo draw path.
    fn build_grid_node(
        &mut self,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
    ) -> Option<gsk::RenderNode> {
        let snapshot = new_snapshot();
        if !self.emit_grid(&snapshot, terminal, width, height) {
            return None;
        }
        snapshot.to_node()
    }

    /// Emit the frame's whole grid into `snapshot` — the theme background and
    /// cell backgrounds, the cells and cursor
    /// ([`Self::emit_backgrounds`] + [`Self::emit_cells`]) and the kitty
    /// images ([`Self::emit_images`]) — the node form of `draw_inner`'s cairo
    /// pass minus the accent. `false` — no fonts or metrics yet, or no live
    /// frame — asks the caller to fall back to the cairo draw path.
    fn emit_grid(
        &mut self,
        snapshot: &gtk4::Snapshot,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
    ) -> bool {
        if !self.emit_backgrounds(snapshot, terminal, width, height) {
            return false;
        }
        if !self.emit_cells(snapshot, terminal) {
            return false;
        }
        self.emit_images(snapshot, terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{state, terminal};
    use super::*;
    use crate::fontconfig::ThemeColours;
    use crate::guard::Poisoned;
    use crate::render::font_lock;
    use crate::render::parity;

    use crate::render::DrawState;

    /// Run one `snapshot_grid` call and render its output to a backed
    /// surface, so the frame's pixels are inspectable.
    fn drawn_frame(
        state: &mut DrawState,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        draw_offset: i32,
        animating: bool,
    ) -> cairo::ImageSurface {
        let snapshot = parity::snapshot();
        assert!(
            state.snapshot_grid(&snapshot, terminal, width, height, draw_offset, animating),
            "the frame was emitted as nodes"
        );
        let node = snapshot.to_node().expect("snapshot produced a node");
        let surface = parity::backed_surface(width, height, 1.0, parity::BACKDROP);
        parity::draw_node(&node, &surface);
        surface
    }

    /// The tween's first frame builds the retained node and draws it.
    #[test]
    fn the_first_tween_frame_builds_the_grid_node_and_draws_it() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut terminal = terminal();
        terminal.push_pty_data(b"hello");
        let snapshot = parity::snapshot();
        assert!(
            draw_state.snapshot_grid(&snapshot, &mut terminal, 64, 64, 0, true),
            "the tween frame was emitted"
        );
        assert!(
            draw_state.grid_node.is_some(),
            "the first tween frame built the retained node"
        );
        let node = snapshot.to_node().expect("snapshot produced a node");
        let mut surface = parity::backed_surface(64, 64, 1.0, parity::BACKDROP);
        parity::draw_node(&node, &surface);
        let mut bare = parity::backed_surface(64, 64, 1.0, parity::BACKDROP);
        assert!(
            parity::diff(&mut surface, &mut bare).differing > 0,
            "the tween frame drew the grid"
        );
    }

    /// Later tween frames reuse the cached node — content that changed
    /// mid-tween does not show until the tween ends — while the non-tween
    /// draw after it does rebuild.
    #[test]
    fn later_tween_frames_reuse_the_cached_node() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut terminal = terminal();
        terminal.push_pty_data(b"first");
        let mut first = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 0, true);

        // New content, same cols and height: the tween keeps drawing the
        // frame it cached on its first frame.
        terminal.push_pty_data(b"\x1b[1;1H\x1b[41msecond\x1b[0m");
        let mut second = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 0, true);
        parity::assert_exact(
            &mut first,
            &mut second,
            "the second tween frame reuses the cached node",
        );

        // The non-tween draw rebuilds from the same terminal: it shows the
        // new content.
        let mut rebuilt = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 0, false);
        assert!(
            parity::diff(&mut rebuilt, &mut second).differing > 0,
            "the non-tween draw rebuilt the grid from the new content"
        );
    }

    /// Every non-tween draw rebuilds the grid: two frames with different
    /// terminal content render differently.
    #[test]
    fn a_non_tween_draw_rebuilds_the_grid_every_frame() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut terminal = terminal();
        terminal.push_pty_data(b"aaa");
        let mut first = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 0, false);
        terminal.push_pty_data(b"\x1b[1;1H\x1b[41mbbb");
        let mut second = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 0, false);
        assert!(
            parity::diff(&mut first, &mut second).differing > 0,
            "the second non-tween draw rebuilt the grid"
        );
    }

    /// The tween's stop relay drops the retained node with the texture cache
    /// ([`Self::drop_grid_cache`]): the next tween frame rebuilds from the
    /// terminal's current content.
    #[test]
    fn dropping_the_cache_invalidates_the_tween_node() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut terminal = terminal();
        terminal.push_pty_data(b"aaa");
        let mut cached = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 0, true);

        state_drop(&mut draw_state);
        terminal.push_pty_data(b"\x1b[1;1H\x1b[41mbbb");
        let mut rebuilt = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 0, true);
        assert!(
            draw_state.grid_node.is_some(),
            "the next tween frame rebuilt the node"
        );
        assert!(
            parity::diff(&mut cached, &mut rebuilt).differing > 0,
            "the node was rebuilt from the new content"
        );
    }

    /// The stop relay's name for the drop, spelled out so the test reads as
    /// the lifecycle it pins (`tween_cache_drop` hook → `drop_grid_cache`).
    fn state_drop(state: &mut DrawState) {
        state.drop_grid_cache();
    }

    /// The cached node is drawn translated to the dock offset, and the
    /// translate does not disturb the reuse: the same offset draws the same
    /// cached frame twice.
    #[test]
    fn the_tween_frame_draws_the_cached_node_translated() {
        let _font = font_lock::guard();
        let mut draw_state = state();
        let mut terminal = terminal();
        terminal.push_pty_data(b"hello");
        let mut unmoved = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 0, true);
        let mut moved = drawn_frame(&mut draw_state, &mut terminal, 64, 64, 16, true);
        assert!(
            parity::diff(&mut unmoved, &mut moved).differing > 0,
            "the dock offset translated the cached grid"
        );
    }

    /// A frame before the first `cell_metrics_update` has no fonts and a
    /// zero cell pitch: node emission reports `false` — tween and non-tween
    /// alike — and the cairo draw path draws the frame instead (render P2).
    #[test]
    fn a_frame_before_the_metrics_exist_falls_back_to_the_cairo_path() {
        let _font = font_lock::guard();
        let mut draw_state = DrawState::new(Poisoned::new(), None, ThemeColours::default());
        let mut terminal = terminal();
        let snapshot = parity::snapshot();
        assert!(
            !draw_state.snapshot_grid(&snapshot, &mut terminal, 64, 64, 0, false),
            "no metrics yet: the non-tween frame falls back"
        );
        assert!(
            !draw_state.snapshot_grid(&snapshot, &mut terminal, 64, 64, 0, true),
            "no metrics yet: the tween frame falls back"
        );
        assert!(
            draw_state.grid_node.is_none(),
            "no node was cached from a failed build"
        );
    }
}
