//! Native terminal rendering for command output.
//!
//! Bash/terminal tool output arrives as a raw PTY byte stream that can contain
//! ANSI SGR (colors/styles), cursor movement, line erases, and carriage returns
//! (progress bars rewriting a line). ratatui paints text verbatim and does not
//! interpret these, so without this module the scrollback shows literal escape
//! codes like `[1m[36m`.
//!
//! [`render_terminal_lines`] feeds the stream through a minimal, line-oriented
//! VTE emulator (built on the `vte` parser) and produces styled
//! [`Line`]s plus de-escaped plain text — what a terminal would actually
//! display. Unlike a screen/grid emulator it keeps an unbounded, fully-styled
//! transcript that maps onto the pager's line model.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use vte::{Params, Parser, Perform};

use crate::render::theme::color_support::quantize;

/// Bound transcript growth against pathological cursor jumps. Tool output is
/// already truncated upstream; these only guard against escape-code abuse.
const MAX_ROWS: usize = 50_000;
const MAX_COLS: usize = 8_192;

/// A single rendered transcript line: styled spans plus de-escaped plain text.
pub struct RenderedLine {
    pub line: Line<'static>,
    pub plain: String,
}

/// Parse a raw terminal stream (ANSI SGR + cursor/erase + carriage return) into
/// styled lines. `base` is the default style for text without an SGR override.
///
/// Deterministic and idempotent: a fresh emulator per call, safe to invoke from
/// both the render path and the height-cache path.
pub fn render_terminal_lines(raw: &str, base: Style) -> Vec<RenderedLine> {
    if raw.is_empty() {
        return Vec::new();
    }
    let mut sink = TermSink::new(base);
    let mut parser = Parser::new();
    parser.advance(&mut sink, raw.as_bytes());
    sink.finish()
}

/// De-escaped, cursor-resolved plain text of a terminal stream, for
/// clipboard/search. Lines are joined with `\n`.
pub fn render_terminal_plain(raw: &str) -> String {
    render_terminal_lines(raw, Style::default())
        .into_iter()
        .map(|rl| rl.plain)
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    style: Style,
}

struct TermSink {
    base: Style,
    cur: Style,
    rows: Vec<Vec<Cell>>,
    row: usize,
    col: usize,
}

impl TermSink {
    fn new(base: Style) -> Self {
        Self {
            base,
            cur: base,
            rows: vec![Vec::new()],
            row: 0,
            col: 0,
        }
    }

    fn ensure_row(&mut self) {
        if self.row >= MAX_ROWS {
            self.row = MAX_ROWS - 1;
        }
        while self.rows.len() <= self.row {
            self.rows.push(Vec::new());
        }
    }

    fn put(&mut self, ch: char) {
        if self.col >= MAX_COLS {
            return;
        }
        self.ensure_row();
        let blank = Cell {
            ch: ' ',
            style: self.base,
        };
        let line = &mut self.rows[self.row];
        if self.col >= line.len() {
            line.resize(self.col + 1, blank);
        }
        line[self.col] = Cell {
            ch,
            style: self.cur,
        };
        self.col += 1;
    }

    fn newline(&mut self) {
        self.row += 1;
        self.col = 0;
        self.ensure_row();
    }

    fn erase_line(&mut self, mode: u16) {
        self.ensure_row();
        let blank = Cell {
            ch: ' ',
            style: self.base,
        };
        let line = &mut self.rows[self.row];
        match mode {
            0 => line.truncate(self.col.min(line.len())),
            1 => {
                let end = (self.col + 1).min(line.len());
                line[..end].fill(blank);
            }
            2 => line.clear(),
            _ => {}
        }
    }

    fn erase_display(&mut self, mode: u16) {
        match mode {
            0 => {
                self.ensure_row();
                let len = self.rows[self.row].len();
                self.rows[self.row].truncate(self.col.min(len));
                self.rows.truncate(self.row + 1);
            }
            2 | 3 => {
                self.rows.clear();
                self.rows.push(Vec::new());
                self.row = 0;
                self.col = 0;
            }
            _ => {}
        }
    }

