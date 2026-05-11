//! ChatView widget — scrollable message list with streaming support.
//!
//! Architecture: **Paragraph::scroll()** approach.
//!
//! 1. All content is built into a single Vec<Line>.
//! 2. Tool/error boxes are drawn as styled border lines (─│┌┐└┘).
//! 3. Rendered via `Paragraph::new(text).wrap(Wrap).scroll((offset, 0))`.
//! 4. `Paragraph::line_count(width)` gives exact total height.
//! 5. Scrolling is handled entirely by ratatui — no manual skip/take.
//!
//! This eliminates ALL measurement/render mismatch because ratatui does
//! both the measurement (line_count) and rendering (scroll) with the same
//! internal wrapping logic.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap},
};
use tui_markdown;
use crate::Theme;
use crate::theme::ThemeStyles;

// ── Tool Call Tracker ─────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ToolCallTracker {
    active: HashMap<String, usize>,
}

impl ToolCallTracker {
    fn register(&mut self, id: String, index: usize) -> bool {
        if self.active.contains_key(&id) { return false; }
        self.active.insert(id, index);
        true
    }
    fn find_and_remove(&mut self, id: &str) -> Option<usize> { self.active.remove(id) }
    fn remove(&mut self, id: &str) { self.active.remove(id); }
    fn get(&self, id: &str) -> Option<usize> { self.active.get(id).copied() }
    fn clear(&mut self) { self.active.clear(); }
}

// ── Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    Requested,
    Executing,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text { content: String },
    Thinking { content: String, collapsed: bool },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
        result: Option<(String, bool)>,
        status: ToolCallStatus,
    },
    ToolResult { tool_name: String, content: String, is_error: bool },
    Error { title: String, message: String, retryable: bool },
    Image { mime_type: String, base64_data: String },
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content_blocks: Vec<ContentBlock>,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct StreamingState {
    pub message: ChatMessage,
    pub active_content_index: usize,
}

// ── ChatViewState ──────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct ChatViewState {
    pub messages: Vec<ChatMessage>,
    pub streaming: Option<StreamingState>,
    pub scroll_offset: u16,
    pub spinner_frame: usize,
    pub content_height: u16,
    pub last_code_block: Option<String>,
    pub pending_images: Vec<(String, String)>,
    tool_tracker: ToolCallTracker,
}

impl ChatViewState {
    pub fn new() -> Self { Self::default() }

    pub fn scroll_to_bottom(&mut self, visible: u16) {
        self.scroll_offset = self.content_height.saturating_sub(visible);
    }
    pub fn scroll_up(&mut self, n: u16) { self.scroll_offset = self.scroll_offset.saturating_sub(n); }
    pub fn scroll_down(&mut self, n: u16) { self.scroll_offset = self.scroll_offset.saturating_add(n); }
    pub fn scroll_to_top(&mut self) { self.scroll_offset = 0; }

