//! ChatView widget — scrollable message list with streaming support.
//!
//! All content is unified into a single `Vec<Line>`:
//! - Markdown text → `tui_markdown::from_str()` returns pre-styled `Line`s
//! - Structural elements (separators, role labels, tool boxes) → manually styled `Line`s
//! - Both are pushed in order → correct interleaving → correct scroll

use ratatui::{
    buffer::Buffer,
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget},
};
use tui_markdown;

use crate::Theme;
use crate::theme::ThemeStyles;
use super::markdown;

// ── Types ──────────────────────────────────────────────────────────────

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
    ToolCall { id: String, name: String, arguments: String },
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
    pub line_buffer: String,
}

// ── ChatViewState ──────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct ChatViewState {
    pub messages: Vec<ChatMessage>,
    pub streaming: Option<StreamingState>,
    pub scroll_offset: u16,
    pub auto_scroll: bool,
    pub spinner_frame: usize,
    pub content_height: u16,
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
            line_buffer: String::new(),
        });
    }

    pub fn stream_text(&mut self, text: &str) {
        if let Some(ref mut s) = self.streaming {
            if let Some(ContentBlock::Text { ref mut content }) = s.message.content_blocks.first_mut() {
                content.push_str(text);
            } else {
                s.message.content_blocks.insert(0, ContentBlock::Text { content: text.to_string() });
            }
        }
    }

    pub fn stream_tool_call(&mut self, id: String, name: String, arguments: String) {
        if let Some(ref mut s) = self.streaming {
            s.message.content_blocks.push(ContentBlock::ToolCall { id, name, arguments });
        }
    }

    pub fn stream_tool_result(&mut self, tool_name: String, content: String, is_error: bool) {
        if let Some(ref mut s) = self.streaming {
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

    pub fn push_message(&mut self, msg: ChatMessage) { self.messages.push(msg); }

    pub fn push_system_message(&mut self, content: String) {
        self.messages.push(ChatMessage {
            role: MessageRole::System,
            content_blocks: vec![ContentBlock::Text { content }],
            timestamp: 0,
        });
    }

    pub fn append_streaming_line(&mut self, text: &str) {
        if let Some(ref mut s) = self.streaming { s.line_buffer.push_str(text); }
    }

    pub fn flush_streaming_line(&mut self) {
        if let Some(ref mut s) = self.streaming {
            if !s.line_buffer.is_empty() {
                self.stream_text(&s.line_buffer);
                s.line_buffer.clear();
            }
        }
    }
}

// ── Box-drawing helpers ────────────────────────────────────────────────

fn bordered_header(label: &str, width: usize) -> String {
    let inner = label.len() + 2;
    let right = width.saturating_sub(inner + 2);
    format!("\u{250c}{} {} {}", "\u{2500}", label, "\u{2500}".repeat(right.max(1)))
}

fn bordered_footer(width: usize) -> String {
    format!("\u{2514}{}\u{2510}", "\u{2500}".repeat(width.saturating_sub(2).max(1)))
}

/// User message left-border stripe: solid bar + tinted space.
fn user_stripe(styles: &ThemeStyles) -> Vec<Span<'static>> {
    vec![
        Span::styled("\u{258c}", styles.user_border),
        Span::styled(" ", styles.user_bg),
    ]
}

/// Render markdown content via tui-markdown, converting to owned Lines.
fn markdown_lines(content: &str) -> Vec<Line<'static>> {
    let text: ratatui::text::Text = tui_markdown::from_str(content);
    text.lines.into_iter().map(|l| {
        let spans: Vec<Span<'static>> = l.spans
            .into_iter()
            .map(|s| Span::styled(s.content.into_owned(), s.style))
            .collect();
        Line::from(spans)
    }).collect()
}

/// Style for structural lines based on kind.
fn structural_style(kind: LineKind, styles: &ThemeStyles) -> Style {
    match kind {
        LineKind::Normal => styles.normal,
        LineKind::CodeBlock => markdown::code_block_style(styles.normal),
        LineKind::Heading(lvl) => markdown::heading_style(styles.normal, lvl),
        LineKind::ListItem => styles.normal,
        LineKind::HorizontalRule => styles.muted,
        LineKind::RoleLabel => styles.primary.add_modifier(Modifier::BOLD),
        LineKind::ToolCallHeader | LineKind::ToolCallBody | LineKind::ToolCallFooter
        | LineKind::ToolResultHeader | LineKind::ToolResultBody | LineKind::ToolResultFooter => {
            styles.muted.bg(Color::Indexed(234))
        }
        LineKind::ErrorHeader | LineKind::ErrorBody | LineKind::ErrorFooter => {
            styles.error.bg(Color::Rgb(60, 20, 30))
        }
        LineKind::TableBorder => markdown::table_border_style(styles.normal),
        LineKind::TableHeader => markdown::table_header_style(styles.normal),
        LineKind::TableRow => styles.normal,
    }
}

