//! Minimal markdown → InlineSegment renderer with full inline styling.
use anstyle::{Color as AnsiColorEnum, Effects, RgbColor};
use oxicode_vtui_compat::ui_protocol::{InlineSegment, InlineTextStyle};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::sync::{Arc, LazyLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Cached once at module scope — never rebuild per call.
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

// Per-line syntax-highlight memo for `render_code_block`. Same `(lang, line,
// width, theme)` triple → same highlight output; LLMs re-streaming a code
// block pay zero cost the second time around. Bounded at 4096 entries so a
// runaway stream can't OOM the UI thread; cleared on theme changes via the
// `theme_epoch` key (active syntax theme name) so re-themes never serve a
// stale palette.
type SyntectMemoMap =
    std::collections::HashMap<(String, String, usize, String), Vec<InlineSegment>>;
thread_local! {
    static SYNTECT_LINE_MEMO: std::cell::RefCell<SyntectMemoMap> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}
const SYNTECT_MEMO_CAP: usize = 4096;
fn memo_get(lang: &str, line: &str, width: usize, theme: &str) -> Option<Vec<InlineSegment>> {
    SYNTECT_LINE_MEMO.with(|m| {
        m.borrow()
            .get(&(lang.to_string(), line.to_string(), width, theme.to_string()))
            .cloned()
    })
}

fn memo_put(lang: &str, line: &str, width: usize, theme: &str, segs: Vec<InlineSegment>) {
    SYNTECT_LINE_MEMO.with(|m| {
        let mut map = m.borrow_mut();
        if map.len() >= SYNTECT_MEMO_CAP {
            // Clear-and-rebuild rather than tracking insertion: a stream
            // that hammers the same 200 lines repeatedly would otherwise
            // evict the lines it's about to re-request.
            map.clear();
        }
        map.insert(
            (lang.to_string(), line.to_string(), width, theme.to_string()),
            segs,
        );
    });
}

/// Increment-only fast-path cache for the streaming assistant render site.
///
/// Holds the last `(text, width)` pair plus the lines `render_markdown`
/// produced for it. `render_markdown_cached` is a thin wrapper that
/// returns the cached lines when the new call's `(text, width)` exactly
/// equals the cached one — the streaming typewriter advances a few bytes
/// each frame, but most frames the visible message and viewport are
/// unchanged, so the equality fast-path is the common case. Anything else
/// (different text, different width, first call) falls through to the
/// full renderer and refreshes the cache.
#[derive(Default, Debug)]
pub struct MdRenderCache {
    prev_text: String,
    prev_width: usize,
    lines: Vec<Vec<InlineSegment>>,
    /// Fast-path hit counter; exposed via `debug_hits` so the streaming
    /// render tests can assert the cache is being consulted.
    hits: usize,
}

impl MdRenderCache {
    /// Number of times the fast path returned the cached lines without
    /// invoking `render_markdown`. Test-only accessor — kept on the type
    /// so the test module can read it without a `pub(crate)` escape.
    pub fn debug_hits(&self) -> usize {
        self.hits
    }
}

/// Cached counterpart of [`render_markdown`]. When `(text, width)` matches
/// the previous call, returns a clone of the cached lines without invoking
/// the full parser — the streaming typewriter re-renders many times per
/// second but most frames have not actually changed. A miss always
/// refreshes the cache.
pub fn render_markdown_cached(
    text: &str,
    width: usize,
    cache: &mut MdRenderCache,
) -> Vec<Vec<InlineSegment>> {
    if width == cache.prev_width && text == cache.prev_text && !cache.lines.is_empty() {
        cache.hits += 1;
        return cache.lines.clone();
    }
    let lines = render_markdown(text, width);
    cache.prev_text = text.to_string();
    cache.prev_width = width;
    cache.lines = lines.clone();
    lines
}

/// Parse markdown text into styled InlineSegment lines.
///
/// `width` is the usable cell width of the destination surface. Tables
/// are the only block that pre-computes its own geometry — a table built
/// wider than the viewport wraps at the terminal edge and every border
/// row breaks. Pass the scrollback content width; other blocks wrap at
/// render time as before.
pub fn render_markdown(text: &str, width: usize) -> Vec<Vec<InlineSegment>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    let mut lines: Vec<Vec<InlineSegment>> = Vec::new();
    let mut cur: Vec<InlineSegment> = Vec::new();
    let mut effects: Effects = Effects::default();
    let mut code_buf: Option<CodeBlockState> = None;
    let mut table_buf: Option<TableState> = None;
    let mut list_stack: Vec<ListLevel> = Vec::new();
    let ss = &*SYNTAX_SET;

    for event in Parser::new_ext(text, opts) {
        if let Some(tb) = &mut table_buf {
            match event {
                Event::Text(t) | Event::Html(t) | Event::Code(t) => tb.current_cell.push_str(&t),
                Event::End(TagEnd::TableCell) => {
                    tb.current_row.push(std::mem::take(&mut tb.current_cell));
                }
                Event::End(TagEnd::TableRow) => {
                    tb.rows.push(std::mem::take(&mut tb.current_row));
                }
                Event::End(TagEnd::TableHead) => {
                    // pulldown-cmark 0.13 does not emit End(TableRow) for the
                    // head — the head IS the row, so the accumulated cells
                    // are still in `current_row`. Move them straight into
                    // `header` so the body parser starts clean.
                    if tb.header.is_empty() {
                        tb.header = std::mem::take(&mut tb.current_row);
                    }
                }
                Event::End(TagEnd::Table) => {
                    let tb = table_buf.take().unwrap();
                    let table_lines = render_table(&tb.header, &tb.rows, width);
                    lines.extend(table_lines);
                    lines.push(Vec::new());
                }
                _ => {}
            }
            continue;
        }

        // Code block capture mode
        if let Some(cb) = &mut code_buf {
            match event {
                Event::Text(t) => cb.code.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let cb = code_buf.take().unwrap();
                    flush_line(&mut cur, &mut lines);
                    let block_lines = render_code_block(&cb.code, cb.lang.as_deref(), ss, width);
                    lines.extend(block_lines);
                    lines.push(Vec::new());
                }
                _ => {}
            }
            continue;
        }

        // Flat main match
        match event {
            // ── Code block ─────────────────────────────────────────────
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_line(&mut cur, &mut lines);
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => Some(l.to_string()),
                    CodeBlockKind::Indented => None,
                };
                code_buf = Some(CodeBlockState {
                    code: String::new(),
                    lang,
                });
            }

            // ── Tables ─────────────────────────────────────────────────
            Event::Start(Tag::Table(_)) => {
                flush_line(&mut cur, &mut lines);
                table_buf = Some(TableState::default());
            }

            // ── Lists ──────────────────────────────────────────────────
            Event::Start(Tag::List(start)) => {
                list_stack.push(ListLevel {
                    is_ordered: start.is_some(),
                    index: start.map(|n| n.saturating_sub(1)).unwrap_or(0),
                });
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                flush_line(&mut cur, &mut lines);
                let depth = list_stack.len();
                if let Some(top) = list_stack.last_mut() {
                    let indent = " ".repeat((depth.saturating_sub(1)) * 2);
                    let marker = if top.is_ordered {
                        let n = top.index + 1;
                        top.index += 1;
                        format!("{}{}. ", indent, n)
                    } else {
                        format!("{}• ", indent)
                    };
                    let seg = InlineSegment {
                        text: marker,
                        style: Arc::new(InlineTextStyle::default()),
                    };
                    merge_or_push(&mut cur, seg);
                }
            }

            // ── Block-level ────────────────────────────────────────────
            Event::Start(Tag::Paragraph) => {}

            Event::Start(Tag::Heading { level, .. }) => {
                flush_line(&mut cur, &mut lines);
                {
                    effects = effects.insert(Effects::BOLD);
                };
                {
                    effects = effects.insert(if level == HeadingLevel::H1 {
                        Effects::UNDERLINE
                    } else {
                        Effects::default()
                    });
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                {
                    effects = effects.remove(Effects::BOLD | Effects::UNDERLINE);
                };
                flush_line(&mut cur, &mut lines);
            }

            Event::Start(Tag::BlockQuote(_)) => {
                {
                    effects = effects.insert(Effects::DIMMED);
                };
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                {
                    effects = effects.remove(Effects::DIMMED);
                };
            }

            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => {
                flush_line(&mut cur, &mut lines);
            }

            Event::Rule => {
                flush_line(&mut cur, &mut lines);
                lines.push(vec![InlineSegment {
                    text: "\u{2500}".repeat(40),
                    style: Arc::new(InlineTextStyle::default().dim()),
                }]);
            }

            // ── Inline formatting ──────────────────────────────────────
            Event::Start(Tag::Emphasis) => {
                {
                    effects = effects.insert(Effects::ITALIC);
                };
            }
            Event::End(TagEnd::Emphasis) => {
                {
                    effects = effects.remove(Effects::ITALIC);
                };
            }

            Event::Start(Tag::Strong) => {
                {
                    effects = effects.insert(Effects::BOLD);
                };
            }
            Event::End(TagEnd::Strong) => {
                {
                    effects = effects.remove(Effects::BOLD);
                };
            }

            Event::Start(Tag::Strikethrough) => {
                {
                    effects = effects.insert(Effects::STRIKETHROUGH);
                };
            }
            Event::End(TagEnd::Strikethrough) => {
                {
                    effects = effects.remove(Effects::STRIKETHROUGH);
                };
            }

            Event::Start(Tag::Link { .. }) => {
                {
                    effects = effects.insert(Effects::UNDERLINE);
                };
            }
            Event::End(TagEnd::Link) => {
                {
                    effects = effects.remove(Effects::UNDERLINE);
                };
            }

            // ── Text content ───────────────────────────────────────────
            Event::Text(t) | Event::Html(t) => {
                let style = apply_effects(InlineTextStyle::default(), effects);
                let seg = InlineSegment {
                    text: t.to_string(),
                    style: Arc::new(style),
                };
                merge_or_push(&mut cur, seg);
            }

            Event::Code(t) => {
                let seg = InlineSegment {
                    text: t.to_string(),
                    style: Arc::new(InlineTextStyle::default().bold()),
                };
                merge_or_push(&mut cur, seg);
            }

            Event::SoftBreak | Event::HardBreak => {
                flush_line(&mut cur, &mut lines);
            }

            Event::FootnoteReference(t) => {
                let seg = InlineSegment {
                    text: format!("[^{}]", t),
                    style: Arc::new(InlineTextStyle::default().dim()),
                };
                merge_or_push(&mut cur, seg);
            }

            _ => {}
        }
    }

    flush_line(&mut cur, &mut lines);
    lines
}

