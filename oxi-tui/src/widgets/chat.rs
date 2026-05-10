//! ChatView widget — scrollable message list with streaming support.
//!
//! Content is built as a list of segments (text, tool boxes, errors, etc.)
//! and rendered using proper ratatui widgets:
//! - Tool/error boxes → `Block::bordered()` + `Paragraph`
//! - Text → `Paragraph` with `Wrap`
//! - Scrolling via virtual y-coordinates with segment-level clipping.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap},
};
use tui_markdown;
use unicode_width::UnicodeWidthChar;

use crate::Theme;
use crate::theme::ThemeStyles;

// ── Types ──────────────────────────────────────────────────────────────

/// Status of a tool call — tracks lifecycle from request to completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    /// LLM requested the tool; execution has not started yet.
    Requested,
    /// Tool is currently executing.
    Executing,
    /// Tool finished (result field is populated).
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
    ToolCall { id: String, name: String, arguments: String, result: Option<(String, bool)>, status: ToolCallStatus },
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
    /// Last code block extracted from assistant text (for copy functionality)
    pub last_code_block: Option<String>,
    code_block_active: bool,
    code_block_buf: String,
    /// Pending images awaiting user action
    pub pending_images: Vec<(String, String)>,
    /// Map of active tool call IDs to their content_blocks index.
    /// Used for ID-based result matching instead of position-based.
    active_tool_calls: HashMap<String, usize>,
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
        self.active_tool_calls.clear();
    }

    /// Alias for streaming text update.
    pub fn stream_text_delta(&mut self, delta: &str) {
        self.append_text(delta);
        self.update_last_code_block(delta);
        self.last_code_block = None;
    }

    /// Core text append to the active streaming content block.
    fn append_text(&mut self, text: &str) {
        if let Some(ref mut s) = self.streaming {
            if let Some(ContentBlock::Text { ref mut content }) = s.message.content_blocks.first_mut() {
                content.push_str(text);
            } else {
                s.message.content_blocks.insert(0, ContentBlock::Text { content: text.to_string() });
            }
        }
    }

    /// Alias used by app.rs for the same operation.
    pub fn stream_text(&mut self, text: &str) {
        self.append_text(text);
    }

    /// Returns true when a streaming message is in progress.
    pub fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    /// Update the last code block when new text arrives.
    fn update_last_code_block(&mut self, delta: &str) {
        if let Some(ref mut s) = self.streaming {
            if let Some(ContentBlock::Text { ref mut content }) = s.message.content_blocks.first_mut() {
                if let Some(code) = extract_last_code_block(content) {
                    self.last_code_block = Some(code);
                }
            }
        }
    }

    /// Called when streaming finishes — finalize code block extraction.
    pub fn refresh_last_code_block(&mut self) {
        if let Some(ref s) = self.streaming {
            if let Some(ContentBlock::Text { ref content, .. }) = s.message.content_blocks.first() {
                if let Some(code) = extract_last_code_block(content) {
                    self.last_code_block = Some(code);
                }
            }
        }
    }

    /// Update status of a tool call by ID. No-op if ID not found.
    pub fn set_tool_status(&mut self, id: &str, status: ToolCallStatus) {
        if let Some(ref mut s) = self.streaming {
            if let Some(&idx) = self.active_tool_calls.get(id) {
                if let Some(block) = s.message.content_blocks.get_mut(idx) {
                    if let ContentBlock::ToolCall { status: ref mut curr_status, .. } = block {
                        *curr_status = status;
                        return;
                    }
                }
            }
        }
    }

    pub fn stream_tool_call(&mut self, id: String, name: String, arguments: String, status: ToolCallStatus) {
        tracing::debug!("[TUI] stream_tool_call: id={:?}, name={:?}, status={:?}, streaming={}",
            id, name, status, self.streaming.is_some());
        if let Some(ref mut s) = self.streaming {
            // Guard against duplicate IDs (e.g. double ToolCall events from agent)
            if self.active_tool_calls.contains_key(&id) {
                tracing::debug!("[TUI] Duplicate ToolCall ID {:?} — ignoring", id);
                return;
            }
            let idx = s.message.content_blocks.len();
            s.message.content_blocks.push(ContentBlock::ToolCall {
                id: id.clone(),
                name,
                arguments,
                result: None,
                status,
            });
            self.active_tool_calls.insert(id, idx);
            tracing::debug!("[TUI] ToolCall pushed at idx={}, blocks count={}",
                idx, s.message.content_blocks.len());
        }
    }

    pub fn stream_tool_result(&mut self, tool_call_id: Option<String>, tool_name: String, content: String, is_error: bool) {
        tracing::debug!("[TUI] stream_tool_result: tool_call_id={:?}, tool_name={:?}, streaming={}",
            tool_call_id, tool_name, self.streaming.is_some());
        if let Some(ref mut s) = self.streaming {
            // ── Try ID-based matching first ──
            if let Some(ref id) = tool_call_id {
                if let Some(&idx) = self.active_tool_calls.get(id) {
                    if let Some(block) = s.message.content_blocks.get_mut(idx) {
                        if let ContentBlock::ToolCall { ref mut result, ref mut status, .. } = block {
                            *result = Some((content, is_error));
                            *status = ToolCallStatus::Done;
                            self.active_tool_calls.remove(id);
                            tracing::debug!("[TUI] ID-matched result merged into ToolCall at idx={}", idx);
                            return;
                        }
                    }
                } else {
                    tracing::debug!("[TUI] ToolResult for unknown ID {:?} — falling back", id);
                }
            }

            // ── Fallback: last block is ToolCall → merge ──
            if let Some(last) = s.message.content_blocks.last_mut() {
                if matches!(last, ContentBlock::ToolCall { .. }) {
                    if let ContentBlock::ToolCall { ref mut result, ref mut status, .. } = last {
                        *result = Some((content, is_error));
                        *status = ToolCallStatus::Done;
                        // Remove from active_tool_calls (by value search)
                        if let Some(ref id) = tool_call_id {
                            self.active_tool_calls.remove(id);
                        }
                        tracing::debug!("[TUI] Fallback merge result into last ToolCall");
                        return;
                    }
                }
            }
            // ── Final fallback: push as standalone result ──
            tracing::debug!("[TUI] PUSHING standalone ToolResult");
            s.message.content_blocks.push(ContentBlock::ToolResult { tool_name, content, is_error });
        } else {
            tracing::debug!("[TUI] FALLBACK: streaming is None, ToolResult discarded");
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
        self.active_tool_calls.clear();
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

    pub fn append_streaming_line(&mut self, text: &str) {
        if let Some(ref mut s) = self.streaming { s.line_buffer.push_str(text); }
    }

    pub fn flush_streaming_line(&mut self) {
        if self.streaming.is_none() { return; }
        let (buf, is_empty) = {
            let s = self.streaming.as_mut().unwrap();
            (s.line_buffer.trim_end().to_string(), s.line_buffer.is_empty())
        };
        if !is_empty {
            if let Some(ref mut s) = self.streaming {
                s.line_buffer.clear();
            }
            self.append_text(&buf);
        }
    }
}

// ── Code block extraction ────────────────────────────────────────────

/// Fix bare code fences (``` without a language) → ```text.
/// Also normalizes unknown/bare language tokens to "text".
///
/// Handles:
/// - ```        (bare, immediate newline)
/// - ```        (bare, followed by space/tab before newline)
/// - ```        (bare, at end-of-string / EOF)
/// - ```
/// (empty language is the most common culprit of
/// `Could not find syntax for code block: ""` warnings)
///
/// Also maps tui-markdown unsupported languages to "text" so syntect
/// can at least render them without warnings.
fn fix_bare_code_fences(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Backtick is ASCII — byte-level detection is safe.
        if bytes[i] == b'`' && i + 2 < len && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
            let after = &bytes[i + 3..];
            // "Bare" = next char is whitespace or nothing (EOF)
            let is_bare = after.first().map_or(true, |&c| c == b'\n' || c == b'\r' || c == b'\t' || c == b' ');

            if is_bare {
                result.push_str("```text");
                i += 3;
                while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                continue;
            }

            // Has a language token — check if it needs remapping
            let lang_end = after.iter().position(|&c| c == b'\n' || c == b'\r').unwrap_or(after.len());
            let lang_str = String::from_utf8_lossy(&after[..lang_end]).trim().to_lowercase();
            let needs_remap = lang_str == "text"
                || lang_str == "plaintext"
                || lang_str == "plain"
                || lang_str == "none";

            if needs_remap && !lang_str.is_empty() {
                result.push_str("```text");
                // Skip the original lang token, land on the newline/next char
                i += 3 + lang_end;
                continue;
            }
        }

        // ── Copy one full UTF-8 character ──
        // This is the critical fix: bytes[i] as char would break
        // multi-byte UTF-8 (Korean, CJK, emoji, etc.).
        let ch = content[i..].chars().next().unwrap();
        result.push(ch);
        i += ch.len_utf8();
    }
    result
}

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


