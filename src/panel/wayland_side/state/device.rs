//! The device-pixel winsize push (device-pixel-cell-reports D1/D4): the pty
//! side of the scale-aware reports. The terminal keeps its logical grid —
//! the mouse encoder maps surface-local logical motion with it (D2) — while
//! the winsize ioctl carries the device cell, so the existing
//! `cols × cell_w` product in [`crate::pty::apply_winsize`] yields the
//! device pixel fields with no signature change there.
//!
//! The scale-note handlers ([`PanelState::note_integer_scale`],
//! [`PanelState::note_preferred_scale`]) re-push through here after the
//! renderer sync whenever the device pixel size actually changed; the fd
//! comes from the startup and the grid is re-derived from the sizing's live
//! columns and last configure height, pushed through the same sink the
//! configures use.

use std::os::fd::RawFd;

use crate::pty::apply_winsize;

use super::super::buffers::{FractionalScale, Scale};
use super::super::sizing::Grid;
use super::PanelState;

/// One logical cell dimension in device pixels (device-pixel-cell-reports
/// D1/D5): the logical cell times the resolved scale, rounded half up in
/// exact 1/120 arithmetic — the same rule as the buffers'
/// [`FractionalScale::scale_dimension`] and the size report's multiply, so
/// a reported size never disagrees with a drawn one by rounding-path alone.
/// Falls back to the logical value when the product does not fit `u32`: a
/// cell that size has no drawable grid, and the push below stays alive.
fn device_cell(logical: u32, scale: FractionalScale) -> u32 {
    scale.scale_dimension(logical).unwrap_or(logical)
}

/// Apply a pushed grid to the pty: the winsize ioctl with the grid's column
/// and row counts and the device cell's pixel size, `SIGWINCH` raised inside
/// [`apply_winsize`] on success. The single grid push sink in
/// [`super::super::glue`] runs this after the terminal's own `push_size`,
/// so the terminal and the pty change together; the child's
/// `TIOCGWINSZ` reads the result.
pub(crate) fn apply_pty_size(fd: RawFd, grid: Grid, scale: FractionalScale) {
    let Ok(rows) = i32::try_from(grid.rows()) else {
        return;
    };
    let Some((cell_w, cell_h)) = device_cell_px(grid, scale) else {
        return;
    };
    let _ = apply_winsize(fd, i32::from(grid.cols()), rows, cell_w, cell_h);
}

/// The grid's cell in device pixels at `scale`; `None` for a negative cell
/// dimension, which no derived grid has.
fn device_cell_px(grid: Grid, scale: FractionalScale) -> Option<(u32, u32)> {
    Some((
        device_cell(u32::try_from(grid.cell_width()).ok()?, scale),
        device_cell(u32::try_from(grid.cell_height()).ok()?, scale),
    ))
}

impl PanelState {
    /// The session's resolved output scale (device-pixel-cell-reports D4):
    /// the scale the grid sink pushes the winsize at. Scale 1 until the
    /// session binds — the compositor's preferred scale arrives with the
    /// surfaces' events — so an early push reports scale-1 pixels briefly
    /// and the first scale note's re-push self-heals it.
    pub(crate) fn resolved_scale(&self) -> FractionalScale {
        self.session.as_ref().map_or_else(
            || FractionalScale::from_120ths(120),
            |session| session.scale.resolved(),
        )
    }

    /// Apply one scale note, sync the renderer and re-push the winsize when
    /// the resolved scale moved the device pixels (device-pixel-cell-reports
    /// D4): the sequence both scale-note handlers share.
    pub(crate) fn change_scale(&mut self, note: impl FnOnce(&mut Scale)) {
        let before = self.resolved_scale();
        if let Some(session) = &mut self.session {
            note(&mut session.scale);
        }
        self.sync_renderer_scale();
        let after = self.resolved_scale();
        self.repush_for_scale(before, after);
    }

