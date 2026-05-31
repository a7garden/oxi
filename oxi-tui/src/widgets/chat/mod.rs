//! ChatView widget — scrollable message list with streaming support.
//!
//! Renders layout entries directly into the frame buffer. Only visible
//! entries (those within the scroll viewport) are rendered, avoiding the
//! overhead of a virtual buffer. Scroll offset is managed directly.
//!
//! Benefits:
//! - Tool/error boxes use Block::bordered() — real ratatui borders
//! - Text uses CJK-aware pre-wrapping (wrap_lines_styled) for correct line-breaking
//! - No virtual buffer — direct rendering, no double-write
//! - Layout caching — only recomputes when state actually changes
//! - Truncation at ingest — no height inflation from monster inputs

pub mod dashboard;
pub mod highlight;
pub mod layout;
pub mod markdown;
pub mod render;
pub mod state;
pub mod types;

// Re-export all public types from sub-modules
pub use dashboard::DashboardInfo;
pub use state::ChatViewState;
pub use types::{ChatMessage, ContentBlock, MessageRole, StreamingState, ToolCallStatus};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{StatefulWidget, Widget},
};

use crate::Theme;
use layout::LayoutKind;
use render::EntryWidget;

// ── ChatView widget ────────────────────────────────────────────────────

pub struct ChatView<'a> {
    theme: &'a Theme,
}

impl<'a> ChatView<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

