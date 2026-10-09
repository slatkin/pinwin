//! The panel thread's glue (replace-gtk-with-wayland D2): the pieces
//! the thread builds for itself before the surfaces bind — the font setup
//! it measures its cell with, the renderer the frames and the tween's wide
//! draw read, the shared terminal and the repaint flag its callbacks latch,
//! and the pty the terminal's writer comes from. The thread assembles them
//! in [`super::run_thread`], so the sizing and the surfaces meet the
//! measured cell from the first configure (D3).
//!
//! Panics never cross back into calloop or the compositor (D5): the pieces
//! here are constructors whose failure paths return errors instead of
//! panicking, and the closure they hand out only sets a flag — the terminal
//! wraps its own callbacks in the shared guard.

use std::cell::{Cell, RefCell};
use std::os::fd::RawFd;
use std::rc::Rc;

use crate::guard::Poisoned;
use crate::layout::Accent;
use crate::pty::Pty;
use crate::render::png::PngCrateDecoder;
use crate::term::Terminal;

use super::super::handshake::StartOutcome;
use super::buffers::FractionalScale;
use super::renderer::{FontSetup, FontSetupError, Renderer};
use super::seat::SeatLinks;
use super::sizing::Grid;
use super::state::{PanelState, Wiring, apply_pty_size};
use super::tween_draw::TweenRender;

/// The font the thread starts with: [`FontSetup::load`] — the one
/// font load the panel performs, on the thread that owns the text pass —
/// mapped onto the start handshake.
///
/// # Errors
/// [`StartOutcome::Internal`] when the font cannot be resolved at all; the
/// `monospace` fallback is normal operation, not this error.
pub(crate) fn font_start() -> Result<FontSetup, StartOutcome> {
    font_outcome(FontSetup::load())
}

/// The mapping the start handshake sees for the font load: an unresolvable
/// font is not the spec's "no display" case — the display may be fine — so
/// the start fails through the internal path, the same class a caught panic
/// reports (D5). A function of its input so the tests drive the mapping
/// through the broken-`FontBook` seam (`FontBook::fail_all_loads`).
pub(crate) fn font_outcome(
    setup: Result<FontSetup, FontSetupError>,
) -> Result<FontSetup, StartOutcome> {
    match setup {
        Ok(setup) => Ok(setup),
        // The error's class is the decision, not its text: the handshake
        // reports the outcome, and the cause stays in the load's error
        // chain for the log.
        Err(_error) => Err(StartOutcome::Internal),
    }
}

/// The thread's renderer: [`Renderer::new`] over the font setup
/// the thread measured, with the Ghostty theme and the startup accent, at
/// scale 1 — the resolved scale until the compositor reports a preferred
/// one, which [`super::state::PanelState::sync_renderer_scale`] fixes after
/// the bind and on every scale change. Wrapped in an `Rc<RefCell<_>>` so
/// the tween's render bundle and the live frames borrow it without owning
/// it.
///
/// # Errors
/// [`StartOutcome::Internal`] when the scale yields no painter metrics —
/// unreachable for a measured cell's positive pitch, handled rather than
/// unwrapped (D5).
pub(crate) fn thread_renderer(
    accent: Option<Accent>,
    setup: FontSetup,
) -> Result<Rc<RefCell<Renderer>>, StartOutcome> {
    let theme = crate::fontconfig::load_theme_colours();
    Renderer::new(setup, 1.0, theme, accent)
        .map(|renderer| Rc::new(RefCell::new(renderer)))
        .ok_or(StartOutcome::Internal)
}

/// The thread's byte path — the route the child's output takes through the
/// panel thread: the shared terminal, the repaint flag its callbacks
/// latch, the stale pre-resize grid width its output clears, and the pty
/// the writer and the fd slot come from. [`byte_path`] builds it from the
/// host-supplied fd; the terminal is an `Rc<RefCell<_>>` because the seat
/// links and the tween's render bundle hold clones, and the pty read
/// source feeds it from the same thread (replace-gtk-with-wayland D2).
///
/// The `queue_draw` closure [`byte_path`] hands the terminal runs inside
/// the terminal's callbacks: it clears the stale grid record — the
/// terminal produced output, so the drawn grid is the live one again —
/// and sets the repaint flag, which the loop reads and clears after each
/// dispatch; the draw step turns the flag into a frame.
pub(crate) struct BytePath {
    terminal: Rc<RefCell<Terminal>>,
    repaint: Rc<Cell<bool>>,
    stale_grid_px: Rc<Cell<i32>>,
    pty: Pty,
}

