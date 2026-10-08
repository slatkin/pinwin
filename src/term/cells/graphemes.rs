//! Grapheme-cluster joining and Nerd Font constraint widths for the frame
//! protocol, ported from `src/cells.zig` (`graphemeExtend`, `mergeGrapheme`,
//! `isSymbol`, `isGraphicsElement`, `constraintWidth`).
//!
//! libghostty-vt already joins ordinary combining marks, but it leaves emoji
//! modifiers, variation selectors, keycaps and the base after a zero-width
//! joiner in their own cells, so an emoji arrives as two or three cells and
//! each half would be drawn on its own. Ghostty's own renderer shows them as
//! one glyph, so pinwin joins them back together here (design D5 of
//! `port-to-rust`).
//!
//! These are pure functions over [`Cell`]; no FFI or toolkit type appears
//! here.

use super::Cell;

/// Codepoints that extend the previous cell's cluster.
fn grapheme_extend(cp: u32) -> bool {
    matches!(cp,
        0x200D // zero-width joiner
        | 0xFE0E | 0xFE0F // variation selectors 15 and 16
        | 0x20E3 // combining enclosing keycap
        | 0x1F3FB..=0x1F3FF // emoji skin tone modifiers
        | 0xE0020..=0xE007F // tag characters (subdivision flags)
    )
}

fn is_regional_indicator(cp: u32) -> bool {
    (0x1F1E6..=0x1F1FF).contains(&cp)
}

/// The first Unicode scalar of `text`, or 0 when it is empty or malformed.
pub(crate) fn first_codepoint(text: &[u8]) -> u32 {
    let Ok(string) = std::str::from_utf8(text) else {
        return 0;
    };
    string.chars().next().map_or(0, u32::from)
}

fn ends_with_zwj(text: &[u8]) -> bool {
    text.ends_with("\u{200D}".as_bytes())
}

// Approved per-instance (#13): a regional-indicator run inside one
// cell's text is a small count.
#[allow(
    clippy::cast_possible_truncation,
    reason = "approved #13: regional-indicator run in one cell is a small count"
)]
fn count_regionals(text: &[u8]) -> u32 {
    let Ok(string) = std::str::from_utf8(text) else {
        return 0;
    };
    string
        .chars()
        .filter(|&cp| is_regional_indicator(u32::from(cp)))
        .count() as u32
}

/// Join `src` into `dst` when the two cells are halves of one grapheme
/// cluster. The joined cell keeps `dst`'s position and width; `src` keeps its
/// colours (its background is still painted) but loses its text.
pub(super) fn merge_grapheme(dst: &mut Cell, src: &mut Cell) -> bool {
    if dst.len == 0 || src.len == 0 {
        return false;
    }
    let dst_text = &dst.text[..dst.len];
    let src_text = &src.text[..src.len];
    let first = first_codepoint(src_text);
    let odd_regionals = is_regional_indicator(first)
        && is_regional_indicator(first_codepoint(dst_text))
        && count_regionals(dst_text) % 2 == 1;

    if !ends_with_zwj(dst_text) && !grapheme_extend(first) && !odd_regionals {
        return false;
    }
    if dst.len + src.len > dst.text.len() {
        return false;
    }

    dst.text[dst.len..dst.len + src_text.len()].copy_from_slice(src_text);
    dst.len += src_text.len();
    src.len = 0;
    true
}

/// Codepoints Ghostty treats as "symbol-like" (`renderer/cell.zig isSymbol`,
/// which uses uucode's `is_symbol` property). Private use areas and the symbol
/// blocks, as documented there.
pub(super) fn is_symbol(cp: u32) -> bool {
    matches!(cp,
        0x2190..=0x21FF // arrows
        | 0x2460..=0x24FF // enclosed alphanumerics
        | 0x2600..=0x26FF // miscellaneous symbols
        | 0x2700..=0x27BF // dingbats
        | 0xE000..=0xF8FF // private use area
        | 0x1F100..=0x1F1FF // enclosed alphanumeric supplement
        | 0x1F300..=0x1F5FF // miscellaneous symbols and pictographs
        | 0x1F600..=0x1F64F // emoticons
        | 0x1F680..=0x1F6FF // transport and map symbols
        | 0xF0000..=0xFFFFD // private use plane 15
        | 0x0010_0000..=0x0010_FFFD // private use plane 16
    )
}

/// Terminal graphics rather than icons: box drawing, blocks, legacy computing
/// and Powerline (`renderer/cell.zig isGraphicsElement`).
fn is_graphics_element(cp: u32) -> bool {
    matches!(cp,
        0x2500..=0x257F
        | 0x2580..=0x259F
        | 0xE0B0..=0xE0D7
        | 0x1FB00..=0x1FBFF
        | 0x1CC00..=0x1CEBF
    )
}

fn is_space_codepoint(cp: u32) -> bool {
    cp == 0x20 || cp == 0x2002
}

