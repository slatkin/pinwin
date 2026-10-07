//! The text pass's draw rules, display-free (replace-gtk-with-wayland
//! D10): colour, the wide cells, the layer order, the cursor's glyph
//! redraw and the tween's draw offset. The placement scenarios live in
//! `tests.rs`.

use crate::render::canvas::Canvas;
use crate::render::font::FontBook;
use crate::render::sprite;
use crate::render::text_pass::CursorText;
use crate::render::text_pass::test_support::{
    HIDE_CURSOR, Rig, block, bytes, cell_at, cells, contains_pixel, default_color, px, theme_bytes,
    theme_fg_bytes, utf8,
};
use crate::render::text_pass::text_owns;
use crate::term::cells::Wide;

/// A rig whose fallback resolves the red circle, or `None` (printed) when
/// no installed font covers it.
fn emoji_rig() -> Option<Rig> {
    let mut book = FontBook::new().expect("the font book opens");
    if !book.has_family(crate::render::text_pass::test_support::FAMILY) {
        println!("skipped: the emoji fallback family is not installed");
        return None;
    }
    if book
        .fallback_face('\u{1F534}')
        .expect("the fallback lookup runs")
        .is_none()
    {
        println!("skipped: no installed font covers U+1F534");
        return None;
    }
    let config = crate::fontconfig::FontConfig {
        family: Some(crate::render::text_pass::test_support::FAMILY.to_owned()),
        size: crate::render::text_pass::test_support::SIZE,
    };
    let faces = book.family_faces(&config).expect("the family's faces load");
    let mut rig = Rig::new()?;
    rig.test = crate::render::text_pass::test_support::from_faces(&faces, book);
    Some(rig)
}

// ---- Colour ----

/// A bold cell and an italic cell draw different pixels than the regular
/// face — the real faces where the family has them, the synthesized ones
/// where it does not.
#[test]
fn a_bold_and_an_italic_cell_draw_unlike_the_regular() {
    let Some(rig) = Rig::new() else { return };
    let paint_style = |sgr: &[u8]| -> Vec<[u8; 4]> {
        let mut terminal = rig.terminal(rig.cell_h);
        terminal.push_pty_data(HIDE_CURSOR);
        let mut data = sgr.to_vec();
        data.extend(b"M\x1b[0m");
        terminal.push_pty_data(&data);
        let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
        let cell = rig.metrics(1.0, rig.cell_h).cell_rect(0, 0);
        block(&canvas, cell, cell.w(), cell.h())
    };
    let regular = paint_style(b"");
    let bold = paint_style(b"\x1b[1m");
    let italic = paint_style(b"\x1b[3m");
    assert!(
        regular.iter().any(|pixel| *pixel != theme_bytes()),
        "the regular cell draws"
    );
    assert_ne!(bold, regular, "the bold cell draws unlike the regular");
    assert_ne!(italic, regular, "the italic cell draws unlike the regular");
}

/// A cell's glyph takes the cell's foreground colour, and a cell without
/// one takes the theme foreground: a full-coverage pixel of the stem reads
/// the exact cached colour.
#[test]
fn a_glyph_takes_the_cell_or_theme_foreground() {
    let Some(rig) = Rig::new() else { return };

    // An explicit SGR red foreground.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"\x1b[31ml\x1b[0m");
    let walked = cells(&mut terminal);
    let cell_with_fg = walked.iter().find(|cell| cell.len > 0).expect("the cell");
    assert!(cell_with_fg.has_fg, "the cell carries the SGR colour");
    let red = bytes([cell_with_fg.fg.r, cell_with_fg.fg.g, cell_with_fg.fg.b]);
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    assert!(
        contains_pixel(&canvas, m.cell_rect(0, 0), red),
        "some stem pixel reads the cell's exact red"
    );

    // No colour named: the theme foreground.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"l");
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    assert!(
        contains_pixel(&canvas, m.cell_rect(0, 0), theme_fg_bytes()),
        "some stem pixel reads the theme foreground"
    );
}

/// An emoji cell draws a colour glyph, not a tinted mask: the red circle
/// U+1F534's dominant channel in the middle of the glyph is red — in the
/// canvas' blue, green, red memory order (design D5), which pins the
/// channel order the glyph path produces.
#[test]
fn an_emoji_draws_its_colour_channels() {
    let Some(rig) = emoji_rig() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(&utf8(0x1F534));
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let cell = rig.metrics(1.0, rig.cell_h).cell_rect(0, 0);
    // The reddest pixel of the cell: the largest red channel against the
    // other two. A wrong channel order (red in the blue or green slot)
    // could not produce a dominant red channel at index 2.
    let reddest = (cell.y()..cell.y() + cell.h())
        .flat_map(move |y| (cell.x()..cell.x() + cell.w()).map(move |x| (x, y)))
        .map(|(x, y)| canvas.pixel(px(x), px(y)).expect("inside"))
        .max_by_key(|pixel| i32::from(pixel[2]) - i32::from(pixel[0].max(pixel[1])))
        .expect("the cell is not empty");
    assert!(
        reddest[2] > 100,
        "the red channel dominates: {reddest:?} (canvas order blue, green, red)"
    );
    assert!(
        reddest[2] > reddest[0] && reddest[2] > reddest[1],
        "red beats the other channels: {reddest:?}"
    );
}

