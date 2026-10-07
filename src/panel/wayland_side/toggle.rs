//! The show/hide toggle on the panel thread (replace-gtk-with-wayland
//! row 9.1, decision 4): the command the host posts flips the panel between
//! its mapped and unmapped states.
//!
//! Hide ends a running width tween at its target — the stop relay, including
//! the deferred grid resize — then attaches a null buffer to the panel
//! surface and commits, which unmaps it, and unmaps the reserve the same
//! way, which releases the held strip. The terminal, the pty source and the
//! child keep running; the grid stays as it is.
//!
//! Show re-sends the layer state an unmap reset — size, anchor, margins,
//! exclusive zone -1 and keyboard interactivity, `on-demand` directly in
//! `on-demand` mode, the launch keeps its no-interactivity first map — and
//! commits without a buffer; the configure that follows draws the current
//! grid and maps the panel, and a new height resizes the grid and the pty
//! as any configure does. A reserve the compositor closed while hidden is
//! recreated on the panel's output.
//!
//! The shown/hidden state is the [`Visibility`] enum on
//! [`PanelState`](super::state::PanelState), not a flag pair: the toggle,
//! the hidden apply and the repaint service branch on it. The decision half
//! is display-free (`port-to-rust` D10) — the visibility flip, the tween
//! relay and the hidden apply's staging run against the headless core, and
//! the surface writes are the session-guarded tail of each path.

use super::sizing::Grid;

use super::apply::SurfaceGeometry;
use super::state::PanelState;
use super::surfaces::{grid_width_px, panel_margins};

/// Whether the panel's surfaces are mapped or unmapped (row 9.1, D4). The
/// panel is shown at start; a toggle flips it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Visibility {
    /// The panel and its reserve are mapped.
    Shown,
    /// The panel and its reserve are unmapped; a show maps them again.
    Hidden,
}

impl PanelState {
    /// The posted toggle command (row 9.1, D4): hide a shown panel, show a
    /// hidden one. The caller answers the command's bounded reply after this
    /// returns — the hide's null-buffer commit, or the show's commit without
    /// a buffer; the rest of a show follows the configure.
    pub(crate) fn toggle(&mut self) {
        match self.visibility {
            Visibility::Shown => {
                let mut push = self.grid_sink();
                self.hide(&mut push);
            }
            Visibility::Hidden => self.show(),
        }
    }

    /// The hide half (D4): end a running tween at its target — the stop
    /// relay, whose deferred grid resize the sizing path pushes through
    /// `push` — then unmap the panel and the reserve with null-buffer
    /// commits. The push sink is a parameter so the display-free tests
    /// observe it (`port-to-rust` D10); the production caller supplies the
    /// state's own grid sink.
    pub(crate) fn hide(&mut self, push: &mut dyn FnMut(Grid)) {
        // The tween's stop relay (row 6.1): the wide cache drops, the driver
        // cancels, the sizing defer lifts and the deferred grid — the
        // target's, `self.applied` was staged at the begin — pushes once.
        self.tween_draw = None;
        self.tween.cancel();
        self.sizing.defer_pushes(false);
        let cols = self.applied.cols();
        self.sizing.apply_columns(cols, push);
        // The unmap: null buffers, committed. The surfaces stay alive; a
        // show re-sends the layer state an unmap reset.
        if let Some(session) = self.session.as_mut()
            && let Some(surfaces) = session.surfaces.as_mut()
        {
            surfaces.hide();
        }
        self.visibility = Visibility::Hidden;
    }

    /// The show half (D4): map the panel and the reserve again with the
    /// applied layout's and the held gap's layer state, committed without a
    /// buffer. The configure that follows draws the current grid and maps
    /// the panel; a new height resizes the grid and the pty as any
    /// configure does.
    fn show(&mut self) {
        // The layer state the show sends: the applied layout's panel
        // geometry and the held gap's reserve one — what a hidden apply
        // stored, or what was live at the hide.
        let geometry = SurfaceGeometry {
            panel_side: self.applied.side(),
            panel_margins: panel_margins(self.applied),
            panel_width: grid_width_px(self.applied.cols(), self.cell),
            reserve_side: self.held.side(),
            reserve_zone: self.held.zone(),
        };
        // A reserve the compositor closed while hidden is recreated on the
        // panel's output, so the show maps the held strip again (D4). No
        // recorded output (the surfaces were closed with it): the show maps
        // without a reservation, the same degradation a refused region takes.
        if self.reserve_is_gone()
            && let Some(output) = self
                .session
                .as_ref()
                .and_then(|session| session.panel_output.clone())
        {
            let qh = self
                .session
                .as_ref()
                .map(|session| session.qh.clone())
                .expect("the session a reserve is recreated in");
            self.create_reserve(&output, &qh);
        }
        if let Some(session) = self.session.as_mut()
            && let Some(surfaces) = session.surfaces.as_mut()
        {
            surfaces.show(&geometry);
        }
        self.visibility = Visibility::Shown;
    }

    /// Whether the session's surfaces lack the reserve: the show recreates
    /// it in that case.
    fn reserve_is_gone(&self) -> bool {
        self.session
            .as_ref()
            .and_then(|session| session.surfaces.as_ref())
            .is_some_and(|surfaces| !surfaces.has_reserve())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Inner, Startup};
    use super::*;
    use crate::guard::Poisoned;
    use crate::layout::{CellSize, Keyboard, Layout, OutputSize, Side};
    use crate::panel::handshake::Handshake;
    use crate::surfaces::PublishOutcome;
    use std::num::NonZeroU16;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    /// A startup for the tests; the thread does not touch the pty fd until
    /// the surfaces push a grid, so a placeholder fd is fine here.
    fn startup() -> Startup {
        Startup {
            fd: -1,
            layout: layout(Side::Left, 40, 0, 0, 0, 0),
            keyboard: Keyboard::OnDemand,
            accent: None,
        }
    }

