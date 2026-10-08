//! The show/hide toggle on the panel thread (replace-gtk-with-wayland
//! D4): the command the host posts flips the panel between
//! its mapped and unmapped states.
//!
//! Hide ends a running width tween at its target — the stop relay, including
//! the deferred grid resize — then attaches a null buffer to the panel
//! surface and commits, which unmaps it, and unmaps the reserve the same
//! way, which releases the held strip. The terminal, the pty source and the
//! child keep running; the grid stays as it is.
//!
//! Show re-sends the layer state an unmap reset — size, anchor, margins,
//! exclusive zone -1, keyboard interactivity, `on-demand` directly in
//! `on-demand` mode, the launch keeps its no-interactivity first map — and
//! the layers themselves (D4): the compositor's shell drops the whole
//! double-buffered state to its defaults on the unmap commit, the default
//! layer being `background`, so a show puts the panel back on `overlay` and
//! the reserve on `bottom` before the remapping commit — and commits without
//! a buffer; the configure that follows draws the current grid and maps the
//! panel, and a new height resizes the grid and the pty as any configure
//! does. A reserve the compositor closed while hidden is recreated on the
//! panel's output.
//!
//! The shown/hidden state is the [`Visibility`] enum on
//! [`PanelState`], not a flag pair: the toggle,
//! the hidden apply and the repaint service branch on it. The decision half
//! is display-free (`port-to-rust` D10) — the visibility flip, the tween
//! relay and the hidden apply's staging run against the headless core, and
//! the surface writes are the session-guarded tail of each path.

use super::sizing::Grid;

use super::apply::SurfaceGeometry;
use super::state::PanelState;
use super::surfaces::{grid_width_px, panel_margins};

/// Whether the panel's surfaces are mapped or unmapped (D4). The
/// panel is shown at start; a toggle flips it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Visibility {
    /// The panel and its reserve are mapped.
    Shown,
    /// The panel and its reserve are unmapped; a show maps them again.
    Hidden,
}

impl PanelState {
    /// The posted toggle command (D4): hide a shown panel, show a
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
        // The tween's stop relay (D4): the wide cache drops, the driver
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

    /// The posted show command (serve-instance-socket D6): show a hidden
    /// panel; a shown panel is left unchanged — the command answers at
    /// once with no commit.
    pub(crate) fn show(&mut self) {
        if self.visibility == Visibility::Hidden {
            self.show_hidden();
        }
    }

