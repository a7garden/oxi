//! ChatView widget — scrollable message list with streaming support.
//!
//! Architecture: flatten everything into a single Vec<Line> with style.
//! Scrolling is simply "which range of lines to show". No segment math.
//! Tool/error boxes are rendered as styled bordered blocks inline.
//!
//! Safe characters only: no exotic Unicode that might render as mojibake.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget},
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

    fn find_and_remove(&mut self, id: &str) -> Option<usize> {
        self.active.remove(id)
    }

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
    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }
    pub fn scroll_down(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
    }
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
            // Fallback: merge into last ToolCall
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
        if let Some(s) = self.streaming.take() {
            self.messages.push(s.message);
        }
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

// ── Line buffer: flatten all content into Vec<Line> ──────────────────
//
// Instead of segments with virtual y-coords, we build a flat list of
// styled Lines. Scrolling is simply skip/take on this list.

/// Flatten a ContentBlock into styled Lines.
fn block_to_lines(block: &ContentBlock, role: MessageRole, styles: &ThemeStyles, width: u16) -> Vec<Line<'static>> {
    match block {
        ContentBlock::Text { content } => {
            let mut lines = md_lines(content);
            if role == MessageRole::User {
                // Indent user text with left stripe visual
                for line in &mut lines {
                    let mut new_spans = vec![Span::styled("  ", Style::default())];
                    new_spans.append(&mut line.spans);
                    line.spans = new_spans;
                }
            }
            lines
        }

        ContentBlock::Thinking { content, collapsed } => {
            let indicator = if *collapsed { ">" } else { "v" };
            let mut lines = vec![
                Line::from(Span::styled(
                    format!("{} Thinking...", indicator),
                    styles.accent,
                )),
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
            let (_icon, name_style) = match status {
                ToolCallStatus::Requested => ("...", styles.muted),
                ToolCallStatus::Executing => ("*run", styles.warning),
                ToolCallStatus::Done => ("ok", styles.success),
            };

            let inner_w = width.saturating_sub(2) as usize; // account for border
            let mut lines = Vec::new();

            // Header line
            lines.push(Line::from(vec![
                Span::styled(" + ", name_style),
                Span::styled(format!("tool: {}", name), Style::default().fg(name_style.fg.unwrap_or(Color::White)).add_modifier(Modifier::BOLD)),
            ]));

            // Argument lines (limited)
            let arg_lines: Vec<&str> = arguments.lines().collect();
            let max_args = if arg_lines.len() <= 3 { 5 } else { 3 };
            for arg in arg_lines.iter().take(max_args) {
                let truncated: String = arg.chars().take(inner_w).collect();
                lines.push(Line::from(Span::styled(format!(" | {}", truncated), styles.muted)));
            }
            if arg_lines.len() > max_args {
                lines.push(Line::from(Span::styled(" | ...", styles.muted)));
            }

            // Divider + result
            if let Some((result_content, is_error)) = result {
                lines.push(Line::from(Span::styled(" |---", if *is_error { styles.error } else { styles.success })));
                let result_style = if *is_error { styles.error } else { styles.normal };
                let r_lines: Vec<&str> = result_content.lines().collect();
                for rl in r_lines.iter().take(6) {
                    let truncated: String = rl.chars().take(inner_w).collect();
                    lines.push(Line::from(Span::styled(format!(" | {}", truncated), result_style)));
                }
                if r_lines.len() > 6 {
                    lines.push(Line::from(Span::styled(" | ...", styles.muted)));
                }
            }

            // Bottom border
            lines.push(Line::from(Span::styled(" +", styles.muted)));

            lines
        }

        ContentBlock::ToolResult { tool_name, content, is_error } => {
            let label = if tool_name.is_empty() {
                if *is_error { "X".to_string() } else { "ok".to_string() }
            } else {
                format!("{} {}", if *is_error { "X" } else { "ok" }, tool_name)
            };
            let border_style = if *is_error { styles.error } else { styles.muted };
            let content_style = if *is_error { styles.error } else { styles.normal };
            let inner_w = width.saturating_sub(2) as usize;

            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(format!(" + {}", label), border_style)));
            for l in content.lines().take(4) {
                let truncated: String = l.chars().take(inner_w).collect();
                lines.push(Line::from(Span::styled(format!(" | {}", truncated), content_style)));
            }
            if content.lines().count() > 4 {
                lines.push(Line::from(Span::styled(" | ...", styles.muted)));
            }
            lines.push(Line::from(Span::styled(" +", border_style)));
            lines
        }

        ContentBlock::Error { title, message, retryable } => {
            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(
                format!(" ! error: {}", title),
                Style::default().fg(Color::White).bg(styles.error.fg.unwrap_or(Color::Red)).bold(),
            )));
            for l in message.lines().take(4) {
                lines.push(Line::from(Span::styled(format!("   {}", l), styles.normal)));
            }
            if *retryable {
                lines.push(Line::from(Span::styled("   retry: this error may be temporary", styles.muted)));
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

/// Build a flat list of all Lines to display, plus a blank-line spacer between messages.
fn build_all_lines(state: &ChatViewState, styles: &ThemeStyles, width: u16) -> Vec<Line<'static>> {
    let mut all_lines: Vec<Line<'static>> = Vec::new();

    for (i, msg) in state.messages.iter().enumerate() {
        // Spacer between messages
        if i > 0 {
            all_lines.push(Line::raw(""));
        }

        // User label
        if msg.role == MessageRole::User {
            // Horizontal rule
            let rule = "-".repeat(width.saturating_sub(2) as usize);
            all_lines.push(Line::from(Span::styled(rule, styles.muted)));
            all_lines.push(Line::from(Span::styled("You", Style::default().add_modifier(Modifier::BOLD).fg(styles.primary.fg.unwrap_or(Color::Blue)))));
        }

        for block in &msg.content_blocks {
            let block_lines = block_to_lines(block, msg.role, styles, width);
            all_lines.extend(block_lines);
        }
    }

    // Streaming message
    if let Some(ref streaming) = state.streaming {
        // Spacer before streaming if there were previous messages
        if !state.messages.is_empty() {
            all_lines.push(Line::raw(""));
        }

        for block in &streaming.message.content_blocks {
            let block_lines = block_to_lines(block, MessageRole::Assistant, styles, width);
            all_lines.extend(block_lines);
        }

        // Spinner line
        let sp = ["|", "/", "-", "\\"];
        let ch = sp[state.spinner_frame % sp.len()];
        all_lines.push(Line::from(Span::styled(
            format!("  {} Working...", ch),
            styles.accent,
        )));
    }

    all_lines
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

        // Build all lines
        let all_lines = build_all_lines(state, &styles, area.width);
        let total_height = all_lines.len() as u16;
        state.content_height = total_height;

        let vis = area.height;
        let max_scroll = total_height.saturating_sub(vis);
        let off = state.scroll_offset.min(max_scroll);

        // Clear area with background
        Block::default().style(Style::default().bg(self.theme.colors.background.to_ratatui())).render(area, buf);

        // Render visible lines
        let skip = off as usize;
        let take = vis as usize;
        let visible_lines: Vec<Line<'_>> = all_lines.into_iter().skip(skip).take(take).collect();

        if !visible_lines.is_empty() {
            let text: ratatui::text::Text = visible_lines.into_iter().collect();
            Paragraph::new(text)
                .style(styles.normal)
                .render(area, buf);
        }

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
        let msg = &s.messages[0];
        assert_eq!(msg.content_blocks.len(), 1);
        match &msg.content_blocks[0] {
            ContentBlock::ToolCall { status, result, .. } => {
                assert_eq!(*status, ToolCallStatus::Done);
                assert!(result.is_some());
            }
            _ => panic!("expected ToolCall block"),
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
        let lines = build_all_lines(&s, &styles, 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn fix_bare_code_fences_basic() {
        let input = "```\ncode\n```";
        let fixed = fix_bare_code_fences(input);
        assert!(fixed.starts_with("```text"));
    }
}
