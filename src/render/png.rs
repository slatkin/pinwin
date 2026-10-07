//! The `png`-crate PNG decoder for the kitty images (row 4.7): the
//! [`crate::term::PngDecoder`] implementation the panel thread hands to the
//! terminal. Straight (non-premultiplied) RGBA8 out, exactly what
//! [`crate::render::canvas::image_pixmap`] takes, as the gdk-pixbuf
//! decoder it replaced did.
//!
//! The crate's transformations do the pixel-format work
//! (`EXPAND` palettes and low bit depths to bytes, `STRIP_16` 16-bit
//! samples to 8); the four remaining 8-bit layouts — gray, gray-alpha,
//! RGB, RGBA — each convert through one pure function, tested on numbers.
//! Interlaced (Adam7) streams decode through the crate's `next_frame`.
//!
//! Limits: a hostile or corrupt PNG must not allocate gigabytes. The
//! header is checked before any pixel buffer exists — zero dimensions
//! reject, and either dimension above `MAX_DIMENSION` rejects — and the
//! decoded-RGBA byte count is computed with checked arithmetic against
//! `MAX_DECODED_BYTES` before the vector is sized. `MAX_DECODED_BYTES`
//! is exactly the largest image `MAX_DIMENSION` admits decoded to RGBA8
//! (16384² × 4 = 1 GiB), so a full-size poster still decodes; the crate's
//! own allocation limiter caps its internal scratch (row buffers, zlib
//! windows) at its default 64 MiB, far above any row a
//! `MAX_DIMENSION`-wide image needs. Every failure path returns `None`;
//! no `unwrap`, no panic.

use std::io::Cursor;

use png::{BitDepth, ColorType, Decoder, Limits, Transformations};

use crate::term::{DecodedPng, PngDecoder};

/// The largest image side the decoder accepts, in pixels. Kitty posters
/// are large but not absurd: 16384² covers any media artwork a terminal
/// shows several times over, and it bounds the decoded RGBA buffer at
/// `MAX_DECODED_BYTES`. The image pass rejects an image beyond the same
/// bound, so the two agree.
pub(crate) const MAX_DIMENSION: u32 = 16_384;

/// The largest decoded image the decoder will build, in bytes of RGBA8:
/// `MAX_DIMENSION`² × 4, checked against the header before the buffer is
/// allocated.
const MAX_DECODED_BYTES: usize = 1_073_741_824;

/// The `png` crate's decoder (`glue_decode_png`'s replacement).
#[derive(Debug)]
pub struct PngCrateDecoder;

impl PngDecoder for PngCrateDecoder {
    fn decode_png(&mut self, data: &[u8]) -> Option<DecodedPng> {
        decode(data)
    }
}

/// Decode one PNG stream to straight RGBA8. `None` for anything the
/// limits reject or the crate cannot decode to the end.
#[must_use]
pub(crate) fn decode(data: &[u8]) -> Option<DecodedPng> {
    let mut decoder = Decoder::new_with_limits(Cursor::new(data), Limits { bytes: 64 << 20 });
    // `EXPAND` turns palettes into RGB (RGBA with a tRNS chunk) and low
    // bit depths into bytes; `STRIP_16` drops 16-bit samples to 8. After
    // both, the output is one of the four 8-bit layouts the converters
    // below take.
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;

    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width == 0 || height == 0 {
        return None;
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }

    let len = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)?;
    if len > MAX_DECODED_BYTES {
        return None;
    }

    let mut buf = vec![0u8; reader.output_buffer_size()?];
    reader.next_frame(&mut buf).ok()?;

    let (color, depth) = reader.output_color_type();
    if depth != BitDepth::Eight {
        // Unreachable with the transformations set; a future crate change
        // that breaks that assumption rejects instead of misreading bytes.
        return None;
    }
    match color {
        ColorType::Grayscale => Some(DecodedPng {
            width,
            height,
            rgba: gray_to_rgba(&buf, width, height)?,
        }),
        ColorType::GrayscaleAlpha => Some(DecodedPng {
            width,
            height,
            rgba: gray_alpha_to_rgba(&buf, width, height)?,
        }),
        ColorType::Rgb => Some(DecodedPng {
            width,
            height,
            rgba: rgb_to_rgba(&buf, width, height)?,
        }),
        ColorType::Rgba => Some(DecodedPng {
            width,
            height,
            rgba: buf,
        }),
        // Indexed is expanded away by `EXPAND`; it cannot appear here.
        ColorType::Indexed => None,
    }
}

/// Expand one byte per pixel (8-bit gray) to opaque RGBA. `None` when the
/// buffer does not hold `width * height` samples.
#[must_use]
fn gray_to_rgba(src: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let pixels = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    if src.len() != pixels {
        return None;
    }
    Some(
        src.iter()
            .flat_map(|gray| [*gray, *gray, *gray, 0xff])
            .collect(),
    )
}

