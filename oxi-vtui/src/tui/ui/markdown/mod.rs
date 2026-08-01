//! Minimal markdown → InlineSegment renderer with full inline styling.
use anstyle::{Color as AnsiColorEnum, Effects, RgbColor};
use oxi_vtui_compat::ui_protocol::{InlineSegment, InlineTextStyle};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::sync::Arc;
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxSet;

/// Parse markdown text into styled InlineSegment lines.
pub fn render_markdown(text: &str) -> Vec<Vec<InlineSegment>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    let mut lines: Vec<Vec<InlineSegment>> = Vec::new();
    let mut cur: Vec<InlineSegment> = Vec::new();
    let effects: Effects = Effects::default();

    let mut code_buf: Option<CodeBlockState> = None;
    let ss = SyntaxSet::load_defaults_newlines();

    for event in Parser::new_ext(text, opts) {
        // Code block capture mode
        if let Some(ref mut cb) = code_buf {
            match event {
                Event::Text(t) => cb.code.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let cb = code_buf.take().unwrap();
                    flush_line(&mut cur, &mut lines);
                    let block_lines = render_code_block(&cb.code, cb.lang.as_deref(), &ss);
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

            // ── Block-level ────────────────────────────────────────────
            Event::Start(Tag::Paragraph) => {}

            Event::Start(Tag::Heading { level, .. }) => {
                flush_line(&mut cur, &mut lines);
                {
                    let _ = effects.insert(Effects::BOLD);
                };
                {
                    let _ = effects.insert(if level == HeadingLevel::H1 {
                        Effects::UNDERLINE
                    } else {
                        Effects::default()
                    });
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                {
                    let _ = effects.remove(Effects::BOLD | Effects::UNDERLINE);
                };
                flush_line(&mut cur, &mut lines);
            }

            Event::Start(Tag::BlockQuote(_)) => {
                {
                    let _ = effects.insert(Effects::DIMMED);
                };
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                {
                    let _ = effects.remove(Effects::DIMMED);
                };
            }

            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => {
                flush_line(&mut cur, &mut lines);
            }
            Event::End(TagEnd::List(_)) => {
                lines.push(Vec::new());
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
                    let _ = effects.insert(Effects::ITALIC);
                };
            }
            Event::End(TagEnd::Emphasis) => {
                {
                    let _ = effects.remove(Effects::ITALIC);
                };
            }

            Event::Start(Tag::Strong) => {
                {
                    let _ = effects.insert(Effects::BOLD);
                };
            }
            Event::End(TagEnd::Strong) => {
                {
                    let _ = effects.remove(Effects::BOLD);
                };
            }

            Event::Start(Tag::Strikethrough) => {
                {
                    let _ = effects.insert(Effects::STRIKETHROUGH);
                };
            }
            Event::End(TagEnd::Strikethrough) => {
                {
                    let _ = effects.remove(Effects::STRIKETHROUGH);
                };
            }

            Event::Start(Tag::Link { .. }) => {
                {
                    let _ = effects.insert(Effects::UNDERLINE);
                };
            }
            Event::End(TagEnd::Link) => {
                {
                    let _ = effects.remove(Effects::UNDERLINE);
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
    let theme = syntect::highlighting::Theme::default();
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

// ── Helpers ─────────────────────────────────────────────────────────────────

struct CodeBlockState {
    code: String,
    lang: Option<String>,
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
