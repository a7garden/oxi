//! Differential rendering backend for ratatui — line-level diff, CSI 2026
//! sync wrap, DECCARA bg-fill optimizer, `force_full_redraw` on resize.
//!
//! Module layout: [`row`] (cell byte encoding + SGR delta), [`deccara`]
//! (Kitty DECCARA bg-fill rectangles), [`caps`] (terminal capability
//! detection; will move to `theme/` in Task 13).
//!
//! Clean-room migration from `oxi-tui-legacy/src/render/mod.rs` (same
//! project). Legacy clippy config did not enforce the lints below; they
//! are no-ops here and will be cleaned up in a follow-up.
#![allow(
    clippy::cast_possible_truncation,
    clippy::doc_markdown,
    clippy::needless_continue,
    clippy::similar_names,
    clippy::trivially_copy_pass_by_ref
)]
pub mod caps;
pub mod deccara;
pub mod row;

pub use caps::TerminalCaps;

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
    style::Modifier,
};
use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::pipeline::diff_backend::row::{Row, all_text_attrs, build_row, write_modifier_delta};

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

/// A ratatui `Backend` wrapper that performs line-level differential rendering.
///
/// Instead of writing every cell to the terminal on each frame, it compares
/// the new frame buffer with the previous one and only writes changed rows.
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
    caps: TerminalCaps,
    /// Whether the DECCARA bg-fill optimizer is active: the terminal must
    deccara_enabled: bool,
    /// OSC8 link spans for the next flush. Populated by [`DiffBackend::set_links`]
    /// before `flush()`; consumed (cleared) by the row-writer when OSC8
    /// emission lands in Plan C PR-7. Until then this is just storage.
    links: Vec<(CellRange, LinkTarget)>,
}

impl<W: io::Write> DiffBackend<W> {
    /// Create a new DiffBackend wrapping the given crossterm backend.
    ///
    /// Terminal capabilities are detected from the environment. To inject
    /// explicit capabilities (e.g. for tests), use [`Self::with_capabilities`].
    pub fn new(inner: ratatui::backend::CrosstermBackend<W>) -> Self {
        Self::with_capabilities(inner, TerminalCaps::detect())
    }
    /// Create a DiffBackend with explicitly provided terminal capabilities.
    pub fn with_capabilities(
        inner: ratatui::backend::CrosstermBackend<W>,
        caps: TerminalCaps,
    ) -> Self {
        DiffBackend {
            inner,
            prev_rows: Vec::new(),
            force_full_redraw: true,
            last_width: 0,
            last_height: 0,
            deccara_enabled: caps.deccara && std::env::var_os("OXI_NO_DECCARA").is_none(),
            caps,
            links: Vec::new(),
        }
    }

    /// Force a full redraw on the next frame.
    pub fn invalidate(&mut self) {
        self.force_full_redraw = true;
    }

    /// Set OSC8 link spans for the next flush.
    ///
    /// Called BEFORE [`Backend::flush`] so row writes can emit inline OSC8
    /// escapes (Plan C PR-7 fills in the emission). For now (foundation)
    /// this just stores the spans — emission is a no-op until PR-7 lands.
    pub fn set_links(&mut self, mut links: LinkCollector) {
        self.links = links.take();
    }
}

// ── OSC8 link collection (stub — emission comes in Plan C PR-7) ─────────

/// Where a link points. `Url` for http/https/other schemes, `File` for absolute paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// External URL (https, http, mailto, etc.).
    Url(String),
    /// Local file with optional line number.
    File {
        /// Absolute path to the file.
        path: PathBuf,
        /// Optional 1-based line number.
        line: Option<u32>,
    },
}

/// A horizontal range of cells on a single row that a link covers.
///
/// Coordinates are absolute (post-layout) screen positions. `x_end` is
/// inclusive — a single-cell link has `x_start == x_end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    /// Row (0-indexed from top of frame).
    pub y: u16,
    /// First cell column (0-indexed, inclusive).
    pub x_start: u16,
    /// Last cell column (0-indexed, inclusive).
    pub x_end: u16,
}