/// How many cells this glyph may use once its Nerd Font constraint is applied
/// (`renderer/cell.zig constraintWidth`): a symbol may extend into the next
/// cell when that cell is empty, so icons do not get squeezed into one cell.
pub(super) fn constraint_width(cell: &Cell, prev_cp: u32, next_cp: u32, at_row_end: bool) -> u32 {
    let cp = first_codepoint(&cell.text[..cell.len]);
    if cell.wide != super::Wide::Narrow {
        return 2;
    }
    if !is_symbol(cp) {
        return 1;
    }
    if at_row_end {
        return 1;
    }
    if prev_cp != 0 && is_symbol(prev_cp) && !is_graphics_element(prev_cp) {
        return 1;
    }
    if next_cp == 0 || is_space_codepoint(next_cp) {
        return 2;
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::cells::Wide;

    /// A cell carrying `text` at column `x`.
    fn cell(x: i32, text: &str) -> Cell {
        let mut cell = Cell {
            x,
            ..Cell::default()
        };
        cell.set_text(text.as_bytes());
        cell
    }

    fn join_cells(dst: &Cell, src: &Cell) -> Cell {
        let mut dst = *dst;
        let mut src = *src;
        assert!(merge_grapheme(&mut dst, &mut src), "cells should merge");
        assert_eq!(src.len, 0, "the source cell loses its text");
        dst
    }

    #[test]
    fn joins_a_zero_width_joiner_in_either_direction() {
        // A ZWJ extends whatever precedes it...
        let joined = join_cells(&cell(0, "a"), &cell(1, "\u{200D}"));
        assert_eq!(joined.text_str(), "a\u{200D}");
        // ...and a base after a trailing ZWJ joins back to it.
        let joined = join_cells(&cell(0, "a\u{200D}"), &cell(1, "b"));
        assert_eq!(joined.text_str(), "a\u{200D}b");
    }

    #[test]
    fn joins_variation_selectors_keycaps_skin_tones_and_tags() {
        let joined = join_cells(&cell(0, "\u{2764}"), &cell(1, "\u{FE0F}"));
        assert_eq!(joined.text_str(), "\u{2764}\u{FE0F}");
        let joined = join_cells(&cell(0, "1"), &cell(1, "\u{20E3}"));
        assert_eq!(joined.text_str(), "1\u{20E3}");
        let joined = join_cells(&cell(0, "\u{1F44D}"), &cell(1, "\u{1F3FB}"));
        assert_eq!(joined.text_str(), "\u{1F44D}\u{1F3FB}");
        let joined = join_cells(&cell(0, "\u{1F3F4}"), &cell(1, "\u{E0067}"));
        assert_eq!(joined.text_str(), "\u{1F3F4}\u{E0067}");
    }

    #[test]
    fn joins_an_odd_regional_pair_but_not_an_even_one() {
        // One regional indicator before the second: they pair up.
        let joined = join_cells(&cell(0, "\u{1F1E6}"), &cell(1, "\u{1F1E7}"));
        assert_eq!(joined.text_str(), "\u{1F1E6}\u{1F1E7}");
        // Two already paired: a third is a new cluster.
        let mut dst = cell(0, "\u{1F1E6}\u{1F1E7}");
        let mut src = cell(1, "\u{1F1E8}");
        assert!(!merge_grapheme(&mut dst, &mut src));
    }

    #[test]
    fn rejects_text_that_is_not_a_cluster_half() {
        let mut dst = cell(0, "a");
        let mut src = cell(1, "b");
        assert!(!merge_grapheme(&mut dst, &mut src));
        // An empty side never joins.
        let mut empty = cell(1, "");
        assert!(!merge_grapheme(&mut dst, &mut empty));
        // A join that would overflow the 32-byte buffer is refused.
        let mut full = cell(0, &"a".repeat(32));
        let mut extra = cell(1, "\u{200D}");
        assert!(!merge_grapheme(&mut full, &mut extra));
    }

    #[test]
    fn constraint_width_follows_the_nerd_font_symbol_rules() {
        // A wide cell always wants two columns.
        let mut wide = cell(0, "a");
        wide.wide = Wide::Wide;
        assert_eq!(constraint_width(&wide, 0, 0x20, false), 2);

        // A non-symbol is always one column.
        assert_eq!(constraint_width(&cell(0, "a"), 0, 0, false), 1);

        // A symbol (the trigram has a Nerd Font constraint in the table)
        // extends into a following space.
        assert!(crate::nerd_font::constraint(0x2630).is_some());
        assert_eq!(constraint_width(&cell(0, "\u{2630}"), 0, 0x20, false), 2);
        // ...but not off the row end, and not before another glyph.
        assert_eq!(constraint_width(&cell(0, "\u{2630}"), 0, 0, true), 1);
        assert_eq!(
            constraint_width(&cell(0, "\u{2630}"), 0, 'x' as u32, false),
            1
        );
        // An undefined next codepoint counts as empty.
        assert_eq!(constraint_width(&cell(0, "\u{2630}"), 0, 0, false), 2);

        // A symbol preceded by a non-graphics symbol stays narrow, so runs of
        // icons do not each claim two cells. 0x1F600 is a symbol (emoticon)
        // that is not a graphics element.
        assert_eq!(
            constraint_width(&cell(1, "\u{2630}"), 0x1F600, 0x20, false),
            1
        );
        // A graphics element (Powerline) is exempt from that rule.
        assert_eq!(
            constraint_width(&cell(1, "\u{2630}"), 0xE0B0, 0x20, false),
            2
        );
    }
}