/// Render a code block with syntect syntax highlighting.
///
/// `width == 0` preserves the historical "no wrap" behavior so internal
/// callers (and tests) that don't pass a viewport still work. Any other
/// value hard-wraps the highlighted output at display-width boundaries —
/// LLMs frequently emit tables inside code fences, and the un-wrapped
/// version overflowed the terminal before this parameter existed.
/// Wrapping happens *after* syntax highlighting so each chunk keeps its
/// token color; tabs expand to four spaces first to keep the math
/// simple (syntect preserves tabs verbatim and a tab stop is variable).
pub fn render_code_block(
    code: &str,
    lang: Option<&str>,
    ss: &SyntaxSet,
    width: usize,
) -> Vec<Vec<InlineSegment>> {
    let syntax = lang
        .and_then(|l| ss.find_syntax_by_token(l))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    // syntect's bundled `ThemeSet` ships only 7 themes (base16-*, Solarized,
    // InspiredGitHub). Many UI themes map to names outside that set; fall back
    // to a real bundled dark theme rather than the plain `Theme::default()` so
    // code is always colored (see `theme::syntax::get_active_syntax_theme`).
    let theme_name: &'static str = crate::get_active_syntax_theme();
    let theme = THEME_SET
        .themes
        .get(theme_name)
        .or_else(|| THEME_SET.themes.get("base16-ocean.dark"))
        .cloned()
        .unwrap_or_default();
    #[allow(unused_mut)]
    let mut h = HighlightLines::new(syntax, &theme);
    let mut lines = Vec::new();

    for line in syntect::util::LinesWithEndings::from(code) {
        // Per-line highlight cache: streaming assistants re-render the
        // same code block every frame while tokens trickle in. The
        // wrapped output is keyed by `(lang, line, width, theme)` so a
        // re-emit of an already-highlighted line is a clone instead of
        // a syntect pass. The cache busts when the active syntax theme
        // changes (keyed into the tuple) — see `SYNTECT_LINE_MEMO`.
        let cache_key_lang = lang.unwrap_or("");
        let highlighted: Vec<InlineSegment> =
            if let Some(seg) = memo_get(cache_key_lang, line, width, theme_name) {
                seg
            } else if let Ok(ranges) = h.highlight_line(line, ss) {
                let seg: Vec<InlineSegment> =
                    ranges
                        .into_iter()
                        .map(|(s, t)| {
                            let fg = s.foreground;
                            InlineSegment {
                                text: t.to_string(),
                                style: Arc::new(InlineTextStyle::default().with_color(Some(
                                    AnsiColorEnum::Rgb(RgbColor(fg.r, fg.g, fg.b)),
                                ))),
                            }
                        })
                        .collect();
                memo_put(cache_key_lang, line, width, theme_name, seg.clone());
                seg
            } else {
                vec![InlineSegment {
                    text: line.to_string(),
                    style: Arc::new(InlineTextStyle::default()),
                }]
            };
        if width == 0 {
            lines.push(highlighted);
        } else {
            for wrapped in wrap_segments_to_rows(&highlighted, width) {
                lines.push(wrapped);
            }
        }
    }
    lines
}

