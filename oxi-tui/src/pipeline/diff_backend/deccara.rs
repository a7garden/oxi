//! DECCARA rectangular background-fill optimizer.
//!
//! Kitty extends VT510 DECCARA ("Change Attributes in Rectangular Area") to all
//! SGR attributes including background color, so a solid background panel can be
//! painted as a single rectangle escape instead of a full-width run of
//! background-styled spaces on every row:
//!
//! ```text
//! <ESC>[2*x                 DECSACE: select rectangle change extent
//! <ESC>[Pt;Pl;Pb;Pr;<sgr>$r DECCARA: apply <sgr> to rows Pt..Pb, cols Pl..Pr
//! <ESC>[*x                  DECSACE: restore default extent
//! ```
//!
//! Coordinates are 1-based and inclusive. This module is a pure,
//! renderer-level planner: it consumes the per-row ratatui [`Cell`]s the
//! [`crate::pipeline::diff_backend::DiffBackend`] would otherwise write,
//! proves which rows have a droppable trailing background fill, and returns
//! the rectangles to emit in their place. It is conservative by construction
//! — any ambiguity yields `None` so the caller keeps the exact original cell
//! output.
//!
//! Adapted from omp's `deccara.ts`, but operating on structured [`Cell`]s
//! (which makes the analysis simpler and more reliable than omp's ANSI-string
//! parser). Clean-room migration from `oxi-tui-legacy/src/render/deccara.rs`
//! (same project).
//!
//! The legacy crate's clippy config did not enforce these lints; they are
//! no-ops here and will be cleaned up in a follow-up. See Task 5 report.
#![allow(
    clippy::cast_possible_truncation,
    clippy::map_unwrap_or,
    clippy::items_after_statements,
    clippy::semicolon_if_nothing_returned,
    clippy::redundant_closure_for_method_calls
)]
use ratatui::buffer::Cell;
use ratatui::style::Color;

/// DECSACE — select the rectangle change extent so DECCARA fills a rectangle.
pub(crate) const DECSACE_RECT: &str = "\x1b[2*x";
/// DECSACE — restore the default (stream) change extent.
pub(crate) const DECSACE_DEFAULT: &str = "\x1b[*x";

/// A row's trailing background fill that can be replaced by a DECCARA rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BgFill {
    /// 0-based column immediately after the last non-space glyph. The fill
    /// covers the half-open range `[content_end .. width)`.
    pub content_end: u16,
    /// The non-default background color of the trailing fill.
    pub bg: Color,
}

/// Encode a background color as an SGR parameter string, or `None` for the
/// default/reset background (which DECCARA cannot usefully paint).
fn bg_sgr(color: Color) -> Option<String> {
    match color {
        Color::Reset => None,
        Color::Black => Some("40".into()),
        Color::Red => Some("41".into()),
        Color::Green => Some("42".into()),
        Color::Yellow => Some("43".into()),
        Color::Blue => Some("44".into()),
        Color::Magenta => Some("45".into()),
        Color::Cyan => Some("46".into()),
        Color::Gray => Some("47".into()),
        Color::DarkGray => Some("100".into()),
        Color::LightRed => Some("101".into()),
        Color::LightGreen => Some("102".into()),
        Color::LightYellow => Some("103".into()),
        Color::LightBlue => Some("104".into()),
        Color::LightMagenta => Some("105".into()),
        Color::LightCyan => Some("106".into()),
        Color::White => Some("107".into()),
        Color::Indexed(i) => Some(format!("48;5;{i}")),
        Color::Rgb(r, g, b) => Some(format!("48;2;{r};{g};{b}")),
    }
}

/// Encode a single DECCARA rectangle. `top`/`bottom` are 1-based inclusive
/// screen rows, `left`/`right` 1-based inclusive columns, `sgr` the raw SGR
/// parameter list to apply.
pub(crate) fn encode_rect(top: u16, left: u16, bottom: u16, right: u16, sgr: &str) -> String {
    format!("\x1b[{top};{left};{bottom};{right};{sgr}$r")
}

