//! ChatView widget — scrollable message list with streaming support.
//!
//! Uses `tui-scrollview` for scrolling. This lets us render each content
//! block as a proper ratatui widget (Block::bordered, Paragraph::wrap, etc.)
//! into a virtual buffer, and the ScrollView handles scrolling/clipping.
//!
//! Benefits over manual approaches:
//! - Tool/error boxes use Block::bordered() — real ratatui borders
//! - Text uses Paragraph::wrap(Wrap) — proper word-wrapping
//! - No measurement/render mismatch — we render once, ScrollView clips
//! - pending_images works — images are tracked in stream methods
//! - Layout caching — only recomputes when state actually changes
//! - Truncation at ingest — no height inflation from monster inputs

use std::collections::HashMap;

use parking_lot::RwLock;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidget, Widget, Wrap},
};
use tui_scrollview::{ScrollView, ScrollViewState, ScrollbarVisibility};
use tui_markdown;
use crate::Theme;
use crate::theme::ThemeStyles;

// ── Limits (truncation at ingest) ──────────────────────────────────────

const MAX_TOOL_ARG_CHARS: usize = 50_000;
const MAX_TOOL_ARG_LINES: usize = 200;
const MAX_TOOL_RESULT_CHARS: usize = 50_000;
const MAX_TOOL_RESULT_LINES: usize = 100;
const MAX_TEXT_CHARS: usize = 500_000;

fn clamp_str(s: String, max_chars: usize, max_lines: usize) -> String {
    let n = s.chars().count();
    let lines = s.lines().count();
    if n <= max_chars && lines <= max_lines {
        return s;
    }
    let truncated: String = s
        .chars()
        .take(max_chars)
        .collect();
    let truncated_lines: Vec<&str> = truncated.lines().take(max_lines).collect();
    let mut result = truncated_lines.join("\n");
    // Add overflow marker if we cut anything
    if n > max_chars || lines > max_lines {
        result.push_str("\n ...");
    }
    result
}

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
}

// ── Layout Cache ──────────────────────────────────────────────────────
//
// Caches the result of compute_layout(). Invalidated when any of these change:
// - messages.len()
// - streaming content block count
// - spinner_frame
// - width
//
// Uses parking_lot::RwLock so multiple readers can access concurrently.

struct LayoutCache {
    /// Last known messages count
    msg_count: usize,
    /// Last known streaming block count
    streaming_len: usize,
    /// Last known spinner frame
    spinner_frame: usize,
    /// Last known width
    width: u16,
    /// Cached layout entries (None = needs recompute)
    entries: Option<Vec<LayoutEntry>>,
    /// Cached total content height
    total_height: u16,
}

impl std::fmt::Debug for LayoutCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutCache")
            .field("msg_count", &self.msg_count)
            .field("streaming_len", &self.streaming_len)
            .field("spinner_frame", &self.spinner_frame)
            .field("width", &self.width)
            .field("entries", &self.entries.as_ref().map(|v| v.len()))
            .field("total_height", &self.total_height)
            .finish()
    }
}

impl Default for LayoutCache {
    fn default() -> Self {
        Self {
            msg_count: 0,
            streaming_len: 0,
            spinner_frame: 0,
            width: 0,
            entries: None,
            total_height: 0,
        }
    }
}

// ── ChatViewState ──────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct ChatViewState {
    pub messages: Vec<ChatMessage>,
    pub streaming: Option<StreamingState>,
    pub spinner_frame: usize,
    pub content_height: u16,
    pub last_code_block: Option<String>,
    pub pending_images: Vec<(String, String)>,
    tool_tracker: ToolCallTracker,
    /// ScrollView state — manages scroll position
    pub scroll_state: ScrollViewState,
    /// Layout cache — guarded by RwLock
    layout_cache: RwLock<LayoutCache>,
}

impl ChatViewState {
    pub fn new() -> Self { Self::default() }