    fn layout(side: Side, cols: u16, top: i32, bottom: i32, left: i32, right: i32) -> Layout {
        Layout::new(
            side,
            NonZeroU16::new(cols).expect("test column count is non-zero"),
            top,
            bottom,
            left,
            right,
        )
    }

    /// The startup cell metrics the tests carry, the way a start command
    /// would (D3).
    fn cell() -> CellSize {
        CellSize::new(9, 16).expect("test cell size is non-zero")
    }

    fn output(width: i32, height: i32) -> OutputSize {
        OutputSize::new(width, height).expect("test output size is non-zero")
    }

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<Inner> {
        Arc::new(Inner {
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
            keyboard: Keyboard::OnDemand,
        })
    }

    fn headless_state() -> PanelState {
        let (tx, _rx) = mpsc::channel();
        PanelState::headless(
            Handshake::new(tx),
            Poisoned::new(),
            live_inner(),
            startup(),
            cell(),
        )
    }

    /// A toggle flips the shown/hidden state and a second one flips it
    /// back: the state is the [`Visibility`] enum, and the toggle's two
    /// halves alternate (the spec's "A toggle hides a shown panel and shows
    /// a hidden one"). The surface writes are the session-guarded tail, so
    /// the headless core exercises the state machine alone.
    #[test]
    fn a_toggle_flips_the_visibility_and_back() {
        let mut state = headless_state();
        assert_eq!(
            state.visibility,
            Visibility::Shown,
            "the panel starts shown"
        );

        state.toggle();
        assert_eq!(state.visibility, Visibility::Hidden, "the first hides");

        state.toggle();
        assert_eq!(state.visibility, Visibility::Shown, "the second shows");
    }

    /// A hide ends a running tween at its target: the stop relay drops the
    /// wide cache, cancels the driver and pushes the deferred grid — the
    /// target's columns — through the sizing path (D4, the relay of the
    /// tween's finish).
    #[test]
    fn a_hide_ends_a_running_tween_at_its_target() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        let mut pushed: Vec<crate::panel::wayland_side::sizing::Grid> = Vec::new();
        let _ = state.stage_animated(
            output(1920, 1080),
            layout(Side::Left, 120, 0, 0, 0, 12),
            200,
            &mut |grid| pushed.push(grid),
        );
        state
            .tween
            .begin(360, 1080, 200, std::time::Instant::now(), None);
        assert!(
            state.tween.is_active(),
            "the test tweens like the apply would"
        );
        assert!(pushed.is_empty(), "the tween holds the deferred push");

        // The hide's push sink is a parameter (the `tween_finished` seam):
        // the test observes the deferred grid resize through it.
        state.hide(&mut |grid| pushed.push(grid));
        assert_eq!(state.visibility, Visibility::Hidden, "the toggle hides");
        assert!(!state.tween.is_active(), "the hide ended the tween");
        assert!(state.tween_draw.is_none(), "the wide cache dropped");
        assert_eq!(pushed.len(), 1, "the deferred grid resize landed");
        assert_eq!(pushed[0].cols(), 120, "at the target's columns");
        assert_eq!(state.applied.cols().get(), 120, "the target layout stored");
    }

    /// While hidden, an animated apply validates and stores the layout and
    /// resizes the grid, with no animation and no tween started (the spec's
    /// "Apply while hidden" scenario, D4): the next show uses the layout.
    #[test]
    fn a_hidden_apply_stores_the_layout_without_starting_a_tween() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        state.toggle();
        assert_eq!(state.visibility, Visibility::Hidden);

        let outcome = state.apply_hidden(output(1920, 1080), layout(Side::Left, 120, 0, 0, 8, 12));
        assert_eq!(outcome, PublishOutcome::Applied, "the hidden apply accepts");
        assert_eq!(
            state.applied.cols().get(),
            120,
            "the layout is stored for the next show"
        );
        assert!(
            !state.tween.is_active(),
            "a hidden apply starts no tween, whatever its duration"
        );
        assert!(state.tween_draw.is_none(), "no wide cache either");
        // The grid resize went through the state's push seam, as it does
        // when shown: a second `apply_columns` of the same columns derives
        // the same grid and pushes nothing, so an empty observation is the
        // evidence the hidden apply already pushed.
        let mut observed: Vec<crate::panel::wayland_side::sizing::Grid> = Vec::new();
        state
            .sizing
            .apply_columns(state.applied.cols(), &mut |grid| {
                observed.push(grid);
            });
        assert!(
            observed.is_empty(),
            "the hidden apply already pushed the grid"
        );
        assert_eq!(
            state.held.zone(),
            8 + 1080 + 12,
            "the held strip follows the stored layout"
        );
    }

    /// A hidden apply the output rejects stages nothing: the verdict is
    /// `InvalidLayout` and the applied layout stays the stored one.
    #[test]
    fn a_hidden_apply_the_output_rejects_stages_nothing() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        state.toggle();

        // A covering 120-column layout is 1080px wide at a 9px cell, past
        // the 600px output: rejected.
        let too_wide = layout(Side::Left, 120, 0, 0, 0, 0).covering();
        assert_eq!(
            state.apply_hidden(output(600, 1080), too_wide),
            PublishOutcome::InvalidLayout
        );
        assert_eq!(
            state.applied.cols().get(),
            40,
            "the rejected layout is not stored"
        );
        assert_eq!(state.held.zone(), 360, "the held gap is untouched");
    }
}