    pub fn start_streaming(&mut self) {
        self.streaming = Some(StreamingState {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content_blocks: Vec::new(),
                timestamp: 0,
            },
            active_content_index: 0,
        });
        self.tool_tracker.clear();
    }

    pub fn stream_text_delta(&mut self, delta: &str) {
        self.append_text(delta);
        self.update_last_code_block(delta);
        self.last_code_block = None;
    }

    fn append_text(&mut self, text: &str) {
        if let Some(ref mut s) = self.streaming {
            if let Some(ContentBlock::Text { ref mut content }) = s.message.content_blocks.first_mut() {
                content.push_str(text);
            } else {
                s.message.content_blocks.insert(0, ContentBlock::Text { content: text.to_string() });
            }
        }
    }

    pub fn is_streaming(&self) -> bool { self.streaming.is_some() }

    fn update_last_code_block(&mut self, _delta: &str) {
        if let Some(ref mut s) = self.streaming {
            if let Some(ContentBlock::Text { ref mut content, .. }) = s.message.content_blocks.first_mut() {
                if let Some(code) = extract_last_code_block(content) {
                    self.last_code_block = Some(code);
                }
            }
        }
    }

    pub fn refresh_last_code_block(&mut self) {
        if let Some(ref s) = self.streaming {
            if let Some(ContentBlock::Text { ref content, .. }) = s.message.content_blocks.first() {
                if let Some(code) = extract_last_code_block(content) {
                    self.last_code_block = Some(code);
                }
            }
        }
    }

    pub fn set_tool_status(&mut self, id: &str, status: ToolCallStatus) {
        if let Some(ref mut s) = self.streaming {
            if let Some(idx) = self.tool_tracker.get(id) {
                if let Some(block) = s.message.content_blocks.get_mut(idx) {
                    if let ContentBlock::ToolCall { status: ref mut curr, .. } = block {
                        *curr = status;
                    }
                }
            }
        }
    }

    pub fn stream_tool_call(&mut self, id: String, name: String, arguments: String, status: ToolCallStatus) {
        if let Some(ref mut s) = self.streaming {
            let idx = s.message.content_blocks.len();
            if !self.tool_tracker.register(id.clone(), idx) { return; }
            s.message.content_blocks.push(ContentBlock::ToolCall {
                id, name, arguments, result: None, status,
            });
        }
    }

    pub fn stream_tool_result(&mut self, tool_call_id: Option<String>, tool_name: String, content: String, is_error: bool) {
        if let Some(ref mut s) = self.streaming {
            if let Some(ref id) = tool_call_id {
                if let Some(idx) = self.tool_tracker.find_and_remove(id) {
                    if let Some(block) = s.message.content_blocks.get_mut(idx) {
                        if let ContentBlock::ToolCall { ref mut result, ref mut status, .. } = block {
                            *result = Some((content, is_error));
                            *status = ToolCallStatus::Done;
                            return;
                        }
                    }
                }
            }
            if let Some(last) = s.message.content_blocks.last_mut() {
                if let ContentBlock::ToolCall { ref mut result, ref mut status, .. } = last {
                    *result = Some((content, is_error));
                    *status = ToolCallStatus::Done;
                    if let Some(ref id) = tool_call_id { self.tool_tracker.remove(id); }
                    return;
                }
            }
            s.message.content_blocks.push(ContentBlock::ToolResult { tool_name, content, is_error });
        }
    }

    pub fn stream_error(&mut self, title: String, message: String, retryable: bool) {
        if let Some(ref mut s) = self.streaming {
            s.message.content_blocks.push(ContentBlock::Error { title, message, retryable });
        }
    }

    pub fn stream_thinking(&mut self, content: String, collapsed: bool) {
        if let Some(ref mut s) = self.streaming {
            s.message.content_blocks.push(ContentBlock::Thinking { content, collapsed });
        }
    }

    pub fn stream_image(&mut self, mime_type: String, base64_data: String) {
        if let Some(ref mut s) = self.streaming {
            s.message.content_blocks.push(ContentBlock::Image { mime_type, base64_data });
        }
    }

    pub fn finish_streaming(&mut self) {
        if let Some(s) = self.streaming.take() { self.messages.push(s.message); }
    }

    pub fn cancel_streaming(&mut self) { self.streaming = None; }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.streaming = None;
        self.scroll_offset = 0;
        self.last_code_block = None;
        self.pending_images.clear();
        self.tool_tracker.clear();
    }

    pub fn push_message(&mut self, msg: ChatMessage) { self.messages.push(msg); }

    pub fn add_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        self.streaming = None;
        self.last_code_block = None;
    }

    pub fn push_system_message(&mut self, content: String) {
        self.messages.push(ChatMessage {
            role: MessageRole::System,
            content_blocks: vec![ContentBlock::Text { content }],
            timestamp: 0,
        });
    }
}

// ── Code block extraction ────────────────────────────────────────────

fn extract_last_code_block(text: &str) -> Option<String> {
    let mut result: Option<String> = None;
    let mut in_block = false;
    let mut block_content = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_block {
                let c = block_content.trim().to_string();
                if !c.is_empty() { result = Some(c); }
                block_content.clear();
                in_block = false;
            } else {
                block_content.clear();
                in_block = true;
            }
        } else if in_block {
            if !block_content.is_empty() { block_content.push('\n'); }
            block_content.push_str(line);
        }
    }
    result
}