    pub fn scroll_to_bottom(&mut self, _visible: u16) {
        self.scroll_state.scroll_to_bottom();
    }
    pub fn scroll_up(&mut self, n: u16) {
        for _ in 0..n { self.scroll_state.scroll_up(); }
    }
    pub fn scroll_down(&mut self, n: u16) {
        for _ in 0..n { self.scroll_state.scroll_down(); }
    }
    pub fn scroll_to_top(&mut self) {
        self.scroll_state.scroll_to_top();
    }

    pub fn start_streaming(&mut self) {
        self.streaming = Some(StreamingState {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content_blocks: Vec::new(),
                timestamp: 0,
            },
        });
        self.tool_tracker.clear();
    }

    pub fn stream_text_delta(&mut self, delta: &str) {
        self.append_text(delta);
        self.update_last_code_block();
    }

    fn append_text(&mut self, text: &str) {
        if let Some(ref mut s) = self.streaming {
            if let Some(ContentBlock::Text { ref mut content }) = s.message.content_blocks.first_mut() {
                // Clamp total text size to prevent unbounded growth
                if content.chars().count() > MAX_TEXT_CHARS {
                    return;
                }
                let new_chars = text.chars().count();
                if content.chars().count() + new_chars > MAX_TEXT_CHARS {
                    // Truncate delta if it would exceed the limit
                    let remaining = MAX_TEXT_CHARS.saturating_sub(content.chars().count());
                    let taken: String = text.chars().take(remaining).collect();
                    content.push_str(&taken);
                } else {
                    content.push_str(text);
                }
            } else {
                let truncated = if text.chars().count() > MAX_TEXT_CHARS {
                    let c: String = text.chars().take(MAX_TEXT_CHARS).collect();
                    format!("{}\n ...", c)
                } else {
                    text.to_string()
                };
                s.message.content_blocks.insert(0, ContentBlock::Text { content: truncated });
            }
        }
    }

    pub fn is_streaming(&self) -> bool { self.streaming.is_some() }

    fn update_last_code_block(&mut self) {
        if let Some(ref s) = self.streaming {
            if let Some(ContentBlock::Text { ref content, .. }) = s.message.content_blocks.first() {
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
        // If streaming has already finished (e.g., MessageEnd came before
        // ToolExecutionStart), start a new streaming message so the tool
        // call block is visible in the UI.
        if self.streaming.is_none() {
            self.start_streaming();
        }
        if let Some(ref mut s) = self.streaming {
            let idx = s.message.content_blocks.len();
            if !self.tool_tracker.register(id.clone(), idx) { return; }
            s.message.content_blocks.push(ContentBlock::ToolCall {
                id,
                name,
                arguments: clamp_str(arguments, MAX_TOOL_ARG_CHARS, MAX_TOOL_ARG_LINES),
                result: None,
                status,
            });
        }
    }

    pub fn stream_tool_result(&mut self, tool_call_id: Option<String>, tool_name: String, content: String, is_error: bool) {
        if self.streaming.is_none() {
            self.start_streaming();
        }
        if let Some(ref mut s) = self.streaming {
            if let Some(ref id) = tool_call_id {
                if let Some(idx) = self.tool_tracker.find_and_remove(id) {
                    if let Some(block) = s.message.content_blocks.get_mut(idx) {
                        if let ContentBlock::ToolCall { ref mut result, ref mut status, .. } = block {
                            *result = Some((
                                clamp_str(content, MAX_TOOL_RESULT_CHARS, MAX_TOOL_RESULT_LINES),
                                is_error,
                            ));
                            *status = ToolCallStatus::Done;
                            return;
                        }
                    }
                }
            }
            if let Some(last) = s.message.content_blocks.last_mut() {
                if let ContentBlock::ToolCall { ref mut result, ref mut status, .. } = last {
                    *result = Some((
                        clamp_str(content, MAX_TOOL_RESULT_CHARS, MAX_TOOL_RESULT_LINES),
                        is_error,
                    ));
                    *status = ToolCallStatus::Done;
                    if let Some(ref id) = tool_call_id { self.tool_tracker.remove(id); }
                    return;
                }
            }
            s.message.content_blocks.push(ContentBlock::ToolResult {
                tool_name,
                content: clamp_str(content, MAX_TOOL_RESULT_CHARS, MAX_TOOL_RESULT_LINES),
                is_error,
            });
        }
    }

    pub fn stream_error(&mut self, title: String, message: String, retryable: bool) {
        if let Some(ref mut s) = self.streaming {
            s.message.content_blocks.push(ContentBlock::Error {
                title,
                message: clamp_str(message, 5000, 50),
                retryable,
            });
        }
    }

    pub fn stream_thinking(&mut self, content: String, collapsed: bool) {
        if let Some(ref mut s) = self.streaming {
            // If the last block is already a Thinking block, append to it.
            // Otherwise create a new one.
            if let Some(ContentBlock::Thinking { content: existing, .. }) = s.message.content_blocks.last_mut() {
                existing.push_str(&content);
                *existing = clamp_str(existing.clone(), 50_000, 200);
            } else {
                s.message.content_blocks.push(ContentBlock::Thinking {
                    content: clamp_str(content, 50_000, 200),
                    collapsed,
                });
            }
        }
    }

    pub fn stream_image(&mut self, mime_type: String, base64_data: String) {
        if let Some(ref mut s) = self.streaming {
            // Track for Ctrl+I viewer
            self.pending_images.push((base64_data.clone(), mime_type.clone()));
            s.message.content_blocks.push(ContentBlock::Image { mime_type, base64_data });
        }
    }

    pub fn finish_streaming(&mut self) {
        if let Some(s) = self.streaming.take() {
            self.messages.push(s.message);
        }
        // Invalidate cache
        let mut cache = self.layout_cache.write();
        cache.entries = None;
    }

    pub fn cancel_streaming(&mut self) {
        self.streaming = None;
        // Invalidate cache
        let mut cache = self.layout_cache.write();
        cache.entries = None;
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.streaming = None;
        self.scroll_state = ScrollViewState::default();
        self.last_code_block = None;
        self.pending_images.clear();
        self.tool_tracker.clear();
        let mut cache = self.layout_cache.write();
        cache.entries = None;
    }

    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        let mut cache = self.layout_cache.write();
        cache.entries = None;
    }

    pub fn add_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        self.streaming = None;
        self.last_code_block = None;
        let mut cache = self.layout_cache.write();
        cache.entries = None;
    }

    pub fn push_system_message(&mut self, content: String) {
        self.messages.push(ChatMessage {
            role: MessageRole::System,
            content_blocks: vec![ContentBlock::Text { content }],
            timestamp: 0,
        });
        let mut cache = self.layout_cache.write();
        cache.entries = None;
    }

    /// Get cached layout entries, recomputing if needed.
    fn get_layout(&self, width: u16) -> Vec<LayoutEntry> {
        let msg_count = self.messages.len();
        let streaming_len = self.streaming.as_ref().map(|s| s.message.content_blocks.len()).unwrap_or(0);
        let spinner = self.spinner_frame;

        {
            let cache = self.layout_cache.read();
            if cache.entries.is_some()
                && cache.msg_count == msg_count
                && cache.streaming_len == streaming_len
                && cache.spinner_frame == spinner
                && cache.width == width
            {
                return cache.entries.clone().unwrap();
            }
        }

        // Recompute outside the read lock
        let entries = compute_layout(self, width);
        let total_height = entries.last().map(|e| e.y.saturating_add(e.height)).unwrap_or(0);

        {
            let mut cache = self.layout_cache.write();
            cache.msg_count = msg_count;
            cache.streaming_len = streaming_len;
            cache.spinner_frame = spinner;
            cache.width = width;
            cache.entries = Some(entries.clone());
            cache.total_height = total_height;
        }

        entries
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
/// Also skips trailing whitespace to prevent tui-markdown treating
/// "``` text" as a code language "text".
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
                while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') { i += 1; }
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

// ── Layout calculation ────────────────────────────────────────────────

/// Measure wrapped height using ratatui's Paragraph::line_count.
fn measure_wrapped_height(lines: &[Line<'_>], width: u16) -> u16 {
    if width < 1 { return lines.len() as u16; }
    let text: ratatui::text::Text = lines.iter().cloned().collect();
    let para = Paragraph::new(text).wrap(Wrap { trim: false });
    para.line_count(width) as u16
}

/// Calculate the layout: list of (y, height, block_ref) for each piece of content.
#[derive(Clone)]
struct LayoutEntry {
    y: u16,
    height: u16,
    kind: LayoutKind,
}

#[derive(Clone)]
enum LayoutKind {
    Spacer,
    Rule,
    Label { text: String, style: Style },
    Text { lines: Vec<Line<'static>>, is_user: bool },
    ToolBox {
        name: String,
        arguments: String,
        result: Option<(String, bool)>,
        status: ToolCallStatus,
    },
    ToolResultBox {
        tool_name: String,
        content: String,
        is_error: bool,
    },
    ErrorBox {
        title: String,
        message: String,
        retryable: bool,
    },
    Thinking { content: String, collapsed: bool },
    Image { mime_type: String, size_str: String },
    Spinner { frame: usize },
}

fn compute_layout(state: &ChatViewState, width: u16) -> Vec<LayoutEntry> {
    let mut entries = Vec::new();
    let mut y: u16 = 0;
    // Reserve 1 column for the vertical scrollbar so content doesn't overlap.
    let usable_width = width.saturating_sub(1);
    let inner_w = usable_width.saturating_sub(2); // Block::bordered inner width

    for (i, msg) in state.messages.iter().enumerate() {
        if i > 0 {
            entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spacer });
            y += 1;
        }
        if msg.role == MessageRole::User {
            entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Rule });
            y += 1;
            entries.push(LayoutEntry {
                y, height: 1,
                kind: LayoutKind::Label {
                    text: "You".to_string(),
                    style: Style::default().add_modifier(Modifier::BOLD),
                },
            });
            y += 1;
        }
        for block in &msg.content_blocks {
            let kind = block_to_layout_kind(block, msg.role);
            let h = measure_kind(&kind, usable_width, inner_w);
            entries.push(LayoutEntry { y, height: h, kind });
            y += h;
        }
    }

    if let Some(ref streaming) = state.streaming {
        if !state.messages.is_empty() {
            entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spacer });
            y += 1;
        }
        for block in &streaming.message.content_blocks {
            let kind = block_to_layout_kind(block, MessageRole::Assistant);
            let h = measure_kind(&kind, usable_width, inner_w);
            entries.push(LayoutEntry { y, height: h, kind });
            y += h;
        }
        entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spinner { frame: state.spinner_frame } });
        y += 1;
    }

    entries
}