/// Render markdown content via tui-markdown, converting to owned Lines.
fn markdown_lines_internal(content: &str) -> Vec<Line<'static>> {
    let preprocessed = fix_bare_code_fences(content);
    let text: ratatui::text::Text = tui_markdown::from_str(&preprocessed);
    text.lines.into_iter().map(|l| {
        let spans: Vec<Span<'static>> = l.spans
            .into_iter()
            .map(|s| Span::styled(s.content.into_owned(), s.style))
            .collect();
        Line::from(spans)
    }).collect()
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

        // Build segments
        let segments = build_segments(state, area.width);

        // Calculate total height and scroll
        let total_height: u16 = segments.last()
            .map(|s| s.y.saturating_add(s.height))
            .unwrap_or(0);
        state.content_height = total_height;
        let vis = area.height;
        let max_scroll = total_height.saturating_sub(vis);
        let off = state.scroll_offset.min(max_scroll);

        // Clear area
        Block::default().style(styles.normal).render(area, buf);

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
                .thumb_symbol("\u{2588}")
                .render(area, buf, &mut sb);
        }
    }
}

// ── Segment-based content rendering ───────────────────────────────────
//
// Instead of flattening everything into Vec<Line> with manual border
// characters, we build a list of "segments" — each an independent
// renderable unit — and render them using proper ratatui widgets:
//   - Tool/error boxes → Block::bordered() + Paragraph
//   - Text → Paragraph with Wrap
//   - Rules → Line widget
//   - User messages → Block::borders(LEFT) + Paragraph