// ---- Wide cells ----

/// A wide cell draws its head and not its tail: the CJK head cell carries
/// the glyph's ink, and the tail — which the walk emits first — draws
/// nothing as itself.
#[test]
fn a_wide_cell_draws_its_head_and_not_its_tail() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data("\u{6f22}".as_bytes());
    let walked = cells(&mut terminal);
    let head = walked
        .iter()
        .find(|cell| cell.wide == Wide::WIDE)
        .expect("the wide head");
    let tail = walked
        .iter()
        .find(|cell| cell.wide == Wide::SPACER_TAIL)
        .expect("the spacer tail");

    // The walk emits the tail before its head (the one-cell lookahead).
    let tail_index = walked
        .iter()
        .position(|cell| cell.wide == Wide::SPACER_TAIL)
        .expect("the tail is walked");
    let head_index = walked
        .iter()
        .position(|cell| cell.wide == Wide::WIDE)
        .expect("the head is walked");
    assert!(tail_index < head_index, "the tail is emitted first");

    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);

    // The head carries the glyph's ink. The glyph colour here is the theme
    // foreground (no SGR colour named), so the blend fraction reads off
    // any channel: a half-covered or better pixel — CJK strokes are thin
    // enough that no pixel may read exactly full coverage.
    let head_rect = m.cell_rect(head.x, head.y);
    let half = i32::from(theme_bytes()[0]).midpoint(i32::from(theme_fg_bytes()[0]));
    let strongest = |rect: crate::render::geom::DeviceRect| {
        (rect.y()..rect.y() + rect.h())
            .flat_map(move |y| (rect.x()..rect.x() + rect.w()).map(move |x| (x, y)))
            .filter_map(|(x, y)| canvas.pixel(px(x), px(y)))
            .map(|pixel| i32::from(pixel[0]))
            .max()
            .expect("the cell is inside the canvas")
    };
    assert!(
        strongest(head_rect) >= half,
        "the head cell carries the glyph's ink"
    );

    // The tail draws nothing as itself: the pure routing on the walked
    // tail cell declines it. Its area may still carry the head glyph's
    // ink — a CJK glyph's advance spans into the tail and text is not
    // clipped to its cell, on the GTK path or here — so no pixel
    // assertion can separate the two draws.
    let frame = rig.frame(1.0, rig.cell_h, 0.0);
    assert!(
        !text_owns(&m, &frame, tail),
        "the tail is not the text pass's cell"
    );
}

// ---- Layer order ----

/// The bands draw after the text: a letter's underline band reads the
/// exact band colour across its whole width — over the glyph's ink too —
/// where the same letter without the flag leaves glyph pixels in the
/// band's rows.
#[test]
fn the_underline_band_draws_over_the_glyph() {
    let Some(rig) = Rig::new() else { return };
    // The band rows of the rule, on the font's pitch at scale 1: the
    // underline hangs from the ascent plus one logical pixel.
    let m = rig.metrics(1.0, rig.cell_h);
    let band_top = crate::render::geom::device_px((rig.ascent + 1.0) * 1.0);

    // Without the flag: the glyph really has ink in the band's rows.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"g");
    let plain = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let cell = m.cell_rect(0, 0);
    assert!(
        (cell.x()..cell.x() + cell.w())
            .any(|x| plain.pixel(px(x), px(band_top)) != Some(theme_bytes())),
        "the glyph has ink at the band's row, so the overlap is real"
    );

    // With the flag: the band covers the row exactly, glyph or no glyph.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"\x1b[4mg\x1b[0m");
    let underlined = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    for x in cell.x()..cell.x() + cell.w() {
        assert_eq!(
            underlined.pixel(px(x), px(band_top)),
            Some(theme_fg_bytes()),
            "the band reads the exact band colour at x {x}"
        );
    }
}