    /// Re-push the winsize after a scale note (device-pixel-cell-reports
    /// D4): the current grid re-derived from the sizing's live columns and
    /// last configure height, pushed through the grid sink at the new scale
    /// — the sink never repeats a grid on its own, so the grid is derived
    /// explicitly here. Before the first configure there is no live grid,
    /// and when the device pixel size is unchanged, both are a no-op: the
    /// column and row counts never change with the scale.
    pub(crate) fn repush_for_scale(&mut self, before: FractionalScale, after: FractionalScale) {
        let Some(grid) = self.sizing.current_grid() else {
            return;
        };
        if device_cell_px(grid, before) == device_cell_px(grid, after) {
            return;
        }
        let mut push = self.grid_sink_at(after);
        push(grid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Poisoned;
    use crate::layout::{CellSize, Keyboard, Layout, Side};
    use crate::panel::handshake::Handshake;
    use crate::panel::wayland_side::sizing::Sizing;
    use crate::panel::wayland_side::tween_draw::TweenRender;
    use crate::panel::wayland_side::{Inner, Startup, glue};
    use std::cell::Cell;
    use std::num::NonZeroU16;
    use std::os::fd::AsRawFd;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    use super::super::Wiring;

    /// The test cell, the thread's measured pitch the sizing derives from.
    fn cell() -> CellSize {
        CellSize::new(9, 16).expect("test cell size is non-zero")
    }

    fn cols() -> NonZeroU16 {
        NonZeroU16::new(40).expect("test column count is non-zero")
    }

    fn scale_1() -> FractionalScale {
        FractionalScale::from_120ths(120)
    }

    /// Scale 1.8, the fractional scale the spec's scenario names.
    fn scale_1_8() -> FractionalScale {
        FractionalScale::from_120ths(216)
    }

    /// Read back the master's winsize.
    fn read_winsize(fd: RawFd) -> libc::winsize {
        let mut ws = libc::winsize {
            ws_col: 0,
            ws_row: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: `fd` is open and `ws` is writable for the call.
        let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut ws) };
        assert!(result >= 0, "TIOCGWINSZ");
        ws
    }

    /// Block `SIGWINCH` on this test thread for its lifetime: the winsize
    /// ioctls under test raise it, and the pty module's tests record the
    /// signal process-wide — blocking it here keeps this test's raises out
    /// of their recording windows when the tests share one process. The
    /// mask is never restored: the pending signal dies with the thread, and
    /// a restore would deliver it into the shared process instead.
    fn block_sigwinch() {
        // SAFETY: `set` and `old` are writable sigsets for the calls.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&raw mut set);
            libc::sigaddset(&raw mut set, libc::SIGWINCH);
            let mut old: libc::sigset_t = std::mem::zeroed();
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, &raw mut old),
                0,
                "block SIGWINCH"
            );
        }
    }

    /// A live handle state like a started panel's, for the thread-side
    /// tests.
    fn live_inner() -> Arc<Inner> {
        Arc::new(Inner {
            poisoned: Poisoned::new(),
            live: AtomicBool::new(true),
        })
    }

    /// A state wired to the thread's byte path over a real pty
    /// master: the production sink pushes the terminal grid and the pty
    /// winsize through it (D10 — the compositor paths stay out).
    fn state_over_pty(master: &std::fs::File) -> PanelState {
        let startup = Startup::new(
            master.as_raw_fd(),
            Layout::new(Side::Left, cols(), 0, 0, 0, 0),
            Keyboard::OnDemand,
            None,
        );
        let path = glue::byte_path(Poisoned::new(), startup.fd());
        let poisoned = Poisoned::new();
        let draw_offset = Rc::new(Cell::new(0.0));
        let focused = Rc::new(Cell::new(false));
        let links = glue::seat_links(
            path.terminal(),
            path.repaint(),
            &draw_offset,
            &focused,
            poisoned.clone(),
        );
        let mut state = PanelState::new(
            Handshake::new(mpsc::channel().0),
            poisoned.clone(),
            live_inner(),
            startup,
            cell(),
            Wiring {
                repaint: Rc::clone(path.repaint()),
                stale_grid_px: Rc::clone(path.stale_grid_px()),
                draw_offset,
                seat_links: links,
                render: TweenRender {
                    terminal: Rc::clone(path.terminal()),
                    renderer: glue::test_renderer(),
                },
            },
        );
        // Record the configure height the re-push re-derives its grid
        // from; the sink records without touching the pty fd.
        state.sizing.configure(1080, &mut |_| {});
        state
    }

    /// The winsize push reports the device cell (device-pixel-cell-reports
    /// D1/D4): at scale 1 the logical pixels, at 1.8 the rounded device
    /// pixels — the same 16-pixel cell the size report answers (9 × 1.8),
    /// with the text area the columns times that cell.
    #[test]
    fn the_winsize_push_reports_the_device_cell_at_the_resolved_scale() {
        let master = std::fs::File::open("/dev/ptmx").expect("open /dev/ptmx");
        block_sigwinch();
        let grid = Sizing::started(cols(), cell(), 1080)
            .current_grid()
            .expect("a started sizing has a live grid");
        assert_eq!((grid.cols(), grid.rows()), (40, 67));

        apply_pty_size(master.as_raw_fd(), grid, scale_1());
        let ws = read_winsize(master.as_raw_fd());
        assert_eq!((ws.ws_col, ws.ws_row), (40, 67));
        assert_eq!(
            (ws.ws_xpixel, ws.ws_ypixel),
            (360, 1072),
            "scale 1 reports the logical pixels"
        );

        apply_pty_size(master.as_raw_fd(), grid, scale_1_8());
        let ws = read_winsize(master.as_raw_fd());
        assert_eq!((ws.ws_col, ws.ws_row), (40, 67));
        assert_eq!(
            (ws.ws_xpixel, ws.ws_ypixel),
            (640, 1943),
            "scale 1.8 reports 40 columns times the 16-pixel device cell"
        );
    }

    /// A scale change re-pushes the winsize only when the device pixel size
    /// changed (device-pixel-cell-reports D4): the re-push carries the new
    /// device pixels while the terminal keeps its logical grid, a repeated
    /// scale is a no-op, and so is a units move that rounds to the same
    /// device cell.
    #[test]
    fn a_scale_change_repushes_the_winsize_only_when_device_pixels_change() {
        let master = std::fs::File::open("/dev/ptmx").expect("open /dev/ptmx");
        block_sigwinch();
        let mut state = state_over_pty(&master);

        // The scale change re-pushes at the new device size.
        state.repush_for_scale(scale_1(), scale_1_8());
        let ws = read_winsize(master.as_raw_fd());
        assert_eq!((ws.ws_col, ws.ws_row), (40, 67), "the grid stays");
        assert_eq!(
            (ws.ws_xpixel, ws.ws_ypixel),
            (640, 1943),
            "the re-push carries the new device pixels"
        );
        let terminal = &state.render.terminal;
        let term = terminal.borrow();
        assert_eq!((term.cols(), term.rows()), (40, 67));
        assert_eq!(
            (term.cell_w(), term.cell_h()),
            (9, 16),
            "the terminal keeps its logical cell"
        );
        drop(term);
        assert!(state.repaint.get(), "the re-push latched a repaint");

        // A repeated scale pushes nothing.
        state.repaint.set(false);
        state.repush_for_scale(scale_1_8(), scale_1_8());
        assert!(!state.repaint.get(), "the same scale re-pushes nothing");

        // A units move that rounds to the same device cell pushes nothing:
        // 121/120 rounds the 9-pixel cell to 9 and the 16-pixel cell to 16.
        state.repush_for_scale(scale_1(), FractionalScale::from_120ths(121));
        assert!(
            !state.repaint.get(),
            "an unchanged device cell re-pushes nothing"
        );

        // The way back re-pushes too: the device pixels follow the scale
        // down as well as up.
        state.repush_for_scale(scale_1_8(), scale_1());
        let ws = read_winsize(master.as_raw_fd());
        assert_eq!(
            (ws.ws_xpixel, ws.ws_ypixel),
            (360, 1072),
            "the re-push restored the scale-1 pixels"
        );
        assert!(state.repaint.get(), "the way back latched a repaint");
    }
}