/// A segment of content at a virtual y-position.
struct Segment {
    y: u16,
    height: u16,
    kind: SegKind,
}

/// Content kinds — each rendered with appropriate ratatui widgets.
enum SegKind {
    /// Markdown text (assistant/system messages).
    Text(Vec<Line<'static>>),
    /// Markdown text with user-message left stripe.
    UserText(Vec<Line<'static>>),
    /// Horizontal rule separator.
    Rule,
    /// Role label line ("You", etc.).
    Label { text: String, style: Style },
    /// Tool call box with optional merged result.
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
    /// Streaming spinner indicator.
    Spinner { frame: usize },
}

// ── Height measurement ────────────────────────────────────────────────

/// Measure wrapped height of text lines within a given width.
fn measure_wrapped_height(lines: &[Line<'_>], width: u16) -> u16 {
    if width == 0 { return lines.len() as u16; }
    let w = width as usize;
    lines.iter().map(|line| {
        let lw: usize = line.spans.iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        if lw == 0 { 1u16 } else { ((lw + w - 1) / w).max(1) as u16 }
    }).sum()
}

fn measure_tool_box_height(arguments: &str, result: &Option<(String, bool)>) -> u16 {
    let arg_count = arguments.lines().count();
    let max_args = if arg_count <= 3 { 5 } else { 3 };
    let mut h: u16 = 2; // top + bottom border
    h += arg_count.min(max_args) as u16;
    if arg_count > max_args { h += 1; }
    if let Some((rc, _)) = result {
        h += 1; // divider
        let rn = rc.lines().count();
        h += rn.min(6) as u16;
        if rn > 6 { h += 1; }
    }
    h
}

fn measure_tool_result_height(content: &str) -> u16 {
    let n = content.lines().count();
    2 + n.min(4) as u16 + if n > 4 { 1 } else { 0 }
}

fn measure_error_height(message: &str, retryable: bool) -> u16 {
    let n = message.lines().count();
    2 + n.min(4) as u16 + if retryable { 1 } else { 0 }
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
        SegKind::Text(lines) => measure_wrapped_height(lines, width.saturating_sub(2)),
        SegKind::UserText(lines) => measure_wrapped_height(lines, width.saturating_sub(4)),
        SegKind::Rule => 1,
        SegKind::Label { .. } => 1,
        SegKind::ToolBox { arguments, result, .. } => measure_tool_box_height(arguments, result),
        SegKind::ToolResultBox { content, .. } => measure_tool_result_height(content),
        SegKind::ErrorBox { message, retryable, .. } => measure_error_height(message, *retryable),
        SegKind::Thinking { content, collapsed } => measure_thinking_height(content, *collapsed),
        SegKind::Image { .. } => 2,
        SegKind::Spinner { .. } => 1,
    }
}

// ── Segment building ──────────────────────────────────────────────────

fn content_block_to_segkind(block: &ContentBlock, role: MessageRole) -> SegKind {
    match block {
        ContentBlock::Text { content } => {
            let lines = markdown_lines_internal(content);
            if role == MessageRole::User { SegKind::UserText(lines) } else { SegKind::Text(lines) }
        }
        ContentBlock::Thinking { content, collapsed } =>
            SegKind::Thinking { content: content.clone(), collapsed: *collapsed },
        ContentBlock::ToolCall { id: _, name, arguments, result, status } =>
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
                text: "You".to_string(), style: Style::default().bold(),
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
        if !streaming.line_buffer.is_empty() {
            let txt = streaming.line_buffer.trim_end().to_string();
            if !txt.is_empty() {
                let lines = vec![Line::from(Span::raw(txt))];
                let h = measure_wrapped_height(&lines, width.saturating_sub(4));
                segments.push(Segment { y, height: h, kind: SegKind::Text(lines) });
                y += h;
            }
        }
        segments.push(Segment { y, height: 1, kind: SegKind::Spinner { frame: state.spinner_frame } });
        y += 1;
    }

    segments
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
            let vis: Vec<Line<'static>> = lines.iter().skip(skip).take(rect.height as usize).cloned().collect();
            Paragraph::new(vis).wrap(Wrap { trim: false }).render(rect, buf);
        }

        SegKind::UserText(lines) => {
            // Left stripe: ▌ + space (2 cells)
            for row in rect.top()..rect.bottom() {
                if rect.x + 1 < rect.x + rect.width {
                    buf[(rect.x, row)].set_char('\u{258c}').set_style(styles.user_border);
                    buf[(rect.x + 1, row)].set_char(' ').set_style(styles.user_bg);
                }
            }
            let text_rect = Rect {
                x: rect.x + 2,
                y: rect.y,
                width: rect.width.saturating_sub(2),
                height: rect.height,
            };
            let skip = rows_hidden as usize;
            let vis: Vec<Line<'static>> = lines.iter().skip(skip).take(text_rect.height as usize).cloned().collect();
            Paragraph::new(vis)
                .style(styles.normal)
                .wrap(Wrap { trim: false })
                .render(text_rect, buf);
        }

        SegKind::Rule => {
            let line = "\u{2500}".repeat(rect.width as usize);
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
            let ind = if *collapsed { "\u{25b8}" } else { "\u{25be}" };
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
            let vis: Vec<Line<'static>> = lines.iter().skip(skip).take(rect.height as usize).cloned().collect();
            Paragraph::new(vis).render(rect, buf);
        }

        SegKind::Image { mime_type, size_str } => {
            let lines = vec![
                Line::from(Span::styled(format!("[image: {}, {}]", mime_type, size_str), styles.normal)),
                Line::from(Span::styled("  Ctrl+I -> open in viewer", styles.muted)),
            ];
            let skip = rows_hidden as usize;
            let vis: Vec<Line<'static>> = lines.iter().skip(skip).take(rect.height as usize).cloned().collect();
            Paragraph::new(vis).render(rect, buf);
        }

        SegKind::Spinner { frame } => {
            let sp = ["\u{25d0}", "\u{25d3}", "\u{25d1}", "\u{25d2}"];
            let ch = sp[frame % sp.len()];
            Paragraph::new(Line::from(Span::styled(
                format!("  {} Working", ch), styles.accent,
            ))).render(rect, buf);
        }
    }
}