fn block_to_layout_kind(block: &ContentBlock, role: MessageRole) -> LayoutKind {
    match block {
        ContentBlock::Text { content } => {
            let lines = md_lines(content);
            LayoutKind::Text { lines, is_user: role == MessageRole::User }
        }
        ContentBlock::Thinking { content, collapsed } =>
            LayoutKind::Thinking { content: content.clone(), collapsed: *collapsed },
        ContentBlock::ToolCall { name, arguments, result, status, .. } =>
            LayoutKind::ToolBox { name: name.clone(), arguments: arguments.clone(), result: result.clone(), status: *status },
        ContentBlock::ToolResult { tool_name, content, is_error } =>
            LayoutKind::ToolResultBox { tool_name: tool_name.clone(), content: content.clone(), is_error: *is_error },
        ContentBlock::Error { title, message, retryable } =>
            LayoutKind::ErrorBox { title: title.clone(), message: message.clone(), retryable: *retryable },
        ContentBlock::Image { mime_type, base64_data } => {
            let sz = base64_data.len() * 3 / 4;
            let sz_str = if sz >= 1_048_576 { format!("{:.1} MB", sz as f64 / 1_048_576.0) }
                else if sz >= 1024 { format!("{:.1} KB", sz as f64 / 1024.0) }
                else { format!("{} B", sz) };
            LayoutKind::Image { mime_type: mime_type.clone(), size_str: sz_str }
        }
    }
}

