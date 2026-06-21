//! Differential rendering backend for ratatui.
//!
//! Wraps `CrosstermBackend` with a line-level diffing layer. Only changed rows
//! are written to the terminal, dramatically reducing I/O for streaming AI chat
//! where most of the screen stays static between frames.
//!
//! ## How it works
//!
//! 1. Collect all cells from ratatui's `draw()` iterator into row buffers
//! 2. Compare each row against the previous frame using fast u64 checksums
//! 3. For changed rows: move cursor + write cells directly via crossterm
//! 4. For unchanged rows: skip entirely
//! 5. Wrap output in CSI 2026 synchronized update mode to prevent tearing

pub mod ansi;
#[allow(missing_docs)]
mod deccara;
pub mod diff;
pub mod image;
pub mod mermaid;
pub mod terminal;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

/// Mermaid diagram rendering: ASCII conversion, cache, and box border output.
pub use mermaid::{
    MermaidColorMode, MermaidRenderOptions, clear_mermaid_cache, render_ascii_diagram,
    render_mermaid_ascii,
};

use std::fmt;
use std::io;

use crossterm::{
    cursor::MoveTo,
    style::{
        Attribute as CAttribute, Color as CColor, Print, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
};
use ratatui::{
    backend::{Backend, IntoCrossterm, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
    style::{Color, Modifier},
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error type for DiffBackend.
#[derive(Debug)]
pub struct DiffBackendError(io::Error);

impl fmt::Display for DiffBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for DiffBackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<io::Error> for DiffBackendError {
    fn from(e: io::Error) -> Self {
        DiffBackendError(e)
    }
}

// ---------------------------------------------------------------------------
// Row buffer
// ---------------------------------------------------------------------------

/// Compact row representation for diff comparison.
/// Stores cell data as raw bytes for fast comparison.
type Row = Vec<u8>;

/// Build a compact byte representation of a row of cells.
fn build_row<'a, I>(cells: I) -> Row
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
fn modifier_delta_codes(prev: Modifier, curr: Modifier) -> Vec<&'static str> {
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
fn write_modifier_delta<W: io::Write>(
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
fn all_text_attrs() -> Modifier {
    Modifier::BOLD
        | Modifier::DIM
        | Modifier::ITALIC
        | Modifier::UNDERLINED
        | Modifier::REVERSED
        | Modifier::CROSSED_OUT
}

// ---------------------------------------------------------------------------
// DiffBackend
// ---------------------------------------------------------------------------

/// A ratatui `Backend` wrapper that performs line-level differential rendering.
///
/// Instead of writing every cell to the terminal on each frame, it compares the
/// new frame buffer with the previous one and only writes changed rows.
pub struct DiffBackend<W: io::Write> {
    /// The underlying crossterm backend.
    inner: ratatui::backend::CrosstermBackend<W>,
    /// Previous frame rows for diff comparison.
    prev_rows: Vec<Row>,
    /// Whether we need to force a full redraw.
    force_full_redraw: bool,
    /// Terminal width at last draw (for resize detection).
    last_width: u16,
    /// Terminal height at last draw (for resize detection).
    last_height: u16,
    /// Detected terminal capabilities. Gates escape-sequence emission
    /// (e.g. CSI 2026 synchronized output) so unsupported features aren't sent.
    caps: terminal::TerminalCapabilities,
    /// Whether the DECCARA bg-fill optimizer is active: the terminal must
    /// advertise DECCARA (Kitty/Ghostty) and `OXI_NO_DECCARA` must be unset.
    deccara_enabled: bool,
}

impl<W: io::Write> DiffBackend<W> {
    /// Create a new DiffBackend wrapping the given crossterm backend.
    ///
    /// Terminal capabilities are detected from the environment. To inject
    /// explicit capabilities (e.g. for tests), use [`Self::with_capabilities`].
    pub fn new(inner: ratatui::backend::CrosstermBackend<W>) -> Self {
        Self::with_capabilities(inner, terminal::TerminalCapabilities::detect())
    }

    /// Create a DiffBackend with explicitly provided terminal capabilities.
    pub fn with_capabilities(
        inner: ratatui::backend::CrosstermBackend<W>,
        caps: terminal::TerminalCapabilities,
    ) -> Self {
        DiffBackend {
            inner,
            prev_rows: Vec::new(),
            force_full_redraw: true,
            last_width: 0,
            last_height: 0,
            deccara_enabled: caps.deccara && std::env::var_os("OXI_NO_DECCARA").is_none(),
            caps,
        }
    }

    /// Force a full redraw on the next frame.
    pub fn invalidate(&mut self) {
        self.force_full_redraw = true;
    }
}

impl<W: io::Write> Backend for DiffBackend<W> {
    type Error = DiffBackendError;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // Collect all cells into row groups
        let mut row_cells: Vec<Vec<(u16, u16, &'a Cell)>> = Vec::new();
        let mut max_col: u16 = 0;
        let mut max_row: u16 = 0;

        for (x, y, cell) in content {
            let yidx = y as usize;
            while row_cells.len() <= yidx {
                row_cells.push(Vec::new());
            }
            max_col = max_col.max(x);
            max_row = max_row.max(y);
            row_cells[yidx].push((x, y, cell));
        }

        let term_w = max_col + 1;
        let term_h = max_row + 1;

        // Check for resize — force full redraw
        if term_w != self.last_width || term_h != self.last_height {
            self.force_full_redraw = true;
            self.last_width = term_w;
            self.last_height = term_h;
        }

        // Build compact rows for comparison
        let new_rows: Vec<Row> = row_cells
            .iter()
            .map(|cells| build_row(cells.iter().map(|&(x, y, c)| (x, y, c))))
            .collect();

        if self.force_full_redraw || self.prev_rows.is_empty() {
            // Full redraw — delegate to crossterm.
            // NOTE: `row_cells` is consumed here (into_iter). This is safe
            // because we `return` immediately after — the diff branch below
            // never executes in this case.
            let all_cells: Vec<(u16, u16, &'a Cell)> = row_cells.into_iter().flatten().collect();
            self.inner.draw(all_cells.into_iter())?;
            self.prev_rows = new_rows;
            self.force_full_redraw = false;
            return Ok(());
        }

        // --- Differential rendering with Synchronized Update (CSI 2026) ---
        // Begin synchronized output (CSI 2026) to prevent mid-frame tearing.
        // Only emit when the terminal is known to support it (safe-default
        // on; disable via `OXI_NO_SYNC_OUTPUT=1` or an unsupported terminal).
        if self.caps.synchronized_output {
            let _ = crossterm::queue!(self.inner, crossterm::style::Print("\x1b[?2026h"));
            self.inner.flush()?;
        }

        // Reset any residual attributes
        crossterm::execute!(self.inner, SetAttribute(CAttribute::Reset))?;

        // Find changed rows
        let max_rows = new_rows.len().max(self.prev_rows.len());

        // DECCARA bg-fill optimizer (Kitty/Ghostty): analyze the rows that will
        // be repainted for a droppable trailing background fill and plan
        // coalesced rectangles. No-op unless the terminal advertises DECCARA.
        let deccara_plan = if self.deccara_enabled {
            let fills: Vec<Option<deccara::BgFill>> = (0..max_rows)
                .map(|row_idx| {
                    let new_row = new_rows.get(row_idx);
                    let prev_row = self.prev_rows.get(row_idx);
                    let will_write = match (new_row, prev_row) {
                        (Some(nr), Some(pr)) => nr != pr,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if !will_write {
                        return None;
                    }
                    let cells = row_cells.get(row_idx)?;
                    let pairs: Vec<(u16, &Cell)> = cells.iter().map(|&(x, _y, c)| (x, c)).collect();
                    deccara::analyze_row(&pairs, term_w)
                })
                .collect();
            deccara::plan_fills(&fills, term_w, 0)
        } else {
            deccara::DeccaraPlan::default()
        };
        for row_idx in 0..max_rows {
            let new_row = new_rows.get(row_idx);
            let prev_row = self.prev_rows.get(row_idx);

            match (new_row, prev_row) {
                (Some(nr), Some(pr)) if nr == pr => continue, // Unchanged — skip
                (None, Some(_)) => {
                    // Row was removed — clear it
                    crossterm::execute!(
                        self.inner,
                        MoveTo(0, row_idx as u16),
                        crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine)
                    )?;
                }
                (Some(_), _) => {
                    // Row is new or changed — write it
                    crossterm::execute!(self.inner, MoveTo(0, row_idx as u16))?;

                    // Write cells for this row
                    if let Some(cells) = row_cells.get(row_idx) {
                        let cutoff = deccara_plan.cutoffs.get(row_idx).copied().flatten();
                        let mut last_x: u16 = 0;
                        let mut last_fg: Option<CColor> = None;
                        let mut last_bg: Option<CColor> = None;
                        let mut last_mod: Option<Modifier> = None;

                        for &(x, _y, cell) in cells {
                            // DECCARA: the trailing fill region is painted by a
                            // rectangle at frame end — stop writing at the cutoff.
                            if cutoff.is_some_and(|c| x >= c) {
                                break;
                            }
                            // Move cursor if there's a gap
                            if x > last_x {
                                crossterm::execute!(self.inner, MoveTo(x, row_idx as u16))?;
                            }

                            // Set style only if changed
                            let fg = cell.fg.into_crossterm();
                            if last_fg.as_ref() != Some(&fg) {
                                crossterm::execute!(self.inner, SetForegroundColor(fg))?;
                                last_fg = Some(fg);
                            }

                            let bg = cell.bg.into_crossterm();
                            if last_bg.as_ref() != Some(&bg) {
                                crossterm::execute!(self.inner, SetBackgroundColor(bg))?;
                                last_bg = Some(bg);
                            }

                            let modifier = cell.modifier;
                            if last_mod != Some(modifier) {
                                // Emit only the attribute delta. On the first
                                // cell of a changed row (`last_mod == None`)
                                // the terminal's attribute state is unknown —
                                // skipped rows may have left attributes active
                                // — so force-clear everything by diffing from
                                // the all-attributes mask. These targeted SGR
                                // off/on codes never touch fg/bg, so the fg/bg
                                // delta tracking above stays valid (no SGR-0
                                // reset-and-reapply needed).
                                let prev_attrs = last_mod.unwrap_or_else(all_text_attrs);
                                write_modifier_delta(&mut self.inner, prev_attrs, modifier)?;
                                last_mod = Some(modifier);
                            }

                            crossterm::execute!(self.inner, Print(cell.symbol()))?;
                            last_x = x + 1;
                        }
                        // DECCARA: erase the previous frame's glyphs from the
                        // fill region (the rectangle only repaints attributes,
                        // not glyphs, so stale text must be cleared first).
                        if let Some(c) = cutoff {
                            crossterm::execute!(self.inner, MoveTo(c, row_idx as u16))?;
                            let _ =
                                crossterm::queue!(self.inner, crossterm::style::Print("\x1b[K"));
                        }
                    }
                }
                (None, None) => unreachable!(),
            }
        }

        // DECCARA: emit the coalesced background-fill rectangles. They overlay
        // the cleared trailing regions and must sit inside the synchronized-
        // update window (before its end marker) so the frame is atomic.
        if self.deccara_enabled && !deccara_plan.sequence.is_empty() {
            let _ = crossterm::queue!(self.inner, crossterm::style::Print(&deccara_plan.sequence));
        }

        // CSI 2026: End synchronized update — flush all changes atomically.
        if self.caps.synchronized_output {
            let _ = crossterm::queue!(self.inner, crossterm::style::Print("\x1b[?2026l"));
        }
        ratatui::backend::Backend::flush(self)?;

        self.prev_rows = new_rows;
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()?;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()?;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.inner.get_cursor_position()?)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)?;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.force_full_redraw = true;
        self.prev_rows.clear();
        self.inner.clear()?;
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> Result<(), Self::Error> {
        self.force_full_redraw = true;
        self.inner.clear_region(clear_type)?;
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(self.inner.size()?)
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(self.inner.window_size()?)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()?;
        Ok(())
    }
}

// Write trait delegation — required for crossterm's execute!() macro
impl<W: io::Write> io::Write for DiffBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        std::io::Write::flush(&mut self.inner)
    }
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

    // ── DECCARA integration (wire-format proof) ─────────────────────────

    /// An `io::Write` that records every byte into a shared buffer, so a test
    /// can read what `DiffBackend` actually emitted without a real terminal.
    #[derive(Default, Clone)]
    struct RecordingWriter(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
    impl io::Write for RecordingWriter {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Build a full-width, 3-row frame where every cell is a space under `bg`.
    fn bg_frame(width: u16, bg: Color) -> Vec<(u16, u16, Cell)> {
        (0..width)
            .flat_map(|x| {
                (0..3u16).map(move |y| {
                    let mut c = Cell::new(" ");
                    c.bg = bg;
                    (x, y, c)
                })
            })
            .collect()
    }

    fn draw_cells<W: io::Write>(backend: &mut DiffBackend<W>, cells: &[(u16, u16, Cell)]) {
        backend
            .draw(cells.iter().map(|&(x, y, ref c)| (x, y, c)))
            .unwrap();
    }

    #[test]
    fn deccara_emits_rectangle_for_full_bg_rows() {
        // With caps.deccara = true, a changed full-width solid-background
        // block must be painted by a single DECCARA rectangle, not a run of
        // background-styled spaces.
        let recorder = RecordingWriter::default();
        let caps = terminal::TerminalCapabilities {
            deccara: true,
            synchronized_output: false, // isolate DECCARA bytes from CSI 2026
            ..Default::default()
        };
        let mut backend = DiffBackend::with_capabilities(
            ratatui::backend::CrosstermBackend::new(recorder.clone()),
            caps,
        );

        // Frame 1 establishes prev_rows (force-full-redraw path).
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(0, 0, 0)));
        // Frame 2 changes every row's bg → diff path with DECCARA active.
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(10, 20, 30)));

        let buf = recorder.0.borrow();
        let emitted = String::from_utf8_lossy(&buf);
        assert!(
            emitted.contains("\x1b[2*x"),
            "missing DECSACE rect: {emitted:?}"
        );
        // One rectangle: 0-based rows 0..=2 → 1-based 1..=3, cols 1..=20,
        // bg 48;2;10;20;30.
        assert!(
            emitted.contains("\x1b[1;1;3;20;48;2;10;20;30$r"),
            "missing DECCARA rect: {emitted:?}"
        );
        assert!(
            emitted.contains("\x1b[*x"),
            "missing DECSACE default: {emitted:?}"
        );
    }

    #[test]
    fn deccara_inactive_when_capability_off() {
        // With caps.deccara = false, no DECCARA escape must leak — the rows
        // are written as ordinary (space) cells.
        let recorder = RecordingWriter::default();
        let caps = terminal::TerminalCapabilities {
            deccara: false,
            synchronized_output: false,
            ..Default::default()
        };
        let mut backend = DiffBackend::with_capabilities(
            ratatui::backend::CrosstermBackend::new(recorder.clone()),
            caps,
        );
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(0, 0, 0)));
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(10, 20, 30)));

        let buf = recorder.0.borrow();
        let emitted = String::from_utf8_lossy(&buf);
        assert!(
            !emitted.contains("\x1b[2*x"),
            "DECCARA leaked while disabled: {emitted:?}"
        );
    }
}