/// Fix bare code fences (``` without a language) to ```text.
fn fix_bare_code_fences(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'`' && i + 2 < len && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
            let after = &bytes[i + 3..];
            let is_bare = after.first().map_or(true, |&c| c == b'\n' || c == b'\r' || c == b'\t' || c == b' ');
            if is_bare {
                result.push_str("```text");
                i += 3;
                while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') { i += 1; }
                continue;
            }
            let lang_end = after.iter().position(|&c| c == b'\n' || c == b'\r').unwrap_or(after.len());
            let lang_str = String::from_utf8_lossy(&after[..lang_end]).trim().to_lowercase();
            if (lang_str == "text" || lang_str == "plaintext" || lang_str == "plain" || lang_str == "none") && !lang_str.is_empty() {
                result.push_str("```text");
                i += 3 + lang_end;
                continue;
            }
        }
        let ch = content[i..].chars().next().unwrap();
        result.push(ch);
        i += ch.len_utf8();
    }
    result
}

/// Convert markdown to styled Lines via tui-markdown.
fn md_lines(content: &str) -> Vec<Line<'static>> {
    let preprocessed = fix_bare_code_fences(content);
    let text: ratatui::text::Text<'_> = tui_markdown::from_str(&preprocessed);
    text.lines.into_iter().map(|l| {
        let spans: Vec<Span<'static>> = l.spans
            .into_iter()
            .map(|s| Span::styled(s.content.into_owned(), s.style))
            .collect();
        Line::from(spans)
    }).collect()
}

// ── Build all content as Vec<Line> ────────────────────────────────────

/// Build the complete content as a flat list of styled Lines.
/// ratatui handles word-wrapping via Wrap, so we don't wrap ourselves.
fn build_lines(state: &ChatViewState, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (i, msg) in state.messages.iter().enumerate() {
        // Spacer between messages
        if i > 0 { lines.push(Line::raw("")); }

        if msg.role == MessageRole::User {
            // Separator rule
            lines.push(Line::from(Span::styled(
                "\u{2500}".repeat(40), // ─ horizontal line
                styles.muted,
            )));
            // Label
            lines.push(Line::from(Span::styled(
                "You".to_string(),
                Style::default().add_modifier(Modifier::BOLD).fg(styles.primary.fg.unwrap_or(ratatui::style::Color::Blue)),
            )));
        }

        for block in &msg.content_blocks {
            let block_lines = block_to_lines(block, msg.role, styles);
            lines.extend(block_lines);
        }
    }

    if let Some(ref streaming) = state.streaming {
        if !state.messages.is_empty() { lines.push(Line::raw("")); }
        for block in &streaming.message.content_blocks {
            let block_lines = block_to_lines(block, MessageRole::Assistant, styles);
            lines.extend(block_lines);
        }
        // Spinner
        let sp = ["|", "/", "-", "\\"];
        let ch = sp[state.spinner_frame % sp.len()];
        lines.push(Line::from(Span::styled(format!("  {} Working...", ch), styles.accent)));
    }

    lines
}

