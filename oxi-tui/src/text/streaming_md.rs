//! Streaming markdown renderer with checkpoint optimization.
//!
//! Stable content behind a paragraph break or closed fenced code block is
//! parsed once into styled blocks. Only the unstable tail is parsed again
//! while a response is streaming. Rendering at any width wraps each cached
//! block on demand, so the cache is not bound to a particular column count.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::Line,
};

use crate::{text::wrap::wrap_lines, theme::Theme};

#[derive(Clone, Copy, Debug)]
enum BlockKind {
    Normal,
    Heading,
    Code,
}
#[derive(Clone, Debug)]
struct ParsedBlock {
    kind: BlockKind,
    text: String,
}

/// Incremental markdown state for streaming assistant responses.
#[derive(Debug, Default)]
pub struct StreamingMarkdown {
    /// Frozen, parsed blocks from content behind the last checkpoint.
    frozen_blocks: Vec<ParsedBlock>,
    /// Byte offset in the complete input where frozen content ends.
    checkpoint: usize,
    /// Accumulated text since the last checkpoint.
    pending_tail: String,
    /// Complete accumulated input, retained for width invalidation.
    full_text: String,
}

impl StreamingMarkdown {
    /// Create an empty streaming renderer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one streamed token and freeze any newly stable prefix.
    pub fn push_token(&mut self, token: &str) {
        self.pending_tail.push_str(token);
        self.full_text.push_str(token);
        self.advance_checkpoint();
    }

    /// Scan the tail for the last stable markdown boundary.
    fn advance_checkpoint(&mut self) {
        let paragraph_boundary = self.pending_tail.rfind("\n\n").map(|offset| offset + 2);
        let code_boundary = last_closed_fence(&self.pending_tail);
        let Some(boundary) = paragraph_boundary.max(code_boundary) else {
            return;
        };

        let stable = self.pending_tail[..boundary].to_owned();
        self.pending_tail.drain(..boundary);
        self.checkpoint += boundary;
        self.frozen_blocks.extend(parse_markdown_blocks(&stable));
    }

    /// Return frozen lines followed by freshly parsed tail lines.
    #[must_use]
    pub fn lines(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        let mut blocks = self.frozen_blocks.clone();
        blocks.extend(parse_markdown_blocks(&self.pending_tail));
        render_blocks(&blocks, width, theme)
    }

    /// Discard cached blocks so the complete response can be parsed afresh.
    pub fn invalidate(&mut self) {
        let full = std::mem::take(&mut self.full_text);
        self.frozen_blocks.clear();
        self.pending_tail = full;
        self.checkpoint = 0;
    }
}

/// Return the byte offset just after the last complete fenced code block.
fn last_closed_fence(text: &str) -> Option<usize> {
    let mut in_block = false;
    let mut last_close = None;
    let mut search_from = 0;

    while let Some(relative) = text[search_from..].find("```") {
        let offset = search_from + relative;
        let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
        if text[line_start..offset].trim().is_empty() {
            if in_block {
                last_close = Some(offset + 3);
                in_block = false;
            } else {
                in_block = true;
            }
        }
        search_from = offset + 3;
    }
    last_close
}

