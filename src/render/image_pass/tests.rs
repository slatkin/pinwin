//! The image pass's tests (row 4.7, `replace-gtk-with-wayland` D10): the
//! "Images are not clipped" pixel tests through the terminal's real kitty
//! path, the crop, alpha and channel-order rules, the cache contract and
//! the layer order, all display-free.

use super::ImagePass;
use crate::render::canvas::{Canvas, CanvasColor};
use crate::render::geom::{DeviceRect, PainterMetrics, device_px};
use crate::render::png::PngCrateDecoder;
use crate::render::text_pass::test_support;
use crate::term::Terminal;
use crate::term::cells::Image;

/// The image id the captured mbv 0.22.5 bytes carry; the raw-image helper
/// reuses it so the placeholder cell's diacritics (copied from the mbv
/// replay) resolve it.
const IMAGE_ID: u32 = 536_687_111;

/// The test theme, the same one the other render tests pin.
const THEME: crate::fontconfig::ThemeColours = crate::fontconfig::ThemeColours {
    background: [10, 20, 30],
    foreground: [200, 150, 100],
};

/// The theme background's canvas bytes: blue, green, red, alpha.
fn theme_bytes() -> [u8; 4] {
    [30, 20, 10, 255]
}

/// A sink with nowhere to write (the tests never write to a pty).
struct NullSink;
impl crate::term::PtySink for NullSink {
    fn write_pty(&mut self, _data: &[u8]) {}
}

/// Base64-encode `data` (the kitty transmission carries the raw pixels
/// base64-encoded).
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[usize::try_from((n >> 18) & 63).expect("6 bits")].into());
        out.push(TABLE[usize::try_from((n >> 12) & 63).expect("6 bits")].into());
        out.push(
            if chunk.len() > 1 {
                TABLE[usize::try_from((n >> 6) & 63).expect("6 bits")]
            } else {
                b'='
            }
            .into(),
        );
        out.push(
            if chunk.len() > 2 {
                TABLE[usize::try_from(n & 63).expect("6 bits")]
            } else {
                b'='
            }
            .into(),
        );
    }
    out
}

/// The pty bytes that transmit one raw RGBA image (the mbv transmission's
/// form: `a=T,U=1,f=32,t=d`, one chunk) and print its unicode-placeholder
/// cell at `(col, row)` — the cell whose foreground colour carries the
/// image id's low 24 bits, with the same placeholder run the captured mbv
/// stream prints.
fn raw_image_apc(w: u32, h: u32, pixels: &[u8], col: i32, row: i32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(
        format!(
            "\x1b_Gi={IMAGE_ID},a=T,U=1,f=32,t=d,s={w},v={h},q=2;{}\x1b\\",
            base64(pixels)
        )
        .as_bytes(),
    );
    bytes.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());
    bytes.extend_from_slice(b"\x1b[38;2;253;50;7m");
    bytes.extend_from_slice("\u{10EEEE}\u{305}\u{305}\u{484}".as_bytes());
    bytes
}