/// Collects link spans emitted by widgets during `render()`.
///
/// DiffBackend will emit OSC8 sequences inline during row writes (Plan C
/// PR-7), inside the CSI 2026 window. For now this is just storage.
#[derive(Debug, Default, Clone)]
pub struct LinkCollector {
    spans: Vec<(CellRange, LinkTarget)>,
}

impl LinkCollector {
    /// Create an empty collector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a link span. Order of `add` calls defines emission order
    /// (Plan C PR-7 will sort by row/column before writing).
    pub fn add(&mut self, range: CellRange, target: LinkTarget) {
        self.spans.push((range, target));
    }

    /// True when no spans have been collected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Number of collected spans.
    #[must_use]
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Take the collected spans, leaving an empty collector.
    pub fn take(&mut self) -> Vec<(CellRange, LinkTarget)> {
        std::mem::take(&mut self.spans)
    }
}

impl<W: io::Write> Backend for DiffBackend<W> {
    type Error = DiffBackendError;

    #[allow(clippy::too_many_lines)] // Migrated verbatim from legacy; the ~190-line diff loop is inherent.
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
                            if cutoff.is_some_and(|c| x >= c) {
                                break;
                            }
                            if x > last_x {
                                crossterm::execute!(self.inner, MoveTo(x, row_idx as u16))?;
                            }
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
                                let prev_attrs = last_mod.unwrap_or_else(all_text_attrs);
                                write_modifier_delta(&mut self.inner, prev_attrs, modifier)?;
                                last_mod = Some(modifier);
                            }
                            crossterm::execute!(self.inner, Print(cell.symbol()))?;
                            last_x = x + 1;
                        }
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
    use ratatui::style::Color;

    /// Records every byte written so a test can read what `DiffBackend`
    /// actually emitted without a real terminal.
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

    /// Build a backend + recorder pair with a given `caps` override.
    fn backend_with_caps(caps: TerminalCaps) -> (DiffBackend<RecordingWriter>, RecordingWriter) {
        let recorder = RecordingWriter::default();
        let backend = DiffBackend::with_capabilities(
            ratatui::backend::CrosstermBackend::new(recorder.clone()),
            caps,
        );
        (backend, recorder)
    }

    /// No-sync / no-deccara caps (the common default for testing the diff path).
    fn plain_caps() -> TerminalCaps {
        TerminalCaps {
            synchronized_output: false,
            deccara: false,
            ..TerminalCaps::default()
        }
    }

    /// An identical second frame must skip every row (no glyph bytes emitted).
    #[test]
    fn unchanged_second_frame_emits_no_cell_bytes() {
        let (mut backend, recorder) = backend_with_caps(plain_caps());
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(0, 0, 0)));
        recorder.0.borrow_mut().clear();
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(0, 0, 0)));
        let buf = recorder.0.borrow();
        let emitted = String::from_utf8_lossy(&buf);
        assert!(!emitted.contains(' '), "glyph bytes leaked: {emitted:?}");
        assert!(
            buf.len() < 16,
            "unchanged frame should be ~empty, got {} bytes",
            buf.len()
        );
    }

    /// A frame at a different width must force a full redraw.
    #[test]
    fn resize_forces_full_redraw() {
        let (mut backend, recorder) = backend_with_caps(plain_caps());
        draw_cells(&mut backend, &bg_frame(20, Color::Reset));
        recorder.0.borrow_mut().clear();
        draw_cells(&mut backend, &bg_frame(40, Color::Reset));
        let buf = recorder.0.borrow();
        assert!(
            buf.len() > 40,
            "resized frame should emit cell bytes, got {}",
            buf.len()
        );
    }

    /// `invalidate()` flips the flag so the next frame is a full redraw.
    #[test]
    fn invalidate_forces_full_redraw() {
        let (mut backend, recorder) = backend_with_caps(plain_caps());
        draw_cells(&mut backend, &bg_frame(10, Color::Rgb(0, 0, 0)));
        recorder.0.borrow_mut().clear();
        backend.invalidate();
        draw_cells(&mut backend, &bg_frame(10, Color::Rgb(1, 1, 1)));
        let buf = recorder.0.borrow();
        assert!(!buf.is_empty(), "invalidate() should force a full redraw");
    }

    /// CSI 2026 synchronization must wrap changed-cell bytes on diff frames.
    #[test]
    fn csi_2026_emits_sync_wrappers_around_diff_writes() {
        let caps = TerminalCaps {
            synchronized_output: true,
            ..plain_caps()
        };
        let (mut backend, recorder) = backend_with_caps(caps);

        draw_cells(&mut backend, &[(0, 0, Cell::new("A"))]);
        backend.flush().unwrap();
        recorder.0.borrow_mut().clear();

        draw_cells(&mut backend, &[(0, 0, Cell::new("B"))]);
        backend.flush().unwrap();

        let buf = recorder.0.borrow();
        let emitted = String::from_utf8_lossy(&buf);
        let begin = emitted
            .find("\x1b[?2026h")
            .expect("CSI 2026 begin must be emitted");
        let changed_cell = emitted.find('B').expect("changed cell must be emitted");
        let end = emitted
            .rfind("\x1b[?2026l")
            .expect("CSI 2026 end must be emitted");
        assert!(
            begin < changed_cell && changed_cell < end,
            "changed cell byte must be inside sync window: {emitted:?}"
        );
    }

    /// With caps.deccara = true, a changed full-width solid-bg block must be
    /// painted by a single DECCARA rectangle.
    #[test]
    fn deccara_emits_rectangle_for_full_bg_rows() {
        let caps = TerminalCaps {
            deccara: true,
            ..plain_caps()
        };
        let (mut backend, recorder) = backend_with_caps(caps);
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(0, 0, 0)));
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(10, 20, 30)));
        let buf = recorder.0.borrow();
        let emitted = String::from_utf8_lossy(&buf);
        assert!(
            emitted.contains("\x1b[2*x"),
            "missing DECSACE rect: {emitted:?}"
        );
        assert!(
            emitted.contains("\x1b[1;1;3;20;48;2;10;20;30$r"),
            "missing DECCARA rect: {emitted:?}"
        );
        assert!(
            emitted.contains("\x1b[*x"),
            "missing DECSACE default: {emitted:?}"
        );
    }

    /// No DECCARA escape must leak when the capability is off.
    #[test]
    fn deccara_inactive_when_capability_off() {
        let (mut backend, recorder) = backend_with_caps(plain_caps());
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(0, 0, 0)));
        draw_cells(&mut backend, &bg_frame(20, Color::Rgb(10, 20, 30)));
        let buf = recorder.0.borrow();
        let emitted = String::from_utf8_lossy(&buf);
        assert!(
            !emitted.contains("\x1b[2*x"),
            "DECCARA leaked while disabled: {emitted:?}"
        );
    }

    #[test]
    fn link_collector_add_and_take() {
        let mut c = LinkCollector::new();
        assert!(c.is_empty());
        c.add(
            CellRange {
                y: 0,
                x_start: 0,
                x_end: 4,
            },
            LinkTarget::Url("https://example.com".into()),
        );
        assert_eq!(c.len(), 1);
        let taken = c.take();
        assert_eq!(taken.len(), 1);
        assert!(c.is_empty());
    }

    #[test]
    fn diff_backend_accepts_set_links_without_emitting() {
        // Foundation: `set_links` is a no-op storage. Emission comes in
        // Plan C PR-7 (OSC8 inside the CSI 2026 window).
        let mut backend = DiffBackend::with_capabilities(
            ratatui::backend::CrosstermBackend::new(Vec::<u8>::new()),
            plain_caps(),
        );
        let mut links = LinkCollector::new();
        links.add(
            CellRange {
                y: 0,
                x_start: 0,
                x_end: 3,
            },
            LinkTarget::Url("https://example.com".into()),
        );
        backend.set_links(links);
        // No assertion on bytes — the test only verifies the API is
        // callable and that `set_links` accepts the spans without
        // touching the underlying writer.
    }
}
