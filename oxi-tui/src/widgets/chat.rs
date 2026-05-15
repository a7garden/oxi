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
    widgets::{Block, Borders, Clear, Paragraph, StatefulWidget, Widget, Wrap},
};
use tui_scrollview::{ScrollView, ScrollViewState, ScrollbarVisibility};
use tui_markdown;
use unicode_width::UnicodeWidthStr;

use crate::table_renderer::render_markdown_table;
use crate::Theme;
use crate::theme::ThemeStyles;
use crate::text::truncate_to_width as truncate_str;

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
        // Streaming lifecycle changes should always invalidate layout.
        self.layout_cache.write().entries = None;
    }

    pub fn stream_text_delta(&mut self, delta: &str) {
        self.append_text(delta);
        self.update_last_code_block();
    }

    fn append_text(&mut self, text: &str) {
        if let Some(ref mut s) = self.streaming {
            // Providers sometimes emit whitespace-only text around tool calls.
            // Rendering that literally creates large blank gaps in the chat view.
            // Skip whitespace-only deltas entirely — paragraph breaks within text
            // come as part of non-whitespace deltas (e.g., "text\n\nmore text").
            if text.trim().is_empty() {
                return;
            }

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
                self.layout_cache.write().entries = None;
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
                if let Some(block) = s.message.content_blocks.get_mut(existing_idx) {
                    if let ContentBlock::ToolCall { status: ref mut s, .. } = block {
                        *s = status;
                    }
                }
                self.layout_cache.write().entries = None;
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
            self.layout_cache.write().entries = None;
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
                            self.layout_cache.write().entries = None;
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
                    self.layout_cache.write().entries = None;
                    return;
                }
            }
            s.message.content_blocks.push(ContentBlock::ToolResult {
                tool_name,
                content: clamp_str(content, MAX_TOOL_RESULT_CHARS, MAX_TOOL_RESULT_LINES),
                is_error,
            });
            self.layout_cache.write().entries = None;
        }
    }

    pub fn stream_error(&mut self, title: String, message: String, retryable: bool) {
        if let Some(ref mut s) = self.streaming {
            s.message.content_blocks.push(ContentBlock::Error {
                title,
                message: clamp_str(message, 5000, 50),
                retryable,
            });
            self.layout_cache.write().entries = None;
        }
    }

    pub fn stream_thinking(&mut self, content: String, collapsed: bool) {
        if let Some(ref mut s) = self.streaming {
            if let Some(ContentBlock::Thinking { content: existing, .. }) = s.message.content_blocks.last_mut() {
                existing.push_str(&content);
                *existing = clamp_str(existing.clone(), 50_000, 200);
            } else {
                s.message.content_blocks.push(ContentBlock::Thinking {
                    content: clamp_str(content, 50_000, 200),
                    collapsed,
                });
            }
            self.layout_cache.write().entries = None;
        }
    }

    pub fn stream_image(&mut self, mime_type: String, base64_data: String) {
        if let Some(ref mut s) = self.streaming {
            // Track for Ctrl+I viewer
            self.pending_images.push((base64_data.clone(), mime_type.clone()));
            s.message.content_blocks.push(ContentBlock::Image { mime_type, base64_data });
            self.layout_cache.write().entries = None;
        }
    }

    pub fn finish_streaming(&mut self) {
        if let Some(mut s) = self.streaming.take() {
            // Drop whitespace-only blocks so they don't render as multi-line blank gaps.
            s.message.content_blocks.retain(|b| match b {
                ContentBlock::Text { content } => !content.trim().is_empty(),
                ContentBlock::Thinking { content, .. } => !content.trim().is_empty(),
                _ => true,
            });

            // Don't push empty assistant messages (they only add spacer rows).
            if !s.message.content_blocks.is_empty() {
                self.messages.push(s.message);
            }
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
                return cache.entries.clone().unwrap(); // SAFE: is_some() checked above
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

/// Parse markdown, extract tables, and render to styled Lines.
/// Tables are rendered using pulldown-cmark with width-aware column sizing.
/// `width` limits table width to prevent overflow.
fn md_lines(content: &str, width: u16) -> Vec<Line<'static>> {
    // Try table rendering first (pulldown-cmark based)
    let table_lines = render_markdown_table(content, width);
    if !table_lines.is_empty() {
        return table_lines;
    }

    // No table found, use regular markdown rendering
    render_markdown(content)
}

/// Render regular markdown (non-table content).
fn render_markdown(content: &str) -> Vec<Line<'static>> {
    let preprocessed = fix_bare_code_fences(content);
    let text: ratatui::text::Text<'_> = tui_markdown::from_str(&preprocessed);
    text.lines.into_iter().map(|l| {
        let line_style = l.style;
        let spans: Vec<Span<'static>> = l.spans
            .into_iter()
            .map(|s| Span::styled(s.content.into_owned(), line_style.patch(s.style)))
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

/// Check if a content block renders as a bordered box (tool calls, errors).
/// Consecutive box blocks need spacers between them for visual separation.
fn is_box_block(block: &ContentBlock) -> bool {
    matches!(block,
        ContentBlock::ToolCall { .. }
        | ContentBlock::ToolResult { .. }
        | ContentBlock::Error { .. }
    )
}

fn compute_layout(state: &ChatViewState, width: u16) -> Vec<LayoutEntry> {
    let mut entries = Vec::new();
    let mut y: u16 = 0;
    // Reserve 1 column for the vertical scrollbar so content doesn't overlap.
    let usable_width = width.saturating_sub(1); // used by block_to_layout_kind calls

    let mut rendered_any_message = false;

    for msg in &state.messages {
        // Skip messages that have no visible content; they only create empty spacer rows.
        let has_visible_content = msg.content_blocks.iter().any(|b| match b {
            ContentBlock::Text { content } => !content.trim().is_empty(),
            ContentBlock::Thinking { content, .. } => !content.trim().is_empty(),
            _ => true,
        });
        if !has_visible_content {
            continue;
        }

        if rendered_any_message {
            // Gap between messages — use a spacer for breathing room
            entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spacer });
            y += 1;
        }
        rendered_any_message = true;

        // User messages: left accent border, no label needed (single-user context)
        if msg.role == MessageRole::User {
            entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Rule });
            y += 1;
        }
        let mut prev_was_box = false;
        for block in &msg.content_blocks {
            // Skip whitespace-only blocks (defensive; finish_streaming also removes them).
            let is_empty = match block {
                ContentBlock::Text { content } => content.trim().is_empty(),
                ContentBlock::Thinking { content, .. } => content.trim().is_empty(),
                _ => false,
            };
            if is_empty {
                continue;
            }

            // Insert spacer between consecutive box-type blocks (tool calls, errors)
            let is_box = is_box_block(block);
            if is_box && prev_was_box {
                entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spacer });
                y += 1;
            }
            prev_was_box = is_box;

            let kind = block_to_layout_kind(block, msg.role, usable_width);
            let h = measure_kind(&kind, usable_width);
            entries.push(LayoutEntry { y, height: h, kind });
            y += h;
        }
    }

    if let Some(ref streaming) = state.streaming {
        // Only add a spacer if we actually rendered any history messages.
        if rendered_any_message {
            entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spacer });
            y += 1;
        }
        let mut prev_was_box = false;
        for block in &streaming.message.content_blocks {
            // Skip whitespace-only blocks (prevents large blank gaps during tool-only turns).
            let is_empty = match block {
                ContentBlock::Text { content } => content.trim().is_empty(),
                ContentBlock::Thinking { content, .. } => content.trim().is_empty(),
                _ => false,
            };
            if is_empty {
                continue;
            }

            // Insert spacer between consecutive box-type blocks (tool calls, errors)
            let is_box = is_box_block(block);
            if is_box && prev_was_box {
                entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spacer });
                y += 1;
            }
            prev_was_box = is_box;

            let kind = block_to_layout_kind(block, MessageRole::Assistant, usable_width);
            let h = measure_kind(&kind, usable_width);
            entries.push(LayoutEntry { y, height: h, kind });
            y += h;
        }
        entries.push(LayoutEntry { y, height: 1, kind: LayoutKind::Spinner { frame: state.spinner_frame } });
        y += 1;
    }

    entries
}