/// An 8x16 image: a uniform border of `border` around an interior of
/// `interior` — the pattern whose edge pixels prove the whole image is
/// visible (dropping the last source column or row would pull the
/// interior into the edge pixels).
fn bordered(border: [u8; 3], interior: [u8; 3]) -> Vec<u8> {
    let (w, h) = (8u32, 16u32);
    let mut pixels = Vec::new();
    for row in 0..h {
        for col in 0..w {
            let edge = col == 0 || col == w - 1 || row == 0 || row == h - 1;
            let rgb = if edge { border } else { interior };
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    pixels
}

/// A terminal at the 8-column, 4-row grid with the 8x16 cell pitch.
fn terminal() -> Terminal {
    let mut terminal = Terminal::new(
        crate::guard::Poisoned::new(),
        NullSink,
        PngCrateDecoder,
        || {},
    );
    assert!(terminal.push_size(8, 4, 8, 16));
    terminal
}

/// Metrics for the 8x16 pitch at `scale`, over a 64x64 logical frame.
fn metrics(scale: f64) -> PainterMetrics {
    PainterMetrics::new(8.0, 16.0, 12.0, scale).expect("test metrics are valid")
}

/// One frame cycle: begin the frame, walk its cells (the virtual
/// placements need the placeholder origins the walk records), paint the
/// image pass into `canvas`, end the frame.
fn frame_pass(terminal: &mut Terminal, canvas: &mut Canvas, images: &mut ImagePass, scale: f64) {
    assert!(terminal.frame_begin(), "frame began");
    while terminal.cell_next().is_some() {}
    images.paint(canvas, &metrics(scale), terminal, 0);
    terminal.frame_end();
}

/// A 64x64-logical canvas filled with the theme background.
fn themed_canvas(scale: f64) -> Canvas {
    let device = u32::try_from(device_px(f64::from(64) * scale)).expect("frame fits u32");
    let mut canvas = Canvas::new(device, device).expect("canvas size is valid");
    canvas.fill_rect(
        0,
        0,
        i32::try_from(device).expect("fits"),
        i32::try_from(device).expect("fits"),
        CanvasColor::from_theme(crate::term::cells::Rgb {
            r: THEME.background[0],
            g: THEME.background[1],
            b: THEME.background[2],
        }),
    );
    canvas
}

/// The snapped device rectangle of the 8x16 placement at cell (1, 1).
fn placement_rect(scale: f64) -> DeviceRect {
    metrics(scale).logical_rect(8.0, 16.0, 8.0, 16.0)
}

/// A hand-built image of `w` x `h` straight RGBA pixels, source rect the
/// whole image at its own size.
fn hand_image(pixels: &[u8], w: i32, h: i32) -> Image {
    Image {
        image_id: 7,
        generation: 1,
        image_w: w,
        image_h: h,
        sx: 0,
        sy: 0,
        sw: w,
        sh: h,
        pixels: pixels.as_ptr(),
        ..Image::default()
    }
}

/// The whole image is visible at every scale: the destination's first and
/// last columns and rows hold the source's border colour, the centre holds
/// the interior, and nothing draws outside the destination rectangle.
#[test]
fn a_placement_shows_the_whole_image_at_every_scale() {
    for scale in [1.0, 1.5, 1.8] {
        let border = [250u8, 10, 10];
        let interior = [10u8, 10, 250];
        let mut terminal = terminal();
        terminal.push_pty_data(&raw_image_apc(8, 16, &bordered(border, interior), 1, 1));

        let mut canvas = themed_canvas(scale);
        let mut images = ImagePass::new();
        frame_pass(&mut terminal, &mut canvas, &mut images, scale);

        let rect = placement_rect(scale);
        let border_bytes = [border[2], border[1], border[0], 255];
        let interior_bytes = [interior[2], interior[1], interior[0], 255];
        let px = |x: i32, y: i32| -> [u8; 4] {
            canvas
                .pixel(
                    u32::try_from(x).expect("fits"),
                    u32::try_from(y).expect("fits"),
                )
                .expect("inside the canvas")
        };

        // The four edges read the source's border: the whole image is
        // there, no part cut off at the right or bottom.
        for x in rect.x()..rect.x() + rect.w() {
            assert_eq!(
                px(x, rect.y()),
                border_bytes,
                "top edge at {scale}, x = {x}"
            );
            assert_eq!(
                px(x, rect.y() + rect.h() - 1),
                border_bytes,
                "bottom edge at {scale}, x = {x}"
            );
        }
        for y in rect.y()..rect.y() + rect.h() {
            assert_eq!(
                px(rect.x(), y),
                border_bytes,
                "left edge at {scale}, y = {y}"
            );
            assert_eq!(
                px(rect.x() + rect.w() - 1, y),
                border_bytes,
                "right edge at {scale}, y = {y}"
            );
        }
        // The centre is the interior: the image scaled, not smeared.
        let cx = rect.x() + rect.w() / 2;
        let cy = rect.y() + rect.h() / 2;
        assert_eq!(px(cx, cy), interior_bytes, "centre at {scale}");

        // Nothing drew outside the destination rectangle.
        assert_eq!(
            px(rect.x() - 1, rect.y() - 1),
            theme_bytes(),
            "above-left at {scale}"
        );
        assert_eq!(
            px(rect.x() + rect.w(), rect.y() + rect.h()),
            theme_bytes(),
            "below-right at {scale}"
        );
        assert_eq!(
            px(rect.x() - 1, rect.y() + rect.h() / 2),
            theme_bytes(),
            "left of the image at {scale}"
        );
        assert_eq!(
            px(rect.x() + rect.w(), rect.y() + rect.h() / 2),
            theme_bytes(),
            "right of the image at {scale}"
        );
    }
}

/// A placement with a source crop shows exactly that crop: the hand-built
/// 4x4 image cropped to its bottom right 2x2 and drawn at a 4x4
/// destination reads the crop's pixels, scaled, and nothing else.
#[test]
fn a_placements_source_crop_shows_exactly_that_crop() {
    // 4x4, pixel (col, row) = channel value col * 10 + row.
    let mut pixels = Vec::new();
    for row in 0..4i32 {
        for col in 0..4i32 {
            let v = u8::try_from(col * 10 + row).expect("fits");
            pixels.extend_from_slice(&[v, 0, 0, 255]);
        }
    }
    let img = hand_image(&pixels, 4, 4);
    let img = Image {
        sx: 2,
        sy: 2,
        sw: 2,
        sh: 2,
        ..img
    };

    let mut canvas = themed_canvas(1.0);
    let mut images = ImagePass::new();
    let rect = DeviceRect::new(0, 0, 4, 4).expect("test rect is valid");
    images.draw_placement(&mut canvas, &rect, &img, 0);

    let px = |x: u32, y: u32| canvas.pixel(x, y).expect("inside the canvas");
    // The crop upscaled 2x: each source pixel fills a 2x2 block. Source
    // (2,2)=22, (3,2)=32, (2,3)=23, (3,3)=33 — red is the canvas' third
    // channel.
    assert_eq!(px(0, 0)[2], 22);
    assert_eq!(px(3, 0)[2], 32, "top right block");
    assert_eq!(px(0, 3)[2], 23, "bottom left block");
    assert_eq!(px(3, 3)[2], 33, "bottom right block");
    // Outside the destination: theme.
    assert_eq!(canvas.pixel(4, 0), Some(theme_bytes()), "right of the crop");
    assert_eq!(canvas.pixel(0, 4), Some(theme_bytes()), "below the crop");
}

/// A half-transparent image blends source-over over the theme background,
/// and a pure red source pixel reads red on the canvas — the canvas stores
/// blue, green, red, alpha (D5).
#[test]
fn a_half_transparent_image_blends_and_red_reads_red() {
    // 1x1: pure red at half alpha.
    let pixels = [255u8, 0, 0, 128];
    let img = hand_image(&pixels, 1, 1);

    let mut canvas = themed_canvas(1.0);
    let mut images = ImagePass::new();
    let rect = DeviceRect::new(0, 0, 1, 1).expect("test rect is valid");
    images.draw_placement(&mut canvas, &rect, &img, 0);

    // Premultiplied source: (128, 0, 0, 128) in the canvas' blue, green,
    // red order. Over the theme background (30, 20, 10) with keep = 127:
    // blue 0 + 15 = 15, green 0 + 10 = 10, red 128 + 5 = 133, alpha 255.
    assert_eq!(canvas.pixel(0, 0), Some([15, 10, 133, 255]));

    // The channel-order half: an opaque pure red pixel reads red. A new
    // image id, so the cache does not answer with the first image.
    let red = [255u8, 0, 0, 255];
    let img = hand_image(&red, 1, 1);
    let img = Image { image_id: 9, ..img };
    let mut canvas = themed_canvas(1.0);
    images.draw_placement(&mut canvas, &rect, &img, 0);
    assert_eq!(canvas.pixel(0, 0), Some([0, 0, 255, 255]), "red reads red");
}

/// A second frame with the same placement is all hits; a retransmitted
/// image (a new generation) misses once and drops its stale entries; an
/// image no placement drew this frame is evicted; a cache over its budget
/// clears while the handed-out pixmaps stay valid.
#[test]
fn the_cache_reuses_evicts_and_clears() {
    let border = [250u8, 10, 10];
    let interior = [10u8, 10, 250];
    let pixels = bordered(border, interior);
    let apc = raw_image_apc(8, 16, &pixels, 1, 1);

    let mut terminal = terminal();
    terminal.push_pty_data(&apc);
    let mut images = ImagePass::new();

    let mut canvas = themed_canvas(1.0);
    frame_pass(&mut terminal, &mut canvas, &mut images, 1.0);
    assert_eq!(images.misses(), 1, "the first frame resamples");
    assert_eq!(images.hits(), 0);

    frame_pass(&mut terminal, &mut canvas, &mut images, 1.0);
    assert_eq!(images.hits(), 1, "the same placement is a hit");
    assert_eq!(images.misses(), 1);

    // A retransmission bumps the generation: one miss, and the stale
    // entry went with it.
    terminal.push_pty_data(&apc);
    frame_pass(&mut terminal, &mut canvas, &mut images, 1.0);
    assert_eq!(images.misses(), 2, "the new generation resamples");
    assert_eq!(images.cached(), 1, "the stale entry is dropped");

    // An image whose placeholder cell is overwritten is not placed, so
    // the next frame evicts it.
    terminal.push_pty_data(b"\x1b[2;2H ");
    frame_pass(&mut terminal, &mut canvas, &mut images, 1.0);
    assert_eq!(images.cached(), 0, "an undrawn image is evicted");
}

/// A cache over its budget clears; the pixmaps handed out before the
/// clear stay valid through their `Arc`.
#[test]
fn the_byte_budget_clears_and_handed_out_pixmaps_stay_valid() {
    // One 4x4 entry costs 64 bytes; a budget of 64 holds exactly one.
    let mut images = ImagePass::with_limits(16, 64);
    let pixels = [
        255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 9, 9, 9, 255,
    ];
    let first = hand_image(&pixels, 2, 2);
    let mut canvas = themed_canvas(1.0);
    let rect_a = DeviceRect::new(0, 0, 4, 4).expect("test rect is valid");
    images.draw_placement(&mut canvas, &rect_a, &first, 0);
    assert_eq!(images.cached(), 1);
    let handed_out = images.pixmaps();
    assert_eq!(handed_out.len(), 1);

    // A second image id at another destination size does not fit: the
    // cache clears and the new entry starts alone.
    let other = [0u8, 0, 255, 255];
    let second = hand_image(&other, 1, 1);
    let second = Image {
        image_id: 8,
        ..second
    };
    let rect_b = DeviceRect::new(10, 0, 6, 6).expect("test rect is valid");
    images.draw_placement(&mut canvas, &rect_b, &second, 0);
    assert_eq!(images.clears(), 1, "the budget cleared");
    assert_eq!(images.cached(), 1, "the new entry starts alone");
    assert_eq!(images.misses(), 2);

    // The handed-out pixmap survived the clear: its bytes still read.
    let pixmap = &handed_out[0];
    assert_eq!((pixmap.width(), pixmap.height()), (4, 4));
    assert_eq!(pixmap.data()[2], 255, "the red channel byte survives");
}

/// A degenerate placement — a zero size, or null pixels — draws nothing
/// and does not panic.
#[test]
fn degenerate_placements_draw_nothing() {
    let mut canvas = themed_canvas(1.0);
    let mut images = ImagePass::new();

    let pixels = [1u8, 2, 3, 4];
    let zero_sized = hand_image(&pixels, 1, 1);
    let zero_sized = Image {
        w: 0,
        h: 4,
        ..zero_sized
    };
    let rect = DeviceRect::new(0, 0, 0, 4).expect("an empty rect is valid");
    images.draw_placement(&mut canvas, &rect, &zero_sized, 0);

    let null_pixels = Image {
        pixels: std::ptr::null(),
        ..hand_image(&pixels, 1, 1)
    };
    let rect = DeviceRect::new(0, 0, 4, 4).expect("test rect is valid");
    images.draw_placement(&mut canvas, &rect, &null_pixels, 0);

    // The refused placements left the theme fill in place.
    assert_eq!(canvas.pixel(0, 0), Some(theme_bytes()));
    assert_eq!(canvas.pixel(2, 2), Some(theme_bytes()));
}

/// The layer order through the whole painter: an image draws above the
/// cursor and below the focus accent. The image covers the cursor's cell,
/// so its interior reads the image's colour, not the cursor's; the
/// focused frame's accent still paints over the image at the window's
/// edges.
#[test]
fn an_image_draws_above_the_cursor_and_below_the_accent() {
    use std::num::NonZeroU16;

    let Some(rig) = test_support::Rig::new() else {
        return;
    };
    let border = [250u8, 10, 10];
    let interior = [10u8, 250, 10];
    let mut terminal = rig.terminal(16);
    terminal.push_pty_data(&raw_image_apc(8, 16, &bordered(border, interior), 0, 0));

    let frame = rig.frame(1.0, 16, 0.0);
    let (w, h) = frame.device_size();
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    let mut images = ImagePass::new();
    crate::render::painter::paint_frame(
        &mut canvas,
        &rig.metrics(1.0, 16),
        &frame,
        &mut terminal,
        &mut rig.pass(),
        &mut images,
    );

    // The cursor sits at cell (0, 0), under the image: the image's
    // interior wins, not the cursor's default foreground.
    let interior_bytes = [interior[2], interior[1], interior[0], 255];
    assert_eq!(
        canvas.pixel(4, 8),
        Some(interior_bytes),
        "the image covers the cursor"
    );

    // Now the focused frame with an accent: the accent's edge strokes
    // paint over the image, which covers the window corner.
    let accent = crate::layout::Accent::new([255, 0, 0], NonZeroU16::new(2).expect("nonzero"));
    let mut terminal = rig.terminal(16);
    terminal.push_pty_data(&raw_image_apc(8, 16, &bordered(border, interior), 0, 0));
    let frame = crate::render::geom::FrameInput::new(
        frame.logical_size().0,
        frame.logical_size().1,
        w,
        h,
        0.0,
        true,
        THEME,
        Some(accent),
    );
    let mut canvas = Canvas::new(w, h).expect("canvas size is valid");
    let mut images = ImagePass::new();
    crate::render::painter::paint_frame(
        &mut canvas,
        &rig.metrics(1.0, 16),
        &frame,
        &mut terminal,
        &mut rig.pass(),
        &mut images,
    );
    assert_eq!(
        canvas.pixel(0, 0),
        Some([0, 0, 255, 255]),
        "the accent paints over the image"
    );
    assert_eq!(
        canvas.pixel(4, 8),
        Some(interior_bytes),
        "the image still shows inside the accent"
    );
}

/// The cached pixmap for a placement can be inspected: the 1:1 case blits
/// the whole premultiplied image without a resample and still caches it.
#[test]
fn the_one_to_one_placements_skip_the_resample_and_stay_cached() {
    let pixels = [255u8, 0, 0, 255, 0, 255, 0, 255];
    let img = hand_image(&pixels, 2, 1);
    let mut canvas = themed_canvas(1.0);
    let mut images = ImagePass::new();
    let rect = DeviceRect::new(0, 0, 2, 1).expect("test rect is valid");
    images.draw_placement(&mut canvas, &rect, &img, 0);
    assert_eq!(canvas.pixel(0, 0), Some([0, 0, 255, 255]));
    assert_eq!(canvas.pixel(1, 0), Some([0, 255, 0, 255]));
    assert_eq!(images.cached(), 1, "the 1:1 result is cached");
    assert_eq!(images.misses(), 1);

    // And the same placement next frame is a hit.
    images.draw_placement(&mut canvas, &rect, &img, 0);
    assert_eq!(images.hits(), 1);
}