fn measure_kind(kind: &LayoutKind, width: u16, inner_w: u16) -> u16 {
    match kind {
        LayoutKind::Spacer | LayoutKind::Rule | LayoutKind::Label { .. } | LayoutKind::Spinner { .. } => 1,
        LayoutKind::Text { lines, is_user } => {
            let w = if *is_user { width.saturating_sub(2) } else { width };
            measure_wrapped_height(lines, w)
        }
        LayoutKind::ToolBox { arguments, result, .. } => {
            let arg_count = arguments.lines().count();
            let max_args = if arg_count <= 3 { 5 } else { 3 };
            let arg_lines: Vec<Line<'static>> = arguments.lines().take(max_args)
                .map(|l| Line::from(Span::raw(l.to_string()))).collect();
            let mut h: u16 = 2; // top + bottom border
            h += measure_wrapped_height(&arg_lines, inner_w);
            if arg_count > max_args { h += 1; }
            if let Some((rc, _)) = result {
                h += 1; // divider
                let r_lines: Vec<Line<'static>> = rc.lines().take(6)
                    .map(|l| Line::from(Span::raw(l.to_string()))).collect();
                h += measure_wrapped_height(&r_lines, inner_w);
                if rc.lines().count() > 6 { h += 1; }
            }
            h
        }
        LayoutKind::ToolResultBox { content, .. } => {
            let lines: Vec<Line<'static>> = content.lines().take(4)
                .map(|l| Line::from(Span::raw(l.to_string()))).collect();
            2 + measure_wrapped_height(&lines, inner_w) + if content.lines().count() > 4 { 1 } else { 0 }
        }
        LayoutKind::ErrorBox { message, retryable, .. } => {
            let lines: Vec<Line<'static>> = message.lines().take(4)
                .map(|l| Line::from(Span::raw(l.to_string()))).collect();
            2 + measure_wrapped_height(&lines, inner_w) + if *retryable { 1 } else { 0 }
        }
        LayoutKind::Thinking { content, collapsed } => {
            if *collapsed { 1 + if content.lines().next().is_some() { 1 } else { 0 } }
            else { 1 + content.lines().count() as u16 }
        }
        LayoutKind::Image { .. } => 2,
    }
}

