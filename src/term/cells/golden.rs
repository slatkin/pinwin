//! Golden output of the cell, cursor, colour and image passes for one fixed
//! VT stream (`adopt-libghostty-rs` design A7). The fixture was generated
//! from the hand-written FFI; a port must reproduce it byte for byte.
//! Regenerate on purpose with `UPDATE_GOLDEN=1 cargo test golden`.

use std::fmt::Write as _;

use super::Image;
use crate::guard::Poisoned;
use crate::term::{DecodedPng, PngDecoder, PtySink, Terminal};

const FIXTURE: &str = "src/term/cells/golden.txt";

struct NullSink;
impl PtySink for NullSink {
    fn write_pty(&mut self, _data: &[u8]) {}
}

/// Answers every PNG with the same 2x2 RGBA image.
struct FixedPng;
impl PngDecoder for FixedPng {
    fn decode_png(&mut self, _data: &[u8]) -> Option<DecodedPng> {
        Some(DecodedPng {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
            ],
        })
    }
}

/// Twelve lines scroll a 6-row screen, then cursor moves overwrite it with
/// wide, styled and coloured text and one kitty PNG placement.
fn stream() -> Vec<u8> {
    let mut s = String::new();
    for n in 1..=12 {
        let _ = write!(s, "line {n}\r\n");
    }
    s.push_str("\x1b[1;1H\x1b[0mplain text");
    s.push_str("\x1b[2;1H\x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[7minverse\x1b[0m \x1b[4munder\x1b[0m");
    s.push_str("\x1b[3;1H\u{65e5}\u{672c}\u{8a9e} a\u{1f600}b");
    s.push_str("\x1b[4;1H\x1b[38;2;10;200;30mtrue\x1b[48;2;200;10;30m color\x1b[0m ");
    s.push_str("\x1b[31;44mpal\x1b[38;5;208m256\x1b[0m");
    s.push_str("\x1b[5;12H\x1b_Ga=T,f=100,i=7,q=2;AAAA\x1b\\");
    s.push_str("\x1b[6;7H");
    s.into_bytes()
}

fn image_text(out: &mut String, img: &Image) {
    // SAFETY: `pixels` points at `image_w * image_h * 4` bytes that live
    // until `frame_end`.
    let px = unsafe {
        std::slice::from_raw_parts(
            img.pixels,
            usize::try_from(img.image_w * img.image_h * 4).expect("pixel count"),
        )
    };
    let _ = writeln!(
        out,
        "image id={} gen={} z={} dst={},{} {}x{} src={},{} {}x{} full={}x{} px={px:?}",
        img.image_id,
        img.generation,
        img.z,
        img.x,
        img.y,
        img.w,
        img.h,
        img.sx,
        img.sy,
        img.sw,
        img.sh,
        img.image_w,
        img.image_h
    );
}

fn render() -> String {
    let mut t = Terminal::new(Poisoned::new(), NullSink, FixedPng, || {});
    assert!(t.push_size(24, 6, 8, 16));
    t.push_pty_data(&stream());
    assert!(t.frame_begin());
    let mut out = String::new();
    while let Some(c) = t.cell_next() {
        let _ = writeln!(
            out,
            "cell {},{} wide={:?} cw={} text={:?} fg={}{:?} bg={}{:?} flags={:#x}",
            c.x,
            c.y,
            c.wide,
            c.cw,
            c.text_str(),
            u8::from(c.has_fg),
            c.fg,
            u8::from(c.has_bg),
            c.bg,
            c.flags.bits()
        );
    }
    let _ = writeln!(out, "cursor {:?}", t.cursor());
    let _ = writeln!(out, "colors {:?}", t.colors());
    while let Some(img) = t.image_next() {
        image_text(&mut out, &img);
    }
    t.frame_end();
    out
}

#[test]
fn cells_cursor_colors_and_images_match_the_golden_fixture() {
    let actual = render();
    let path = format!("{}/{FIXTURE}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &actual).expect("write fixture");
    }
    let expected = std::fs::read_to_string(&path).expect("read fixture");
    assert!(actual == expected, "golden output changed; see {FIXTURE}");
}