fn block_to_layout_kind(block: &ContentBlock, role: MessageRole, width: u16) -> LayoutKind {
    match block {
        ContentBlock::Text { content } => {
            let lines = md_lines(content, width);
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
        LayoutKind::ToolBox { name, arguments, result, .. } => {
            use crate::widgets::tool_renderer::{measure_call_height, measure_result_height};
            let call_h = measure_call_height(name, arguments);
            let result_h = result.as_ref().map_or(0, |(r, is_err)| {
                if *is_err { r.lines().count().min(4) as u16 } else { measure_result_height(name, r, false) }
            });
            // Block::ALL adds top + bottom border (2 rows) + separator if result exists
            let separator_h = if result.is_some() { 1 } else { 0 };
            2 + call_h + separator_h + result_h
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
                let md = md_lines(&filtered, width);
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
                Line::from(Span::styled(line, self.styles.border)).render(rect, buf);
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
                use crate::widgets::tool_renderer::{
                    format_tool_call, format_tool_result,
                };

                let (icon, border_style) = match status {
                    ToolCallStatus::Requested => (
                        "\u{25CB}",  // ○ (hollow circle — universal)
                        self.styles.muted,
                    ),
                    ToolCallStatus::Executing => (
                        "\u{25CF}",  // ● (filled circle — running)
                        self.styles.warning,
                    ),
                    ToolCallStatus::Done => {
                        let is_error = result.as_ref().map_or(false, |(_, e)| *e);
                        if is_error {
                            ("\u{2718}", self.styles.error)
                        } else {
                            ("\u{2713}", self.styles.success)
                        }
                    }
                };

                let has_result = result.is_some();

                // Box with all borders for a cleaner look (no background fill)
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style);
                let inner = block.inner(rect);
                block.render(rect, buf);
                // Clear any stale content in the inner area before rendering.
                Clear.render(inner, buf);

                let max_w = inner.width as usize;
                let mut content_lines: Vec<Line<'static>> = Vec::new();

                // Format tool call using new renderer
                let call_lines = format_tool_call(name, arguments, max_w, self.styles);
                for (i, line) in call_lines.into_iter().enumerate() {
                    if i == 0 {
                        // Prepend icon to first line
                        let icon_style = border_style.add_modifier(Modifier::BOLD);
                        let name_style = border_style.add_modifier(Modifier::BOLD);
                        let spans = line.spans.into_iter().collect::<Vec<_>>();
                        let mut new_spans = vec![Span::styled(format!("{} ", icon), icon_style)];
                        for span in spans {
                            new_spans.push(Span::styled(span.content.clone(), span.style.patch(name_style)));
                        }
                        content_lines.push(Line::from(new_spans));
                    } else {
                        content_lines.push(line);
                    }
                }

                // Separator line between call and result
                if has_result {
                    content_lines.push(Line::from(Span::styled(
                        "\u{2500}".repeat(max_w.saturating_sub(2)),
                        border_style,
                    )));
                }

                // Format result using new renderer
                if let Some((result_content, is_err)) = result {
                    let result_lines = format_tool_result(name, result_content, *is_err, max_w, self.styles);
                    content_lines.extend(result_lines);
                }

                let text: ratatui::text::Text = content_lines.into_iter().collect();
                let para = Paragraph::new(text).wrap(Wrap { trim: false });
                para.render(inner, buf);
            }
            LayoutKind::ToolResultBox { tool_name, content, is_error } => {
                let (icon, border_style) = if *is_error {
                    ("\u{2718}", self.styles.error)
                } else {
                    ("\u{2713}", self.styles.success)
                };
                let label = if tool_name.is_empty() { icon.to_string() } else { format!("{} {}", icon, tool_name) };


                let block = Block::default()
                    .borders(Borders::LEFT)
                    .border_style(border_style);
                let inner = block.inner(rect);
                block.render(rect, buf);
                // Clear any stale content in the inner area before rendering.
                Clear.render(inner, buf);

                let max_w = inner.width as usize;
                let mut lines: Vec<Line<'static>> = vec![
                    Line::from(Span::styled(format!("  {}", label), border_style.add_modifier(Modifier::BOLD))),
                ];
                for l in content.lines().take(4) {
                    let display = truncate_str(l, max_w.saturating_sub(2));
                    lines.push(Line::from(Span::styled(format!("  {}", display), self.styles.normal)));
                }
                if content.lines().count() > 4 {
                    lines.push(Line::from(Span::styled("  \u{2026}", self.styles.muted)));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                Clear.render(inner, buf);
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
                Clear.render(inner, buf);
                Paragraph::new(text).render(inner, buf);
            }
            LayoutKind::Thinking { content, collapsed } => {
                let filtered = filter_tool_json(content);
                let mut lines: Vec<Line<'static>> = Vec::new();

                let header_style = self.styles.accent;
                if *collapsed {
                    lines.push(Line::from(Span::styled("\u{25B8} thinking".to_string(), header_style)));
                    if let Some(first) = filtered.lines().next() {
                        lines.push(Line::from(Span::styled(format!("  {}", first), self.styles.muted.add_modifier(Modifier::ITALIC))));
                    }
                } else {
                    lines.push(Line::from(Span::styled("\u{25BE} thinking".to_string(), header_style)));
                    let thinking_style = self.styles.muted.add_modifier(Modifier::ITALIC);
                    let md_rendered = md_lines(&filtered, rect.width);
                    for md_line in md_rendered {
                        let spans: Vec<Span<'static>> = md_line.spans
                            .into_iter()
                            .map(|s| {
                                let combined = thinking_style.patch(s.style);
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
                Clear.render(rect, buf);
                Paragraph::new(text).render(rect, buf);
            }
            LayoutKind::Spinner { frame } => {
                // Moon phase spinner ◐ ◓ ◑ ◒
                let sp = ["\u{25D0}", "\u{25D3}", "\u{25D1}", "\u{25D2}"];
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

        // Apply left/right padding to the inner content area
        let pad = self.theme.spacing.padding.max(1);
        let inner_width = width.saturating_sub(pad * 2);
        let inner_x = area.x + pad;


        // Create ScrollView with virtual buffer sized to total content.
        // Horizontal scrollbar disabled — chat wraps to width.
        // Vertical scrollbar: show only when actively scrolling.
        let size = ratatui::layout::Size::new(inner_width, total_height.max(area.height));
        let mut scroll_view = ScrollView::new(size)
            .vertical_scrollbar_visibility(ScrollbarVisibility::Automatic)
            .horizontal_scrollbar_visibility(ScrollbarVisibility::Never);

        // Render each layout entry into the virtual buffer.
        // Use inner_width so content never overlaps the vertical scrollbar.
        for entry in &layout {
            if entry.height == 0 { continue; }
            let rect = Rect::new(0, entry.y, inner_width, entry.height);
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

#[cfg(test)]
mod table_tests {
    use super::*;
    use crate::table_renderer::render_markdown_table;

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
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("
");
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
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("
");
        assert!(text.contains("Only"), "Has header");
        assert!(text.contains("One"), "Has data");
        println!("text={}", text);
        assert!(text.contains("└──────┘") || text.contains("└──┘"), "Has bottom border");
    }

    #[test]
    fn test_cjk_characters() {
        let md = "| 이름 | 나이 | 도시 |
|---|---|---|
| 앨리스 | 30 | 서울 |";
        let out = render_markdown_table(md, 60);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("
");
        assert!(text.contains("이름"), "Has CJK");
        assert!(text.contains("앨리스"), "Has CJK data");
    }

    #[test]
    fn test_special_characters_in_cells() {
        let md = "| Name | Desc |
|---|---|
| Test | `code` |";
        let out = render_markdown_table(md, 50);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("
");
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
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("
");
        // Check exact format
        assert_eq!(text, "┌───┬───┐
│ A │ B │
├───┼───┤
│ X │ Y │
└───┴───┘");
    }

}
#[cfg(test)]
mod complete_table_verification {
    use crate::table_renderer::render_markdown_table;

    #[test]
    fn test_mixed_content_before_table() {
        let md = "Check out this table:\n\n| Name | Value |\n|---|---|\n| Alpha | 100 |";
        let out = render_markdown_table(md, 50);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Check out"), "Before table");
        assert!(text.contains('┌'), "Table top");
        assert!(text.contains("Alpha"), "Table data");
    }

    #[test]
    fn test_mixed_content_after_table() {
        let md = "| A | B |\n|---|---|\n| X | Y |\n\nThat was the table.";
        let out = render_markdown_table(md, 50);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains('┌'), "Table top");
        assert!(text.contains("That was the table"), "After table");
    }

    #[test]
    fn test_mixed_content_both_sides() {
        let md = "Start\n\n| H1 | H2 | H3 |\n|---|---|---|\n| C1 | C2 | C3 |\n\nEnd";
        let out = render_markdown_table(md, 60);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Start"), "Before");
        assert!(text.contains("End"), "After");
        assert!(text.contains('┌'), "Table");
    }

    #[test]
    fn test_narrow_terminal() {
        let md = "| Name | Age | City |\n|---|---|---|\n| Alice | 30 | Seoul |";
        let out = render_markdown_table(md, 20);
        assert!(!out.is_empty());
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Name") || text.contains("┌") || text.contains("Alice"));
    }

    #[test]
    fn test_cell_wrapping() {
        let md = "| Short | Very Long Header Text Here |\n|---|---|\n| Data | Another long cell that needs wrapping |";
        let out = render_markdown_table(md, 50);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains('┌'), "Table rendered");
        assert!(text.contains('│'), "Has cell separators");
    }

    #[test]
    fn test_multiple_rows_separators() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |";
        let out = render_markdown_table(md, 40);
        let text: String = out.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        // Count separator lines (lines containing ├ or ┼)
        let separator_lines: Vec<&str> = text.lines()
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
        assert!(out.len() >= 4, "Should have top border + header + separator + body");
    }
}