    fn apply_sgr(&mut self, params: &Params) {
        if params.is_empty() {
            self.cur = self.base;
            return;
        }
        let groups: Vec<&[u16]> = params.iter().collect();
        let mut i = 0;
        while i < groups.len() {
            let code = groups[i].first().copied().unwrap_or(0);
            match code {
                0 => self.cur = self.base,
                1 => self.cur = self.cur.add_modifier(Modifier::BOLD),
                2 => self.cur = self.cur.add_modifier(Modifier::DIM),
                3 => self.cur = self.cur.add_modifier(Modifier::ITALIC),
                4 => self.cur = self.cur.add_modifier(Modifier::UNDERLINED),
                7 => self.cur = self.cur.add_modifier(Modifier::REVERSED),
                22 => self.cur = self.cur.remove_modifier(Modifier::BOLD | Modifier::DIM),
                23 => self.cur = self.cur.remove_modifier(Modifier::ITALIC),
                24 => self.cur = self.cur.remove_modifier(Modifier::UNDERLINED),
                27 => self.cur = self.cur.remove_modifier(Modifier::REVERSED),
                30..=37 => self.cur.fg = Some(quantize(ansi16(code - 30))),
                39 => self.cur.fg = self.base.fg,
                40..=47 => self.cur.bg = Some(quantize(ansi16(code - 40))),
                49 => self.cur.bg = self.base.bg,
                90..=97 => self.cur.fg = Some(quantize(ansi16_bright(code - 90))),
                100..=107 => self.cur.bg = Some(quantize(ansi16_bright(code - 100))),
                38 => {
                    if let Some(c) = ext_color(&groups, &mut i) {
                        self.cur.fg = Some(quantize(c));
                    }
                }
                48 => {
                    if let Some(c) = ext_color(&groups, &mut i) {
                        self.cur.bg = Some(quantize(c));
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn finish(mut self) -> Vec<RenderedLine> {
        // `str::lines()` ignores a single trailing newline; mirror that so a
        // command ending in `\n` does not gain a spurious blank line.
        if self.rows.last().is_some_and(|r| r.is_empty()) {
            self.rows.pop();
        }
        let base = self.base;
        self.rows
            .into_iter()
            .map(|cells| row_to_line(cells, base))
            .collect()
    }
}

impl Perform for TermSink {
    fn print(&mut self, c: char) {
        self.put(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.newline(),
            b'\r' => self.col = 0,
            b'\t' => self.col = (self.col / 8 + 1) * 8,
            0x08 => self.col = self.col.saturating_sub(1),
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        match action {
            'm' => self.apply_sgr(params),
            'K' => self.erase_line(first_param(params, 0)),
            'J' => self.erase_display(first_param(params, 0)),
            'A' => self.row = self.row.saturating_sub(first_param(params, 1) as usize),
            'B' => {
                let n = first_param(params, 1) as usize;
                self.row = (self.row + n).min(self.rows.len().saturating_sub(1));
            }
            'C' => self.col = (self.col + first_param(params, 1) as usize).min(MAX_COLS),
            'D' => self.col = self.col.saturating_sub(first_param(params, 1) as usize),
            'G' => {
                self.col = (first_param(params, 1) as usize)
                    .saturating_sub(1)
                    .min(MAX_COLS)
            }
            _ => {}
        }
    }
}

/// First parameter value, substituting `default` for a missing or `0` value
/// (CSI cursor ops treat `0` as `1`; erase ops pass `0` as the default).
fn first_param(params: &Params, default: u16) -> u16 {
    match params.iter().next().and_then(|p| p.first().copied()) {
        Some(0) | None => default,
        Some(v) => v,
    }
}

/// Map a 0-7 ANSI color index to a named ratatui color.
fn ansi16(n: u16) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

/// Map a 0-7 bright ANSI color index to a named ratatui color.
fn ansi16_bright(n: u16) -> Color {
    match n {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

/// Resolve an extended color (`38`/`48`) in either `;` (advancing `i` over the
/// consumed groups) or `:` subparameter form. Returns an un-quantized color.
fn ext_color(groups: &[&[u16]], i: &mut usize) -> Option<Color> {
    let g = groups[*i];
    if g.len() >= 2 {
        return parse_ext(&g[1..]);
    }
    match groups.get(*i + 1).and_then(|p| p.first().copied())? {
        5 => {
            let idx = groups.get(*i + 2).and_then(|p| p.first().copied())?;
            *i += 2;
            Some(Color::Indexed(idx as u8))
        }
        2 => {
            let r = groups.get(*i + 2).and_then(|p| p.first().copied())?;
            let g = groups.get(*i + 3).and_then(|p| p.first().copied())?;
            let b = groups.get(*i + 4).and_then(|p| p.first().copied())?;
            *i += 4;
            Some(Color::Rgb(r as u8, g as u8, b as u8))
        }
        _ => None,
    }
}

/// Parse the subparameter form of an extended color, e.g. `[5, n]` (256) or
/// `[2, r, g, b]` (with an optional leading colorspace id). Un-quantized.
fn parse_ext(sub: &[u16]) -> Option<Color> {
    match sub.first().copied()? {
        5 => sub.get(1).map(|n| Color::Indexed(*n as u8)),
        2 => {
            let vals = &sub[1..];
            let (r, g, b) = match vals.len() {
                3 => (vals[0], vals[1], vals[2]),
                n if n >= 4 => (vals[n - 3], vals[n - 2], vals[n - 1]),
                _ => return None,
            };
            Some(Color::Rgb(r as u8, g as u8, b as u8))
        }
        _ => None,
    }
}

fn row_to_line(cells: Vec<Cell>, base: Style) -> RenderedLine {
    let mut end = cells.len();
    while end > 0 && cells[end - 1].ch == ' ' && cells[end - 1].style == base {
        end -= 1;
    }
    let cells = &cells[..end];
    if cells.is_empty() {
        return RenderedLine {
            line: Line::default(),
            plain: String::new(),
        };
    }
    let plain: String = cells.iter().map(|c| c.ch).collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut style = cells[0].style;
    for c in cells {
        if c.style != style {
            spans.push(Span::styled(std::mem::take(&mut buf), style));
            style = c.style;
        }
        buf.push(c.ch);
    }
    spans.push(Span::styled(buf, style));
    RenderedLine {
        line: Line::from(spans),
        plain,
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
