//! Per-row diff machinery.
//!
//! Two responsibilities live here:
//!
//! 1. `Row` + `build_row` — compact byte representation of a row of cells
//!    used for line-level diffing between consecutive frames.
//! 2. SGR attribute delta helpers (`modifier_delta_codes`, `write_modifier_delta`,
//!    `all_text_attrs`) — minimal attribute transition encoder so a
//!    modifier change between adjacent cells only emits the few bytes needed
//!    to flip the differing bits (replaces the older "SGR 0 + re-apply" path).
//!
//! Clean-room migration from `oxi-tui-legacy/src/render/mod.rs`
//! (same project) and `oxi-tui-legacy/src/render/diff.rs`.

use std::io;

use ratatui::buffer::Cell;
use ratatui::style::{Color, Modifier};

/// Compact row representation for diff comparison.
///
/// Stores cell data as raw bytes for fast equality checking. Equal `Row`s
/// imply the corresponding screen rows are byte-for-byte identical.
pub(crate) type Row = Vec<u8>;

/// Build a compact byte representation of a row of cells.
///
/// Each cell encodes: symbol bytes + `0xFF` separator + fg (4 bytes) +
/// bg (4 bytes) + modifier bits (8 bytes, little-endian).
pub(crate) fn build_row<'a, I>(cells: I) -> Row
where
    I: Iterator<Item = (u16, u16, &'a Cell)>,
{
    let mut row = Vec::new();
    for (_, _, cell) in cells {
        // Encode: symbol bytes + fg + bg + modifier
        row.extend_from_slice(cell.symbol().as_bytes());
        row.push(0xFF); // separator
        row.extend_from_slice(&color_to_bytes(&cell.fg));
        row.extend_from_slice(&color_to_bytes(&cell.bg));
        row.extend_from_slice(&cell.modifier.bits().to_le_bytes());
    }
    row
}

fn color_to_bytes(color: &Color) -> [u8; 4] {
    match color {
        Color::Reset => [0, 0, 0, 0],
        Color::Black => [1, 0, 0, 0],
        Color::Red => [2, 0, 0, 0],
        Color::Green => [3, 0, 0, 0],
        Color::Yellow => [4, 0, 0, 0],
        Color::Blue => [5, 0, 0, 0],
        Color::Magenta => [6, 0, 0, 0],
        Color::Cyan => [7, 0, 0, 0],
        Color::Gray => [8, 0, 0, 0],
        Color::DarkGray => [9, 0, 0, 0],
        Color::LightRed => [10, 0, 0, 0],
        Color::LightGreen => [11, 0, 0, 0],
        Color::LightYellow => [12, 0, 0, 0],
        Color::LightBlue => [13, 0, 0, 0],
        Color::LightMagenta => [14, 0, 0, 0],
        Color::LightCyan => [15, 0, 0, 0],
        Color::White => [16, 0, 0, 0],
        Color::Indexed(i) => [17, *i, 0, 0],
        Color::Rgb(r, g, b) => [*r, *g, *b, 0xFF],
    }
}

// ---------------------------------------------------------------------------
// Modifier delta — minimal SGR attribute transitions
// ---------------------------------------------------------------------------

/// Compute the SGR parameter codes that transition text attributes from `prev`
/// to `curr` **without affecting foreground/background color**.
///
/// Replaces the older "Reset (SGR 0) then re-apply everything" strategy, which
/// cleared fg/bg on every modifier change and forced a costly fg/bg re-emit.
/// Here each attribute is toggled independently, so a modifier change between
/// two cells only costs the few bytes needed to flip the differing bits.
///
/// Bold and dim share SGR 22 ("normal intensity"): removing either emits 22 and
/// re-adds whichever remains. Italic / underline / reverse / crossed-out each
/// have a distinct off code and are handled independently.
pub(crate) fn modifier_delta_codes(prev: Modifier, curr: Modifier) -> Vec<&'static str> {
    if prev == curr {
        return Vec::new();
    }
    let added = curr & !prev;
    let removed = prev & !curr;
    let mut codes: Vec<&'static str> = Vec::new();

    // Bold/dim share SGR 22 (clears both). Removing either requires 22, then
    // re-enabling whichever (if any) should remain.
    if removed.intersects(Modifier::BOLD | Modifier::DIM) {
        codes.push("22");
        if curr.contains(Modifier::BOLD) {
            codes.push("1");
        }
        if curr.contains(Modifier::DIM) {
            codes.push("2");
        }
    } else {
        if added.contains(Modifier::BOLD) {
            codes.push("1");
        }
        if added.contains(Modifier::DIM) {
            codes.push("2");
        }
    }
    if removed.contains(Modifier::ITALIC) {
        codes.push("23");
    }
    if added.contains(Modifier::ITALIC) {
        codes.push("3");
    }
    if removed.contains(Modifier::UNDERLINED) {
        codes.push("24");
    }
    if added.contains(Modifier::UNDERLINED) {
        codes.push("4");
    }
    if removed.contains(Modifier::REVERSED) {
        codes.push("27");
    }
    if added.contains(Modifier::REVERSED) {
        codes.push("7");
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        codes.push("29");
    }
    if added.contains(Modifier::CROSSED_OUT) {
        codes.push("9");
    }

    codes
}