/// Analyze a full-width row of cells for a droppable trailing background fill.
///
/// Returns `None` unless the row spans the *full* `width` contiguously (no
/// gaps, columns `0..width`) **and** the trailing region after the last
/// non-space glyph is entirely spaces under a single, constant, non-default
/// background.
///
/// `cells` is `(column, cell)` pairs in arbitrary order; they are sorted here.
pub(crate) fn analyze_row(cells: &[(u16, &Cell)], width: u16) -> Option<BgFill> {
    if width == 0 || cells.is_empty() {
        return None;
    }

    // Build a column → cell map, rejecting gaps or out-of-range columns.
    let mut by_col: Vec<Option<&Cell>> = vec![None; width as usize];
    for &(x, cell) in cells {
        if x >= width {
            return None; // cell outside the row — not a clean full-width fill
        }
        if by_col[x as usize].is_some() {
            return None; // duplicate column — refuse to reason about it
        }
        by_col[x as usize] = Some(cell);
    }
    // Every column must be present (full-width, contiguous, no gaps).
    if by_col.iter().any(|c| c.is_none()) {
        return None;
    }

    // content_end = column just past the last non-space glyph (0 if the whole
    // row is spaces).
    let mut content_end = 0u16;
    for col in 0..width {
        let cell = by_col[col as usize].expect("checked present");
        if cell.symbol() != " " {
            content_end = col + 1;
        }
    }
    analyze_trailing(&by_col, content_end, width)
}

/// Given the trailing region starts at `content_end`, verify every space in
/// `[content_end .. width)` shares a single non-default background, and return
/// the fill. `by_col` is a full-width column map (all `Some`).
fn analyze_trailing(by_col: &[Option<&Cell>], content_end: u16, width: u16) -> Option<BgFill> {
    if content_end >= width {
        return None; // no trailing region
    }
    let first = by_col[content_end as usize].expect("full-width");
    let bg = first.bg;
    bg_sgr(bg)?; // default/reset background — nothing to paint
    for c in &by_col[content_end as usize + 1..width as usize] {
        let cell = c.expect("full-width");
        if cell.symbol() != " " || cell.bg != bg {
            return None; // non-space or drifted background — refuse
        }
    }
    Some(BgFill { content_end, bg })
}

/// A coalesced rectangle candidate covering one or more adjacent rows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FillCandidate {
    /// First screen row (0-based) of the group.
    top: u16,
    /// One past the last screen row (0-based) of the group.
    bottom: u16,
    /// 0-based column where the fill starts (rectangle left = left + 1).
    content_end: u16,
    bg: Color,
}

/// Per-frame plan: rectangles to emit, paired with each row's content cutoff so
/// the caller knows how many leading cells still need to be written.
#[derive(Debug, Default, Clone)]
pub(crate) struct DeccaraPlan {
    /// For each analyzed row: the cutoff column. Cells `[0 .. cutoff)` are
    /// written normally; `[cutoff .. width)` is left to the rectangles. `None`
    /// means "no fill — write the whole row". Indexed parallel to the input.
    pub cutoffs: Vec<Option<u16>>,
    /// The DECSACE-wrapped rectangle batch to emit after the rows, or empty.
    pub sequence: String,
}