/// Soft-wrap a flat segment list into rows that each fit `width` (display
/// cells). Splits a segment mid-text when crossing a boundary so styles
/// stay attached to their content; expands `\t` to four spaces on the way
/// through so a tab character never silently costs one cell and gets
/// pushed off the edge by adjacent chars.
fn wrap_segments_to_rows(segs: &[InlineSegment], width: usize) -> Vec<Vec<InlineSegment>> {
    let mut rows: Vec<Vec<InlineSegment>> = Vec::new();
    let mut cur_row: Vec<InlineSegment> = Vec::new();
    let mut cur_buf = String::new();
    let mut cur_style: Option<Arc<InlineTextStyle>> = None;
    let mut cur_w: usize = 0;

    for seg in segs {
        for ch in seg.text.chars() {
            if ch == '\t' {
                // Pad to the next 4-cell boundary so visual indentation
                // lines up with the source's intent.
                let pad = 4 - (cur_w % 4);
                for _ in 0..pad {
                    if cur_w + 1 > width {
                        flush_wrap_chunk(
                            &mut cur_buf,
                            &mut cur_style,
                            &mut cur_row,
                            &mut rows,
                            &mut cur_w,
                        );
                    }
                    cur_buf.push(' ');
                    if cur_style.is_none() {
                        cur_style = Some(Arc::clone(&seg.style));
                    }
                    cur_w += 1;
                }
                continue;
            }
            let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
            // Zero-width characters: always attach to the current chunk.
            if ch_w == 0 {
                cur_buf.push(ch);
                if cur_style.is_none() {
                    cur_style = Some(Arc::clone(&seg.style));
                }
                continue;
            }
            // Char wider than the row by itself: drop (preferable to a
            // terminal-breaking overflow when the viewport is narrow).
            if ch_w > width {
                continue;
            }
            if cur_w + ch_w > width {
                flush_wrap_chunk(
                    &mut cur_buf,
                    &mut cur_style,
                    &mut cur_row,
                    &mut rows,
                    &mut cur_w,
                );
            }
            cur_buf.push(ch);
            if cur_style.is_none() {
                cur_style = Some(Arc::clone(&seg.style));
            }
            cur_w += ch_w;
        }
    }
    flush_wrap_chunk(
        &mut cur_buf,
        &mut cur_style,
        &mut cur_row,
        &mut rows,
        &mut cur_w,
    );
    if rows.is_empty() {
        rows.push(Vec::new());
    }
    rows
}