// ── Box rendering with ratatui Block ──────────────────────────────────

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
        ToolCallStatus::Requested => ("\u{23f3}", styles.muted.fg.unwrap_or(Color::White)),
        ToolCallStatus::Executing => ("\u{2699}", styles.warning.fg.unwrap_or(Color::Yellow)),
        ToolCallStatus::Done => ("\u{2713}", styles.success.fg.unwrap_or(Color::Green)),
    };

    // Build inner content lines
    let mut content_lines: Vec<Line<'static>> = Vec::new();
    let arg_count = arguments.lines().count();
    let max_args = if arg_count <= 3 { 5 } else { 3 };
    for l in arguments.lines().take(max_args) {
        content_lines.push(Line::from(Span::styled(l.to_string(), styles.normal)));
    }
    if arg_count > max_args {
        content_lines.push(Line::from(Span::styled(" ...", styles.muted)));
    }
    if let Some((result_content, is_error)) = result {
        content_lines.push(Line::from(Span::styled(
            "\u{2500}".repeat(40),
            if *is_error { styles.error } else { styles.success },
        )));
        let rn = result_content.lines().count();
        for l in result_content.lines().take(6) {
            content_lines.push(Line::from(Span::styled(
                l.to_string(),
                if *is_error { styles.error } else { styles.normal },
            )));
        }
        if rn > 6 {
            content_lines.push(Line::from(Span::styled(" ...", styles.muted)));
        }
    }

    // Determine visible borders based on which rows are clipped
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
            Style::default().fg(label_fg).bold(),
        ));
    }

    let inner = block.inner(rect);
    block.render(rect, buf);

    // Content starts after top border. Skip rows that were scrolled past.
    let content_skip = rows_hidden.saturating_sub(top_border_rows) as usize;
    let vis: Vec<Line<'static>> = content_lines.into_iter()
        .skip(content_skip)
        .take(inner.height as usize)
        .collect();
    if !vis.is_empty() {
        Paragraph::new(vis).render(inner, buf);
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
    let (check, border_style) = if is_error {
        ("X", styles.error)
    } else {
        ("ok", styles.muted)
    };
    let label = if tool_name.is_empty() { check.to_string() } else { format!("{} {}", check, tool_name) };
    let label_fg = if is_error { Color::White } else { styles.success.fg.unwrap_or(Color::Green) };

    let mut content_lines: Vec<Line<'static>> = Vec::new();
    for l in content.lines().take(4) {
        content_lines.push(Line::from(Span::styled(l.to_string(), if is_error { styles.error } else { styles.normal })));
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
            Style::default().fg(label_fg).bold(),
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
            Style::default().fg(Color::White).bold(),
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
        s.stream_text("Hi");
        s.finish_streaming();
        assert!(s.streaming.is_none());
        assert_eq!(s.messages.len(), 1);
    }
}