/// Plan DECCARA rectangles for a contiguous block of rows.
///
/// `fills[k]` is the analysis for screen row `first_row + k` (0-based), or
/// `None`. Vertically adjacent rows with an identical `content_end`/`bg` span
/// coalesce into one rectangle. Rectangles are emitted only when the trailing
/// space bytes they remove exceed their own bytes plus the per-frame DECSACE
/// wrapper cost, so the plan never exceeds the original byte count.
pub(crate) fn plan_fills(fills: &[Option<BgFill>], width: u16, first_row: u16) -> DeccaraPlan {
    let n = fills.len();
    let mut cutoffs: Vec<Option<u16>> = vec![None; n];
    let mut candidates: Vec<FillCandidate> = Vec::new();

    let mut k = 0;
    while k < n {
        let Some(head) = &fills[k] else {
            cutoffs[k] = None;
            k += 1;
            continue;
        };
        // Extend over adjacent rows sharing the exact same fill span.
        let mut end = k;
        while end + 1 < n {
            match &fills[end + 1] {
                Some(next) if next.content_end == head.content_end && next.bg == head.bg => {
                    end += 1
                }
                _ => break,
            }
        }
        candidates.push(FillCandidate {
            top: first_row + k as u16,
            bottom: first_row + end as u16 + 1,
            content_end: head.content_end,
            bg: head.bg,
        });
        // Tentatively mark these rows' cutoffs; cleared if the group is rejected.
        cutoffs[k..=end].fill(Some(head.content_end));
        k = end + 1;
    }

    if candidates.is_empty() {
        return DeccaraPlan {
            cutoffs,
            sequence: String::new(),
        };
    }

    // Byte economics. Each trailing space cell currently costs ~1 byte (" ").
    // In exchange we add, per kept row, an EL ("clear to end of line", `\x1b[K`
    // — 3 bytes) to erase the previous frame's glyphs from the fill region
    // (DECCARA only repaints attributes, not glyphs), plus one rectangle
    // escape per group and a single DECSACE wrapper per frame.
    const CLEAR_COST: usize = "\x1b[K".len();
    let wrapper_bytes = DECSACE_RECT.len() + DECSACE_DEFAULT.len();
    let mut removed_total: usize = 0;
    let mut added_total: usize = 0;
    let mut kept: Vec<bool> = vec![false; candidates.len()];

    for (i, c) in candidates.iter().enumerate() {
        let rows = (c.bottom - c.top) as usize;
        let dropped_cells = rows * (width as usize - c.content_end as usize);
        let Some(sgr) = bg_sgr(c.bg) else {
            continue; // shouldn't happen (analyzer rejects default bg)
        };
        let rect = encode_rect(c.top + 1, c.content_end + 1, c.bottom, width, &sgr);
        // Net savings must beat the rect AND the per-row EL clears.
        let added = rect.len() + CLEAR_COST * rows;
        if dropped_cells > added {
            kept[i] = true;
            removed_total += dropped_cells;
            added_total += added;
        }
    }

    // The wrapper is worth it only if total bytes removed exceed the wrapper
    // plus all per-group added bytes (rects + per-row EL clears).
    if removed_total > wrapper_bytes + added_total {
        let mut sequence = String::from(DECSACE_RECT);
        for (i, c) in candidates.iter().enumerate() {
            if kept[i] {
                let sgr = bg_sgr(c.bg).unwrap_or_default();
                sequence.push_str(&encode_rect(
                    c.top + 1,
                    c.content_end + 1,
                    c.bottom,
                    width,
                    &sgr,
                ));
            }
        }
        sequence.push_str(DECSACE_DEFAULT);
        // Rows whose group was rejected must fall back to full writes.
        for (i, c) in candidates.iter().enumerate() {
            if !kept[i] {
                cutoffs[(c.top - first_row) as usize..=(c.bottom - first_row - 1) as usize]
                    .fill(None);
            }
        }
        DeccaraPlan { cutoffs, sequence }
    } else {
        // Net loss — emit nothing, write every row in full.
        for c in &candidates {
            cutoffs[(c.top - first_row) as usize..=(c.bottom - first_row - 1) as usize].fill(None);
        }
        DeccaraPlan {
            cutoffs,
            sequence: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn cell(sym: &str, bg: Color) -> Cell {
        // Cell::new takes a &'static str; set_symbol accepts a dynamic &str.
        let mut c = Cell::new(" ");
        c.set_symbol(sym);
        c.bg = bg;
        c
    }
    /// Build owned `(col, cell)` pairs for a full-width row from a per-column
    /// `(symbol, bg)` spec, padding trailing columns with bg-styled spaces
    /// matching the last spec entry.
    fn row(spec: &[(char, Color)], width: u16) -> Vec<(u16, Cell)> {
        let pad_bg = spec.last().map(|(_, bg)| *bg).unwrap_or(Color::Reset);
        let mut v: Vec<(u16, Cell)> = spec
            .iter()
            .enumerate()
            .map(|(i, (sym, bg))| (i as u16, cell(&sym.to_string(), *bg)))
            .collect();
        while (v.len() as u16) < width {
            let i = v.len() as u16;
            v.push((i, cell(" ", pad_bg)));
        }
        v
    }

    fn refs(cells: &[(u16, Cell)]) -> Vec<(u16, &Cell)> {
        cells.iter().map(|(x, c)| (*x, c)).collect()
    }

    #[test]
    fn bg_sgr_rgb_and_default() {
        assert_eq!(bg_sgr(Color::Reset), None);
        assert_eq!(bg_sgr(Color::Rgb(10, 20, 30)), Some("48;2;10;20;30".into()));
        assert_eq!(bg_sgr(Color::Indexed(4)), Some("48;5;4".into()));
        assert_eq!(bg_sgr(Color::Blue), Some("44".into()));
    }

    #[test]
    fn full_bg_row_is_fillable() {
        // 10 cols, all spaces under a non-default bg.
        let spec: Vec<(char, Color)> = vec![(' ', Color::Rgb(1, 2, 3)); 10];
        let cells = row(&spec, 10);
        let fill = analyze_row(&refs(&cells), 10).expect("fillable");
        assert_eq!(fill.content_end, 0);
        assert_eq!(fill.bg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn content_then_uniform_bg_trailing_is_fillable() {
        // "Hi" then 8 spaces, all under the same bg.
        let mut spec = vec![('H', Color::Rgb(1, 2, 3)), ('i', Color::Rgb(1, 2, 3))];
        for _ in 0..8 {
            spec.push((' ', Color::Rgb(1, 2, 3)));
        }
        let cells = row(&spec, 10);
        let fill = analyze_row(&refs(&cells), 10).expect("fillable");
        assert_eq!(fill.content_end, 2);
    }

    #[test]
    fn default_bg_trailing_is_not_fillable() {
        // Trailing spaces under Reset (default) bg — nothing to paint.
        let spec = vec![('H', Color::Reset), ('i', Color::Reset)];
        let cells = row(&spec, 10);
        assert!(analyze_row(&refs(&cells), 10).is_none());
    }

    #[test]
    fn mixed_bg_trailing_is_not_fillable() {
        // Trailing spaces alternate bg — inconsistent, refuse.
        let mut spec = vec![('H', Color::Rgb(1, 2, 3))];
        spec.push((' ', Color::Rgb(1, 2, 3)));
        spec.push((' ', Color::Rgb(9, 9, 9))); // drift
        let cells = row(&spec, 10);
        assert!(analyze_row(&refs(&cells), 10).is_none());
    }

    #[test]
    fn non_full_width_row_is_not_fillable() {
        // Build only 3 cells for a width-10 row (the `row` helper would pad to
        // full width, defeating the point) → gap → refuse.
        let cells: Vec<(u16, Cell)> = (0..3u16)
            .map(|i| (i, cell(" ", Color::Rgb(1, 2, 3))))
            .collect();
        assert!(analyze_row(&refs(&cells), 10).is_none());
    }

    #[test]
    fn encode_rect_is_well_formed() {
        let s = encode_rect(1, 4, 3, 10, "48;2;1;2;3");
        assert_eq!(s, "\x1b[1;4;3;10;48;2;1;2;3$r");
    }

    #[test]
    fn plan_coalesces_adjacent_identical_rows() {
        // Two identical full-bg rows at first_row=5 → one rectangle.
        let fill = BgFill {
            content_end: 0,
            bg: Color::Rgb(1, 2, 3),
        };
        let fills = vec![Some(fill.clone()), Some(fill)];
        let plan = plan_fills(&fills, 40, 5);
        // Width 40, 2 rows → 80 dropped bytes >> rect+wrapper cost.
        assert!(!plan.sequence.is_empty());
        assert!(plan.sequence.contains(DECSACE_RECT));
        assert!(plan.sequence.contains(DECSACE_DEFAULT));
        // One rect spanning rows 5..7 (1-based 6..7), cols 1..40.
        assert!(plan.sequence.contains("6;1;7;40;"));
        assert_eq!(plan.cutoffs, vec![Some(0), Some(0)]);
    }

    #[test]
    fn plan_rejects_when_not_worth_it() {
        // A single row with only a 1-col trailing fill: dropped ~1 byte,
        // rect costs far more → rejected, no sequence.
        let mut spec = vec![('H', Color::Rgb(1, 2, 3))];
        spec.push((' ', Color::Rgb(1, 2, 3)));
        let cells = row(&spec, 2);
        let fill = analyze_row(&refs(&cells), 2).expect("fillable");
        let plan = plan_fills(&[Some(fill)], 2, 0);
        assert!(plan.sequence.is_empty());
        assert_eq!(plan.cutoffs, vec![None]);
    }

    #[test]
    fn plan_keeps_rows_without_fills_full() {
        let fills = vec![None, None];
        let plan = plan_fills(&fills, 40, 0);
        assert!(plan.sequence.is_empty());
        assert_eq!(plan.cutoffs, vec![None, None]);
    }
}