/// Prefix style based on role.
fn role_prefix_style(role: MessageRole, styles: &ThemeStyles) -> Style {
    match role {
        MessageRole::User => styles.primary,
        MessageRole::Assistant => styles.accent,
        MessageRole::System => styles.muted,
    }
}

/// Parse inline markdown spans within a text line.
fn inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let segments = markdown::parse_inline(text);
    segments.into_iter().map(|seg| {
        let style = match &seg {
            markdown::Segment::Normal(_) => base,
            markdown::Segment::Bold(_) => markdown::bold_style(base),
            markdown::Segment::Italic(_) => markdown::italic_style(base),
            markdown::Segment::Code(_) => markdown::code_style(base),
            markdown::Segment::Link { .. } => markdown::link_style(base),
        };
        let s: &str = match &seg {
            markdown::Segment::Normal(s) => s,
            markdown::Segment::Bold(s) => s,
            markdown::Segment::Italic(s) => s,
            markdown::Segment::Code(s) => s,
            markdown::Segment::Link { text, .. } => text,
        };
        Span::styled(s.to_string(), style)
    }).collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LineKind {
    Normal, CodeBlock, Heading(u8), ListItem, HorizontalRule, RoleLabel,
    ToolCallHeader, ToolCallBody, ToolCallFooter,
    ToolResultHeader, ToolResultBody, ToolResultFooter,
    ErrorHeader, ErrorBody, ErrorFooter,
    TableBorder, TableHeader, TableRow,
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
        let mut lines: Vec<Line<'static>> = Vec::new();
        let sep_w = area.width.saturating_sub(4) as usize;

        // ── Push a structural (non-markdown) line ──
        let push = |lines: &mut Vec<Line>, role, text: &str, kind: LineKind| {
            let base = structural_style(kind, &styles);
            match kind {
                // Full-width lines: just styled text
                LineKind::CodeBlock | LineKind::HorizontalRule | LineKind::RoleLabel
                | LineKind::TableBorder
                | LineKind::ToolCallHeader | LineKind::ToolCallFooter
                | LineKind::ToolResultHeader | LineKind::ToolResultFooter
                | LineKind::ErrorHeader | LineKind::ErrorFooter => {
                    lines.push(Line::from(Span::styled(text.to_string(), base)));
                }
                // Tool/error body: indent + styled text
                LineKind::ToolCallBody | LineKind::ToolResultBody | LineKind::ErrorBody => {
                    lines.push(Line::from(Span::styled(text.to_string(), base)));
                }
                // Normal / list / heading / table: inline parse + role prefix
                LineKind::Normal | LineKind::ListItem | LineKind::Heading(_)
                | LineKind::TableHeader | LineKind::TableRow => {
                    let mut spans = Vec::new();
                    if role == MessageRole::User {
                        spans.extend(user_stripe(&styles));
                    } else {
                        spans.push(Span::styled(" ", role_prefix_style(role, &styles)));
                        spans.push(Span::styled(" ", base));
                    }
                    spans.push(Span::styled(" ", base));
                    spans.extend(inline_spans(text, base));
                    lines.push(Line::from(spans));
                }
            }
        };

        // ── Completed messages ──
        for (i, msg) in state.messages.iter().enumerate() {
            if msg.role == MessageRole::User && i > 0 {
                push(&mut lines, msg.role, &"\u{2500}".repeat(sep_w), LineKind::HorizontalRule);
            }
            if msg.role == MessageRole::User {
                push(&mut lines, msg.role, "You", LineKind::RoleLabel);
            }
            push_blocks(&mut lines, msg.role, &msg.content_blocks, &styles);
        }

        // ── Streaming message ──
        if let Some(ref streaming) = state.streaming {
            push_blocks(&mut lines, MessageRole::Assistant, &streaming.message.content_blocks, &styles);
            if !streaming.line_buffer.is_empty() {
                let txt = streaming.line_buffer.trim_end().to_string();
                if !txt.is_empty() {
                    push(&mut lines, MessageRole::Assistant, &txt, LineKind::Normal);
                }
            }
            let sp = ["\u{25d0}", "\u{25d3}", "\u{25d1}", "\u{25d2}"];
            let ch = sp[state.spinner_frame % sp.len()];
            push(&mut lines, MessageRole::Assistant, &format!("  {} Working", ch), LineKind::Normal);
        }

        // ── Scroll ──
        state.content_height = lines.len() as u16;
        let vis = area.height as usize;
        let max = state.content_height.saturating_sub(vis as u16);
        let off = state.scroll_offset.min(max);

        // ── Render ──
        Paragraph::new(lines)
            .block(Block::default().style(styles.normal))
            .scroll((off, 0))
            .render(area, buf);

        // ── Scrollbar ──
        if self.scrollbar && max > 0 {
            let mut sb = ScrollbarState::new(state.content_height as usize)
                .position(off as usize)
                .viewport_content_length(vis);
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None).end_symbol(None).track_symbol(None)
                .thumb_symbol("\u{2588}")
                .render(area, buf, &mut sb);
        }
    }
}

