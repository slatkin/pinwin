//! The tween frame's GSK snapshot branch (poc-gsk-texture-grid task 2.1):
//! during a width tween the once-per-tween grid cache is presented as a
//! texture node instead of a Cairo blit, so the texture uploads once per
//! tween and each frame's CPU work is a transform. Split from `mod.rs` to
//! keep the cairo draw row at its line budget; the same-file privacy rules
//! still apply, since this is a child module of [`super`].
//!
//! The branch mirrors [`DrawState::draw`]'s tween arm: background first, the
//! cached grid translated to the docked edge, then the focus accent. The
//! widget half that calls it lives in [`crate::surfaces::area`]; a panic in
//! this body is caught there by the shared D5 guard and the frame falls back
//! to the cairo draw path.

use gtk4::gdk;
use gtk4::graphene;
use gtk4::prelude::SnapshotExt as _;

use super::DrawState;

impl DrawState {
    /// The tween frame as GSK nodes (poc-gsk-texture-grid task 2.1): the
    /// theme background, the cached grid texture translated to the dock
    /// offset, and the focus accent as a cairo node. Reports whether the
    /// nodes were emitted; `false` — not animating, or no cache yet — asks
    /// the widget to chain to the parent snapshot, which runs the ordinary
    /// cairo draw path. That path is also what builds the cache on the
    /// tween's first frame, so a tween costs exactly one cairo frame.
    ///
    /// The texture is appended at the cache's logical size
    /// ([`Self::grid_cache_logical_size`]) so fractional output scales stay
    /// crisp; the accent is drawn after the translate is popped, in the
    /// window coordinates [`Self::draw`]'s accent uses.
    pub fn snapshot_tween(
        &self,
        snapshot: &gtk4::Snapshot,
        width: i32,
        height: i32,
        draw_offset: i32,
        animating: bool,
    ) -> bool {
        if !animating {
            return false;
        }
        let Some(texture) = self.grid_cache_texture() else {
            return false;
        };
        let Some((logical_w, logical_h)) = self.grid_cache_logical_size() else {
            return false;
        };
        let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);

        // The background fills the whole widget, as draw_inner's fill does.
        let bg = &self.theme_background;
        snapshot.append_color(
            &gdk::RGBA::new(
                f32::from(bg.r) / 255.0,
                f32::from(bg.g) / 255.0,
                f32::from(bg.b) / 255.0,
                1.0,
            ),
            &bounds,
        );

        // The cached grid at the docked edge: translate, texture, pop.
        snapshot.save();
        snapshot.translate(&graphene::Point::new(draw_offset as f32, 0.0));
        snapshot.append_texture(
            &texture,
            &graphene::Rect::new(0.0, 0.0, logical_w as f32, logical_h as f32),
        );
        snapshot.restore();

        // The focus accent keeps its cairo routine, appended as a cairo node.
        let cr = snapshot.append_cairo(&bounds);
        self.draw_focus_accent(&cr, width, height);
        drop(cr);
        true
    }
}