/// Convert a ContentBlock to styled Lines.
fn block_to_lines(block: &ContentBlock, role: MessageRole, styles: &ThemeStyles) -> Vec<Line<'static>> {
    match block {
        ContentBlock::Text { content } => {
            let mut md = md_lines(content);
            if role == MessageRole::User {
                for line in &mut md {
                    let mut new_spans = vec![Span::styled("  ", Style::default())];
                    new_spans.append(&mut line.spans);
                    line.spans = new_spans;
                }
            }
            md
        }

        ContentBlock::Thinking { content, collapsed } => {
            let ind = if *collapsed { ">" } else { "v" };
            let mut lines = vec![
                Line::from(Span::styled(format!("{} Thinking...", ind), styles.accent)),
            ];
            if !*collapsed {
                for l in content.lines() {
                    lines.push(Line::from(Span::styled(format!("  {}", l), styles.muted)));
                }
            } else if let Some(first) = content.lines().next() {
                lines.push(Line::from(Span::styled(format!("  {}", first), styles.muted)));
            }
            lines
        }

        ContentBlock::ToolCall { name, arguments, result, status, .. } => {
            let (icon, name_fg) = match status {
                ToolCallStatus::Requested => ("...", styles.muted.fg.unwrap_or(ratatui::style::Color::White)),
                ToolCallStatus::Executing => ("*run", styles.warning.fg.unwrap_or(ratatui::style::Color::Yellow)),
                ToolCallStatus::Done => ("ok", styles.success.fg.unwrap_or(ratatui::style::Color::Green)),
            };
            let mut lines = Vec::new();
            // Top border with title
            lines.push(Line::from(Span::styled(
                format!("\u{250c} {} tool: {}", icon, name),
                Style::default().fg(name_fg).add_modifier(Modifier::BOLD),
            )));
            // Arguments
            let arg_count = arguments.lines().count();
            let max_args = if arg_count <= 3 { 5 } else { 3 };
            for arg in arguments.lines().take(max_args) {
                lines.push(Line::from(Span::styled(format!("\u{2502} {}", arg), styles.muted)));
            }
            if arg_count > max_args {
                lines.push(Line::from(Span::styled("\u{2502} ...", styles.muted)));
            }
            // Result
            if let Some((result_content, is_error)) = result {
                let div_style = if *is_error { styles.error } else { styles.success };
                lines.push(Line::from(Span::styled(
                    format!("\u{251c}{}", "\u{2500}".repeat(20)),
                    div_style,
                )));
                let result_style = if *is_error { styles.error } else { styles.normal };
                let r_count = result_content.lines().count();
                for rl in result_content.lines().take(6) {
                    lines.push(Line::from(Span::styled(format!("\u{2502} {}", rl), result_style)));
                }
                if r_count > 6 {
                    lines.push(Line::from(Span::styled("\u{2502} ...", styles.muted)));
                }
            }
            // Bottom border
            lines.push(Line::from(Span::styled("\u{2514}", styles.muted)));
            lines
        }

        ContentBlock::ToolResult { tool_name, content, is_error } => {
            let (check, border_fg) = if *is_error {
                ("X", styles.error.fg.unwrap_or(ratatui::style::Color::Red))
            } else {
                ("ok", styles.success.fg.unwrap_or(ratatui::style::Color::Green))
            };
            let label = if tool_name.is_empty() { check.to_string() } else { format!("{} {}", check, tool_name) };
            let content_style = if *is_error { styles.error } else { styles.normal };
            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(
                format!("\u{250c} {}", label),
                Style::default().fg(border_fg).add_modifier(Modifier::BOLD),
            )));
            let n = content.lines().count();
            for l in content.lines().take(4) {
                lines.push(Line::from(Span::styled(format!("\u{2502} {}", l), content_style)));
            }
            if n > 4 {
                lines.push(Line::from(Span::styled("\u{2502} ...", styles.muted)));
            }
            lines.push(Line::from(Span::styled("\u{2514}", Style::default().fg(border_fg))));
            lines
        }

        ContentBlock::Error { title, message, retryable } => {
            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(
                format!("\u{26a0} error: {}", title),
                Style::default().fg(ratatui::style::Color::White).bg(styles.error.fg.unwrap_or(ratatui::style::Color::Red)).add_modifier(Modifier::BOLD),
            )));
            for l in message.lines().take(4) {
                lines.push(Line::from(Span::styled(format!("  {}", l), styles.normal)));
            }
            if *retryable {
                lines.push(Line::from(Span::styled("  retry: this error may be temporary", styles.muted)));
            }
            lines
        }

        ContentBlock::Image { mime_type, base64_data } => {
            let sz = base64_data.len() * 3 / 4;
            let sz_str = if sz >= 1_048_576 {
                format!("{:.1} MB", sz as f64 / 1_048_576.0)
            } else if sz >= 1024 {
                format!("{:.1} KB", sz as f64 / 1024.0)
            } else {
                format!("{} B", sz)
            };
            vec![
                Line::from(Span::styled(format!("[image: {}, {}]", mime_type, sz_str), styles.normal)),
                Line::from(Span::styled("  Ctrl+I -> open in viewer", styles.muted)),
            ]
        }
    }
}