// ── Rendering into ScrollView ─────────────────────────────────────────

/// A wrapper widget that renders a single content block.
struct EntryWidget<'a> {
    entry: &'a LayoutKind,
    styles: &'a ThemeStyles,
}

impl<'a> EntryWidget<'a> {
    fn new(entry: &'a LayoutKind, styles: &'a ThemeStyles) -> Self { Self { entry, styles } }
}

impl Widget for EntryWidget<'_> {
    fn render(self, rect: Rect, buf: &mut Buffer) {
        match &self.entry {
            LayoutKind::Spacer => { /* empty line, already cleared */ }
            LayoutKind::Rule => {
                let line = "-".repeat(rect.width as usize);
                Line::from(Span::styled(line, self.styles.muted)).render(rect, buf);
            }
            LayoutKind::Label { text, style } => {
                Paragraph::new(Line::from(Span::styled(text.clone(), *style))).render(rect, buf);
            }
            LayoutKind::Text { lines, is_user } => {
                let text: ratatui::text::Text = lines.iter().cloned().collect();
                if *is_user {
                    let block = Block::default()
                        .borders(Borders::LEFT)
                        .border_style(self.styles.user_border);
                    let inner = block.inner(rect);
                    block.render(rect, buf);
                    Paragraph::new(text).style(self.styles.normal).wrap(Wrap { trim: false }).render(inner, buf);
                } else {
                    Paragraph::new(text).style(self.styles.normal).wrap(Wrap { trim: false }).render(rect, buf);
                }
            }
            LayoutKind::ToolBox { name, arguments, result, status } => {
                let (icon, label_fg) = match status {
                    ToolCallStatus::Requested => ("...", self.styles.muted.fg.unwrap_or(ratatui::style::Color::White)),
                    ToolCallStatus::Executing => ("*run", self.styles.warning.fg.unwrap_or(ratatui::style::Color::Yellow)),
                    ToolCallStatus::Done => ("ok", self.styles.success.fg.unwrap_or(ratatui::style::Color::Green)),
                };
                let block = Block::bordered()
                    .border_style(self.styles.muted)
                    .title(Span::styled(
                        format!(" {} tool: {} ", icon, name),
                        Style::default().fg(label_fg).add_modifier(Modifier::BOLD),
                    ));
                let inner = block.inner(rect);
                block.render(rect, buf);

                let mut content_lines: Vec<Line<'static>> = Vec::new();
                let arg_count = arguments.lines().count();
                let max_args = if arg_count <= 3 { 5 } else { 3 };
                for arg in arguments.lines().take(max_args) {
                    content_lines.push(Line::from(Span::styled(arg.to_string(), self.styles.muted)));
                }
                if arg_count > max_args {
                    content_lines.push(Line::from(Span::styled(" ...", self.styles.muted)));
                }
                if let Some((result_content, _)) = result {
                    content_lines.push(Line::from(Span::styled("", self.styles.muted)));
                    let rn = result_content.lines().count();
                    for rl in result_content.lines().take(6) {
                        content_lines.push(Line::from(Span::styled(rl.to_string(), self.styles.normal)));
                    }
                    if rn > 6 {
                        content_lines.push(Line::from(Span::styled(" ...", self.styles.muted)));
                    }
                }
                let text: ratatui::text::Text = content_lines.into_iter().collect();
                Paragraph::new(text).wrap(Wrap { trim: false }).render(inner, buf);
            }
            LayoutKind::ToolResultBox { tool_name, content, is_error } => {
                let (check, border_style) = if *is_error { ("X", self.styles.error) } else { ("ok", self.styles.muted) };
                let label = if tool_name.is_empty() { check.to_string() } else { format!("{} {}", check, tool_name) };
                let label_fg = if *is_error {
                    ratatui::style::Color::White
                } else {
                    self.styles.success.fg.unwrap_or(ratatui::style::Color::Green)
                };
                let content_style = if *is_error { self.styles.error } else { self.styles.normal };
                let block = Block::bordered()
                    .border_style(border_style)
                    .title(Span::styled(
                        format!(" {} ", label),
                        Style::default().fg(label_fg).add_modifier(Modifier::BOLD),
                    ));
                let inner = block.inner(rect);
                block.render(rect, buf);

                let mut lines: Vec<Line<'static>> = Vec::new();
                for l in content.lines().take(4) {
                    lines.push(Line::from(Span::styled(l.to_string(), content_style)));
                }
                if content.lines().count() > 4 {
                    lines.push(Line::from(Span::styled(" ...", self.styles.muted)));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).wrap(Wrap { trim: false }).render(inner, buf);
            }
            LayoutKind::ErrorBox { title, message, retryable } => {
                let block = Block::bordered()
                    .border_style(self.styles.error)
                    .title(Span::styled(
                        format!(" error: {} ", title),
                        Style::default().fg(ratatui::style::Color::White).add_modifier(Modifier::BOLD),
                    ));
                let inner = block.inner(rect);
                block.render(rect, buf);

                let mut lines: Vec<Line<'static>> = Vec::new();
                for l in message.lines().take(4) {
                    lines.push(Line::from(Span::styled(l.to_string(), self.styles.normal)));
                }
                if *retryable {
                    lines.push(Line::from(Span::styled("retry: this error may be temporary", self.styles.muted)));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).wrap(Wrap { trim: false }).render(inner, buf);
            }
            LayoutKind::Thinking { content, collapsed } => {
                let ind = if *collapsed { ">" } else { "v" };
                let mut lines: Vec<Line<'static>> = vec![
                    Line::from(Span::styled(format!("{} Thinking...", ind), self.styles.accent)),
                ];
                if !*collapsed {
                    for l in content.lines() {
                        lines.push(Line::from(Span::styled(format!("  {}", l), self.styles.muted)));
                    }
                } else if let Some(first) = content.lines().next() {
                    lines.push(Line::from(Span::styled(format!("  {}", first), self.styles.muted)));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).render(rect, buf);
            }
            LayoutKind::Image { mime_type, size_str } => {
                let lines = vec![
                    Line::from(Span::styled(format!("[image: {}, {}]", mime_type, size_str), self.styles.normal)),
                    Line::from(Span::styled("  Ctrl+I -> open in viewer", self.styles.muted)),
                ];
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).render(rect, buf);
            }
            LayoutKind::Spinner { frame } => {
                let sp = ["|", "/", "-", "\\"];
                let ch = sp[frame % sp.len()];
                Paragraph::new(Line::from(Span::styled(
                    format!("  {} Working...", ch), self.styles.accent,
                ))).render(rect, buf);
            }
        }
    }
}