    /// The show half (D4): map the panel and the reserve again with the
    /// applied layout's and the held gap's layer state, committed without a
    /// buffer. The configure that follows draws the current grid and maps
    /// the panel; a new height resizes the grid and the pty as any
    /// configure does. The posted show reaches this only from hidden —
    /// [`PanelState::show`] short-circuits a shown panel first (D6).
    fn show_hidden(&mut self) {
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
        // The show-time configure must attach a buffer even for an idle
        // panel (D4): the remap happens when that configure draws,
        // and a frame gate whose last frame was clean plans nothing — the
        // panel would stay unmapped until pty output or a focus change.
        // Forcing the next frame to repaint everything makes the show's
        // configure always present. The gate lives on the thread's one
        // renderer, so the flag survives until that draw consumes it.
        self.render.renderer.borrow_mut().invalidate();
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
    use super::super::renderer::{FontSetup, Renderer};
    use super::super::tween_draw::TweenRender;
    use super::super::{Inner, Startup};
    use super::*;
    use crate::fontconfig::{FontConfig, ThemeColours};
    use crate::guard::Poisoned;
    use crate::layout::{CellSize, Keyboard, Layout, OutputSize, Side};
    use crate::panel::handshake::{Handshake, PublishOutcome};
    use crate::render::font::FontBook;
    use crate::render::frame_gate::{Damage, FrameOutcome};
    use crate::render::geom::{FrameInput, device_px};
    use crate::term::{PngDecoder, PtySink, Terminal};
    use std::cell::RefCell;
    use std::num::NonZeroU16;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    /// The family the tests pin; CI installs it. The same family the
    /// renderer tests pin, and the same skip when the machine lacks it.
    const FAMILY: &str = "JetBrainsMono Nerd Font";
    /// The size in points the tests run at: the Ghostty default 11.
    const SIZE: f64 = 11.0;
    /// The test theme, distinct from every colour the tests draw.
    const THEME: ThemeColours = ThemeColours {
        background: [10, 20, 30],
        foreground: [200, 150, 100],
    };

    /// A sink with nowhere to write (the tests never write to a pty).
    struct NullSink;
    impl PtySink for NullSink {
        fn write_pty(&mut self, _data: &[u8]) {}
    }

    /// Decodes nothing; these tests place no kitty images.
    struct NoDecoder;
    impl PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
            None
        }
    }

    /// The test family's font setup, or `None` (printed) when the machine
    /// lacks it — the font-dependent tests skip with a printed message only
    /// then; a broken `FontBook` or a failed lookup is an `expect`, so a
    /// regression cannot hide behind "not installed".
    fn setup() -> Option<FontSetup> {
        let book = FontBook::new().expect("the font book opens");
        if !book.has_family(FAMILY) {
            println!("skipped: {FAMILY} is not installed");
            return None;
        }
        Some(FontSetup::resolve_over(book, &config()).expect("the family resolves"))
    }

    /// The config the tests resolve: the test family at the test size.
    fn config() -> FontConfig {
        FontConfig {
            family: Some(FAMILY.to_owned()),
            size: SIZE,
        }
    }

    /// The test setup's cell as the plain `(width, height)` pair.
    fn cell_of(setup: &FontSetup) -> (i32, i32) {
        let cell = setup.cell().expect("the cell fits");
        (cell.width().get(), cell.height().get())
    }

    /// A renderer over `setup`'s font at scale 1, with the test theme.
    fn renderer(setup: FontSetup) -> Renderer {
        Renderer::new(setup, 1.0, THEME, None).expect("the renderer builds")
    }

    /// An 8-column, 4-row terminal at the `cell` pitch, with a line of text
    /// pushed into its first row.
    fn terminal(cell: (i32, i32)) -> Rc<RefCell<Terminal>> {
        let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
        assert!(
            terminal.push_size(8, 4, cell.0, cell.1),
            "the test grid pushes"
        );
        Rc::new(RefCell::new(terminal))
    }

    /// Frame input for the 8-column, 4-row frame at `cell`, scale 1.
    fn frame(cell: (i32, i32)) -> FrameInput {
        let (cw, ch) = cell;
        let (lw, lh) = (8 * cw, 4 * ch);
        let device_w = u32::try_from(device_px(f64::from(lw))).expect("frame fits u32");
        let device_h = u32::try_from(device_px(f64::from(lh))).expect("frame fits u32");
        FrameInput::new(
            u32::try_from(lw).expect("logical fits u32"),
            u32::try_from(lh).expect("logical fits u32"),
            device_w,
            device_h,
            0.0,
            false,
            THEME,
            None,
        )
    }

    /// A startup for the tests; the thread does not touch the pty fd until
    /// the surfaces push a grid, so a placeholder fd is fine here.
    fn startup() -> Startup {
        Startup::new(
            -1,
            layout(Side::Left, 40, 0, 0, 0, 0),
            Keyboard::OnDemand,
            None,
        )
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
        })
    }

    fn headless_state() -> PanelState {
        let (tx, _rx) = mpsc::channel();
        let poisoned = Poisoned::new();
        PanelState::new(
            Handshake::new(tx),
            poisoned.clone(),
            live_inner(),
            startup(),
            cell(),
            super::super::glue::test_wiring(&poisoned),
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

    /// The posted show branches on the visibility (serve-instance-socket
    /// D6): a shown state is left shown — the show half never runs — and a
    /// hidden one is shown again, like the toggle's show half.
    #[test]
    fn a_show_branches_on_the_visibility() {
        let mut state = headless_state();
        state.show();
        assert_eq!(
            state.visibility,
            Visibility::Shown,
            "the shown state stays shown"
        );

        state.toggle();
        assert_eq!(state.visibility, Visibility::Hidden);
        state.show();
        assert_eq!(state.visibility, Visibility::Shown, "the hidden shows");
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

    /// A configure that arrives while hidden records the size and drives
    /// the grid sizing — a new height resizes the grid and the pty as any
    /// configure does — and the panel stays hidden (D4): the stale configure
    /// queued before the hide's commit maps nothing, and the show's own
    /// configure draws and maps later.
    #[test]
    fn a_hidden_configure_records_the_height_and_pushes_the_grid() {
        let mut state = headless_state();
        state.sizing.configure(1080, &mut |_| {});
        state.toggle();
        assert_eq!(state.visibility, Visibility::Hidden);

        let mut pushed: Vec<crate::panel::wayland_side::sizing::Grid> = Vec::new();
        state.configure_hidden_panel(1920, 1040, &mut |grid| pushed.push(grid));
        assert_eq!(
            state.panel_size,
            Some((1920, 1040)),
            "the hidden configure records the size"
        );
        assert_eq!(pushed.len(), 1, "a new height resizes the grid and the pty");
        assert_eq!(pushed[0].rows(), 1040 / 16, "the rows follow the height");
        assert_eq!(
            state.visibility,
            Visibility::Hidden,
            "the configure maps nothing"
        );

        // A repeat configure at the same height pushes nothing more.
        let mut pushed: Vec<crate::panel::wayland_side::sizing::Grid> = Vec::new();
        state.configure_hidden_panel(1920, 1040, &mut |grid| pushed.push(grid));
        assert!(pushed.is_empty(), "the same height pushes nothing");
    }

    /// A show invalidates the frame gate (D4): after a clean frame, a hide
    /// and a show make the show-time configure's draw present anyway — an
    /// idle panel would otherwise stay unmapped until pty output or a focus
    /// change remounted the frame. The draw here is the same gated draw the
    /// configure path runs ([`super::super::present`]).
    #[test]
    fn a_show_invalidates_the_gate_so_an_idle_panel_maps() {
        let Some(setup) = setup() else {
            return;
        };
        let cell = cell_of(&setup);
        let mut state = headless_state();
        let term = terminal(cell);
        term.borrow_mut().push_pty_data(b"\x1b[?25lhi");
        let test_frame = frame(cell);
        state.render = TweenRender {
            terminal: Rc::clone(&term),
            renderer: Rc::new(RefCell::new(renderer(setup))),
        };

        // Two identical draws: the second plans nothing, the gate's clean
        // verdict — the case that left a re-shown idle panel unmapped.
        {
            let panel = &state.render;
            let mut renderer = panel.renderer.borrow_mut();
            let _first = renderer.draw(&mut term.borrow_mut(), &test_frame);
            assert_eq!(
                renderer.draw(&mut term.borrow_mut(), &test_frame),
                FrameOutcome::Clean,
                "the idle panel's gate is clean"
            );
        };

        // Hide and show: the show invalidates, so the next draw — the
        // show-time configure's — presents the whole surface.
        state.toggle();
        state.toggle();
        let panel = &state.render;
        let (w, h) = test_frame.device_size();
        assert_eq!(
            panel
                .renderer
                .borrow_mut()
                .draw(&mut term.borrow_mut(), &test_frame),
            FrameOutcome::Damage(Damage::whole_surface(w, h)),
            "the shown panel's configure draw presents"
        );
    }

    /// A show on a shown state commits nothing (serve-instance-socket D6,
    /// the spec's "Show a shown panel" scenario): the show half never runs,
    /// so the frame gate is not invalidated and the next idle draw stays
    /// clean — no forced repaint, no commit. The hidden path's
    /// invalidation is pinned by
    /// [`a_show_invalidates_the_gate_so_an_idle_panel_maps`].
    #[test]
    fn a_show_on_a_shown_state_leaves_the_gate_clean() {
        let Some(setup) = setup() else {
            return;
        };
        let cell = cell_of(&setup);
        let mut state = headless_state();
        let term = terminal(cell);
        term.borrow_mut().push_pty_data(b"\x1b[?25lhi");
        let test_frame = frame(cell);
        state.render = TweenRender {
            terminal: Rc::clone(&term),
            renderer: Rc::new(RefCell::new(renderer(setup))),
        };

        // Two identical draws: the second plans nothing, the gate's clean
        // verdict.
        {
            let panel = &state.render;
            let mut renderer = panel.renderer.borrow_mut();
            let _first = renderer.draw(&mut term.borrow_mut(), &test_frame);
            assert_eq!(
                renderer.draw(&mut term.borrow_mut(), &test_frame),
                FrameOutcome::Clean,
                "the idle panel's gate is clean"
            );
        };

        // A show on the shown panel changes nothing: the gate stays clean,
        // so the next draw plans nothing and no commit follows.
        state.show();
        assert_eq!(state.visibility, Visibility::Shown);
        let panel = &state.render;
        assert_eq!(
            panel
                .renderer
                .borrow_mut()
                .draw(&mut term.borrow_mut(), &test_frame),
            FrameOutcome::Clean,
            "the show on a shown panel invalidated nothing"
        );
    }
}