/// A sprite cell is drawn by the sprite pass only: the text pass skips the
/// cells [`sprite::cell_sprite`] owns, so a one-eighth block with an
/// explicit background shows the sprite's exact rectangles and nothing
/// else — no font glyph blended on top.
#[test]
fn a_sprite_cell_draws_no_text_glyph_on_top() {
    let Some(rig) = Rig::new() else { return };
    let m = rig.metrics(1.0, rig.cell_h);
    let frame = rig.frame(1.0, rig.cell_h, 0.0);

    // The routing, pure: exactly the cells the sprite pass declines.
    for cp in [0x2588, 0x2581, 0x2801, 0xE0B0] {
        assert!(
            !text_owns(&m, &frame, &cell_at(0, cp)),
            "0x{cp:04X} is the sprite pass's cell"
        );
    }
    for cp in [u32::from(b'A'), 0x2591, 0x1F534] {
        assert!(
            text_owns(&m, &frame, &cell_at(0, cp)),
            "0x{cp:04X} is the text pass's cell"
        );
    }

    // The pixels: the bottom-eighth block's cell reads the sprite's
    // rectangle and the explicit background, nothing between.
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(HIDE_CURSOR);
    terminal.push_pty_data(b"\x1b[31;44m\xE2\x96\x81\x1b[0m");
    let walked = cells(&mut terminal);
    let block_cell = walked
        .iter()
        .find(|cell| cell.len > 0)
        .expect("the block cell");
    let fg = bytes([block_cell.fg.r, block_cell.fg.g, block_cell.fg.b]);
    let bg = bytes([block_cell.bg.r, block_cell.bg.g, block_cell.bg.b]);
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let cell = m.cell_rect(block_cell.x, block_cell.y);
    let Some((_, sprite::Primitive::Rects(sprite_rects))) =
        sprite::cell_sprite(&m, &frame, block_cell)
    else {
        panic!("the one-eighth block is a rect sprite");
    };
    for y in cell.y()..cell.y() + cell.h() {
        for x in cell.x()..cell.x() + cell.w() {
            let covered = sprite_rects
                .iter()
                .any(|r| x >= r.x() && x < r.x() + r.w() && y >= r.y() && y < r.y() + r.h());
            assert_eq!(
                canvas.pixel(px(x), px(y)),
                Some(if covered { fg } else { bg }),
                "the sprite cell at ({x}, {y}) is sprite or background, never text"
            );
        }
    }
}

// ---- The cursor's glyph redraw ----

/// A block cursor redraws the glyph under it in the terminal's default
/// background: a pixel inside a glyph stem that read the theme foreground
/// without the cursor reads the default background under the block.
#[test]
fn a_block_cursor_redraws_the_glyph_in_the_default_background() {
    let Some(rig) = Rig::new() else { return };
    // The same letter, once with the cursor hidden, once under a block
    // cursor parked on the cell.
    let mut hidden = rig.terminal(rig.cell_h);
    hidden.push_pty_data(b"\x1b[?25lX");
    let mut blocked = rig.terminal(rig.cell_h);
    blocked.push_pty_data(b"X\x1b[1;1H");
    let visible = {
        assert!(blocked.frame_begin(), "frame began");
        let visible = blocked.cursor().is_some();
        blocked.frame_end();
        visible
    };
    assert!(visible, "the block cursor is visible");
    let default_background = default_color(&mut blocked, true);
    let default_foreground = default_color(&mut blocked, false);

    let hidden_canvas = rig.painted(&mut hidden, 1.0, rig.cell_h, 0.0);
    let blocked_canvas = rig.painted(&mut blocked, 1.0, rig.cell_h, 0.0);
    let cell = rig.metrics(1.0, rig.cell_h).cell_rect(0, 0);
    // A full-coverage stem pixel of the hidden draw: exactly the theme
    // foreground (no SGR colour named). Under the block cursor the same
    // pixel reads exactly the default background — the redraw drew the
    // glyph in the background colour on top of the block fill.
    let stem = (cell.y()..cell.y() + cell.h())
        .flat_map(move |y| (cell.x()..cell.x() + cell.w()).map(move |x| (x, y)))
        .find(|(x, y)| hidden_canvas.pixel(px(*x), px(*y)) == Some(theme_fg_bytes()))
        .expect("the glyph has a full-coverage pixel");
    assert_eq!(
        blocked_canvas.pixel(px(stem.0), px(stem.1)),
        Some(default_background),
        "the stem pixel reads the default background under the block"
    );
    // The block fill shows where the glyph has no ink.
    let bare = (cell.y()..cell.y() + cell.h())
        .flat_map(move |y| (cell.x()..cell.x() + cell.w()).map(move |x| (x, y)))
        .find(|(x, y)| hidden_canvas.pixel(px(*x), px(*y)) == Some(theme_bytes()))
        .expect("the glyph does not fill the cell");
    assert_eq!(
        blocked_canvas.pixel(px(bare.0), px(bare.1)),
        Some(default_foreground),
        "the bare pixel reads the block's fill"
    );
}