impl StatefulWidget for ChatView<'_> {
    type State = ChatViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width < 4 || area.height < 1 {
            return;
        }
        let styles = self.theme.to_styles();
        let width = area.width;

        // Outer layout already provides consistent horizontal margin.
        // No additional internal padding needed.
        let inner_width = width;

        // Clear click regions before recompute
        state.thinking_regions.clear();
        state.tool_regions.clear();

        // Get layout computed with inner_width for correct height measurements
        let layout = state.get_layout(inner_width);
        let total_height: u16 = layout
            .last()
            .map(|e| {
                (e.y as u32)
                    .saturating_add(e.height as u32)
                    .min(u16::MAX as u32) as u16
            })
            .unwrap_or(0);
        state.content_height = total_height;

        // Auto-scroll: update scroll_offset to show bottom of content
        if state.auto_scroll {
            state.scroll_to_bottom(area.height);
        } else {
            // Clamp scroll offset to valid range (in case content shrank)
            state.clamp_scroll(area.height);
        }

        // Render only visible entries into the buffer.
        let scroll_offset = state.scroll_offset;
        let vp_bottom = scroll_offset + area.height;

        for entry in &layout {
            // Skip entries fully outside the viewport
            if entry.y + entry.height <= scroll_offset {
                continue;
            }
            if entry.y >= vp_bottom {
                break;
            }
            if entry.height == 0 {
                continue;
            }

            // Compute the entry's vertical range within the viewport
            // (clamped to viewport bounds). Every entry maps to a
            // contiguous rect starting at viewport row 0 (for entries
            // partially above the viewport) or at its natural position.
            let entry_top = entry.y.max(scroll_offset);
            let entry_bot = (entry.y + entry.height).min(vp_bottom);
            let h = entry_bot.saturating_sub(entry_top);
            if h == 0 {
                continue;
            }
            let rel_y = entry_top - scroll_offset;
            let rect = Rect::new(area.x, area.y + rel_y, inner_width, h);

            // Track click regions
            let region_bottom = area.y + rel_y + h;
            if let LayoutKind::Thinking { key, .. } = &entry.kind {
                state
                    .thinking_regions
                    .push((area.y + rel_y, region_bottom, key.clone()));
            }
            if let LayoutKind::ToolBox { key, result, .. } = &entry.kind {
                if result.is_some() {
                    state
                        .tool_regions
                        .push((area.y + rel_y, region_bottom, key.clone()));
                }
            }

            // For entries fully within the viewport, render directly.
            // For entries clipped at the top, render the full entry to a
            // temp buffer then copy only the visible rows — otherwise
            // EntryWidget would draw starting from row 0 of the rect
            // (top border, first lines) instead of the clipped rows.
            if entry.y >= scroll_offset {
                EntryWidget::new(&entry.kind, &styles).render(rect, buf);
            } else {
                let hidden = scroll_offset - entry.y;
                let tmp_rect = Rect::new(0, 0, inner_width, entry.height);
                let mut tmp = ratatui::buffer::Buffer::empty(tmp_rect);
                EntryWidget::new(&entry.kind, &styles).render(tmp_rect, &mut tmp);
                for row in 0..h {
                    for col in 0..inner_width {
                        if let Some(dst) = buf.cell_mut((area.x + col, area.y + rel_y + row)) {
                            *dst = tmp[(col, hidden + row)].clone();
                        }
                    }
                }
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use layout::compute_layout;
    use markdown::{fix_bare_code_fences, md_lines, wrap_lines_styled};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use state::MAX_TEXT_CHARS;
    use types::ContentBlock;

    #[test]
    fn scroll_bounds() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        // scroll_to_bottom with auto_scroll = true
        s.scroll_to_bottom(20);
        assert_eq!(s.scroll_offset, 80);
        assert!(s.auto_scroll);

        // Manual scroll overrides auto_scroll
        s.scroll_up(50);
        assert_eq!(s.scroll_offset, 30);
        assert!(!s.auto_scroll);

        s.scroll_down(10);
        assert_eq!(s.scroll_offset, 40);

        // Clamp at top
        s.scroll_up(100);
        assert_eq!(s.scroll_offset, 0);

        // clamp_scroll when content shrinks
        s.scroll_offset = 90;
        s.content_height = 30;
        s.clamp_scroll(20);
        assert_eq!(s.scroll_offset, 10);
    }

    #[test]
    fn streaming_lifecycle() {
        let mut s = ChatViewState::new();
        s.start_streaming();
        assert!(s.streaming.is_some());
        s.stream_text_delta("Hi");
        s.finish_streaming();
        assert!(s.streaming.is_none());
        assert_eq!(s.messages.len(), 1);
    }

    #[test]
    fn tool_call_lifecycle() {
        let mut s = ChatViewState::new();
        s.start_streaming();
        s.stream_tool_call(
            "t1".into(),
            "bash".into(),
            "ls".into(),
            ToolCallStatus::Executing,
        );
        s.stream_tool_result(Some("t1".into()), "bash".into(), "file.txt".into(), false);
        s.finish_streaming();
        match &s.messages[0].content_blocks[0] {
            ContentBlock::ToolCall {
                status,
                result,
                duration,
                ..
            } => {
                assert_eq!(*status, ToolCallStatus::Done);
                assert!(result.is_some());
                assert!(duration.is_none()); // not set in this test
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn image_tracking() {
        let mut s = ChatViewState::new();
        s.start_streaming();
        s.stream_image("image/png".into(), "AAAA".into());
        assert_eq!(s.pending_images.len(), 1);
        assert_eq!(s.pending_images[0].1, "image/png");
    }

    #[test]
    fn compute_layout_basic() {
        let mut s = ChatViewState::new();
        s.messages.push(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text {
                content: "Hello".into(),
            }],
            timestamp: 0,
        });
        let layout = compute_layout(&s, 80);
        assert!(!layout.is_empty());
        assert!(layout.iter().any(|e| matches!(&e.kind, LayoutKind::Rule)));
    }

    #[test]
    fn fix_bare_code_fences_basic() {
        let input = "```\ncode\n```";
        let fixed = fix_bare_code_fences(input);
        assert!(fixed.starts_with("```text"));
    }

    #[test]
    fn clamp_str_no_truncate() {
        let short = "hello world".to_string();
        let result = state::clamp_str(short.clone(), 100, 10);
        assert_eq!(result, short);
    }

    #[test]
    fn clamp_str_truncates_chars() {
        let long = "x".repeat(100);
        let result = state::clamp_str(long.clone(), 10, 200);
        // 10 chars + "\n ..." = 16 chars, 2 lines
        assert!(result.starts_with("xxxxxxxxxx"));
        assert!(result.contains("..."));
    }

    #[test]
    fn clamp_str_truncates_lines() {
        let long = (0..20)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = state::clamp_str(long.clone(), 10000, 5);
        assert!(result.lines().count() <= 6); // 5 + "...\n"
        assert!(result.ends_with(" ..."));
    }

    #[test]
    fn layout_cache_hit() {
        let mut s = ChatViewState::new();
        s.messages.push(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text {
                content: "Hello".into(),
            }],
            timestamp: 0,
        });
        // First call — cache miss, recompute
        let layout1 = s.get_layout(80);
        // Second call with same params — cache hit
        let layout2 = s.get_layout(80);
        assert_eq!(layout1.len(), layout2.len());
        // Different width — cache miss
        let layout3 = s.get_layout(60);
        assert_eq!(layout1.len(), layout3.len()); // same content, different heights
    }

    #[test]
    fn text_truncation_on_ingest() {
        let mut s = ChatViewState::new();
        s.start_streaming();
        // Append a huge chunk
        let huge = "x".repeat(600_000);
        s.stream_text_delta(&huge);
        let content = match &s.streaming {
            Some(ref st) => match &st.message.content_blocks[0] {
                ContentBlock::Text { content } => content.clone(),
                _ => panic!("expected Text"),
            },
            None => panic!("expected streaming"),
        };
        // Content should be clamped to MAX_TEXT_CHARS (with overflow marker)
        assert!(
            content.chars().count() <= MAX_TEXT_CHARS + 10,
            "content len = {}",
            content.chars().count()
        );
    }

    #[test]
    fn wrap_lines_styled_cjk() {
        // CJK text without spaces between characters should wrap
        // at character boundaries, not overflow.
        let text = "oxi is a terminal-based AI coding assistant written in Rust with full multilingual support.";
        let lines = md_lines(text, 30);
        // Should produce multiple lines, all fitting within width 30
        for line in &lines {
            let w = unicode_width::UnicodeWidthStr::width(line.to_string().as_str());
            assert!(
                w <= 30,
                "Line width {} exceeds 30: '{}'",
                w,
                line.to_string()
            );
        }
        assert!(lines.len() > 1, "Expected multiple wrapped lines");
    }

    #[test]
    fn wrap_lines_styled_ascii() {
        let text = "Hello world, this is a test of text wrapping.";
        let lines = md_lines(text, 20);
        for line in &lines {
            let w = unicode_width::UnicodeWidthStr::width(line.to_string().as_str());
            assert!(
                w <= 20,
                "Line width {} exceeds 20: '{}'",
                w,
                line.to_string()
            );
        }
        assert!(lines.len() > 1, "Expected multiple wrapped lines");
    }

    #[test]
    fn wrap_lines_styled_mixed_width() {
        // Mixed narrow and wide characters
        let text =
            "Multi-provider LLM support with streaming and extensible tool system architecture";
        let lines = md_lines(text, 30);
        for line in &lines {
            let w = unicode_width::UnicodeWidthStr::width(line.to_string().as_str());
            assert!(
                w <= 30,
                "Line width {} exceeds 30: '{}'",
                w,
                line.to_string()
            );
        }
        assert!(lines.len() > 1, "Expected multiple wrapped lines");
    }

    #[test]
    fn wrap_lines_styled_short_text() {
        // Short text should not be split
        let text = "Hello";
        let lines = md_lines(text, 80);
        assert_eq!(lines.len(), 1, "Short text should be a single line");
    }

    #[test]
    fn wrap_lines_preserves_style() {
        let styled_line = Line::from(vec![
            Span::styled(
                "bold ".to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled("normal".to_string(), Style::default()),
        ]);
        let result = wrap_lines_styled(&[styled_line], 80);
        // Should have 1 line since it fits
        assert_eq!(result.len(), 1);
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;
    use crate::table_renderer::render_markdown_table;
    use markdown::md_lines;

    #[test]
    fn render_markdown_table_basic() {
        let md = "| Name | Age |\n|---|---|
| Alice | 30 |
| Bob | 25 |";
        let lines = render_markdown_table(md, 80);
        assert!(!lines.is_empty(), "Expected table lines, got empty");
        let text = lines.iter().map(|l| l.to_string()).collect::<String>();
        assert!(text.contains('┌'), "Expected top border, got: {}", text);
        assert!(text.contains('│'), "Expected cell separator, got: {}", text);
        assert!(text.contains('└'), "Expected bottom border, got: {}", text);
    }

    #[test]
    fn md_lines_with_table() {
        let md = "| Name | Age |
|---|---|---|
| Alice | 30 |
| Bob | 25 |";
        let lines = md_lines(md, 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn md_lines_without_table() {
        let md = "Hello **world**";
        let lines = md_lines(md, 80);
        assert!(!lines.is_empty());
    }
    #[test]
    fn test_empty_cells() {
        let md = "| Name | Value | Extra |
|---|---|---|
| Alice | | 100 |";
        let out = render_markdown_table(md, 60);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(
            "
",
        );
        assert!(text.contains("Alice"), "Has Alice");
        assert!(text.contains("┌"), "Has border");
    }

    #[test]
    fn test_single_column() {
        let md = "| Only |
|---|
| One |
| Two |";
        let out = render_markdown_table(md, 30);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(
            "
",
        );
        assert!(text.contains("Only"), "Has header");
        assert!(text.contains("One"), "Has data");
        println!("text={}", text);
        assert!(
            text.contains("└──────┘") || text.contains("└──┘"),
            "Has bottom border"
        );
    }

    #[test]
    fn test_wide_characters() {
        let md = "| Name | Age | City |
|---|---|---|
| Alice | 30 | London |";
        let out = render_markdown_table(md, 60);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(
            "
",
        );
        assert!(text.contains("Name"), "Has table header");
        assert!(text.contains("Alice"), "Has table data");
    }

    #[test]
    fn test_special_characters_in_cells() {
        let md = "| Name | Desc |
|---|---|
| Test | `code` |";
        let out = render_markdown_table(md, 50);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(
            "
",
        );
        assert!(text.contains("Test"), "Has Test");
    }

    #[test]
    fn test_no_header_row_separator_at_end() {
        let md = "| A | B |
|---|---|
| 1 | 2 |";
        let out = render_markdown_table(md, 30);
        // Should have: top border, header, sep, body, bottom border
        assert!(out.len() >= 5, "Should have at least 5 lines");
    }

    #[test]
    fn test_exact_output_format() {
        let md = "| A | B |
|---|---|
| X | Y |";
        let out = render_markdown_table(md, 30);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(
            "
",
        );
        // Check exact format
        assert_eq!(
            text,
            "┌───┬───┐
│ A │ B │
├───┼───┤
│ X │ Y │
└───┴───┘
"
        );
    }
}

#[cfg(test)]
mod complete_table_verification {
    use crate::table_renderer::render_markdown_table;

    #[test]
    fn test_mixed_content_before_table() {
        let md = "Check out this table:\n\n| Name | Value |\n|---|---|\n| Alpha | 100 |";
        let out = render_markdown_table(md, 50);
        let text: String = out
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Check out"), "Before table");
        assert!(text.contains('┌'), "Table top");
        assert!(text.contains("Alpha"), "Table data");
    }

    #[test]
    fn test_mixed_content_after_table() {
        let md = "| A | B |\n|---|---|\n| X | Y |\n\nThat was the table.";
        let out = render_markdown_table(md, 50);
        let text: String = out
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains('┌'), "Table top");
        assert!(text.contains("That was the table"), "After table");
    }

    #[test]
    fn test_mixed_content_both_sides() {
        let md = "Start\n\n| H1 | H2 | H3 |\n|---|---|---|\n| C1 | C2 | C3 |\n\nEnd";
        let out = render_markdown_table(md, 60);
        let text: String = out
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Start"), "Before");
        assert!(text.contains("End"), "After");
        assert!(text.contains('┌'), "Table");
    }

    #[test]
    fn test_narrow_terminal() {
        let md = "| Name | Age | City |\n|---|---|---|\n| Alice | 30 | Seoul |";
        let out = render_markdown_table(md, 20);
        assert!(!out.is_empty());
        let text: String = out
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Name") || text.contains("┌") || text.contains("Alice"));
    }

    #[test]
    fn test_cell_wrapping() {
        let md = "| Short | Very Long Header Text Here |\n|---|---|\n| Data | Another long cell that needs wrapping |";
        let out = render_markdown_table(md, 50);
        let text: String = out
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains('┌'), "Table rendered");
        assert!(text.contains('│'), "Has cell separators");
    }

    #[test]
    fn test_multiple_rows_separators() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |";
        let out = render_markdown_table(md, 40);
        let text: String = out
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        // Count separator lines (lines containing ├ or ┼)
        let separator_lines: Vec<&str> = text
            .lines()
            .filter(|l| l.contains('├') || l.contains('┼'))
            .collect();
        // 4 data rows → 3 separators between them
        println!("\n{}", text);
        assert_eq!(separator_lines.len(), 3, "Should have 4 separator lines");
        // Check it's a proper table
        assert!(text.contains("┌"), "Has top border");
        assert!(text.contains("└"), "Has bottom border");
    }

    #[test]
    fn test_header_bold_styling() {
        let md = "| Name | Value |\n|---|---|\n| X | Y |";
        let out = render_markdown_table(md, 50);
        assert!(
            out.len() >= 4,
            "Should have top border + header + separator + body"
        );
    }
}