/// Flush the in-progress chunk to `cur_row`, and push the row to `rows`
/// when it has content. Resets `cur_w` to 0.
fn flush_wrap_chunk(
    buf: &mut String,
    style: &mut Option<Arc<InlineTextStyle>>,
    row: &mut Vec<InlineSegment>,
    rows: &mut Vec<Vec<InlineSegment>>,
    used: &mut usize,
) {
    if !buf.is_empty()
        && let Some(s) = style.take()
    {
        row.push(InlineSegment {
            text: std::mem::take(buf),
            style: s,
        });
    }
    if !row.is_empty() {
        rows.push(std::mem::take(row));
    }
    *used = 0;
}
/// Render a GFM table with box-drawing borders, fitted to `max_w`.
///
/// Natural column widths come from cell contents; when the table would
/// exceed `max_w`, the widest columns shrink one cell at a time (labels
/// keep their width, prose columns pay) and cell text wraps inside its
/// column, expanding short rows to as many physical lines as their
/// tallest cell needs. The table never exceeds the viewport, so border
/// rows never wrap at the terminal edge.
fn render_table(header: &[String], rows: &[Vec<String>], max_w: usize) -> Vec<Vec<InlineSegment>> {
    let num_cols = std::cmp::max(
        header.len(),
        rows.iter().map(|r| r.len()).max().unwrap_or(0),
    );
    if num_cols == 0 {
        return Vec::new();
    }

    // Natural column widths from display width.
    let mut col_width: Vec<usize> = vec![0; num_cols];
    for (c, cell) in header.iter().enumerate() {
        col_width[c] = std::cmp::max(col_width[c], cell.width());
    }
    for row in rows {
        for (c, cell) in row.iter().enumerate() {
            col_width[c] = std::cmp::max(col_width[c], cell.width());
        }
    }

    // Fit: total = Σ(w + 2) + (n − 1) + 2 border chars = Σw + 3n + 1.
    // The +1 accounts for inter-column separators being n − 1, not n;
    // the old formula (3n) under-budgeted by one cell and the last
    // column silently grew past the viewport on narrow widths.
    let chrome = 3 * num_cols + 1;
    let budget = max_w.saturating_sub(chrome);
    while col_width.iter().sum::<usize>() > budget {
        // Take one cell from the widest column; stop once every column
        // is down to the floor of 1.
        let widest = col_width
            .iter()
            .enumerate()
            .max_by_key(|&(i, w)| (w, std::cmp::Reverse(i)))
            .filter(|&(_, w)| *w > 1)
            .map(|(i, _)| i);
        match widest {
            Some(i) => col_width[i] -= 1,
            None => break,
        }
    }

    let mut out: Vec<Vec<InlineSegment>> = Vec::new();

    let border = |l: &str, j: &str, r: &str| {
        format!(
            "{l}{}{r}",
            col_width
                .iter()
                .map(|w| "─".repeat(w + 2))
                .collect::<Vec<_>>()
                .join(j)
        )
    };

    let plain = |s: String| {
        vec![InlineSegment {
            text: s,
            style: Arc::new(InlineTextStyle::default()),
        }]
    };
    let bold = |s: String| {
        vec![InlineSegment {
            text: s,
            style: Arc::new(InlineTextStyle::default().bold()),
        }]
    };

    out.push(plain(border("┌", "┬", "┐")));

    let mut cell_rows: Vec<(&[String], bool)> = vec![(header, true)];
    cell_rows.extend(rows.iter().map(|r| (r.as_slice(), false)));
    for (cells, is_header) in cell_rows {
        // Wrap every cell to its column width, then emit one physical
        // row per line of the tallest cell.
        let wrapped: Vec<Vec<String>> = col_width
            .iter()
            .enumerate()
            .map(|(c, &w)| wrap_cell(cells.get(c).map(String::as_str).unwrap_or(""), w))
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for line_idx in 0..height {
            let text = format_wrapped_row(&wrapped, line_idx, &col_width);
            let segs = if is_header { bold(text) } else { plain(text) };
            out.push(segs);
        }
        if is_header {
            out.push(plain(border("├", "┼", "┤")));
        }
    }

    out.push(plain(border("└", "┴", "┘")));
    out
}

