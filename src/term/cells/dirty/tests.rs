//! The dirty capture's tests: what the pinned render state
//! reports after real pty traffic, and the row filter's effect on the
//! walk. GTK-free (`replace-gtk-with-wayland` D10).

use crate::guard::Poisoned;
use crate::term::cells::mbv_replay_bytes;
use crate::term::{PngDecoder, PtySink, Terminal};

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

/// An 8-column, 4-row terminal at the 8x16 cell pitch the render tests use.
fn terminal() -> Terminal {
    let mut terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
    assert!(terminal.push_size(8, 4, 8, 16));
    terminal
}

/// An 8-column, 4-row terminal at the 8x16 cell pitch the render tests use,
/// with its first frame drawn and consumed: after that, the dirty capture
/// reports only what the pty traffic since changed.
fn drawn_terminal() -> Terminal {
    let mut terminal = terminal();
    assert!(terminal.frame_begin());
    terminal.frame_end();
    terminal
}

/// Walk a frame's whole cell list, leaving the frame open.
fn walked(terminal: &mut Terminal) -> Vec<crate::term::cells::Cell> {
    let mut cells = Vec::new();
    while let Some(cell) = terminal.cell_next() {
        cells.push(cell);
    }
    cells
}

/// The first frame after creation is a full redraw with every row dirty,
/// and a frame drawn over an unchanged terminal is clean: nothing to
/// redraw, nothing to commit.
#[test]
fn the_first_frame_is_full_and_an_unchanged_frame_is_clean() {
    let mut terminal = terminal();

    assert!(terminal.frame_begin());
    assert_eq!(terminal.frame_dirty(), crate::term::cells::FrameDirty::Full);
    assert_eq!(terminal.frame_dirty_rows(), &[0, 1, 2, 3]);
    terminal.frame_end();

    assert!(terminal.frame_begin());
    assert_eq!(
        terminal.frame_dirty(),
        crate::term::cells::FrameDirty::Clean,
        "the consumed frame left nothing dirty"
    );
    assert_eq!(terminal.frame_dirty_rows().len(), 0);
    assert!(!terminal.frame_has_images());
    terminal.frame_end();
}

/// Typing marks the typed row dirty, and only it: the partial repaint's
/// input.
#[test]
fn typing_marks_the_typed_row_dirty() {
    let mut terminal = drawn_terminal();
    terminal.push_pty_data(b"hi");

    assert!(terminal.frame_begin());
    assert_eq!(
        terminal.frame_dirty(),
        crate::term::cells::FrameDirty::Partial
    );
    assert_eq!(terminal.frame_dirty_rows(), &[0]);
    terminal.frame_end();
}

/// A cursor move marks the rows of the old and the new cursor cell dirty —
/// the render state covers the cursor's own rows, though not its restyle.
#[test]
fn a_cursor_move_marks_both_cursor_rows_dirty() {
    let mut terminal = drawn_terminal();
    terminal.push_pty_data(b"\x1b[2;3H"); // row 2, column 3 (1-based)

    assert!(terminal.frame_begin());
    assert_eq!(
        terminal.frame_dirty(),
        crate::term::cells::FrameDirty::Partial
    );
    assert_eq!(terminal.frame_dirty_rows(), &[0, 1], "old and new row");
    terminal.frame_end();
}

/// A viewport scroll is a full redraw: every row is reported dirty.
#[test]
fn a_scroll_reports_a_full_frame() {
    let mut terminal = drawn_terminal();
    terminal.push_pty_data(b"hi\n\n\n\x1b[1S");

    assert!(terminal.frame_begin());
    assert_eq!(terminal.frame_dirty(), crate::term::cells::FrameDirty::Full);
    assert_eq!(terminal.frame_dirty_rows(), &[0, 1, 2, 3]);
    terminal.frame_end();

    // Consumed: the next frame is clean again.
    assert!(terminal.frame_begin());
    assert_eq!(
        terminal.frame_dirty(),
        crate::term::cells::FrameDirty::Clean
    );
    terminal.frame_end();
}

/// A terminal that carries kitty placements reports them, so the frame
/// gate can escalate; a terminal without any does not.
#[test]
fn frame_has_images_follows_the_placements() {
    let mut terminal = terminal();
    assert!(terminal.frame_begin());
    assert!(!terminal.frame_has_images(), "no placements yet");
    terminal.frame_end();

    terminal.push_pty_data(&mbv_replay_bytes());
    assert!(terminal.frame_begin());
    assert!(terminal.frame_has_images(), "the replay places one image");
    terminal.frame_end();
}

/// The dirty readers answer `Clean` and empty when no frame is open.
#[test]
fn the_dirty_readers_answer_clean_without_an_open_frame() {
    let terminal = Terminal::new(Poisoned::new(), NullSink, NoDecoder, || {});
    assert_eq!(
        terminal.frame_dirty(),
        crate::term::cells::FrameDirty::Clean
    );
    assert_eq!(terminal.frame_dirty_rows().len(), 0);
    assert!(!terminal.frame_has_images());
}

/// The partial repaint's row filter clips the frame walk to the rows it
/// names, for the open frame and every rewind of it; the next
/// `frame_begin` clears it.
#[test]
fn the_walk_filter_clips_the_walk_to_its_rows() {
    let mut terminal = drawn_terminal();
    terminal.push_pty_data(b"hi");

    assert!(terminal.frame_begin());
    terminal.frame_walk_rows(&[1]);
    assert!(
        walked(&mut terminal).iter().all(|cell| cell.y == 1),
        "only row 1's cells reach the walk"
    );
    // A rewind re-opens the same filtered walk: every pass of the frame
    // sees the same rows.
    terminal.frame_rewind();
    assert!(
        walked(&mut terminal).iter().all(|cell| cell.y == 1),
        "the rewind keeps the filter"
    );
    terminal.frame_end();

    // The next frame is unfiltered again: the walk covers every row. (The
    // cell walk's pending flush can repeat one cell at the end, so the
    // assertion is set membership, not an exact list.)
    assert!(terminal.frame_begin());
    let ys: Vec<i32> = walked(&mut terminal).iter().map(|cell| cell.y).collect();
    for row in 0..4 {
        assert!(ys.contains(&row), "the unfiltered walk reaches row {row}");
    }
    terminal.frame_end();
}