impl BytePath {
    /// The pty the read source's fd and fd slot come from.
    #[must_use]
    pub(crate) fn pty(&self) -> &Pty {
        &self.pty
    }

    /// The wiring bundle [`PanelState::new`] takes: the seat links and the
    /// render bundle over this path's terminal, with fresh focus and
    /// draw-offset cells. The state reads its repaint latch from the seat
    /// links, which hold this path's.
    pub(crate) fn wiring(&self, renderer: Rc<RefCell<Renderer>>, poisoned: Poisoned) -> Wiring {
        Wiring {
            stale_grid_px: Rc::clone(&self.stale_grid_px),
            seat_links: seat_links(
                &self.terminal,
                &self.repaint,
                &Rc::new(Cell::new(0.0)),
                &Rc::new(Cell::new(false)),
                poisoned,
            ),
            render: TweenRender {
                terminal: Rc::clone(&self.terminal),
                renderer,
            },
        }
    }
}

impl std::fmt::Debug for BytePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BytePath")
            .field("repaint", &self.repaint)
            .field("stale_grid_px", &self.stale_grid_px)
            .field("pty", &self.pty)
            .finish_non_exhaustive()
    }
}

pub(crate) fn byte_path(poisoned: Poisoned, fd: RawFd) -> BytePath {
    let pty = Pty::new(poisoned.clone(), fd);
    let repaint = Rc::new(Cell::new(false));
    let stale_grid_px = Rc::new(Cell::new(0));
    // The shared output-scale note (device-pixel-cell-reports D3): scale 1
    // until the compositor's preferred scale arrives, when
    // `sync_renderer_scale` publishes into it and the terminal's
    // `size_report` starts answering device pixels.
    let scale_note = Rc::new(Cell::new(120));
    let terminal = Rc::new(RefCell::new(Terminal::with_scale_note(
        poisoned,
        pty.writer(),
        PngCrateDecoder,
        {
            let repaint = Rc::clone(&repaint);
            let stale = Rc::clone(&stale_grid_px);
            move || {
                stale.set(0);
                repaint.set(true);
            }
        },
        Rc::clone(&scale_note),
    )));
    BytePath {
        terminal,
        repaint,
        stale_grid_px,
        pty,
    }
}

/// The seat links the thread's pieces build: the
/// encoders' terminal, the draw offset the frames publish, the focus flag
/// the renderer draws the accent from, the redraw latch the seat's
/// `queue_draw` sets, and the shared latch. The seat handlers route the
/// toolkit's events into them, and the focus flag and the offset cell are
/// already the draw's sources.
pub(crate) fn seat_links(
    terminal: &Rc<RefCell<Terminal>>,
    repaint: &Rc<Cell<bool>>,
    draw_offset: &Rc<Cell<f64>>,
    focused: &Rc<Cell<bool>>,
    poisoned: Poisoned,
) -> SeatLinks {
    SeatLinks {
        terminal: Rc::clone(terminal),
        draw_offset: Rc::clone(draw_offset),
        focused: Rc::clone(focused),
        queue_draw: Rc::clone(repaint),
        poisoned,
    }
}

impl PanelState {
    /// The one grid push sink: every push the sizing decides — a
    /// plain apply's, a mid-tween configure's and the deferred tween-end's —
    /// runs through it, each exactly once (the sizing's pushed-grid memory
    /// never repeats one). The closure owns clones of the shared terminal
    /// and the repaint flag, so the callers hold `&mut self` while it runs.
    /// The winsize goes out
    /// at the session's resolved scale (device-pixel-cell-reports D4).
    pub(crate) fn grid_sink(&self) -> impl FnMut(Grid) + 'static {
        self.grid_sink_at(self.resolved_scale())
    }

    /// The grid push sink at an explicit scale (device-pixel-cell-reports
    /// D4): the scale-note re-push passes the new scale directly, so the
    /// re-push pushes the new device pixels even though the session already
    /// resolved them — and stays unit-testable without a session. Every
    /// other caller uses [`PanelState::grid_sink`].
    pub(crate) fn grid_sink_at(&self, output_scale: FractionalScale) -> impl FnMut(Grid) + 'static {
        let terminal = Rc::clone(&self.render.terminal);
        let repaint = Rc::clone(&self.repaint);
        let stale = Rc::clone(&self.stale_grid_px);
        let fd = self.startup.fd();
        move |grid| {
            push_grid(
                &terminal,
                &repaint,
                stale.as_ref(),
                fd,
                output_scale,
                grid,
            );
        }
    }
}