/// Hard-wrap one cell to its column's display width (CJK-aware).
fn wrap_cell(text: &str, w: usize) -> Vec<String> {
    if w == 0 {
        return vec![String::new()];
    }
    if text.width() <= w {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in text.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + ch_w > w && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += ch_w;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// One physical row line: line `i` of every wrapped cell, padded to the
/// column width so the right border stays aligned under CJK content.
fn format_wrapped_row(wrapped: &[Vec<String>], line_idx: usize, col_width: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(col_width.len());
    for (c, &w) in col_width.iter().enumerate() {
        let text = wrapped
            .get(c)
            .and_then(|lines| lines.get(line_idx))
            .map(String::as_str)
            .unwrap_or("");
        let pad = w.saturating_sub(text.width());
        parts.push(format!(" {text}{} ", " ".repeat(pad)));
    }
    format!("│{}│", parts.join("│"))
}
// ── Helpers ─────────────────────────────────────────────────────────────────

struct CodeBlockState {
    code: String,
    lang: Option<String>,
}

#[derive(Default)]
struct TableState {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    current_cell: String,
    current_row: Vec<String>,
}

struct ListLevel {
    is_ordered: bool,
    index: u64,
}

fn flush_line(cur: &mut Vec<InlineSegment>, lines: &mut Vec<Vec<InlineSegment>>) {
    if !cur.is_empty() {
        lines.push(std::mem::take(cur));
    }
}

fn merge_or_push(cur: &mut Vec<InlineSegment>, seg: InlineSegment) {
    if let Some(last) = cur.last_mut() {
        if last.style == seg.style {
            last.text.push_str(&seg.text);
            return;
        }
    }
    cur.push(seg);
}

fn apply_effects(mut style: InlineTextStyle, effects: Effects) -> InlineTextStyle {
    if effects.contains(Effects::BOLD) {
        style = style.bold();
    }
    if effects.contains(Effects::ITALIC) {
        style = style.italic();
    }
    if effects.contains(Effects::UNDERLINE) {
        style = style.underline();
    }
    if effects.contains(Effects::DIMMED) {
        style = style.dim();
    }
    if effects.contains(Effects::STRIKETHROUGH) {
        style.effects |= Effects::STRIKETHROUGH;
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &[InlineSegment]) -> String {
        line.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn unordered_list_has_markers() {
        let out = render_markdown("- a\n- b\n", 200);
        // Find the lines that contain "a" and "b".
        let combined: Vec<String> = out.iter().map(|l| line_text(l)).collect();
        let line_a = combined
            .iter()
            .find(|l| l.contains('a'))
            .expect("line with 'a'");
        let line_b = combined
            .iter()
            .find(|l| l.contains('b'))
            .expect("line with 'b'");
        assert!(line_a.contains('\u{2022}'), "missing bullet in: {line_a:?}");
        assert!(line_b.contains('\u{2022}'), "missing bullet in: {line_b:?}");
    }

    #[test]
    fn ordered_list_has_numbers() {
        let out = render_markdown("1. first\n2. second\n", 200);
        let combined: Vec<String> = out.iter().map(|l| line_text(l)).collect();
        let has_one = combined
            .iter()
            .any(|l| l.contains("1.") && l.contains("first"));
        let has_two = combined
            .iter()
            .any(|l| l.contains("2.") && l.contains("second"));
        assert!(has_one, "missing '1.' marker in {combined:?}");
        assert!(has_two, "missing '2.' marker in {combined:?}");
    }

    #[test]
    fn table_renders_borders() {
        let md = "| h1 | h2 |\n|----|----|\n| a  | b  |\n| c  | d  |\n";
        let out = render_markdown(md, 200);
        let combined: Vec<String> = out.iter().map(|l| line_text(l)).collect();
        let bar_lines = combined.iter().filter(|l| l.contains('\u{2502}')).count();
        assert!(bar_lines >= 3, "expected ≥3 lines with │, got {combined:?}");
        let has_top_or_bottom = combined
            .iter()
            .any(|l| l.contains('\u{250C}') || l.contains('\u{2514}'));
        assert!(
            has_top_or_bottom,
            "expected ┌ or └ in output, got {combined:?}"
        );
    }

    #[test]
    fn inline_still_works() {
        let out = render_markdown("**bold**", 200);
        let bold_found = out.iter().any(|line| {
            line.iter()
                .any(|seg| seg.style.effects.contains(anstyle::Effects::BOLD))
        });
        assert!(bold_found, "expected BOLD effect in rendered segments");
    }
    #[test]
    fn table_cell_keeps_inline_code() {
        // Inline code (backticks) arrives as Event::Code, not Event::Text — the
        // table router must capture it or the cell renders blank.
        let md = "| type | example |\n|------|----------|\n| foo  | `bar`    |\n";
        let out = render_markdown(md, 200);
        let joined: String = out
            .iter()
            .map(|l| line_text(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("bar"),
            "inline code `bar` dropped from table cell: {joined:?}"
        );
    }

    #[test]
    fn table_cjk_columns_align() {
        // Wide chars (CJK) have display width 2; padding must use display width
        // so every data row has the same width and the │ borders line up.
        let md = "| a | b  |\n|---|----|\n| 中 | x  |\n| 1 | yy |\n";
        let out = render_markdown(md, 200);
        let rows: Vec<String> = out
            .iter()
            .map(|l| line_text(l))
            .filter(|l| l.starts_with('\u{2502}'))
            .collect();
        let widths: Vec<usize> = rows
            .iter()
            .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
            .collect();
        let first = widths[0];
        assert!(
            widths.iter().all(|&w| w == first),
            "CJK column misalignment — row display widths differ: {widths:?}\n{rows:?}"
        );
    }

    #[test]
    fn table_fits_the_given_width_and_wraps_cells() {
        // — natural width overflows — must shrink to fit. Border rows
        // never exceed the width, and the long cell wraps inside its
        // column instead of breaking the table.
        let md = "\
| colA | colB |\n\
|------|------|\n\
| alpha | xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx |\n";
        let width = 30usize;
        let out = render_markdown(md, width);
        let rows: Vec<String> = out.iter().map(|l| line_text(l)).collect();
        assert!(!rows.is_empty(), "table produced no rows");
        for (i, row) in rows.iter().enumerate() {
            let w = unicode_width::UnicodeWidthStr::width(row.as_str());
            assert!(w <= width, "row {i} overflows: {w} > {width}\n{row}");
        }
        let borders: Vec<usize> = rows
            .iter()
            .filter(|r| r.starts_with('┌') || r.starts_with('├') || r.starts_with('└'))
            .map(|r| unicode_width::UnicodeWidthStr::width(r.as_str()))
            .collect();
        assert_eq!(borders.len(), 3, "top/mid/bottom borders");
        assert!(
            borders.iter().all(|&w| w == borders[0]),
            "border widths differ: {borders:?}"
        );
        // The long cell must have wrapped into multiple physical rows.
        let data_rows = rows.iter().filter(|r| r.starts_with('│')).count();
        assert!(
            data_rows > 1,
            "the long cell should wrap to multiple rows, got {data_rows}\n{rows:?}"
        );
    }

    #[test]
    fn table_narrower_than_viewport_keeps_natural_width() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = render_markdown(md, 200);
        let top = out
            .iter()
            .map(|l| line_text(l))
            .find(|l| l.starts_with('┌'))
            .expect("top border");
        assert!(
            unicode_width::UnicodeWidthStr::width(top.as_str()) <= 200,
            "natural width exceeds viewport"
        );
    }
    #[test]
    fn code_block_hard_wraps_to_given_width() {
        // 200-char ASCII line inside a ``` fence — must wrap to the
        // requested width and never overflow it. Concatenated row text
        // contains the full original line so the wrap is lossless.
        let line: String = "a".repeat(200);
        let md = format!("```\n{line}\n```\n");
        let out = render_markdown(&md, 80);
        let rows: Vec<String> = out.iter().map(|l| line_text(l)).collect();
        assert!(!rows.is_empty(), "code block produced no rows");
        for (i, row) in rows.iter().enumerate() {
            let w = unicode_width::UnicodeWidthStr::width(row.as_str());
            assert!(w <= 80, "code row {i} overflows: {w} > 80\n{row}");
        }
        let joined: String = rows.join("");
        assert!(
            joined.contains(&line),
            "concatenated code rows must contain the full original line\njoined={joined:?}\nwant={line:?}"
        );
    }

    #[test]
    fn code_block_width_zero_keeps_natural_lines() {
        // width == 0 preserves the old "no wrap" behavior so existing
        // callers (tests, internal markdown channels) keep working.
        let line: String = "z".repeat(40);
        let md = format!("```\n{line}\n```\n");
        let out = render_markdown(&md, 0);
        let rows: Vec<String> = out.iter().map(|l| line_text(l)).collect();
        assert!(
            rows.iter().any(|r| r.contains(&line)),
            "width 0 must preserve natural-length lines, got {rows:?}"
        );
    }

    // ── T6: incremental streaming markdown render cache ───────────────────

    /// Flatten a rendered line/segment list to a row of plain text
    /// so we can compare two renderings for equality on `text` alone
    /// without depending on `InlineSegment: PartialEq` (which is not
    /// derived on the protocol type — see
    /// `oxicode_vtui_compat::ui_protocol::style::InlineSegment`).
    fn flatten_lines(lines: &[Vec<InlineSegment>]) -> Vec<String> {
        lines.iter().map(|l| line_text(l)).collect()
    }

    #[test]
    fn cached_prefix_reuses_lines() {
        // First render populates the cache (cold).
        let mut cache = MdRenderCache::default();
        let _ = render_markdown_cached("hello", 80, &mut cache);
        assert_eq!(
            cache.debug_hits(),
            0,
            "first call is a cold render (no cache hit)"
        );

        // Identical (text, width) pair → fast-path hit; the hit counter
        // increments and the returned lines equal the cold render.
        let again = render_markdown_cached("hello", 80, &mut cache);
        assert_eq!(
            cache.debug_hits(),
            1,
            "identical input must register a cache hit"
        );
        assert_eq!(
            flatten_lines(&again),
            flatten_lines(&render_markdown("hello", 80)),
            "fast-path output equals fresh render"
        );

        // Append tokens (streaming assistant flow) → cache updates, no
        // hit; a third identical call again hits the cache.
        let _ = render_markdown_cached("hello world", 80, &mut cache);
        assert_eq!(
            cache.debug_hits(),
            1,
            "different text does not register a hit"
        );
        let _ = render_markdown_cached("hello world", 80, &mut cache);
        assert_eq!(
            cache.debug_hits(),
            2,
            "second identical call after a miss must hit again"
        );
    }

    #[test]
    fn cached_result_equals_fresh_render() {
        // Property: for a streaming assistant flow that appends tokens
        // across 5 frames, every cached output equals the fresh render
        // for the same (text, width). The cache must never produce
        // divergent lines from the source renderer.
        let mut cache = MdRenderCache::default();
        let base = "The quick brown fox jumps over the lazy dog.";
        let appends = ["", " Stream chunk one.", " More.", " Even more."];
        let mut text = base.to_string();
        let width = 40usize;
        for (i, suffix) in std::iter::once("")
            .chain(appends.iter().copied())
            .enumerate()
        {
            if i > 0 {
                text.push_str(suffix);
            }
            let cached = render_markdown_cached(&text, width, &mut cache);
            let fresh = render_markdown(&text, width);
            assert_eq!(
                flatten_lines(&cached),
                flatten_lines(&fresh),
                "cached output diverges from fresh render at step {i}: text={text:?}"
            );
        }
    }

    #[test]
    fn width_change_busts_cache() {
        // First render at width 80.
        let mut cache = MdRenderCache::default();
        let _baseline = render_markdown_cached("# title\n\nbody", 80, &mut cache);
        assert_eq!(cache.debug_hits(), 0);

        // Same text at width 40 must NOT hit — the wrapped output
        // differs and the cache must invalidate.
        let narrowed = render_markdown_cached("# title\n\nbody", 40, &mut cache);
        assert_eq!(
            cache.debug_hits(),
            0,
            "width change must NOT register a fast-path hit"
        );
        assert_eq!(
            flatten_lines(&narrowed),
            flatten_lines(&render_markdown("# title\n\nbody", 40)),
            "narrowed output equals fresh render"
        );

        // Same text at the original width 80 — first time after the
        // bust, this is again a miss; second identical call hits.
        let _ = render_markdown_cached("# title\n\nbody", 80, &mut cache);
        assert_eq!(cache.debug_hits(), 0, "miss after width bust");
        let _ = render_markdown_cached("# title\n\nbody", 80, &mut cache);
        assert_eq!(
            cache.debug_hits(),
            1,
            "subsequent identical input must hit again"
        );
    }
}