// ── ChatView widget ────────────────────────────────────────────────────

pub struct ChatView<'a> {
    theme: &'a Theme,
}

impl<'a> ChatView<'a> {
    pub fn new(theme: &'a Theme) -> Self { Self { theme } }
}

impl StatefulWidget for ChatView<'_> {
    type State = ChatViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width < 4 || area.height < 1 { return; }
        let styles = self.theme.to_styles();
        let width = area.width;

        // Get layout (from cache or recomputed)
        let layout = state.get_layout(width);
        let total_height = layout.last()
            .map(|e| e.y.saturating_add(e.height))
            .unwrap_or(0);
        state.content_height = total_height;

        // Create ScrollView with virtual buffer sized to total content.
        // Horizontal scrollbar disabled — chat wraps to width.
        // Vertical scrollbar reserves 1 column on the right.
        let size = ratatui::layout::Size::new(width, total_height.max(area.height));
        let mut scroll_view = ScrollView::new(size)
            .horizontal_scrollbar_visibility(ScrollbarVisibility::Never);

        // Render each layout entry into the virtual buffer.
        // Use width-1 so content never overlaps the vertical scrollbar.
        let content_w = width.saturating_sub(1);
        for entry in &layout {
            if entry.height == 0 { continue; }
            let rect = Rect::new(0, entry.y, content_w, entry.height);
            let widget = EntryWidget::new(&entry.kind, &styles);
            scroll_view.render_widget(widget, rect);
        }

