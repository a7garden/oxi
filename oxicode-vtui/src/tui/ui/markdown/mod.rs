//! Minimal markdown → InlineSegment renderer with full inline styling.
use anstyle::{Color as AnsiColorEnum, Effects, RgbColor};
use oxicode_vtui_compat::ui_protocol::{InlineSegment, InlineTextStyle};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::sync::{Arc, LazyLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use unicode_width::UnicodeWidthStr;

// Cached once at module scope — never rebuild per call.
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// Parse markdown text into styled InlineSegment lines.
pub fn render_markdown(text: &str) -> Vec<Vec<InlineSegment>> {
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
                    // Header row was already pushed into `rows` on End(TableRow);
                    // move it out into `header` and clear rows for the body.
                    if !tb.rows.is_empty() && tb.header.is_empty() {
                        tb.header = tb.rows.remove(0);
                    }
                }
                Event::End(TagEnd::Table) => {
                    let tb = table_buf.take().unwrap();
                    let table_lines = render_table(&tb.header, &tb.rows);
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
                    let block_lines = render_code_block(&cb.code, cb.lang.as_deref(), ss);
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
pub fn render_code_block(
    code: &str,
    lang: Option<&str>,
    ss: &SyntaxSet,
) -> Vec<Vec<InlineSegment>> {
    let syntax = lang
        .and_then(|l| ss.find_syntax_by_token(l))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    // syntect's bundled `ThemeSet` ships only 7 themes (base16-*, Solarized,
    // InspiredGitHub). Many UI themes map to names outside that set; fall back
    // to a real bundled dark theme rather than the plain `Theme::default()` so
    // code is always colored (see `theme::syntax::get_active_syntax_theme`).
    let theme = THEME_SET
        .themes
        .get(crate::get_active_syntax_theme())
        .or_else(|| THEME_SET.themes.get("base16-ocean.dark"))
        .cloned()
        .unwrap_or_default();
    #[allow(unused_mut)]
    let mut h = HighlightLines::new(syntax, &theme);
    let mut lines = Vec::new();

    for line in syntect::util::LinesWithEndings::from(code) {
        if let Ok(ranges) = h.highlight_line(line, ss) {
            let segs: Vec<InlineSegment> = ranges
                .into_iter()
                .map(|(s, t)| {
                    let fg = s.foreground;
                    InlineSegment {
                        text: t.to_string(),
                        style: Arc::new(
                            InlineTextStyle::default()
                                .with_color(Some(AnsiColorEnum::Rgb(RgbColor(fg.r, fg.g, fg.b)))),
                        ),
                    }
                })
                .collect();
            lines.push(segs);
        } else {
            lines.push(vec![InlineSegment {
                text: line.to_string(),
                style: Arc::new(InlineTextStyle::default()),
            }]);
        }
    }
    lines
}

/// Render a GFM table with box-drawing borders and natural column widths.
fn render_table(header: &[String], rows: &[Vec<String>]) -> Vec<Vec<InlineSegment>> {
    let num_cols = std::cmp::max(
        header.len(),
        rows.iter().map(|r| r.len()).max().unwrap_or(0),
    );
    if num_cols == 0 {
        return Vec::new();
    }

    // Compute natural column widths from display width.
    let mut col_width: Vec<usize> = vec![0; num_cols];
    for (c, cell) in header.iter().enumerate() {
        col_width[c] = std::cmp::max(col_width[c], cell.width());
    }
    for row in rows {
        for (c, cell) in row.iter().enumerate() {
            col_width[c] = std::cmp::max(col_width[c], cell.width());
        }
    }

    let mut out: Vec<Vec<InlineSegment>> = Vec::new();

    // Border builders
    let top = format!(
        "┌{}┐",
        col_width
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┬")
    );
    let mid = format!(
        "├{}┤",
        col_width
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┼")
    );
    let bot = format!(
        "└{}┘",
        col_width
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┴")
    );

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

    out.push(plain(top));

    // Header row
    let header_line = format_row(header, &col_width, num_cols);
    out.push(bold(header_line));
    out.push(plain(mid));

    // Body rows
    for row in rows {
        out.push(plain(format_row(row, &col_width, num_cols)));
    }

    out.push(plain(bot));
    out
}

fn format_row(cells: &[String], col_width: &[usize], num_cols: usize) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(num_cols);
    for (c, &w) in col_width.iter().enumerate() {
        let text = cells.get(c).map(String::as_str).unwrap_or("");
        // Pad to display width `w` (not scalar count) so CJK / wide chars keep
        // columns aligned — `{:<width$}` pads by char count and misaligns them.
        let pad = w.saturating_sub(text.width());
        parts.push(format!(" {}{} ", text, " ".repeat(pad)));
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
        let out = render_markdown("- a\n- b\n");
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
        let out = render_markdown("1. first\n2. second\n");
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
        let out = render_markdown(md);
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
        let out = render_markdown("**bold**");
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
        let out = render_markdown(md);
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
        let out = render_markdown(md);
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
}