/// Expand two bytes per pixel (gray + alpha) to RGBA.
#[must_use]
fn gray_alpha_to_rgba(src: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let pixels = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    if src.len() != pixels.checked_mul(2)? {
        return None;
    }
    Some(
        src.as_chunks::<2>()
            .0
            .iter()
            .flat_map(|ga| [ga[0], ga[0], ga[0], ga[1]])
            .collect(),
    )
}

/// Expand three bytes per pixel (RGB) to opaque RGBA.
#[must_use]
fn rgb_to_rgba(src: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let pixels = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    if src.len() != pixels.checked_mul(3)? {
        return None;
    }
    Some(
        src.as_chunks::<3>()
            .0
            .iter()
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 0xff])
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use png::Encoder;

    /// Encode a PNG with the same crate, for the decoder to read back.
    /// The pixel data is written the sequential way, so only the tiny
    /// interlaced case below hand-builds a real Adam7 stream.
    fn encode(width: u32, height: u32, color: ColorType, depth: BitDepth, raw: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        encode_into(&mut out, width, height, color, depth, None, None, raw);
        out
    }

    /// Encode into `out`; the writer's drop inside this function finishes
    /// the stream, so the caller's `out` is complete on return.
    fn encode_into(
        out: &mut Vec<u8>,
        width: u32,
        height: u32,
        color: ColorType,
        depth: BitDepth,
        palette: Option<Vec<u8>>,
        trns: Option<Vec<u8>>,
        raw: &[u8],
    ) {
        let mut encoder = Encoder::new(out, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        if let Some(palette) = palette {
            encoder.set_palette(palette);
        }
        if let Some(trns) = trns {
            encoder.set_trns(trns);
        }
        let mut writer = encoder.write_header().expect("test header writes");
        writer.write_image_data(raw).expect("test data writes");
    }

    /// The bytes of one pixel, in `color`'s layout, built from RGBA.
    fn sample(color: ColorType, rgba: [u8; 4]) -> Vec<u8> {
        match color {
            ColorType::Grayscale => {
                vec![rgba[0]]
            }
            ColorType::GrayscaleAlpha => vec![rgba[0], rgba[3]],
            ColorType::Rgb => vec![rgba[0], rgba[1], rgba[2]],
            ColorType::Rgba => rgba.to_vec(),
            ColorType::Indexed => Vec::new(),
        }
    }

    #[test]
    fn the_four_eight_bit_layouts_decode_to_rgba() {
        let pixel = [0x12, 0x34, 0x56, 0xaa];
        for (color, depth, raw) in [
            (
                ColorType::Grayscale,
                BitDepth::Eight,
                sample(ColorType::Grayscale, pixel),
            ),
            (
                ColorType::GrayscaleAlpha,
                BitDepth::Eight,
                sample(ColorType::GrayscaleAlpha, pixel),
            ),
            (
                ColorType::Rgb,
                BitDepth::Eight,
                sample(ColorType::Rgb, pixel),
            ),
            (
                ColorType::Rgba,
                BitDepth::Eight,
                sample(ColorType::Rgba, pixel),
            ),
        ] {
            let png = encode(2, 1, color, depth, &raw.repeat(2));
            let decoded = decode(&png).expect("the 8-bit image decodes");
            assert_eq!((decoded.width, decoded.height), (2, 1), "{color:?}");
            let want = match color {
                // Gray keeps only its first channel; the others repeat it.
                ColorType::Grayscale => [pixel[0], pixel[0], pixel[0], 0xff],
                ColorType::GrayscaleAlpha => [pixel[0], pixel[0], pixel[0], pixel[3]],
                ColorType::Rgb => [pixel[0], pixel[1], pixel[2], 0xff],
                _ => pixel,
            };
            assert_eq!(decoded.rgba, want.repeat(2), "{color:?}");
        }
    }

    #[test]
    fn sixteen_bit_samples_strip_to_eight() {
        // 16-bit gray: two samples, big endian, one pixel.
        let raw = [0x12, 0x34, 0x56, 0x78];
        let png = encode(2, 1, ColorType::Grayscale, BitDepth::Sixteen, &raw);
        let decoded = decode(&png).expect("the 16-bit gray image decodes");
        // `STRIP_16` keeps the high byte.
        assert_eq!(
            decoded.rgba,
            [0x12, 0x12, 0x12, 0xff, 0x56, 0x56, 0x56, 0xff]
        );
    }

    #[test]
    fn low_bit_depth_gray_expands_to_bytes() {
        // 1-bit gray, two pixels per byte: 0b10000000 = white, black.
        let png = encode(2, 1, ColorType::Grayscale, BitDepth::One, &[0b1000_0000]);
        let decoded = decode(&png).expect("the 1-bit image decodes");
        assert_eq!(decoded.rgba, [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0xff]);

        // 2-bit: two pixels per byte, MSB first. One byte holds the four
        // pixels 0, 1, 0, 2 — values scale by 255/3: 0, 85, 0, 170.
        let png = encode(4, 1, ColorType::Grayscale, BitDepth::Two, &[0b0001_0010]);
        let decoded = decode(&png).expect("the 2-bit image decodes");
        assert_eq!(
            decoded.rgba,
            [
                0, 0, 0, 0xff, 85, 85, 85, 0xff, 0, 0, 0, 0xff, 170, 170, 170, 0xff
            ]
        );

        // 4-bit: value 9 scales to 9 * 255 / 15 = 153.
        let png = encode(2, 1, ColorType::Grayscale, BitDepth::Four, &[0x90]);
        let decoded = decode(&png).expect("the 4-bit image decodes");
        assert_eq!(decoded.rgba, [153, 153, 153, 0xff, 0, 0, 0, 0xff]);
    }

    #[test]
    fn a_palette_with_trns_decodes_to_rgba() {
        let mut out = Vec::new();
        // Palette entries: opaque red, opaque green, half-alpha blue.
        let palette = vec![0xff, 0, 0, 0, 0xff, 0, 0, 0, 0xff];
        // tRNS for a palette carries one alpha byte per entry, so entries
        // 0 and 1 stay opaque and entry 2 is half alpha.
        let trns = vec![0xff, 0xff, 0x80];
        encode_into(
            &mut out,
            3,
            1,
            ColorType::Indexed,
            BitDepth::Eight,
            Some(palette),
            Some(trns),
            &[0, 1, 2],
        );
        let decoded = decode(&out).expect("the palette image decodes");
        assert_eq!(
            decoded.rgba,
            [0xff, 0, 0, 0xff, 0, 0xff, 0, 0xff, 0, 0, 0xff, 0x80]
        );
    }

    #[test]
    fn a_palette_without_trns_decodes_opaque() {
        let mut out = Vec::new();
        let palette = vec![10, 20, 30, 40, 50, 60];
        encode_into(
            &mut out,
            2,
            1,
            ColorType::Indexed,
            BitDepth::Eight,
            Some(palette),
            None,
            &[1, 0],
        );
        let decoded = decode(&out).expect("the palette image decodes");
        assert_eq!(decoded.rgba, [40, 50, 60, 0xff, 10, 20, 30, 0xff]);
    }

    #[test]
    fn gray_with_trns_gains_alpha() {
        let mut out = Vec::new();
        // tRNS for gray: the 16-bit gray sample that is transparent.
        let trns = vec![0, 0];
        encode_into(
            &mut out,
            2,
            1,
            ColorType::Grayscale,
            BitDepth::Eight,
            None,
            Some(trns),
            &[0, 200],
        );
        let decoded = decode(&out).expect("the gray+tRNS image decodes");
        assert_eq!(decoded.rgba, [0, 0, 0, 0, 200, 200, 200, 0xff]);
    }

    #[test]
    fn an_interlaced_stream_decodes_through_adam7() {
        // A hand-built 4x4 8-bit gray Adam7 PNG: the crate's encoder
        // writes the interlaced flag but sequential data, so the stream
        // below lays the seven passes out itself, each row unfiltered,
        // inside one stored (uncompressed) zlib stream.
        //
        // Pass contents for 4x4, each row one byte (gray), unfiltered —
        // the Adam7 grid the crate's own `Adam7Iterator` documents:
        //
        //     1 6 4 6
        //     7 7 7 7
        //     5 6 5 6
        //     7 7 7 7
        //
        // p1: row 0, column 0 -> [c0]
        // p2 (columns at 4): no columns -> skipped
        // p3 (rows at 4): no rows -> skipped
        // p4: row 0, column 2 -> [c2]
        // p5 (x step 2, y step 4, offset 2): row 2, columns 0 and 2
        // p6 (x step 2 offset 1, y step 2): rows 0 and 2, columns 1 and 3
        // p7 (x step 1, y step 2 offset 1): rows 1 and 3, all four columns
        //
        // The gray value of pixel (col, row) is `col * 10 + row`.
        let pixel = |col: u32, row: u32| -> u8 { u8::try_from(col * 10 + row).expect("fits") };
        let rows: [Vec<u8>; 7] = [
            vec![pixel(0, 0)],
            vec![pixel(2, 0)],
            vec![pixel(0, 2), pixel(2, 2)],
            vec![pixel(1, 0), pixel(3, 0)],
            vec![pixel(1, 2), pixel(3, 2)],
            vec![pixel(0, 1), pixel(1, 1), pixel(2, 1), pixel(3, 1)],
            vec![pixel(0, 3), pixel(1, 3), pixel(2, 3), pixel(3, 3)],
        ];
        // Every pass row: filter byte 0 (None), then the row bytes. A
        // skipped pass (no rows) contributes nothing, not even a filter
        // byte.
        let mut raw = Vec::new();
        for row in &rows {
            if row.is_empty() {
                continue;
            }
            raw.push(0);
            raw.extend_from_slice(row);
        }

        let mut out = Vec::new();
        out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
        write_chunk(&mut out, *b"IHDR", &{
            let mut d = Vec::new();
            d.extend_from_slice(&4u32.to_be_bytes());
            d.extend_from_slice(&4u32.to_be_bytes());
            d.push(8); // bit depth
            d.push(0); // color type gray
            d.push(0); // compression
            d.push(0); // filter
            d.push(1); // interlace: Adam7
            d
        });
        write_chunk(&mut out, *b"IDAT", &zlib_stored(&raw));
        write_chunk(&mut out, *b"IEND", &[]);

        let decoded = decode(&out).expect("the interlaced image decodes");
        assert_eq!((decoded.width, decoded.height), (4, 4));
        let mut want = Vec::new();
        for row in 0..4u32 {
            for col in 0..4u32 {
                let g = pixel(col, row);
                want.extend_from_slice(&[g, g, g, 0xff]);
            }
        }
        assert_eq!(decoded.rgba, want, "every pixel lands from its pass");
    }

    /// One PNG chunk: length, type, data, CRC-32.
    fn write_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
        out.extend_from_slice(
            &u32::try_from(data.len())
                .expect("test chunk fits")
                .to_be_bytes(),
        );
        out.extend_from_slice(&kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(kind, data).to_be_bytes());
    }

    /// A zlib stream of stored (uncompressed) deflate blocks: the two-byte
    /// zlib header, the blocks, and the Adler-32 checksum.
    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut rest = data;
        while !rest.is_empty() {
            let take = rest.len().min(0xffff);
            let (block, tail) = rest.split_at(take);
            let final_block = u8::from(tail.is_empty());
            let len = u16::try_from(take).expect("block fits");
            out.push(final_block);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(block);
            rest = tail;
        }
        if data.is_empty() {
            // An empty stream still needs one empty final block.
            out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
        }
        out.extend_from_slice(&adler32(data).to_be_bytes());
        out
    }

    /// The zlib checksum.
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for byte in data {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        (b << 16) | a
    }

    /// The PNG chunk checksum.
    fn crc32(kind: [u8; 4], data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in kind.iter().chain(data.iter()) {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn a_truncated_stream_is_rejected() {
        let png = encode(2, 2, ColorType::Rgba, BitDepth::Eight, &[0xaa; 16]);
        assert!(decode(&png[..png.len() / 2]).is_none(), "truncated");
        assert!(decode(&png[..8]).is_none(), "signature only");
        assert!(decode(&[]).is_none(), "empty");
        assert!(decode(b"not a png at all").is_none());
    }

    #[test]
    fn a_one_pixel_image_decodes() {
        let png = encode(1, 1, ColorType::Rgba, BitDepth::Eight, &[1, 2, 3, 4]);
        let decoded = decode(&png).expect("the 1x1 image decodes");
        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert_eq!(decoded.rgba, [1, 2, 3, 4]);
    }

    #[test]
    fn a_header_beyond_the_dimension_cap_is_rejected_before_the_alloc() {
        // A valid PNG whose IHDR claims 0x7fffffff wide: the CRC is right,
        // so the header parses and the dimension cap is what rejects it.
        let mut out = Vec::new();
        out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&0x7fff_ffffu32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        write_chunk(&mut out, *b"IHDR", &ihdr);
        write_chunk(&mut out, *b"IDAT", &zlib_stored(&[0]));
        write_chunk(&mut out, *b"IEND", &[]);
        assert!(decode(&out).is_none(), "the huge header is rejected");

        // Zero dimensions reject too (a PNG cannot carry them, but the
        // check is the decoder's own).
        let mut zero = Vec::new();
        zero.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&0u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        write_chunk(&mut zero, *b"IHDR", &ihdr);
        write_chunk(&mut zero, *b"IDAT", &zlib_stored(&[0]));
        write_chunk(&mut zero, *b"IEND", &[]);
        assert!(decode(&zero).is_none(), "the zero-width header is rejected");
    }

    /// The last chunk of `encode`'s stream is IEND: the writer's drop
    /// inside the block scope finished the stream before `out` was read.
    #[test]
    fn the_test_encoder_produces_a_complete_stream() {
        let png = encode(1, 1, ColorType::Rgba, BitDepth::Eight, &[1, 2, 3, 4]);
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert!(
            png.windows(4).any(|w| w == b"IEND"),
            "the stream carries IEND"
        );
    }
}