/// A bar cursor does not redraw the glyph: the letter's ink away from the
/// bar's columns keeps the theme foreground.
#[test]
fn a_bar_cursor_does_not_redraw_the_glyph() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(b"X\x1b[6 q\x1b[1;1H");
    let walked_cursor = {
        assert!(terminal.frame_begin(), "frame began");
        let walked = terminal.cursor().expect("a visible cursor");
        terminal.frame_end();
        walked
    };
    assert_eq!(
        walked_cursor.style,
        crate::term::cells::CursorStyle::Bar,
        "the bar style is set"
    );
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let cell = rig.metrics(1.0, rig.cell_h).cell_rect(0, 0);
    // The bar covers the first device columns; past them the glyph's
    // full-coverage pixels keep the theme foreground — no redraw.
    let kept = (cell.x() + 3..cell.x() + cell.w())
        .flat_map(move |x| (cell.y()..cell.y() + cell.h()).map(move |y| (x, y)))
        .find(|(x, y)| canvas.pixel(px(*x), px(*y)) == Some(theme_fg_bytes()))
        .expect("the glyph keeps ink right of the bar");
    assert!(
        kept.0 >= cell.x() + 3,
        "the ink at ({}, {}) was not redrawn",
        kept.0,
        kept.1
    );
}

/// A wide glyph's tail cell does not redraw: a block cursor parked on the
/// tail fills it with the default foreground and nothing else — the head's
/// glyph is not drawn over the tail's block.
#[test]
fn a_wide_tail_does_not_redraw_the_glyph() {
    let Some(rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data("\u{6f22}\x1b[1;2H".as_bytes());
    let walked_cursor = {
        assert!(terminal.frame_begin(), "frame began");
        let walked = terminal.cursor().expect("a visible cursor");
        terminal.frame_end();
        walked
    };
    assert!(walked_cursor.x == 1, "the cursor sits on the tail cell");
    assert!(walked_cursor.wide_tail, "the vt reports the tail");
    let default_fg = default_color(&mut terminal, false);
    let canvas = rig.painted(&mut terminal, 1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    let tail = m.cell_rect(1, 0);
    for y in tail.y()..tail.y() + tail.h() {
        for x in tail.x()..tail.x() + tail.w() {
            assert_eq!(
                canvas.pixel(px(x), px(y)),
                Some(default_fg),
                "the tail's block keeps its fill at ({x}, {y})"
            );
        }
    }
}

/// The redraw collects the cursor cell's text through the same rule the
/// GTK walk used: the first cell with a glyph at the cursor's position —
/// the walk's return value carries it for the cursor layer to redraw.
#[test]
fn the_walk_collects_the_cursor_cells_text() {
    let Some(mut rig) = Rig::new() else { return };
    let mut terminal = rig.terminal(rig.cell_h);
    terminal.push_pty_data(b"XY\x1b[1;2H");
    assert!(terminal.frame_begin(), "frame began");
    let frame = rig.frame(1.0, rig.cell_h, 0.0);
    let m = rig.metrics(1.0, rig.cell_h);
    let (w, h) = frame.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    let collected = rig
        .test
        .pass
        .paint(&mut canvas, &m, &frame, &mut terminal, 0);
    terminal.frame_end();
    assert_eq!(collected.as_bytes(), b"Y", "the cell at the cursor");
}

/// An empty collection — no cell with a glyph at the cursor's position —
/// leaves [`CursorText::empty`], which the redraw skips.
#[test]
fn an_empty_cursor_text_carries_nothing() {
    let empty = CursorText::empty();
    assert_eq!(empty.as_bytes(), b"", "an empty collection carries nothing");
}

// ---- The tween's draw offset ----

/// A tween draw offset translates the text like the other grid layers: the
/// offset in device pixels along x moves the glyph by exactly that many
/// pixels.
#[test]
fn the_draw_offset_translates_the_text() {
    let Some(rig) = Rig::new() else { return };
    let offset = 5.0;
    let scale = 1.0;
    let mut plain = rig.terminal(rig.cell_h);
    plain.push_pty_data(HIDE_CURSOR);
    plain.push_pty_data(b"MM");
    let mut shifted = rig.terminal(rig.cell_h);
    shifted.push_pty_data(HIDE_CURSOR);
    shifted.push_pty_data(b"MM");
    let base = rig.painted(&mut plain, scale, rig.cell_h, 0.0);
    let moved = rig.painted(&mut shifted, scale, rig.cell_h, offset);
    let m = rig.metrics(scale, rig.cell_h);
    let cell = m.cell_rect(0, 0);
    let shift = crate::render::geom::device_px(offset * scale);
    for y in cell.y()..cell.y() + cell.h() {
        for x in cell.x()..cell.x() + cell.w() {
            assert_eq!(
                moved.pixel(px(x + shift), px(y)),
                base.pixel(px(x), px(y)),
                "the text moved by the offset at ({x}, {y})"
            );
        }
    }
}