/// Push content blocks into the line list in order.
fn push_blocks(
    lines: &mut Vec<Line<'static>>,
    role: MessageRole,
    blocks: &[ContentBlock],
    styles: &ThemeStyles,
) {
    let push = |lines: &mut Vec<Line>, role, text: &str, kind: LineKind| {
        let base = structural_style(kind, styles);
        match kind {
            LineKind::CodeBlock | LineKind::HorizontalRule | LineKind::RoleLabel
            | LineKind::TableBorder
            | LineKind::ToolCallHeader | LineKind::ToolCallFooter
            | LineKind::ToolResultHeader | LineKind::ToolResultFooter
            | LineKind::ErrorHeader | LineKind::ErrorFooter => {
                lines.push(Line::from(Span::styled(text.to_string(), base)));
            }
            LineKind::ToolCallBody | LineKind::ToolResultBody | LineKind::ErrorBody => {
                lines.push(Line::from(Span::styled(text.to_string(), base)));
            }
            LineKind::Normal | LineKind::ListItem | LineKind::Heading(_)
            | LineKind::TableHeader | LineKind::TableRow => {
                let mut spans = Vec::new();
                if role == MessageRole::User {
                    spans.extend(user_stripe(styles));
                } else {
                    spans.push(Span::styled(" ", role_prefix_style(role, styles)));
                    spans.push(Span::styled(" ", base));
                }
                spans.push(Span::styled(" ", base));
                spans.extend(inline_spans(text, base));
                lines.push(Line::from(spans));
            }
        }
    };

    for block in blocks {
        match block {
            ContentBlock::Text { content } => {
                // tui-markdown: push pre-styled lines directly
                lines.extend(markdown_lines(content));
            }
            ContentBlock::Thinking { content, collapsed } => {
                let ind = if *collapsed { "\u{25b8}" } else { "\u{25be}" };
                push(lines, role, &format!("{} Thinking...", ind), LineKind::Normal);
                if !*collapsed {
                    for l in content.lines() {
                        push(lines, role, &format!("  {}", l), LineKind::Normal);
                    }
                } else if let Some(first) = content.lines().next() {
                    push(lines, role, &format!("  {}", first), LineKind::Normal);
                }
            }
            ContentBlock::ToolCall { name, arguments, .. } => {
                push(lines, role, &bordered_header(&format!("tool: {}", name), 60), LineKind::ToolCallHeader);
                let max = if arguments.lines().count() <= 4 { 6 } else { 4 };
                for l in arguments.lines().take(max) {
                    push(lines, role, &format!("  {}", l), LineKind::ToolCallBody);
                }
                if arguments.lines().count() > max {
                    push(lines, role, "  ...", LineKind::ToolCallBody);
                }
                push(lines, role, &bordered_footer(60), LineKind::ToolCallFooter);
            }
            ContentBlock::ToolResult { tool_name, content, is_error } => {
                let check = if *is_error { "X" } else { "ok" };
                let label = if tool_name.is_empty() { check.to_string() } else { format!("{} {}", check, tool_name) };
                push(lines, role, &bordered_header(&label, 60), LineKind::ToolResultHeader);
                for l in content.lines().take(4) {
                    push(lines, role, &format!("  {}", l), LineKind::ToolResultBody);
                }
                if content.lines().count() > 4 {
                    push(lines, role, "  ...", LineKind::ToolResultBody);
                }
                push(lines, role, &bordered_footer(60), LineKind::ToolResultFooter);
            }
            ContentBlock::Error { title, message, retryable } => {
                push(lines, role, &bordered_header(&format!("error: {}", title), 60), LineKind::ErrorHeader);
                for l in message.lines().take(4) {
                    push(lines, role, &format!("  {}", l), LineKind::ErrorBody);
                }
                if *retryable {
                    push(lines, role, "  retry: this error may be temporary", LineKind::ErrorBody);
                }
                push(lines, role, &bordered_footer(60), LineKind::ErrorFooter);
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
                push(lines, role, &format!("[image: {}, {}]", mime_type, sz_str), LineKind::Normal);
                push(lines, role, "  Ctrl+I -> open in viewer", LineKind::Normal);
            }
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
        s.stream_text("Hi");
        s.finish_streaming();
        assert!(s.streaming.is_none());
        assert_eq!(s.messages.len(), 1);
    }
}