/// Parse a markdown prefix into a sequence of styled blocks.
fn parse_markdown_blocks(md: &str) -> Vec<ParsedBlock> {
    let mut blocks: Vec<ParsedBlock> = Vec::new();
    let mut current: Option<ParsedBlock> = None;
    let mut list_depth = 0_u16;

    for event in Parser::new_ext(md, Options::all()) {
        match event {
            Event::Start(Tag::Paragraph) => {
                if current.is_none() {
                    current = Some(ParsedBlock {
                        kind: BlockKind::Normal,
                        text: String::new(),
                    });
                }
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock) => {
                flush_block(&mut current, &mut blocks);
            }
            Event::Start(Tag::Heading { .. }) => {
                flush_block(&mut current, &mut blocks);
                current = Some(ParsedBlock {
                    kind: BlockKind::Heading,
                    text: String::new(),
                });
            }
            Event::Start(Tag::CodeBlock(_)) => {
                flush_block(&mut current, &mut blocks);
                current = Some(ParsedBlock {
                    kind: BlockKind::Code,
                    text: String::new(),
                });
            }
            Event::Start(Tag::Item) => {
                flush_block(&mut current, &mut blocks);
                list_depth = list_depth.saturating_add(1);
                current = Some(ParsedBlock {
                    kind: BlockKind::Normal,
                    text: format!("{}- ", "  ".repeat(list_depth as usize - 1)),
                });
            }
            Event::End(TagEnd::Item) => {
                flush_block(&mut current, &mut blocks);
                list_depth = list_depth.saturating_sub(1);
            }
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => {
                if let Some(block) = current.as_mut() {
                    block.text.push_str(&text);
                } else {
                    current = Some(ParsedBlock {
                        kind: BlockKind::Normal,
                        text: text.into_string(),
                    });
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(block) = current.as_mut() {
                    block.text.push('\n');
                }
            }
            _ => {}
        }
    }
    flush_block(&mut current, &mut blocks);
    blocks
}

fn flush_block(current: &mut Option<ParsedBlock>, blocks: &mut Vec<ParsedBlock>) {
    if let Some(block) = current.take()
        && (!block.text.is_empty() || blocks.is_empty())
    {
        blocks.push(block);
    }
}

fn block_style(kind: BlockKind, theme: &Theme) -> Style {
    match kind {
        BlockKind::Normal => theme.styles.normal,
        BlockKind::Heading => theme.styles.primary.add_modifier(Modifier::BOLD),
        BlockKind::Code => theme.styles.normal.patch(theme.styles.code_bg),
    }
}

/// Wrap each cached block at the requested width, returning the final lines.
fn render_blocks(blocks: &[ParsedBlock], width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        let style = block_style(block.kind, theme);
        lines.extend(
            wrap_lines(&block.text, width as usize)
                .into_iter()
                .map(|line| line.style(style)),
        );
    }
    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    fn text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn push_token_accumulates() {
        let mut markdown = StreamingMarkdown::new();
        markdown.push_token("hello");
        markdown.push_token(" world");
        assert_eq!(markdown.full_text, "hello world");
        assert_eq!(markdown.pending_tail, "hello world");
    }

    #[test]
    fn paragraph_break_advances_checkpoint() {
        let mut markdown = StreamingMarkdown::new();
        markdown.push_token("one\n\ntwo");
        assert_eq!(markdown.checkpoint, 5);
        assert_eq!(markdown.pending_tail, "two");
        assert!(!markdown.frozen_blocks.is_empty());
    }

    #[test]
    fn closed_code_block_advances_checkpoint() {
        let mut markdown = StreamingMarkdown::new();
        markdown.push_token("```rust\nlet x = 1;\n```tail");
        assert_eq!(markdown.checkpoint, "```rust\nlet x = 1;\n```".len());
        assert_eq!(markdown.pending_tail, "tail");
    }

    #[test]
    fn lines_returns_frozen_plus_tail() {
        let mut markdown = StreamingMarkdown::new();
        markdown.push_token("first\n\nsecond");
        let lines = markdown.lines(80, &Theme::default());
        assert_eq!(text(&lines), "first||second");
    }

    #[test]
    fn invalidate_clears_frozen() {
        let mut markdown = StreamingMarkdown::new();
        markdown.push_token("first\n\nsecond");
        markdown.invalidate();
        assert!(markdown.frozen_blocks.is_empty());
        assert_eq!(markdown.checkpoint, 0);
        assert_eq!(markdown.pending_tail, "first\n\nsecond");
    }

    #[test]
    fn lines_adapts_to_narrow_width() {
        let mut markdown = StreamingMarkdown::new();
        markdown.push_token("alpha\n\nbeta gamma delta");
        let lines = markdown.lines(5, &Theme::default());
        let text = text(&lines);
        // 5 columns forces wrapping; no line should exceed 5 visible cols.
        for line in &lines {
            for span in &line.spans {
                assert!(
                    unicode_width::UnicodeWidthStr::width(span.content.as_ref()) <= 5,
                    "line overflowed 5 columns: {span:?}",
                );
            }
        }
        assert!(text.starts_with("alpha|"));
        assert!(text.contains("beta"));
    }
}