/// The tests' wiring bundle (typed-publish-path D4): the pieces
/// [`PanelState::new`] takes, built over a display-free `Terminal` —
/// `Terminal::new` with a sink that discards and a decoder that rejects,
/// its `queue_draw` closure latching the same repaint flag and clearing
/// the same stale record the bundle hands the state, the seat links
/// sharing the terminal and the cells, and the render bundle over
/// [`test_renderer`], which these tests never draw with. The test
/// helpers build their state with it; one that already holds its own
/// renderer wires it through [`test_wiring_with`].
#[cfg(test)]
pub(crate) fn test_wiring(poisoned: &Poisoned) -> Wiring {
    test_wiring_with(poisoned, test_renderer())
}

/// [`test_wiring`] over a caller-supplied renderer handle: a test that
/// already resolved its own font setup wires that renderer through
/// instead of the production fallback.
#[cfg(test)]
pub(crate) fn test_wiring_with(poisoned: &Poisoned, renderer: Rc<RefCell<Renderer>>) -> Wiring {
    byte_path(poisoned.clone(), -1).wiring(renderer, poisoned.clone())
}

/// The tests' renderer: a real one over the monospace fallback (normal
/// operation, no family needed) at scale 1 with the test theme. The tests
/// that build a state never draw with it; the field just needs a valid
/// value.
#[cfg(test)]
pub(crate) fn test_renderer() -> Rc<RefCell<Renderer>> {
    let setup = FontSetup::resolve(&crate::fontconfig::FontConfig {
        family: None,
        size: 11.0,
    })
    .expect("the test renderer's font resolves");
    let renderer = Renderer::new(
        setup,
        1.0,
        crate::render::text_pass::test_support::THEME,
        None,
    )
    .expect("the test renderer builds");
    Rc::new(RefCell::new(renderer))
}

/// Push one derived grid to the terminal and the pty, in this order: the
/// terminal first — a terminal that cannot
/// be allocated leaves the previous grid and the pty winsize in place —
/// then the winsize with `SIGWINCH`, then the repaint request the
/// post-resize repaint needs. The terminal keeps the grid's logical cell
/// (device-pixel-cell-reports D2); only the winsize goes out at the device
/// cell for `scale` (D1/D4). A widening push also records the previous
/// grid's pixel width as the stale pre-resize content still on screen (the
/// vt does not rewrap), the narrowest since the last terminal output; the
/// terminal's own output latches the same flag later and clears the
/// record, when the vt answers the new width.
fn push_grid(
    terminal: &Rc<RefCell<Terminal>>,
    repaint: &Cell<bool>,
    stale: &Cell<i32>,
    fd: RawFd,
    output_scale: FractionalScale,
    grid: Grid,
) {
    // Unreachable for a derived grid — the configure height bounds the rows
    // — handled: no push at all rather than a truncated one.
    let Ok(rows) = i32::try_from(grid.rows()) else {
        return;
    };
    let previous_cols = terminal.borrow().cols();
    let pushed = terminal.borrow_mut().push_size(
        i32::from(grid.cols()),
        rows,
        grid.cell_width(),
        grid.cell_height(),
    );
    if !pushed {
        return;
    }
    if i32::from(grid.cols()) > i32::from(previous_cols)
        && let Some(previous_px) = i32::from(previous_cols).checked_mul(grid.cell_width())
    {
        // The narrowest stale width wins: a widening after a widening
        // without any output in between keeps the narrower of the two.
        let current = stale.get();
        stale.set(if current > 0 {
            current.min(previous_px)
        } else {
            previous_px
        });
    }
    apply_pty_size(fd, grid, output_scale);
    repaint.set(true);
}

#[cfg(test)]
mod tests;
