//! ChatView widget — scrollable message list with streaming support.
//!
//! Segment-based rendering:
//! - Each content block becomes a Segment with a measured height.
//! - Scrolling clips segments at pixel-perfect boundaries.
//! - Tool/error boxes use ratatui Block::bordered() + Paragraph with Wrap.
//! - Text uses Paragraph with Wrap for proper word-breaking.
//!
//! Height is measured via Paragraph::line_count(Wrap) — matches rendering exactly.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap},
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

// ── Segment-based rendering ───────────────────────────────────────────
//
// Each content block maps to a Segment with:
//   - y: virtual y position
//   - height: measured via Paragraph::line_count(Wrap) for exact match
//   - kind: determines how to render
//
// Scrolling clips segments and renders only visible portions.

struct Segment {
    y: u16,
    height: u16,
    kind: SegKind,
}

enum SegKind {
    /// Markdown text (assistant/system messages). Rendered with Wrap.
    Text(Vec<Line<'static>>),
    /// User message text with left accent stripe. Rendered with Wrap.
    UserText(Vec<Line<'static>>),
    /// Horizontal rule separator.
    Rule,
    /// Role label line.
    Label { text: String, style: Style },
    /// Tool call box — rendered as Block::bordered() + Paragraph.
    ToolBox {
        name: String,
        arguments: String,
        result: Option<(String, bool)>,
        status: ToolCallStatus,
    },
    /// Standalone tool result box.
    ToolResultBox {
        tool_name: String,
        content: String,
        is_error: bool,
    },
    /// Error box.
    ErrorBox {
        title: String,
        message: String,
        retryable: bool,
    },
    /// Thinking block.
    Thinking { content: String, collapsed: bool },
    /// Image reference.
    Image { mime_type: String, size_str: String },
    /// Streaming spinner.
    Spinner { frame: usize },
}

// ── Height measurement ────────────────────────────────────────────────

/// Measure wrapped height — uses the same Paragraph::line_count that
/// ratatui uses internally for rendering. Guarantees measurement == rendering.
fn measure_wrapped_height(lines: &[Line<'_>], width: u16) -> u16 {
    if width < 1 { return lines.len() as u16; }
    let text: ratatui::text::Text = lines.iter().cloned().collect();
    let para = Paragraph::new(text).wrap(Wrap { trim: false });
    para.line_count(width) as u16
}

/// Measure a tool box: border (2) + wrapped argument lines + optional result.
fn measure_tool_box_height(arguments: &str, result: &Option<(String, bool)>, inner_width: u16) -> u16 {
    let arg_count = arguments.lines().count();
    let max_args = if arg_count <= 3 { 5 } else { 3 };
    let arg_lines: Vec<Line<'static>> = arguments.lines()
        .take(max_args)
        .map(|l| Line::from(Span::raw(l.to_string())))
        .collect();
    let mut h: u16 = 2; // top + bottom border
    h += measure_wrapped_height(&arg_lines, inner_width);
    if arg_count > max_args { h += 1; }
    if let Some((rc, _)) = result {
        h += 1; // divider line
        let result_lines: Vec<Line<'static>> = rc.lines()
            .take(6)
            .map(|l| Line::from(Span::raw(l.to_string())))
            .collect();
        let rn = rc.lines().count();
        h += measure_wrapped_height(&result_lines, inner_width);
        if rn > 6 { h += 1; }
    }
    h
}

fn measure_tool_result_height(content: &str, inner_width: u16) -> u16 {
    let lines: Vec<Line<'static>> = content.lines()
        .take(4)
        .map(|l| Line::from(Span::raw(l.to_string())))
        .collect();
    let n = content.lines().count();
    2 + measure_wrapped_height(&lines, inner_width) + if n > 4 { 1 } else { 0 }
}

fn measure_error_height(message: &str, retryable: bool, inner_width: u16) -> u16 {
    let lines: Vec<Line<'static>> = message.lines()
        .take(4)
        .map(|l| Line::from(Span::raw(l.to_string())))
        .collect();
    2 + measure_wrapped_height(&lines, inner_width) + if retryable { 1 } else { 0 }
}

fn measure_thinking_height(content: &str, collapsed: bool) -> u16 {
    if collapsed {
        1 + if content.lines().next().is_some() { 1 } else { 0 }
    } else {
        1 + content.lines().count() as u16
    }
}

fn measure_segment(kind: &SegKind, width: u16) -> u16 {
    match kind {
        SegKind::Text(lines) => measure_wrapped_height(lines, width),
        SegKind::UserText(lines) => measure_wrapped_height(lines, width.saturating_sub(2)),
        SegKind::Rule => 1,
        SegKind::Label { .. } => 1,
        SegKind::ToolBox { arguments, result, .. } =>
            measure_tool_box_height(arguments, result, width.saturating_sub(2)),
        SegKind::ToolResultBox { content, .. } =>
            measure_tool_result_height(content, width.saturating_sub(2)),
        SegKind::ErrorBox { message, retryable, .. } =>
            measure_error_height(message, *retryable, width.saturating_sub(2)),
        SegKind::Thinking { content, collapsed } => measure_thinking_height(content, *collapsed),
        SegKind::Image { .. } => 2,
        SegKind::Spinner { .. } => 1,
    }
}

// ── Segment building ──────────────────────────────────────────────────

fn content_block_to_segkind(block: &ContentBlock, role: MessageRole) -> SegKind {
    match block {
        ContentBlock::Text { content } => {
            let lines = md_lines(content);
            if role == MessageRole::User { SegKind::UserText(lines) } else { SegKind::Text(lines) }
        }
        ContentBlock::Thinking { content, collapsed } =>
            SegKind::Thinking { content: content.clone(), collapsed: *collapsed },
        ContentBlock::ToolCall { name, arguments, result, status, .. } =>
            SegKind::ToolBox { name: name.clone(), arguments: arguments.clone(), result: result.clone(), status: *status },
        ContentBlock::ToolResult { tool_name, content, is_error } =>
            SegKind::ToolResultBox { tool_name: tool_name.clone(), content: content.clone(), is_error: *is_error },
        ContentBlock::Error { title, message, retryable } =>
            SegKind::ErrorBox { title: title.clone(), message: message.clone(), retryable: *retryable },
        ContentBlock::Image { mime_type, base64_data } => {
            let sz = base64_data.len() * 3 / 4;
            let sz_str = if sz >= 1_048_576 {
                format!("{:.1} MB", sz as f64 / 1_048_576.0)
            } else if sz >= 1024 {
                format!("{:.1} KB", sz as f64 / 1024.0)
            } else {
                format!("{} B", sz)
            };
            SegKind::Image { mime_type: mime_type.clone(), size_str: sz_str }
        }
    }
}

fn build_segments(state: &ChatViewState, width: u16) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut y: u16 = 0;

    for (i, msg) in state.messages.iter().enumerate() {
        if msg.role == MessageRole::User && i > 0 {
            segments.push(Segment { y, height: 1, kind: SegKind::Rule });
            y += 1;
        }
        if msg.role == MessageRole::User {
            segments.push(Segment { y, height: 1, kind: SegKind::Label {
                text: "You".to_string(),
                style: Style::default().add_modifier(Modifier::BOLD),
            }});
            y += 1;
        }
        for block in &msg.content_blocks {
            let kind = content_block_to_segkind(block, msg.role);
            let h = measure_segment(&kind, width);
            segments.push(Segment { y, height: h, kind });
            y += h;
        }
    }

    if let Some(ref streaming) = state.streaming {
        for block in &streaming.message.content_blocks {
            let kind = content_block_to_segkind(block, MessageRole::Assistant);
            let h = measure_segment(&kind, width);
            segments.push(Segment { y, height: h, kind });
            y += h;
        }
        segments.push(Segment { y, height: 1, kind: SegKind::Spinner { frame: state.spinner_frame } });
        y += 1;
    }

    segments
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

        let segments = build_segments(state, area.width);

        let total_height: u16 = segments.last()
            .map(|s| s.y.saturating_add(s.height))
            .unwrap_or(0);
        state.content_height = total_height;

        let vis = area.height;
        let max_scroll = total_height.saturating_sub(vis);
        let off = state.scroll_offset.min(max_scroll);

        // Clear area
        Block::default().style(Style::default().bg(self.theme.colors.background.to_ratatui())).render(area, buf);

        // Render visible segments
        for seg in &segments {
            let seg_bottom = seg.y.saturating_add(seg.height);
            let view_top = off;
            let view_bottom = off.saturating_add(vis);

            if seg_bottom <= view_top || seg.y >= view_bottom { continue; }

            let screen_top = seg.y.saturating_sub(view_top);
            let screen_bottom = seg_bottom.saturating_sub(view_top).min(vis);
            let render_h = screen_bottom.saturating_sub(screen_top);
            if render_h == 0 { continue; }

            let rect = Rect {
                x: area.x,
                y: area.y + screen_top,
                width: area.width,
                height: render_h,
            };

            let rows_hidden = view_top.saturating_sub(seg.y);
            render_segment(seg, rect, buf, &styles, rows_hidden);
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

// ── Segment rendering ─────────────────────────────────────────────────

fn render_segment(
    seg: &Segment,
    rect: Rect,
    buf: &mut Buffer,
    styles: &ThemeStyles,
    rows_hidden: u16,
) {
    match &seg.kind {
        SegKind::Text(lines) => {
            let skip = rows_hidden as usize;
            let vis: ratatui::text::Text = lines.iter()
                .skip(skip)
                .take(rect.height as usize)
                .cloned()
                .collect();
            Paragraph::new(vis).wrap(Wrap { trim: false }).style(styles.normal).render(rect, buf);
        }

        SegKind::UserText(lines) => {
            // Left stripe via Block with LEFT border + padding
            let stripe_block = Block::default()
                .borders(Borders::LEFT)
                .border_style(styles.user_border)
                .padding(Padding::new(1, 0, 0, 0));
            let text_rect = stripe_block.inner(rect);
            stripe_block.render(rect, buf);
            let skip = rows_hidden as usize;
            let vis: ratatui::text::Text = lines.iter()
                .skip(skip)
                .take(text_rect.height as usize)
                .cloned()
                .collect();
            Paragraph::new(vis)
                .style(styles.normal)
                .wrap(Wrap { trim: false })
                .render(text_rect, buf);
        }

        SegKind::Rule => {
            let line = "-".repeat(rect.width as usize);
            Line::from(Span::styled(line, styles.muted)).render(rect, buf);
        }

        SegKind::Label { text, style } => {
            Paragraph::new(Line::from(Span::styled(text.clone(), *style)))
                .render(rect, buf);
        }

        SegKind::ToolBox { name, arguments, result, status } => {
            render_tool_box(name, arguments, result, status, rect, buf, styles, seg.height, rows_hidden);
        }

        SegKind::ToolResultBox { tool_name, content, is_error } => {
            render_tool_result_box(tool_name, content, *is_error, rect, buf, styles, seg.height, rows_hidden);
        }

        SegKind::ErrorBox { title, message, retryable } => {
            render_error_box(title, message, *retryable, rect, buf, styles, seg.height, rows_hidden);
        }

        SegKind::Thinking { content, collapsed } => {
            let ind = if *collapsed { ">" } else { "v" };
            let mut lines: Vec<Line<'static>> = vec![
                Line::from(Span::styled(format!("{} Thinking...", ind), styles.accent)),
            ];
            if !*collapsed {
                for l in content.lines() {
                    lines.push(Line::from(Span::styled(format!("  {}", l), styles.muted)));
                }
            } else if let Some(first) = content.lines().next() {
                lines.push(Line::from(Span::styled(format!("  {}", first), styles.muted)));
            }
            let skip = rows_hidden as usize;
            let vis: ratatui::text::Text = lines.into_iter().skip(skip).take(rect.height as usize).collect();
            Paragraph::new(vis).render(rect, buf);
        }

        SegKind::Image { mime_type, size_str } => {
            let lines = vec![
                Line::from(Span::styled(format!("[image: {}, {}]", mime_type, size_str), styles.normal)),
                Line::from(Span::styled("  Ctrl+I -> open in viewer", styles.muted)),
            ];
            let skip = rows_hidden as usize;
            let vis: ratatui::text::Text = lines.into_iter().skip(skip).take(rect.height as usize).collect();
            Paragraph::new(vis).render(rect, buf);
        }

        SegKind::Spinner { frame } => {
            let sp = ["|", "/", "-", "\\"];
            let ch = sp[frame % sp.len()];
            Paragraph::new(Line::from(Span::styled(
                format!("  {} Working...", ch), styles.accent,
            ))).render(rect, buf);
        }
    }
}

// ── Box rendering with ratatui Block::bordered() ──────────────────────

fn render_tool_box(
    name: &str,
    arguments: &str,
    result: &Option<(String, bool)>,
    status: &ToolCallStatus,
    rect: Rect,
    buf: &mut Buffer,
    styles: &ThemeStyles,
    natural_height: u16,
    rows_hidden: u16,
) {
    let (icon, label_fg) = match status {
        ToolCallStatus::Requested => ("...", styles.muted.fg.unwrap_or(ratatui::style::Color::White)),
        ToolCallStatus::Executing => ("*run", styles.warning.fg.unwrap_or(ratatui::style::Color::Yellow)),
        ToolCallStatus::Done => ("ok", styles.success.fg.unwrap_or(ratatui::style::Color::Green)),
    };

    // Build inner content lines
    let mut content_lines: Vec<Line<'static>> = Vec::new();
    let arg_count = arguments.lines().count();
    let max_args = if arg_count <= 3 { 5 } else { 3 };
    for l in arguments.lines().take(max_args) {
        content_lines.push(Line::from(Span::styled(l.to_string(), styles.normal)));
    }
    let has_arg_truncate = arg_count > max_args;
    if has_arg_truncate {
        content_lines.push(Line::from(Span::styled(" ...", styles.muted)));
    }
    // Divider placeholder
    content_lines.push(Line::raw("")); // divider
    if let Some((result_content, _)) = result {
        let rn = result_content.lines().count();
        for l in result_content.lines().take(6) {
            content_lines.push(Line::from(Span::styled(l.to_string(), styles.normal)));
        }
        if rn > 6 {
            content_lines.push(Line::from(Span::styled(" ...", styles.muted)));
        }
    }

    // Determine border visibility based on clipping
    let show_top = rows_hidden == 0;
    let show_bottom = rows_hidden + rect.height >= natural_height;
    let top_border_rows = if show_top { 1u16 } else { 0 };

    let mut borders = Borders::LEFT | Borders::RIGHT;
    if show_top { borders |= Borders::TOP; }
    if show_bottom { borders |= Borders::BOTTOM; }

    let mut block = Block::default()
        .borders(borders)
        .border_style(styles.muted);
    if show_top {
        block = block.title(Span::styled(
            format!(" {} tool: {} ", icon, name),
            Style::default().fg(label_fg).add_modifier(Modifier::BOLD),
        ));
    }

    let inner = block.inner(rect);
    block.render(rect, buf);

    // Content skip for scrolled-past rows
    let content_skip = rows_hidden.saturating_sub(top_border_rows) as usize;
    let vis: Vec<Line<'static>> = content_lines.iter().cloned()
        .enumerate()
        .filter_map(|(i, line)| {
            if i < content_skip { return None; }
            let row = i - content_skip;
            if row >= inner.height as usize { return None; }
            Some(line)
        })
        .collect();

    if !vis.is_empty() {
        Paragraph::new(vis).render(inner, buf);
    }

    // Render divider as a full-width horizontal line
    if result.is_some() {
        let arg_lines = max_args.min(arg_count) as usize;
        let truncate_offset = if has_arg_truncate { 1 } else { 0 };
        let divider_in_content = arg_lines + truncate_offset;
        let visible_divider_row = divider_in_content.saturating_sub(content_skip);
        if visible_divider_row < inner.height as usize {
            let divider_y = inner.y + visible_divider_row as u16;
            let dash_count = inner.width.saturating_sub(2) as usize;
            let dash_line = "-".repeat(dash_count.max(1));
            let div_rect = Rect { x: inner.x + 1, y: divider_y, width: (dash_count + 2) as u16, height: 1 };
            let div_style = if let Some((_, is_err)) = result {
                if *is_err { styles.error } else { styles.success }
            } else { styles.muted };
            Line::from(Span::styled(format!(" {} ", dash_line), div_style))
                .render(div_rect, buf);
        }
    }
}

fn render_tool_result_box(
    tool_name: &str,
    content: &str,
    is_error: bool,
    rect: Rect,
    buf: &mut Buffer,
    styles: &ThemeStyles,
    natural_height: u16,
    rows_hidden: u16,
) {
    let (check, border_style) = if is_error { ("X", styles.error) } else { ("ok", styles.muted) };
    let label = if tool_name.is_empty() { check.to_string() } else { format!("{} {}", check, tool_name) };
    let label_fg = if is_error {
        ratatui::style::Color::White
    } else {
        styles.success.fg.unwrap_or(ratatui::style::Color::Green)
    };

    let mut content_lines: Vec<Line<'static>> = Vec::new();
    let content_style = if is_error { styles.error } else { styles.normal };
    for l in content.lines().take(4) {
        content_lines.push(Line::from(Span::styled(l.to_string(), content_style)));
    }
    if content.lines().count() > 4 {
        content_lines.push(Line::from(Span::styled(" ...", styles.muted)));
    }

    let show_top = rows_hidden == 0;
    let show_bottom = rows_hidden + rect.height >= natural_height;
    let top_border_rows = if show_top { 1u16 } else { 0 };

    let mut borders = Borders::LEFT | Borders::RIGHT;
    if show_top { borders |= Borders::TOP; }
    if show_bottom { borders |= Borders::BOTTOM; }

    let mut block = Block::default()
        .borders(borders)
        .border_style(border_style);
    if show_top {
        block = block.title(Span::styled(
            format!(" {} ", label),
            Style::default().fg(label_fg).add_modifier(Modifier::BOLD),
        ));
    }

    let inner = block.inner(rect);
    block.render(rect, buf);

    let content_skip = rows_hidden.saturating_sub(top_border_rows) as usize;
    let vis: Vec<Line<'static>> = content_lines.into_iter()
        .skip(content_skip)
        .take(inner.height as usize)
        .collect();
    if !vis.is_empty() {
        Paragraph::new(vis).render(inner, buf);
    }
}

fn render_error_box(
    title: &str,
    message: &str,
    retryable: bool,
    rect: Rect,
    buf: &mut Buffer,
    styles: &ThemeStyles,
    natural_height: u16,
    rows_hidden: u16,
) {
    let mut content_lines: Vec<Line<'static>> = Vec::new();
    for l in message.lines().take(4) {
        content_lines.push(Line::from(Span::styled(l.to_string(), styles.normal)));
    }
    if retryable {
        content_lines.push(Line::from(Span::styled("retry: this error may be temporary", styles.muted)));
    }

    let show_top = rows_hidden == 0;
    let show_bottom = rows_hidden + rect.height >= natural_height;
    let top_border_rows = if show_top { 1u16 } else { 0 };

    let mut borders = Borders::LEFT | Borders::RIGHT;
    if show_top { borders |= Borders::TOP; }
    if show_bottom { borders |= Borders::BOTTOM; }

    let mut block = Block::default()
        .borders(borders)
        .border_style(styles.error);
    if show_top {
        block = block.title(Span::styled(
            format!(" error: {} ", title),
            Style::default().fg(ratatui::style::Color::White).add_modifier(Modifier::BOLD),
        ));
    }

    let inner = block.inner(rect);
    block.render(rect, buf);

    let content_skip = rows_hidden.saturating_sub(top_border_rows) as usize;
    let vis: Vec<Line<'static>> = content_lines.into_iter()
        .skip(content_skip)
        .take(inner.height as usize)
        .collect();
    if !vis.is_empty() {
        Paragraph::new(vis).render(inner, buf);
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
    fn measure_wrapped_matches_render() {
        // A line longer than width should wrap
        let lines = vec![Line::from(Span::raw("abcdefghij"))];
        let height = measure_wrapped_height(&lines, 5);
        assert_eq!(height, 2, "10-char line in width 5 should be 2 rows");
    }

    #[test]
    fn build_segments_basic() {
        let theme = Theme::dark();
        let styles = theme.to_styles();
        let mut s = ChatViewState::new();
        s.messages.push(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text { content: "Hello".into() }],
            timestamp: 0,
        });
        let segs = build_segments(&s, 80);
        assert!(!segs.is_empty());
        // Should have: Label("You") + Text
        assert!(segs.iter().any(|s| matches!(&s.kind, SegKind::Label { text, .. } if text == "You")));
    }

    #[test]
    fn fix_bare_code_fences_basic() {
        let input = "```\ncode\n```";
        let fixed = fix_bare_code_fences(input);
        assert!(fixed.starts_with("```text"));
    }
}