// ── ChatView widget ────────────────────────────────────────────────────

pub struct ChatView<'a> {
    theme: &'a Theme,
    scrollbar: bool,
}

impl<'a> ChatView<'a> {
    pub fn new(theme: &'a Theme) -> Self { Self { theme, scrollbar: true } }
    pub fn with_scrollbar(mut self, show: bool) -> Self { self.scrollbar = show; self }
}

impl StatefulWidget for ChatView<'_> {
    type State = ChatViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width < 4 || area.height < 1 { return; }
        let styles = self.theme.to_styles();

        // Build content lines
        let lines = build_lines(state, &styles);
        let text: ratatui::text::Text<'_> = lines.into_iter().collect();

        // Measure total height using ratatui's own line_count (with Wrap)
        let para_for_measure = Paragraph::new(text.clone()).wrap(Wrap { trim: false });
        let total_height = para_for_measure.line_count(area.width) as u16;
        state.content_height = total_height;

        let vis = area.height;
        let max_scroll = total_height.saturating_sub(vis);
        let off = state.scroll_offset.min(max_scroll);

        // Clear area
        Block::default()
            .style(Style::default().bg(self.theme.colors.background.to_ratatui()))
            .render(area, buf);

        // Render — ratatui handles wrapping + scrolling
        Paragraph::new(text)
            .style(styles.normal)
            .wrap(Wrap { trim: false })
            .scroll((off, 0))
            .render(area, buf);

        // Scrollbar
        if self.scrollbar && max_scroll > 0 {
            let mut sb = ScrollbarState::new(total_height as usize)
                .position(off as usize)
                .viewport_content_length(vis as usize);
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None).end_symbol(None).track_symbol(None)
                .thumb_symbol("|")
                .render(area, buf, &mut sb);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_bounds() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20);
        assert_eq!(s.scroll_offset, 80);
        s.scroll_up(3);
        assert_eq!(s.scroll_offset, 77);
        s.scroll_down(10);
        assert_eq!(s.scroll_offset, 87);
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
        s.stream_tool_call("t1".into(), "bash".into(), "ls".into(), ToolCallStatus::Executing);
        s.stream_tool_result(Some("t1".into()), "bash".into(), "file.txt".into(), false);
        s.finish_streaming();
        assert_eq!(s.messages.len(), 1);
        match &s.messages[0].content_blocks[0] {
            ContentBlock::ToolCall { status, result, .. } => {
                assert_eq!(*status, ToolCallStatus::Done);
                assert!(result.is_some());
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn build_lines_basic() {
        let theme = Theme::dark();
        let styles = theme.to_styles();
        let mut s = ChatViewState::new();
        s.messages.push(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text { content: "Hello".into() }],
            timestamp: 0,
        });
        let lines = build_lines(&s, &styles);
        assert!(!lines.is_empty());
        assert!(lines.iter().any(|l| l.to_string().contains("You")));
    }

    #[test]
    fn fix_bare_code_fences_basic() {
        let input = "```\ncode\n```";
        let fixed = fix_bare_code_fences(input);
        assert!(fixed.starts_with("```text"));
    }

    #[test]
    fn paragraph_scroll_renders() {
        // Verify that Paragraph::scroll compiles and renders without panic
        let theme = Theme::dark();
        let styles = theme.to_styles();
        let mut s = ChatViewState::new();
        for i in 0..20 {
            s.messages.push(ChatMessage {
                role: MessageRole::System,
                content_blocks: vec![ContentBlock::Text { content: format!("Line {}", i) }],
                timestamp: i as i64,
            });
        }
        let lines = build_lines(&s, &styles);
        let text: ratatui::text::Text<'_> = lines.into_iter().collect();
        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        // Should render with scroll offset without panic
        Paragraph::new(text)
            .style(styles.normal)
            .wrap(Wrap { trim: false })
            .scroll((5, 0))
            .render(area, &mut buf);
    }
}
