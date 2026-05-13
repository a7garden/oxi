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
use unicode_width::UnicodeWidthStr;
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

/// Truncate a string to fit within `max_width` terminal columns.
/// Appends \u{2026} (…) if truncated.
fn truncate_str(s: &str, max_width: usize) -> String {
    if max_width == 0 { return String::new(); }
    let mut width = 0usize;
    let mut end = 0usize;
    for (i, ch) in s.char_indices() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > max_width {
            // Not enough room for ellipsis either
            return s[..end].to_string();
        }
        if width + cw > max_width.saturating_sub(1) {
            // Would overflow if we add ellipsis
            return format!("{}\u{2026}", &s[..end]);
        }
        width += cw;
        end = i + ch.len_utf8();
    }
    s.to_string()
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
    /// Last known streaming text character count (detects content growth)
    streaming_text_len: usize,
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
            .field("streaming_text_len", &self.streaming_text_len)
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
            streaming_text_len: 0,
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
            // Check if this tool call was already registered (e.g., from
            // a prior MessageUpdate that included ToolCall blocks).
            if let Some(existing_idx) = self.tool_tracker.get(&id) {
                // Update the existing block's status (Requested → Executing etc.)
                if let Some(block) = s.message.content_blocks.get_mut(existing_idx) {
                    if let ContentBlock::ToolCall { status: ref mut s, .. } = block {
                        *s = status;
                    }
                }
                return;
            }
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
        self.tool_tracker.clear();
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
        let streaming_text_len = self.streaming.as_ref()
            .and_then(|s| s.message.content_blocks.first())
            .map(|b| match b {
                ContentBlock::Text { content } => content.len(),
                _ => 0,
            })
            .unwrap_or(0);
        let spinner = self.spinner_frame;

        {
            let cache = self.layout_cache.read();
            if cache.entries.is_some()
                && cache.msg_count == msg_count
                && cache.streaming_len == streaming_len
                && cache.streaming_text_len == streaming_text_len
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
            cache.streaming_text_len = streaming_text_len;
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
/// Tracks open/close state so closing fences are left as ```.
fn fix_bare_code_fences(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut in_code = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_code {
                // Closing fence — emit as-is
                result.push_str("```");
                in_code = false;
            } else {
                // Opening fence
                let lang = &trimmed[3..];
                let lang = lang.trim();
                if lang.is_empty() {
                    result.push_str("```text");
                } else {
                    result.push_str(trimmed);
                }
                in_code = true;
            }
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    // Remove trailing newline if original didn't have one
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

/// Convert markdown to styled Lines via tui-markdown.
fn md_lines(content: &str) -> Vec<Line<'static>> {
    let preprocessed = fix_bare_code_fences(content);
    let text: ratatui::text::Text<'_> = tui_markdown::from_str(&preprocessed);
    text.lines.into_iter().map(|l| {
        // tui-markdown uses Line-level styles (e.g. for code blocks).
        // We must merge the line style into each span so nothing is lost.
        let line_style = l.style;
        let spans: Vec<Span<'static>> = l.spans
            .into_iter()
            .map(|s| {
                let merged = line_style.patch(s.style);
                Span::styled(s.content.into_owned(), merged)
            })
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

    for (i, msg) in state.messages.iter().enumerate() {
        if i > 0 {
            // Gap between messages — use a spacer for breathing room
            entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spacer });
            y += 1;
        }
        // User messages: left accent border, no label needed (single-user context)
        if msg.role == MessageRole::User {
            entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Rule });
            y += 1;
        }
        for block in &msg.content_blocks {
            let kind = block_to_layout_kind(block, msg.role);
            let h = measure_kind(&kind, usable_width);
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
            let h = measure_kind(&kind, usable_width);
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

fn measure_kind(kind: &LayoutKind, width: u16) -> u16 {
    match kind {
        LayoutKind::Spacer | LayoutKind::Rule | LayoutKind::Label { .. } | LayoutKind::Spinner { .. } => 1,
        LayoutKind::Text { lines, is_user } => {
            // User text: Block::borders(LEFT) takes 1 col, so inner = width-1
            // Assistant text: no block, renders at full width
            let w = if *is_user { width.saturating_sub(1) } else { width };
            measure_wrapped_height(lines, w)
        }
        LayoutKind::ToolBox { arguments, result, .. } => {
            let mut h: u16 = 1; // header (icon + name)
            // Arguments: max 3 key-value lines
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments) {
                if let Some(obj) = parsed.as_object() {
                    h += obj.len().min(3) as u16;
                }
                // Non-object JSON: render shows nothing, measure 0
            }
            // Invalid JSON: render shows nothing, measure 0
            // Result: max 3 lines + ellipsis
            if let Some((rc, _)) = result {
                let rn = rc.lines().count();
                h += rn.min(3) as u16;
                if rn > 3 { h += 1; }
            }
            h
        }
        LayoutKind::ToolResultBox { content, .. } => {
            // 1 header + content lines (max 4) + optional ellipsis
            let n = content.lines().count().min(4);
            1 + n as u16 + if content.lines().count() > 4 { 1 } else { 0 }
        }
        LayoutKind::ErrorBox { message, retryable, .. } => {
            // Block::bordered() adds top + bottom border (2 rows)
            let n = message.lines().count().min(4);
            2 + n as u16 + if *retryable { 1 } else { 0 }
        }
        LayoutKind::Thinking { content, collapsed } => {
            if *collapsed {
                // Use filtered content for preview — matches render
                let filtered = filter_tool_json(content);
                1 + if filtered.lines().next().is_some() { 1 } else { 0 }
            }
            else {
                // Use filtered content for measurement to match rendering
                let filtered = filter_tool_json(content);
                let md = md_lines(&filtered);
                1 + md.len() as u16
            }
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
                let line = "\u{2500}".repeat(rect.width as usize); // ─
                Line::from(Span::styled(line, Style::default().fg(ratatui::style::Color::Rgb(35, 35, 50)))).render(rect, buf);
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
                    Paragraph::new(text).wrap(Wrap { trim: false }).render(inner, buf);
                } else {
                    // Don't set .style() here — markdown Spans already carry
                    // their own styling (bold, italic, code, headings).
                    // Paragraph::style() would override all per-Span styles.
                    Paragraph::new(text).wrap(Wrap { trim: false }).render(rect, buf);
                }
            }
            LayoutKind::ToolBox { name, arguments, result, status } => {
                let (icon, border_color, bg_color) = match status {
                    ToolCallStatus::Requested => (
                        "\u{29D6}",
                        ratatui::style::Color::Rgb(100, 140, 200),
                        ratatui::style::Color::Rgb(18, 24, 38),
                    ),
                    ToolCallStatus::Executing => (
                        "\u{27F3}",
                        ratatui::style::Color::Rgb(200, 165, 80),
                        ratatui::style::Color::Rgb(32, 28, 16),
                    ),
                    ToolCallStatus::Done => {
                        let is_error = result.as_ref().map_or(false, |(_, e)| *e);
                        if is_error {
                            ("\u{2718}", ratatui::style::Color::Rgb(220, 90, 110), ratatui::style::Color::Rgb(36, 16, 20))
                        } else {
                            ("\u{2713}", ratatui::style::Color::Rgb(120, 180, 90), ratatui::style::Color::Rgb(18, 30, 16))
                        }
                    }
                };

                // Thin left accent border only
                let block = Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(border_color))
                    .style(Style::default().bg(bg_color));
                let inner = block.inner(rect);
                block.render(rect, buf);

                // Max content width = inner.width (no wrapping, pre-truncate instead)
                let max_w = inner.width as usize;
                let mut content_lines: Vec<Line<'static>> = Vec::new();

                // Header: icon + tool name
                let name_style = Style::default().fg(border_color).add_modifier(Modifier::BOLD);
                content_lines.push(Line::from(vec![
                    Span::styled(format!("{} ", icon), name_style),
                    Span::styled(name.clone(), name_style),
                ]));

                let dim = Style::default().fg(ratatui::style::Color::Rgb(100, 108, 135));
                let val_style = Style::default().fg(ratatui::style::Color::Rgb(155, 163, 185));

                // Arguments: compact key: value, truncate to fit inner width
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments) {
                    if let Some(obj) = parsed.as_object() {
                        for (key, v) in obj.iter().take(3) {
                            let val_str = match v {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            // "  key: value" — truncate value to fit
                            let prefix_len = 2 + UnicodeWidthStr::width(key.as_str()) + 2;
                            let avail = max_w.saturating_sub(prefix_len);
                            let display = truncate_str(&val_str, avail);
                            content_lines.push(Line::from(vec![
                                Span::styled(format!("  {}", key), dim),
                                Span::styled(": ", dim),
                                Span::styled(display, val_style),
                            ]));
                        }
                    }
                }

                // Result: max 3 lines, each truncated to inner width
                if let Some((result_content, _)) = result {
                    for rl in result_content.lines().take(3) {
                        let display = truncate_str(rl, max_w.saturating_sub(2));
                        content_lines.push(Line::from(Span::styled(format!("  {}", display), val_style)));
                    }
                    if result_content.lines().count() > 3 {
                        content_lines.push(Line::from(Span::styled("  \u{2026}", dim)));
                    }
                }

                let text: ratatui::text::Text = content_lines.into_iter().collect();
                // No wrap — lines are pre-truncated to exact width
                Paragraph::new(text).render(inner, buf);
            }
            LayoutKind::ToolResultBox { tool_name, content, is_error } => {
                let (icon, border_color, bg_color) = if *is_error {
                    ("\u{2718}", ratatui::style::Color::Rgb(247, 118, 142), ratatui::style::Color::Rgb(45, 20, 24))
                } else {
                    ("\u{2713}", ratatui::style::Color::Rgb(80, 180, 100), ratatui::style::Color::Rgb(20, 34, 22))
                };
                let label = if tool_name.is_empty() { icon.to_string() } else { format!("{} {}", icon, tool_name) };
                let label_style = Style::default().fg(border_color);
                let content_style = if *is_error {
                    Style::default().fg(ratatui::style::Color::Rgb(247, 150, 165))
                } else {
                    Style::default().fg(ratatui::style::Color::Rgb(160, 170, 195))
                };

                let block = Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(border_color))
                    .style(Style::default().bg(bg_color));
                let inner = block.inner(rect);
                block.render(rect, buf);

                let max_w = inner.width as usize;
                let mut lines: Vec<Line<'static>> = vec![
                    Line::from(Span::styled(format!("  {}", label), label_style)),
                ];
                for l in content.lines().take(4) {
                    let display = truncate_str(l, max_w.saturating_sub(2));
                    lines.push(Line::from(Span::styled(format!("  {}", display), content_style)));
                }
                if content.lines().count() > 4 {
                    lines.push(Line::from(Span::styled("  \u{2026}", self.styles.muted)));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                // No wrap — pre-truncated to exact width
                Paragraph::new(text).render(inner, buf);
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

                let max_w = inner.width as usize;
                let mut lines: Vec<Line<'static>> = Vec::new();
                for l in message.lines().take(4) {
                    let display = truncate_str(l, max_w);
                    lines.push(Line::from(Span::styled(display, self.styles.normal)));
                }
                if *retryable {
                    lines.push(Line::from(Span::styled("retry: this error may be temporary", self.styles.muted)));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                // No wrap — pre-truncated to exact width
                Paragraph::new(text).render(inner, buf);
            }
            LayoutKind::Thinking { content, collapsed } => {
                let filtered = filter_tool_json(content);
                let mut lines: Vec<Line<'static>> = Vec::new();

                // Header line with subtle styling
                let header_style = Style::default().fg(ratatui::style::Color::Rgb(147, 130, 220)); // soft purple
                if *collapsed {
                    lines.push(Line::from(Span::styled("\u{25B8} thinking".to_string(), header_style)));
                    if let Some(first) = filtered.lines().next() {
                        let preview_style = Style::default()
                            .fg(ratatui::style::Color::Rgb(90, 85, 130))
                            .add_modifier(Modifier::ITALIC);
                        lines.push(Line::from(Span::styled(format!("  {}", first), preview_style)));
                    }
                } else {
                    lines.push(Line::from(Span::styled("\u{25BE} thinking".to_string(), header_style)));
                    // Render thinking content with tui-markdown in italic style
                    let thinking_style = Style::default()
                        .fg(ratatui::style::Color::Rgb(130, 135, 170))
                        .add_modifier(Modifier::ITALIC);
                    let md_rendered = md_lines(&filtered);
                    for md_line in md_rendered {
                        let spans: Vec<Span<'static>> = md_line.spans
                            .into_iter()
                            .map(|s| {
                                let mut combined = thinking_style;
                                combined = combined.patch(s.style);
                                Span::styled(s.content.into_owned(), combined)
                            })
                            .collect();
                        lines.push(Line::from(spans));
                    }
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
                let sp = ["\u{2850}", "\u{2854}", "\u{2860}", "\u{284E}"];
                // Fallback to simple ASCII if unicode doesn't render
                let sp_ascii = ["|", "/", "-", "\\"];
                let ch = sp[frame % sp.len()];
                let spinner_style = Style::default().fg(ratatui::style::Color::Rgb(187, 154, 247));
                Paragraph::new(Line::from(Span::styled(
                    format!("  {} Working...", ch), spinner_style,
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

/// Filter JSON tool call arrays from thinking text.
/// GLM-5.1 writes tool call plans as `[{\"function\":...}]` inside
/// reasoning_content. We detect `[{\"` and skip to the matching `]`.
fn filter_tool_json(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut result = String::new();
    let mut i = 0;

    while i < len {
        // Detect `[{"` — start of a JSON array containing tool calls
        if chars[i] == '['
            && i + 2 < len
            && chars[i + 1] == '{'
            && chars[i + 2] == '"'
        {
            // Skip to matching `]`
            let mut depth: i32 = 0;
            while i < len {
                match chars[i] {
                    '[' | '{' => depth += 1,
                    ']' | '}' => {
                        depth -= 1;
                        if depth <= 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        result.push(chars[i]);
        i += 1;
    }

    // Trim empty lines but preserve content whitespace
    result.lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