/// Write the minimal SGR sequence transitioning text attributes from `prev` to
/// `curr`. Emits nothing when they are equal. Never touches fg/bg, so the
/// caller's fg/bg delta tracking remains valid across the call.
pub(crate) fn write_modifier_delta<W: io::Write>(
    inner: &mut W,
    prev: Modifier,
    curr: Modifier,
) -> Result<(), io::Error> {
    let codes = modifier_delta_codes(prev, curr);
    if codes.is_empty() {
        return Ok(());
    }
    // ESC [ <params> m  — a single multi-parameter SGR is cheaper than several
    // one-parameter escapes.
    inner.write_all(b"\x1b[")?;
    let mut first = true;
    for code in &codes {
        if !first {
            inner.write_all(b";")?;
        }
        inner.write_all(code.as_bytes())?;
        first = false;
    }
    inner.write_all(b"m")
}

/// The union of every text-attribute bit oxi emits. Used as a synthetic
/// "previous" state on the first cell of a changed row, where the terminal's
/// real attribute state is unknown (skipped rows may have left attributes
/// active). Diffing from this mask force-clears everything via the delta path
/// without a full SGR-0 reset.
pub(crate) fn all_text_attrs() -> Modifier {
    Modifier::BOLD
        | Modifier::DIM
        | Modifier::ITALIC
        | Modifier::UNDERLINED
        | Modifier::REVERSED
        | Modifier::CROSSED_OUT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_no_change_emits_nothing() {
        assert!(modifier_delta_codes(Modifier::BOLD, Modifier::BOLD).is_empty());
        assert!(modifier_delta_codes(Modifier::empty(), Modifier::empty()).is_empty());
    }

    #[test]
    fn delta_bold_to_italic_clears_then_sets() {
        // Bold off shares SGR 22 (normal intensity); italic on is 3.
        assert_eq!(
            modifier_delta_codes(Modifier::BOLD, Modifier::ITALIC),
            vec!["22", "3"]
        );
    }

    #[test]
    fn delta_empty_to_bold_just_sets() {
        assert_eq!(
            modifier_delta_codes(Modifier::empty(), Modifier::BOLD),
            vec!["1"]
        );
    }

    #[test]
    fn delta_bold_to_empty_clears_via_22() {
        assert_eq!(
            modifier_delta_codes(Modifier::BOLD, Modifier::empty()),
            vec!["22"]
        );
    }

    #[test]
    fn delta_bold_dim_to_bold_re_adds_bold() {
        // 22 clears both bold and dim; bold must then be re-enabled.
        assert_eq!(
            modifier_delta_codes(Modifier::BOLD | Modifier::DIM, Modifier::BOLD),
            vec!["22", "1"]
        );
    }

    #[test]
    fn delta_italic_underline_reverse_crossed_each_distinct() {
        // Each independent attribute has its own off code.
        assert_eq!(
            modifier_delta_codes(Modifier::ITALIC, Modifier::empty()),
            vec!["23"]
        );
        assert_eq!(
            modifier_delta_codes(Modifier::UNDERLINED, Modifier::empty()),
            vec!["24"]
        );
        assert_eq!(
            modifier_delta_codes(Modifier::REVERSED, Modifier::empty()),
            vec!["27"]
        );
        assert_eq!(
            modifier_delta_codes(Modifier::CROSSED_OUT, Modifier::empty()),
            vec!["29"]
        );
    }

    #[test]
    fn delta_combine_multiple_adds_into_one_sequence() {
        assert_eq!(
            modifier_delta_codes(Modifier::empty(), Modifier::BOLD | Modifier::ITALIC),
            vec!["1", "3"]
        );
    }

    #[test]
    fn write_delta_emits_single_compound_sgr() {
        let mut out: Vec<u8> = Vec::new();
        write_modifier_delta(
            &mut out,
            Modifier::empty(),
            Modifier::BOLD | Modifier::ITALIC,
        )
        .unwrap();
        assert_eq!(out, b"\x1b[1;3m");
    }

    #[test]
    fn write_delta_no_change_writes_nothing() {
        let mut out: Vec<u8> = Vec::new();
        write_modifier_delta(&mut out, Modifier::BOLD, Modifier::BOLD).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn all_text_attrs_covers_six_bits() {
        let all = all_text_attrs();
        assert!(all.contains(Modifier::BOLD));
        assert!(all.contains(Modifier::DIM));
        assert!(all.contains(Modifier::ITALIC));
        assert!(all.contains(Modifier::UNDERLINED));
        assert!(all.contains(Modifier::REVERSED));
        assert!(all.contains(Modifier::CROSSED_OUT));
    }
}