        // Render the scroll view — handles clipping and scrolling
        scroll_view.render(area, buf, &mut state.scroll_state);
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
        assert!(s.scroll_state.offset().y > 80 || s.scroll_state.offset().y == u16::MAX);
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
        match &s.messages[0].content_blocks[0] {
            ContentBlock::ToolCall { status, result, .. } => {
                assert_eq!(*status, ToolCallStatus::Done);
                assert!(result.is_some());
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
            content_blocks: vec![ContentBlock::Text { content: "Hello".into() }],
            timestamp: 0,
        });
        let layout = compute_layout(&s, 80);
        assert!(!layout.is_empty());
        assert!(layout.iter().any(|e| matches!(&e.kind, LayoutKind::Label { text, .. } if text == "You")));
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
        let result = clamp_str(short.clone(), 100, 10);
        assert_eq!(result, short);
    }

    #[test]
    fn clamp_str_truncates_chars() {
        let long = "x".repeat(100);
        let result = clamp_str(long.clone(), 10, 200);
        // 10 chars + "\n ..." = 16 chars, 2 lines
        assert!(result.starts_with("xxxxxxxxxx"));
        assert!(result.contains("..."));
    }

    #[test]
    fn clamp_str_truncates_lines() {
        let long = (0..20).map(|i| format!("line{}", i)).collect::<Vec<_>>().join("\n");
        let result = clamp_str(long.clone(), 10000, 5);
        assert!(result.lines().count() <= 6); // 5 + "...\n"
        assert!(result.ends_with(" ..."));
    }

    #[test]
    fn layout_cache_hit() {
        let mut s = ChatViewState::new();
        s.messages.push(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text { content: "Hello".into() }],
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
        assert!(content.chars().count() <= MAX_TEXT_CHARS + 10, "content len = {}", content.chars().count());
    }
}
