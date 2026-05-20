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

use std::collections::{HashMap, HashSet};

use parking_lot::RwLock;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, StatefulWidget, Widget},
};
use tui_markdown;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::table_renderer::render_markdown_table;
use crate::text::truncate_to_width as truncate_str;
use crate::theme::ThemeStyles;
use crate::Theme;

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
    let truncated: String = s.chars().take(max_chars).collect();
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
        if self.active.contains_key(&id) {
            return false;
        }
        self.active.insert(id, index);
        true
    }
    fn find_and_remove(&mut self, id: &str) -> Option<usize> {
        self.active.remove(id)
    }
    fn remove(&mut self, id: &str) {
        self.active.remove(id);
    }
    fn get(&self, id: &str) -> Option<usize> {
        self.active.get(id).copied()
    }
    fn clear(&mut self) {
        self.active.clear();
    }
}

// ── Dashboard ──────────────────────────────────────────────────────────

/// Information displayed in the welcome dashboard panel.
#[derive(Debug, Clone)]
pub struct DashboardInfo {
    /// oxi version string.
    pub version: String,
    /// Full model identifier (e.g. "anthropic/claude-sonnet-4").
    pub model_id: String,
    /// Thinking level label (e.g. "medium").
    pub thinking_level: String,
    /// Project directory basename.
    pub project_name: String,
    /// Git branch name, if detected.
    pub git_branch: Option<String>,
    /// Path to AGENTS.md, if found.
    pub agents_md_path: Option<String>,
    /// Registered tool names.
    pub tool_names: Vec<String>,
    /// Loaded skill names.
    pub skill_names: Vec<String>,
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
    Text {
        content: String,
    },
    Thinking {
        content: String,
        collapsed: bool,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
        result: Option<(String, bool)>,
        status: ToolCallStatus,
        /// Formatted execution duration (e.g. "1.2s").
        duration: Option<String>,
    },
    ToolResult {
        tool_name: String,
        content: String,
        is_error: bool,
    },
    Error {
        title: String,
        message: String,
        retryable: bool,
    },
    Image {
        mime_type: String,
        base64_data: String,
    },
    /// Welcome dashboard panel with environment info.
    Dashboard {
        info: DashboardInfo,
    },
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

#[derive(Default)]
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
    /// Vertical scroll offset (0 = top)
    pub scroll_offset: u16,
    /// When true, auto-scroll to bottom on each render (streaming)
    pub auto_scroll: bool,
    /// Layout cache — guarded by RwLock
    layout_cache: RwLock<LayoutCache>,
    /// Expanded thinking block keys: "msg_idx:block_idx".
    /// Blocks not in this set use their default `collapsed` state.
    pub expanded_thinking: HashSet<String>,
    /// Expanded tool result keys: "msg_idx:block_idx".
    pub expanded_tools: HashSet<String>,
    /// Clickable thinking block regions: (y_start, y_end, key).
    /// Populated during render, consumed by click handler.
    pub thinking_regions: Vec<(u16, u16, String)>,
    /// Clickable tool result regions: (y_start, y_end, key).
    pub tool_regions: Vec<(u16, u16, String)>,
}

impl ChatViewState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scroll to bottom of content. `visible_height` is the viewport height.
    pub fn scroll_to_bottom(&mut self, visible_height: u16) {
        self.auto_scroll = true;
        if self.content_height > visible_height {
            self.scroll_offset = self.content_height - visible_height;
        } else {
            self.scroll_offset = 0;
        }
    }
    pub fn scroll_up(&mut self, n: u16) {
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }
    pub fn scroll_down(&mut self, n: u16) {
        // NOTE: We don't clamp to content_height - visible_height here
        // because visible_height is unknown.  render() calls
        // clamp_scroll(area.height) on every frame which performs the
        // correct clamping.
        self.scroll_offset = self.scroll_offset.saturating_add(n);
    }
    pub fn scroll_to_top(&mut self) {
        self.auto_scroll = false;
        self.scroll_offset = 0;
    }
    /// Clamp scroll_offset to [0, content_height - visible_height].
    fn clamp_scroll(&mut self, visible_height: u16) {
        let max_off = self.content_height.saturating_sub(visible_height);
        self.scroll_offset = self.scroll_offset.min(max_off);
    }

    /// Toggle expanded state of a thinking block.
    /// `key` is "msg_idx:block_idx".
    pub fn toggle_thinking(&mut self, key: &str) {
        if self.expanded_thinking.contains(key) {
            self.expanded_thinking.remove(key);
        } else {
            self.expanded_thinking.insert(key.to_string());
        }
        self.layout_cache.write().entries = None;
    }

    /// Check if a thinking block is expanded.
    /// If the key is in `expanded_thinking`, it's expanded (overrides collapsed).
    pub fn is_thinking_expanded(&self, key: &str) -> bool {
        self.expanded_thinking.contains(key)
    }

    /// Toggle expanded state of a tool result block.
    pub fn toggle_tool(&mut self, key: &str) {
        if self.expanded_tools.contains(key) {
            self.expanded_tools.remove(key);
        } else {
            self.expanded_tools.insert(key.to_string());
        }
        self.layout_cache.write().entries = None;
    }

    pub fn start_streaming(&mut self) {
        // Auto-commit any existing streaming before starting new.
        // This prevents tool execution results from being lost when
        // a new MessageStart arrives while tool results are streaming.
        if self.streaming.is_some() {
            self.finish_streaming();
        }
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
            // NOTE: We no longer drop pure-whitespace deltas. In the pi-mono
            // pattern, `new_text` is extracted from the provider's accumulated
            // snapshot, so spaces between words are legitimate content.
            // Dropping them caused word-spacing to vanish (e.g.
            // "hello  world" → "helloworld").
            //
            // Previously we filtered whitespace-only deltas to remove "noise
            // from providers around tool calls", but the proper fix is to
            // handle that at the provider layer, not at the TUI layer.

            if let Some(ContentBlock::Text { ref mut content }) =
                s.message.content_blocks.first_mut()
            {
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
                s.message
                    .content_blocks
                    .insert(0, ContentBlock::Text { content: truncated });
            }
        }
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

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
                if let Some(ContentBlock::ToolCall {
                    status: ref mut curr,
                    ..
                }) = s.message.content_blocks.get_mut(idx)
                {
                    *curr = status;
                }
                self.layout_cache.write().entries = None;
            }
        }
    }

    pub fn stream_tool_call(
        &mut self,
        id: String,
        name: String,
        arguments: String,
        status: ToolCallStatus,
    ) {
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
                if let Some(ContentBlock::ToolCall {
                    status: ref mut s, ..
                }) = s.message.content_blocks.get_mut(existing_idx)
                {
                    *s = status;
                }
                self.layout_cache.write().entries = None;
                return;
            }
            let idx = s.message.content_blocks.len();
            if !self.tool_tracker.register(id.clone(), idx) {
                return;
            }
            s.message.content_blocks.push(ContentBlock::ToolCall {
                id,
                name,
                arguments: clamp_str(arguments, MAX_TOOL_ARG_CHARS, MAX_TOOL_ARG_LINES),
                result: None,
                status,
                duration: None,
            });
            self.layout_cache.write().entries = None;
        }
    }

    pub fn stream_tool_result(
        &mut self,
        tool_call_id: Option<String>,
        tool_name: String,
        content: String,
        is_error: bool,
    ) {
        if self.streaming.is_none() {
            self.start_streaming();
        }
        if let Some(ref mut s) = self.streaming {
            if let Some(ref id) = tool_call_id {
                if let Some(idx) = self.tool_tracker.find_and_remove(id) {
                    if let Some(ContentBlock::ToolCall {
                        ref mut result,
                        ref mut status,
                        ..
                    }) = s.message.content_blocks.get_mut(idx)
                    {
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
            if let Some(ContentBlock::ToolCall {
                ref mut result,
                ref mut status,
                ..
            }) = s.message.content_blocks.last_mut()
            {
                *result = Some((
                    clamp_str(content, MAX_TOOL_RESULT_CHARS, MAX_TOOL_RESULT_LINES),
                    is_error,
                ));
                *status = ToolCallStatus::Done;
                if let Some(ref id) = tool_call_id {
                    self.tool_tracker.remove(id);
                }
                self.layout_cache.write().entries = None;
                return;
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
            if let Some(ContentBlock::Thinking {
                content: existing, ..
            }) = s.message.content_blocks.last_mut()
            {
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
            self.pending_images
                .push((base64_data.clone(), mime_type.clone()));
            s.message.content_blocks.push(ContentBlock::Image {
                mime_type,
                base64_data,
            });
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

    /// Set the formatted duration for a tool call (by ID).
    pub fn set_tool_duration(&mut self, id: &str, dur_str: String) {
        if let Some(ref mut s) = self.streaming {
            for block in &mut s.message.content_blocks {
                if let ContentBlock::ToolCall {
                    id: ref bid,
                    ref mut duration,
                    ..
                } = block
                {
                    if bid == id {
                        *duration = Some(dur_str);
                        self.layout_cache.write().entries = None;
                        return;
                    }
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.streaming = None;
        self.scroll_offset = 0;
        self.auto_scroll = false;
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
        let streaming_len = self
            .streaming
            .as_ref()
            .map(|s| s.message.content_blocks.len())
            .unwrap_or(0);
        let streaming_text_len = self
            .streaming
            .as_ref()
            .and_then(|s| s.message.content_blocks.first())
            .map(|b| match b {
                ContentBlock::Text { content } => content.len(),
                _ => 0,
            })
            .unwrap_or(0);
        let spinner = self.spinner_frame;

        {
            let cache = self.layout_cache.read();
            if cache.msg_count == msg_count
                && cache.streaming_len == streaming_len
                && cache.streaming_text_len == streaming_text_len
                && cache.spinner_frame == spinner
                && cache.width == width
            {
                if let Some(ref entries) = cache.entries {
                    return entries.clone();
                }
            }
        }

        // Recompute outside the read lock
        let entries = compute_layout(self, width);
        let total_height: u16 = entries
            .last()
            .map(|e| {
                (e.y as u32)
                    .saturating_add(e.height as u32)
                    .min(u16::MAX as u32) as u16
            })
            .unwrap_or(0);

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
                if !c.is_empty() {
                    result = Some(c);
                }
                block_content.clear();
                in_block = false;
            } else {
                block_content.clear();
                in_block = true;
            }
        } else if in_block {
            if !block_content.is_empty() {
                block_content.push('\n');
            }
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
                let lang = trimmed.strip_prefix("```").unwrap_or(trimmed).trim();
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
        // render_markdown_table handles table cell wrapping internally,
        // but non-table text (before/after the table) comes from
        // flush_text → tui_markdown and may exceed the width.
        // Apply wrapping to the entire result to catch those cases.
        return wrap_lines_styled(&table_lines, width);
    }

    // No table found, use regular markdown rendering.
    // Apply CJK-aware wrapping before returning so that the layout
    // engine gets correctly-sized lines. Without this, ratatui's
    // Paragraph::wrap would be used at render time, but it only
    // breaks at whitespace — CJK characters that lack spaces between
    // them would be treated as a single giant "word" and overflow.
    let raw_lines = render_markdown(content);
    wrap_lines_styled(&raw_lines, width)
}

/// Wrap styled `Line`s to fit within `width` terminal columns,
/// breaking at word boundaries for Latin text and at character
/// boundaries for CJK text. Preserves per-Span styling.
///
/// This replaces `Paragraph::wrap` for markdown content because
/// ratatui's `WordWrapper` does not handle CJK line-breaking —
/// Korean/Chinese/Japanese characters that lack spaces between
/// them are treated as a single "word" and never get wrapped.
fn wrap_lines_styled(lines: &[Line<'static>], width: u16) -> Vec<Line<'static>> {
    let max_w = width as usize;
    if max_w == 0 {
        return lines.to_vec();
    }

    let mut result = Vec::new();
    for line in lines {
        // Collect all (char, Style) pairs from all Spans
        let mut chars: Vec<(char, Style)> = Vec::new();
        for span in &line.spans {
            for ch in span.content.chars() {
                chars.push((ch, span.style));
            }
        }

        // Measure total width
        let total_w: usize = chars
            .iter()
            .map(|(ch, _)| UnicodeWidthChar::width(*ch).unwrap_or(0))
            .sum();
        if total_w <= max_w {
            result.push(line.clone());
            continue;
        }

        // Break into wrapped lines
        let wrapped = wrap_styled_chars(&chars, max_w);
        result.extend(wrapped);
    }
    result
}

/// Wrap a flat list of (char, Style) into `Line`s that fit `max_width`.
///
/// Uses a word-boundary approach: groups consecutive chars into "words"
/// (separated by whitespace) and fits as many words as possible per line.
/// For CJK characters (which can break between any two characters),
/// each character is its own "word".
fn wrap_styled_chars(chars: &[(char, Style)], max_width: usize) -> Vec<Line<'static>> {
    // Segment chars into "tokens": each is either a whitespace run,
    // a CJK character, or a non-CJK word (consecutive non-ws non-CJK).
    #[derive(Debug)]
    enum Token<'a> {
        Word(&'a [(char, Style)]),
        Space(&'a [(char, Style)]),
    }

    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (ch, _) = chars[i];
        if ch.is_whitespace() {
            let start = i;
            while i < chars.len() && chars[i].0.is_whitespace() {
                i += 1;
            }
            tokens.push(Token::Space(&chars[start..i]));
        } else if is_cjk_breakable(ch) {
            // Each CJK character is its own token
            tokens.push(Token::Word(&chars[i..i + 1]));
            i += 1;
        } else {
            // Non-CJK word: collect until whitespace or CJK
            let start = i;
            while i < chars.len() {
                let (c, _) = chars[i];
                if c.is_whitespace() || is_cjk_breakable(c) {
                    break;
                }
                i += 1;
            }
            tokens.push(Token::Word(&chars[start..i]));
        }
    }

    // Build lines from tokens
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width: usize = 0;
    let mut pending_space: Option<&[(char, Style)]> = None;
    let mut pending_space_width: usize = 0;

    for token in &tokens {
        match token {
            Token::Space(space_chars) => {
                let w: usize = space_chars
                    .iter()
                    .map(|(ch, _)| UnicodeWidthChar::width(*ch).unwrap_or(0))
                    .sum();
                pending_space = Some(space_chars);
                pending_space_width = w;
            }
            Token::Word(word_chars) => {
                let word_width: usize = word_chars
                    .iter()
                    .map(|(ch, _)| UnicodeWidthChar::width(*ch).unwrap_or(0))
                    .sum();

                // Can we fit pending_space + this word?
                let needed = pending_space_width + word_width;

                if current_width + needed <= max_width {
                    // Fits on current line
                    if let Some(space_chars) = pending_space.take() {
                        append_chars_to_spans(space_chars, &mut current_spans);
                        current_width += pending_space_width;
                    }
                    append_chars_to_spans(word_chars, &mut current_spans);
                    current_width += word_width;
                } else if word_width > max_width {
                    // Word is wider than max_width — break at char boundaries
                    // First, flush current line
                    if !current_spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current_spans)));
                        current_width = 0;
                    }
                    // Break the oversized word into fragments that each fit max_width.
                    // All non-last fragments become complete output lines;
                    // the last fragment stays in current_spans so the next token
                    // can potentially join it.
                    let broken = break_styled_word(word_chars, max_width);
                    let broken_len = broken.len();
                    for (idx, broken_spans) in broken.into_iter().enumerate() {
                        if idx < broken_len - 1 {
                            lines.push(Line::from(broken_spans));
                        } else {
                            current_spans = broken_spans;
                            current_width = spans_width(&current_spans);
                        }
                    }
                } else {
                    // Doesn't fit — start new line
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                    // Drop leading space
                    append_chars_to_spans(word_chars, &mut current_spans);
                    current_width = word_width;
                }
                pending_space = None;
                pending_space_width = 0;
            }
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    if lines.is_empty() {
        lines.push(Line::raw(""));
    }

    lines
}

/// Check if a character is CJK — these characters allow line breaks
/// between any two adjacent CJK characters.
fn is_cjk_breakable(ch: char) -> bool {
    matches!(ch,
        '\u{2E80}'..='\u{9FFF}'   | // CJK Unified, Kangxi, etc.
        '\u{A960}'..='\u{A97F}'   | // Hangul Jamo Extended-A
        '\u{AC00}'..='\u{D7AF}'   | // Hangul Syllables (Korean)
        '\u{D7B0}'..='\u{D7FF}'   | // Hangul Jamo Extended-B
        '\u{F900}'..='\u{FAFF}'   | // CJK Compatibility Ideographs
        '\u{FE30}'..='\u{FE4F}'   | // CJK Compatibility Forms
        '\u{FF65}'..='\u{FFDC}'   | // Halfwidth and Fullwidth Forms
        '\u{20000}'..='\u{2A6DF}' | // CJK Extension B
        '\u{2A700}'..='\u{2B73F}' | // CJK Extension C
        '\u{2B740}'..='\u{2B81F}' | // CJK Extension D
        '\u{2F800}'..='\u{2FA1F}'   // CJK Compat Supplement
    )
}

/// Append styled chars to spans, merging adjacent chars with the same style.
fn append_chars_to_spans(chars: &[(char, Style)], spans: &mut Vec<Span<'static>>) {
    for (ch, style) in chars {
        if let Some(last) = spans.last_mut() {
            if last.style == *style {
                last.content.to_mut().push(*ch);
                continue;
            }
        }
        spans.push(Span::styled(ch.to_string(), *style));
    }
}

/// Break an oversized styled word at character boundaries to fit `max_width`.
fn break_styled_word(chars: &[(char, Style)], max_width: usize) -> Vec<Vec<Span<'static>>> {
    let mut result: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_w: usize = 0;

    for (ch, style) in chars {
        let cw = UnicodeWidthChar::width(*ch).unwrap_or(0);
        if current_w + cw > max_width && !current.is_empty() {
            result.push(std::mem::take(&mut current));
            current_w = 0;
        }
        if let Some(last) = current.last_mut() {
            if last.style == *style {
                last.content.to_mut().push(*ch);
            } else {
                current.push(Span::styled(ch.to_string(), *style));
            }
        } else {
            current.push(Span::styled(ch.to_string(), *style));
        }
        current_w += cw;
    }

    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Measure the total unicode display width of a set of Spans.
fn spans_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Render regular markdown (non-table content).
/// Detects fenced code blocks and applies syntax highlighting.
fn render_markdown(content: &str) -> Vec<Line<'static>> {
    let preprocessed = fix_bare_code_fences(content);

    // Split into segments: code blocks vs inline markdown
    let mut segments: Vec<MarkdownSegment> = Vec::new();
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut md_buf = String::new();

    for line in preprocessed.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_code {
                segments.push(MarkdownSegment::Code {
                    lang: std::mem::take(&mut code_lang),
                    content: std::mem::take(&mut code_buf),
                });
                in_code = false;
            } else {
                if !md_buf.is_empty() {
                    segments.push(MarkdownSegment::Markdown(std::mem::take(&mut md_buf)));
                }
                code_lang = trimmed.strip_prefix("```").unwrap_or("").trim().to_string();
                in_code = true;
            }
        } else if in_code {
            if !code_buf.is_empty() {
                code_buf.push('\n');
            }
            code_buf.push_str(line);
        } else {
            if !md_buf.is_empty() {
                md_buf.push('\n');
            }
            md_buf.push_str(line);
        }
    }

    if in_code {
        segments.push(MarkdownSegment::Code {
            lang: code_lang,
            content: code_buf,
        });
    } else if !md_buf.is_empty() {
        segments.push(MarkdownSegment::Markdown(md_buf));
    }

    let mut lines = Vec::new();
    for seg in &segments {
        match seg {
            MarkdownSegment::Markdown(md) => {
                let text: ratatui::text::Text<'_> = tui_markdown::from_str(md);
                for l in text.lines {
                    let line_style = l.style;
                    let spans: Vec<Span<'static>> = l
                        .spans
                        .into_iter()
                        .map(|s| Span::styled(s.content.into_owned(), line_style.patch(s.style)))
                        .collect();
                    lines.push(Line::from(spans));
                }
            }
            MarkdownSegment::Code { lang, content } => {
                lines.extend(highlight_code(content, lang));
            }
        }
    }
    lines
}

/// A segment of markdown — regular text or a fenced code block.
enum MarkdownSegment {
    Markdown(String),
    Code { lang: String, content: String },
}

// ── Code syntax highlighting ──────────────────────────────────────────
//
// Simple token-based highlighter. Detects:
// - Keywords (per language)
// - Strings (single/double quoted)
// - Comments (line)
// - Numbers
// - Type names (PascalCase)
// - Punctuation/operators

/// Code token types for styling.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TokenType {
    Normal,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Punctuation,
}

/// Map token type to a ratatui Style using default theme colors.
fn token_style(token: TokenType) -> Style {
    let dark = crate::theme::ColorScheme::dark();
    let styles = dark.to_styles();
    match token {
        TokenType::Normal => styles.normal,
        TokenType::Keyword => styles.accent.add_modifier(Modifier::BOLD),
        TokenType::String => styles.secondary,
        TokenType::Comment => styles.muted,
        TokenType::Number => styles.warning,
        TokenType::Type => styles.primary,
        TokenType::Function => styles.success,
        TokenType::Punctuation => styles.muted,
    }
}

/// Get keywords for a given language.
fn lang_keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" | "rs" => &[
            "fn", "let", "mut", "if", "else", "match", "loop", "while", "for", "in", "return",
            "break", "continue", "struct", "enum", "impl", "trait", "type", "pub", "mod", "use",
            "crate", "self", "super", "where", "async", "await", "move", "ref", "static", "const",
            "unsafe", "extern", "dyn", "as",
        ],
        "python" | "py" => &[
            "def", "class", "if", "elif", "else", "for", "while", "return", "import", "from", "as",
            "try", "except", "finally", "with", "yield", "lambda", "pass", "break", "continue",
            "and", "or", "not", "in", "is", "True", "False", "None", "self", "async", "await",
            "raise",
        ],
        "javascript" | "js" | "typescript" | "ts" | "tsx" | "jsx" => &[
            "function",
            "const",
            "let",
            "var",
            "if",
            "else",
            "for",
            "while",
            "do",
            "return",
            "class",
            "new",
            "this",
            "super",
            "import",
            "export",
            "from",
            "default",
            "async",
            "await",
            "try",
            "catch",
            "finally",
            "throw",
            "typeof",
            "instanceof",
            "void",
            "null",
            "undefined",
            "true",
            "false",
            "switch",
            "case",
            "break",
            "continue",
            "yield",
            "of",
            "in",
        ],
        "go" => &[
            "func",
            "var",
            "const",
            "type",
            "struct",
            "interface",
            "map",
            "chan",
            "if",
            "else",
            "for",
            "range",
            "return",
            "switch",
            "case",
            "default",
            "break",
            "continue",
            "go",
            "defer",
            "select",
            "package",
            "import",
            "nil",
            "true",
            "false",
        ],
        "bash" | "sh" | "shell" | "zsh" => &[
            "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac",
            "function", "return", "local", "export", "source", "echo", "cd", "exit", "set",
            "unset", "readonly", "shift",
        ],
        "toml" | "yaml" | "yml" | "json" => &["true", "false", "null", "yes", "no"],
        _ => &[],
    }
}

/// Get line comment prefix for a language.
fn line_comment_prefix(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" | "rs" | "javascript" | "js" | "typescript" | "ts" | "tsx" | "jsx" | "go" | "c"
        | "cpp" | "java" | "swift" | "kotlin" => Some("//"),
        "python" | "py" | "bash" | "sh" | "shell" | "zsh" | "toml" => Some("#"),
        "sql" => Some("--"),
        _ => None,
    }
}

/// Check if a string is PascalCase (potential type name).
fn is_pascal_case(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
        && s.chars().any(|c| c.is_lowercase())
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Check if the text before a position ends with `.` (method call).
fn preceded_by_dot(text_before: &str) -> bool {
    text_before.trim_end().ends_with('.')
}

/// Highlight a single code line into styled Spans.
fn highlight_line(line: &str, lang: &str) -> Line<'static> {
    let keywords = lang_keywords(lang);
    let comment_prefix = line_comment_prefix(lang);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // ── Line comment ──
        if let Some(prefix) = comment_prefix {
            if line[i..].starts_with(prefix) {
                let rest: String = chars[i..].iter().collect();
                spans.push(Span::styled(rest, token_style(TokenType::Comment)));
                break;
            }
        }

        // ── String literal (double-quoted) ──
        if chars[i] == '"' {
            let mut end = i + 1;
            while end < chars.len() {
                if chars[end] == '\\' && end + 1 < chars.len() {
                    end += 2;
                } else if chars[end] == '"' {
                    end += 1;
                    break;
                } else {
                    end += 1;
                }
            }
            let s: String = chars[i..end].iter().collect();
            spans.push(Span::styled(s, token_style(TokenType::String)));
            i = end;
            continue;
        }

        // ── String literal (single-quoted) ──
        if chars[i] == '\'' {
            let mut end = i + 1;
            while end < chars.len() {
                if chars[end] == '\\' && end + 1 < chars.len() {
                    end += 2;
                } else if chars[end] == '\'' {
                    end += 1;
                    break;
                } else {
                    end += 1;
                }
            }
            let s: String = chars[i..end].iter().collect();
            spans.push(Span::styled(s, token_style(TokenType::String)));
            i = end;
            continue;
        }

        // ── Number ──
        if chars[i].is_ascii_digit()
            || (chars[i] == '0'
                && i + 1 < chars.len()
                && (chars[i + 1] == 'x' || chars[i + 1] == 'b'))
        {
            let mut end = i;
            if chars[i] == '0' && end + 1 < chars.len() {
                let next = chars[end + 1];
                if next == 'x' || next == 'b' || next == 'o' {
                    end += 2;
                }
            }
            while end < chars.len()
                && (chars[end].is_ascii_hexdigit() || chars[end] == '.' || chars[end] == '_')
            {
                end += 1;
            }
            while end < chars.len() && chars[end].is_ascii_alphabetic() {
                end += 1;
            }
            let s: String = chars[i..end].iter().collect();
            spans.push(Span::styled(s, token_style(TokenType::Number)));
            i = end;
            continue;
        }

        // ── Identifier / keyword ──
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let mut end = i;
            while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let word: String = chars[i..end].iter().collect();
            let before: String = chars[..i].iter().collect();

            let token_type = if keywords.contains(&word.as_str()) {
                TokenType::Keyword
            } else if is_pascal_case(&word) {
                TokenType::Type
            } else if preceded_by_dot(&before) {
                TokenType::Function
            } else {
                TokenType::Normal
            };

            spans.push(Span::styled(word, token_style(token_type)));
            i = end;
            continue;
        }

        // ── Punctuation / operators ──
        let c = chars[i];
        let tok = if c == '('
            || c == ')'
            || c == '{'
            || c == '}'
            || c == '['
            || c == ']'
            || c == ':'
            || c == ';'
            || c == ','
            || c == '.'
        {
            TokenType::Punctuation
        } else {
            TokenType::Normal
        };
        spans.push(Span::styled(c.to_string(), token_style(tok)));
        i += 1;
    }

    if spans.is_empty() {
        spans.push(Span::raw(""));
    }
    Line::from(spans)
}

/// Highlight a code block with language-aware syntax coloring.
fn highlight_code(content: &str, lang: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for line in content.lines() {
        lines.push(highlight_line(line, lang));
    }
    lines
}

// ── Layout calculation ────────────────────────────────────────────────

/// Measure wrapped height using ratatui's Paragraph::line_count.
/// Measure the rendered height of pre-wrapped lines.
///
/// Lines are already wrapped to the correct width by `wrap_lines_styled()`,
/// so we just count them. No need to use `Paragraph::line_count` which
/// would try to re-wrap and produce incorrect results for CJK text.
fn measure_wrapped_height(lines: &[Line<'_>], _width: u16) -> u16 {
    lines.len() as u16
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
    #[allow(dead_code)]
    Label {
        text: String,
        style: Style,
    },
    Text {
        lines: Vec<Line<'static>>,
        is_user: bool,
    },
    ToolBox {
        name: String,
        arguments: String,
        result: Option<(String, bool)>,
        status: ToolCallStatus,
        duration: Option<String>,
        expanded: bool,
        key: String,
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
    Thinking {
        content: String,
        collapsed: bool,
        key: String,
    },
    Image {
        mime_type: String,
        size_str: String,
    },
    Spinner {
        frame: usize,
    },
    Dashboard {
        info: DashboardInfo,
    },
}

/// Check if a content block renders as a bordered box (tool calls, errors).
/// Consecutive box blocks need spacers between them for visual separation.
fn is_box_block(block: &ContentBlock) -> bool {
    matches!(
        block,
        ContentBlock::ToolCall { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Error { .. }
    )
}

fn compute_layout(state: &ChatViewState, width: u16) -> Vec<LayoutEntry> {
    let mut entries = Vec::new();
    let mut y: u32 = 0;

    let mut rendered_any_message = false;
    let mut msg_idx: usize = 0;

    'outer: for msg in &state.messages {
        // Skip messages that have no visible content; they only create empty spacer rows.
        let has_visible_content = msg.content_blocks.iter().any(|b| match b {
            ContentBlock::Text { content } => !content.trim().is_empty(),
            ContentBlock::Thinking { content, .. } => !content.trim().is_empty(),
            _ => true,
        });
        if !has_visible_content {
            msg_idx += 1;
            continue;
        }

        if rendered_any_message {
            // Gap between messages — use a spacer for breathing room
            if y <= u16::MAX as u32 {
                entries.push(LayoutEntry {
                    y: y as u16,
                    height: 1,
                    kind: LayoutKind::Spacer,
                });
            }
            y += 1;
        }
        rendered_any_message = true;

        // User messages: left accent border, no label needed (single-user context)
        if msg.role == MessageRole::User {
            if y <= u16::MAX as u32 {
                entries.push(LayoutEntry {
                    y: y as u16,
                    height: 1,
                    kind: LayoutKind::Rule,
                });
            }
            y += 1;
        }
        let mut prev_was_box = false;
        for (blk_idx, block) in msg.content_blocks.iter().enumerate() {
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
                if y <= u16::MAX as u32 {
                    entries.push(LayoutEntry {
                        y: y as u16,
                        height: 1,
                        kind: LayoutKind::Spacer,
                    });
                }
                y += 1;
            }
            prev_was_box = is_box;

            let key = format!("{}:{}", msg_idx, blk_idx);
            let mut kind = block_to_layout_kind(block, msg.role, width, &key);
            // Override collapsed/expanded state
            #[allow(clippy::collapsible_match)]
            match &mut kind {
                LayoutKind::Thinking {
                    ref mut collapsed,
                    ref key,
                    ..
                } =>
                {
                    #[allow(clippy::collapsible_match)]
                    if state.expanded_thinking.contains(key) {
                        *collapsed = false;
                    }
                }
                LayoutKind::ToolBox {
                    ref mut expanded,
                    ref key,
                    ..
                } =>
                {
                    #[allow(clippy::collapsible_match)]
                    if state.expanded_tools.contains(key) {
                        *expanded = true;
                    }
                }
                _ => {}
            }
            let h = measure_kind(&kind, width, &state.expanded_thinking);
            if y > u16::MAX as u32 {
                break 'outer;
            }
            entries.push(LayoutEntry {
                y: y as u16,
                height: h,
                kind,
            });
            y += h as u32;
        }
        msg_idx += 1;
    }

    if let Some(ref streaming) = state.streaming {
        // Only add a spacer if we actually rendered any history messages.
        if rendered_any_message {
            if y <= u16::MAX as u32 {
                entries.push(LayoutEntry {
                    y: y as u16,
                    height: 1,
                    kind: LayoutKind::Spacer,
                });
            }
            y += 1;
        }
        let mut prev_was_box = false;
        for (blk_idx, block) in streaming.message.content_blocks.iter().enumerate() {
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
                if y <= u16::MAX as u32 {
                    entries.push(LayoutEntry {
                        y: y as u16,
                        height: 1,
                        kind: LayoutKind::Spacer,
                    });
                }
                y += 1;
            }
            prev_was_box = is_box;

            let key = format!("s:{}", blk_idx);
            let mut kind = block_to_layout_kind(block, MessageRole::Assistant, width, &key);
            #[allow(clippy::collapsible_match)]
            match &mut kind {
                LayoutKind::Thinking {
                    ref mut collapsed,
                    ref key,
                    ..
                } =>
                {
                    #[allow(clippy::collapsible_match)]
                    if state.expanded_thinking.contains(key) {
                        *collapsed = false;
                    }
                }
                LayoutKind::ToolBox {
                    ref mut expanded,
                    ref key,
                    ..
                } =>
                {
                    #[allow(clippy::collapsible_match)]
                    if state.expanded_tools.contains(key) {
                        *expanded = true;
                    }
                }
                _ => {}
            }
            let h = measure_kind(&kind, width, &state.expanded_thinking);
            if y <= u16::MAX as u32 {
                entries.push(LayoutEntry {
                    y: y as u16,
                    height: h,
                    kind,
                });
            }
            y += h as u32;
        }
        if y <= u16::MAX as u32 {
            entries.push(LayoutEntry {
                y: y as u16,
                height: 1,
                kind: LayoutKind::Spinner {
                    frame: state.spinner_frame,
                },
            });
        }
    }

    entries
}

fn block_to_layout_kind(
    block: &ContentBlock,
    role: MessageRole,
    width: u16,
    key: &str,
) -> LayoutKind {
    match block {
        ContentBlock::Text { content } => {
            // For user messages, Block::borders(LEFT) takes 1 column,
            // so text must be wrapped to width-1.
            let wrap_w = if role == MessageRole::User {
                width.saturating_sub(1)
            } else {
                width
            };
            let lines = md_lines(content, wrap_w);
            LayoutKind::Text {
                lines,
                is_user: role == MessageRole::User,
            }
        }
        ContentBlock::Thinking { content, collapsed } => {
            // The actual collapsed state is determined by the key lookup later
            // in compute_layout — we store the default here.
            LayoutKind::Thinking {
                content: content.clone(),
                collapsed: *collapsed,
                key: key.to_string(),
            }
        }
        ContentBlock::ToolCall {
            name,
            arguments,
            result,
            status,
            duration,
            ..
        } => LayoutKind::ToolBox {
            name: name.clone(),
            arguments: arguments.clone(),
            result: result.clone(),
            status: *status,
            duration: duration.clone(),
            expanded: false, // overridden in compute_layout
            key: key.to_string(),
        },
        ContentBlock::ToolResult {
            tool_name,
            content,
            is_error,
        } => LayoutKind::ToolResultBox {
            tool_name: tool_name.clone(),
            content: content.clone(),
            is_error: *is_error,
        },
        ContentBlock::Error {
            title,
            message,
            retryable,
        } => LayoutKind::ErrorBox {
            title: title.clone(),
            message: message.clone(),
            retryable: *retryable,
        },
        ContentBlock::Image {
            mime_type,
            base64_data,
        } => {
            let sz = base64_data.len() * 3 / 4;
            let sz_str = if sz >= 1_048_576 {
                format!("{:.1} MB", sz as f64 / 1_048_576.0)
            } else if sz >= 1024 {
                format!("{:.1} KB", sz as f64 / 1024.0)
            } else {
                format!("{} B", sz)
            };
            LayoutKind::Image {
                mime_type: mime_type.clone(),
                size_str: sz_str,
            }
        }
        ContentBlock::Dashboard { info } => LayoutKind::Dashboard { info: info.clone() },
    }
}

fn measure_kind(kind: &LayoutKind, width: u16, expanded_thinking: &HashSet<String>) -> u16 {
    match kind {
        LayoutKind::Spacer
        | LayoutKind::Rule
        | LayoutKind::Label { .. }
        | LayoutKind::Spinner { .. } => 1,
        LayoutKind::Text { lines, is_user } => {
            // User text: Block::borders(LEFT) takes 1 col, so inner = width-1
            // Assistant text: no block, renders at full width
            let w = if *is_user {
                width.saturating_sub(1)
            } else {
                width
            };
            measure_wrapped_height(lines, w)
        }
        LayoutKind::ToolBox {
            name,
            arguments,
            result,
            duration,
            expanded,
            ..
        } => {
            use crate::widgets::tool_renderer::{measure_call_height, measure_result_height};
            // inner width for the bordered block: borders take 2 cols
            let inner_w = width.saturating_sub(2) as usize;
            let call_h = measure_call_height(name, arguments, inner_w);
            let result_h = result.as_ref().map_or(0, |(r, is_err)| {
                if *expanded {
                    // Full result: show all lines, capped at 80
                    let total = r.lines().count();
                    let shown = total.min(80);
                    let ellipsis = if total > 80 { 1 } else { 0 };
                    shown as u16 + ellipsis
                } else if *is_err {
                    let total = r.lines().count();
                    total.min(4) as u16 + if total > 4 { 1 } else { 0 }
                } else {
                    measure_result_height(name, r, false)
                }
            });
            // Block::ALL adds top + bottom border (2 rows) + separator if result exists
            let separator_h = if result.is_some() { 1 } else { 0 };
            // Toggle hint line when result exists
            let toggle_h = if result.is_some() { 1 } else { 0 };
            let _ = duration;
            2 + call_h + separator_h + result_h + toggle_h
        }
        LayoutKind::ToolResultBox { content, .. } => {
            // 1 header + content lines (max 4) + optional ellipsis
            let n = content.lines().count().min(4);
            1 + n as u16 + if content.lines().count() > 4 { 1 } else { 0 }
        }
        LayoutKind::ErrorBox {
            message, retryable, ..
        } => {
            // Block::bordered() adds top + bottom border (2 rows)
            let n = message.lines().count().min(4);
            2 + n as u16 + if *retryable { 1 } else { 0 }
        }
        LayoutKind::Thinking {
            content,
            collapsed,
            key,
        } => {
            let is_expanded = expanded_thinking.contains(key);
            if *collapsed && !is_expanded {
                // Collapsed: header + one preview line
                let filtered = filter_tool_json(content);
                let line_count = filtered.lines().count();
                1 + if line_count > 0 { 1 } else { 0 }
            } else {
                // Expanded: header + filtered content rendered as markdown
                let filtered = filter_tool_json(content);
                let md = md_lines(&filtered, width);
                1 + md.len() as u16
            }
        }
        LayoutKind::Image { .. } => 2,
        LayoutKind::Dashboard { info } => measure_dashboard(info, width),
    }
}

// ── Dashboard helpers ────────────────────────────────────────────────────

/// Count how many lines badges will occupy given the available width.
fn badge_line_count(names: &[String], width: usize) -> usize {
    if names.is_empty() {
        return 0;
    }
    let prefix = 2; // "  " indent
    let mut lines = 1;
    let mut current_width = prefix;

    for name in names {
        // [name] = name display width + 2 brackets + 1 space
        let badge_w = UnicodeWidthStr::width(name.as_str()) + 3;
        if current_width + badge_w > width && current_width > prefix {
            lines += 1;
            current_width = prefix + badge_w;
        } else {
            current_width += badge_w;
        }
    }
    lines
}

/// Build styled badge Lines that wrap at the given width.
fn compute_badge_lines(names: &[String], width: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    if names.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let prefix = 2;
    let mut current_spans: Vec<Span<'static>> = vec![Span::raw("  ")];
    let mut current_width = prefix;

    for name in names {
        let badge_text = format!("[{}] ", name);
        let badge_w = UnicodeWidthStr::width(badge_text.as_str());

        if current_width + badge_w > width && current_width > prefix {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
            current_spans = vec![Span::raw("  ")];
            current_width = prefix;
        }

        current_spans.push(Span::styled(badge_text, styles.primary));
        current_width += badge_w;
    }

    if current_spans.len() > 1 {
        lines.push(Line::from(current_spans));
    }

    lines
}

/// Shorten a path for display (replace home dir with ~).
fn shorten_dashboard_path(path: &str) -> String {
    crate::widgets::tool_renderer::shorten_path(path)
}

/// Measure the dashboard panel height.
fn measure_dashboard(info: &DashboardInfo, width: u16) -> u16 {
    let w = width as usize;
    let mut h: usize = 0;

    h += 1; // version header
    h += 1; // empty line
    h += 1; // model
    h += 1; // project
    h += 1; // agents.md
    h += 1; // empty line

    if !info.tool_names.is_empty() {
        h += 1; // tools header
        h += badge_line_count(&info.tool_names, w);
    }

    h += 1; // empty line

    if !info.skill_names.is_empty() {
        h += 1; // skills header
        h += badge_line_count(&info.skill_names, w);
        h += 1; // empty line
    }

    h += 1; // shortcuts

    h as u16
}

/// Build all dashboard content lines for rendering.
fn dashboard_lines(info: &DashboardInfo, width: u16, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut lines = Vec::new();

    // Version header
    lines.push(Line::from(vec![
        Span::styled("  \u{25C8} ", styles.accent),
        Span::styled(
            format!("oxi v{}", info.version),
            styles.accent.add_modifier(Modifier::BOLD),
        ),
    ]));

    // Empty line
    lines.push(Line::raw(""));

    // Model info
    let model_display = info
        .model_id
        .split('/')
        .next_back()
        .unwrap_or(&info.model_id);
    let provider = info.model_id.split('/').next().unwrap_or("");

    let mut model_spans = vec![
        Span::raw("  "),
        Span::styled("Model     ", styles.muted),
        Span::styled(format!("{} ({})", model_display, provider), styles.normal),
    ];
    if !info.thinking_level.is_empty() {
        model_spans.push(Span::styled(" \u{00B7} ", styles.muted));
        model_spans.push(Span::styled("Thinking: ", styles.muted));
        model_spans.push(Span::styled(info.thinking_level.clone(), styles.warning));
    }
    lines.push(Line::from(model_spans));

    // Project
    let project = if let Some(ref branch) = info.git_branch {
        format!("{} ({})", info.project_name, branch)
    } else {
        info.project_name.clone()
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Project   ", styles.muted),
        Span::styled(project, styles.normal),
    ]));

    // AGENTS.md
    let agents = match &info.agents_md_path {
        Some(p) => shorten_dashboard_path(p),
        None => "not found".to_string(),
    };
    let agents_style = if info.agents_md_path.is_some() {
        styles.success
    } else {
        styles.muted
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("AGENTS.md ", styles.muted),
        Span::styled(agents, agents_style),
    ]));

    // Empty line
    lines.push(Line::raw(""));

    // Tools section header
    if !info.tool_names.is_empty() {
        let header_text = format!("  \u{2500}\u{2500} Tools ({}) ", info.tool_names.len());
        let header_w = UnicodeWidthStr::width(header_text.as_str());
        let sep_remain = w.saturating_sub(header_w);
        let header_full = format!("{}{}", header_text, "\u{2500}".repeat(sep_remain));
        lines.push(Line::from(Span::styled(header_full, styles.border)));

        let badges = compute_badge_lines(&info.tool_names, w, styles);
        lines.extend(badges);
    }

    // Empty line
    lines.push(Line::raw(""));

    // Skills section
    if !info.skill_names.is_empty() {
        let header_text = format!("  \u{2500}\u{2500} Skills ({}) ", info.skill_names.len());
        let header_w = UnicodeWidthStr::width(header_text.as_str());
        let sep_remain = w.saturating_sub(header_w);
        let header_full = format!("{}{}", header_text, "\u{2500}".repeat(sep_remain));
        lines.push(Line::from(Span::styled(header_full, styles.border)));

        let badges = compute_badge_lines(&info.skill_names, w, styles);
        lines.extend(badges);

        lines.push(Line::raw(""));
    }

    // Shortcuts
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Enter ", styles.primary),
        Span::styled("send  \u{00B7}  ", styles.muted),
        Span::styled("/help ", styles.primary),
        Span::styled("commands  \u{00B7}  ", styles.muted),
        Span::styled("Ctrl+C ", styles.primary),
        Span::styled("interrupt", styles.muted),
    ]));

    lines
}

// ── Rendering into ScrollView ─────────────────────────────────────────

/// A wrapper widget that renders a single content block.
struct EntryWidget<'a> {
    entry: &'a LayoutKind,
    styles: &'a ThemeStyles,
}

impl<'a> EntryWidget<'a> {
    fn new(entry: &'a LayoutKind, styles: &'a ThemeStyles) -> Self {
        Self { entry, styles }
    }
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
                    // Lines are already pre-wrapped to the correct width
                    // by wrap_lines_styled(). Do NOT use .wrap() here —
                    // ratatui's WordWrapper does not handle CJK line-breaking.
                    Paragraph::new(text).render(inner, buf);
                } else {
                    // Don't set .style() here — markdown Spans already carry
                    // their own styling (bold, italic, code, headings).
                    // Paragraph::style() would override all per-Span styles.
                    // Lines are pre-wrapped; no .wrap() needed.
                    Paragraph::new(text).render(rect, buf);
                }
            }
            LayoutKind::ToolBox {
                name,
                arguments,
                result,
                status,
                duration,
                expanded,
                key: _,
            } => {
                use crate::widgets::tool_renderer::{format_tool_call, format_tool_result};

                let (icon, border_style) = match status {
                    ToolCallStatus::Requested => (
                        "\u{25CB}", // ○ (hollow circle — universal)
                        self.styles.muted,
                    ),
                    ToolCallStatus::Executing => (
                        "\u{25CF}", // ● (filled circle — running)
                        self.styles.warning,
                    ),
                    ToolCallStatus::Done => {
                        let is_error = result.as_ref().is_some_and(|(_, e)| *e);
                        if is_error {
                            ("\u{2718}", self.styles.error)
                        } else {
                            ("\u{2713}", self.styles.success)
                        }
                    }
                };

                let has_result = result.is_some();

                // Determine background style based on status
                let bg_style = match status {
                    ToolCallStatus::Requested => self.styles.tool_pending_bg,
                    ToolCallStatus::Executing => self.styles.tool_executing_bg,
                    ToolCallStatus::Done => {
                        let is_error = result.as_ref().is_some_and(|(_, e)| *e);
                        if is_error {
                            self.styles.tool_error_bg
                        } else {
                            self.styles.tool_success_bg
                        }
                    }
                };

                // Box with all borders + status background
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .style(bg_style);
                let inner = block.inner(rect);
                block.render(rect, buf);
                // Do NOT Clear the inner area here — Clear resets cells to
                // default style, which removes the block's background color
                // (tool_pending_bg, tool_success_bg, etc.). The buffer is
                // already clean at the start of each frame (ratatui
                // swap_buffers resets it), so stale content is not an issue.

                let max_w = inner.width as usize;
                let mut content_lines: Vec<Line<'static>> = Vec::new();

                // Calculate reserved space for icon prefix and duration suffix
                // on the header line so format_tool_call truncates correctly.
                let icon_prefix_w = UnicodeWidthStr::width(format!("{} ", icon).as_str());
                let duration_suffix_w = duration
                    .as_ref()
                    .map(|d| UnicodeWidthStr::width(format!("  {}", d).as_str()))
                    .unwrap_or(0);
                let header_avail = max_w
                    .saturating_sub(icon_prefix_w)
                    .saturating_sub(duration_suffix_w)
                    .max(20);

                // Format tool call using new renderer (truncate to header_avail)
                let call_lines = format_tool_call(name, arguments, header_avail, self.styles);
                for (i, line) in call_lines.into_iter().enumerate() {
                    if i == 0 {
                        // Prepend icon to first line, append duration if available
                        let icon_style = border_style.add_modifier(Modifier::BOLD);
                        let name_style = border_style.add_modifier(Modifier::BOLD);
                        let spans = line.spans.into_iter().collect::<Vec<_>>();
                        let mut new_spans = vec![Span::styled(format!("{} ", icon), icon_style)];
                        for span in spans {
                            new_spans.push(Span::styled(
                                span.content.clone(),
                                span.style.patch(name_style),
                            ));
                        }
                        // Append duration to header line
                        if let Some(ref dur) = duration {
                            new_spans.push(Span::styled(format!("  {}", dur), self.styles.muted));
                        }
                        content_lines.push(Line::from(new_spans));
                    } else {
                        content_lines.push(line);
                    }
                }

                // Separator line between call and result — full width to match borders
                if has_result {
                    content_lines.push(Line::from(Span::styled(
                        "\u{2500}".repeat(max_w),
                        border_style,
                    )));
                }

                // Format result
                if let Some((result_content, is_err)) = result {
                    if *expanded {
                        // Expanded: show full result (up to 80 lines)
                        let all_lines: Vec<&str> = result_content.lines().collect();
                        let total = all_lines.len();
                        let shown = total.min(80);
                        for line in &all_lines[..shown] {
                            let display =
                                crate::text::truncate_to_width(line, max_w.saturating_sub(2));
                            content_lines.push(Line::from(Span::styled(
                                format!("  {}", display),
                                if *is_err {
                                    self.styles.error
                                } else {
                                    self.styles.normal
                                },
                            )));
                        }
                        if total > 80 {
                            content_lines.push(Line::from(Span::styled(
                                crate::text::truncate_to_width(
                                    &format!("  \u{2026} ({} more lines)", total - 80),
                                    max_w,
                                ),
                                self.styles.muted,
                            )));
                        }
                    } else {
                        // Collapsed: use the formatted preview
                        let result_lines =
                            format_tool_result(name, result_content, *is_err, max_w, self.styles);
                        content_lines.extend(result_lines);
                    }

                    // Toggle hint — truncate to max_w to prevent overflow
                    let total_lines = result_content.lines().count();
                    let toggle_hint = if *expanded {
                        " \u{00B7} click to collapse".to_string()
                    } else {
                        format!(" \u{00B7} {} lines \u{00B7} click to expand", total_lines)
                    };
                    content_lines.push(Line::from(Span::styled(
                        crate::text::truncate_to_width(&toggle_hint, max_w),
                        self.styles.muted,
                    )));
                }

                let text: ratatui::text::Text = content_lines.into_iter().collect();
                let para = Paragraph::new(text);
                para.render(inner, buf);
            }
            LayoutKind::ToolResultBox {
                tool_name,
                content,
                is_error,
            } => {
                let (icon, border_style) = if *is_error {
                    ("\u{2718}", self.styles.error)
                } else {
                    ("\u{2713}", self.styles.success)
                };
                let label = if tool_name.is_empty() {
                    icon.to_string()
                } else {
                    format!("{} {}", icon, tool_name)
                };

                let block = Block::default()
                    .borders(Borders::LEFT)
                    .border_style(border_style);
                let inner = block.inner(rect);
                block.render(rect, buf);

                let max_w = inner.width as usize;
                let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
                    format!("  {}", label),
                    border_style.add_modifier(Modifier::BOLD),
                ))];
                for l in content.lines().take(4) {
                    let display = truncate_str(l, max_w.saturating_sub(2));
                    lines.push(Line::from(Span::styled(
                        format!("  {}", display),
                        self.styles.normal,
                    )));
                }
                if content.lines().count() > 4 {
                    lines.push(Line::from(Span::styled("  \u{2026}", self.styles.muted)));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).render(inner, buf);
            }
            LayoutKind::ErrorBox {
                title,
                message,
                retryable,
            } => {
                let block = Block::bordered()
                    .border_style(self.styles.error)
                    .title(Span::styled(
                        format!(" error: {} ", title),
                        Style::default()
                            .fg(ratatui::style::Color::White)
                            .add_modifier(Modifier::BOLD),
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
                    lines.push(Line::from(Span::styled(
                        "retry: this error may be temporary",
                        self.styles.muted,
                    )));
                }
                let text: ratatui::text::Text = lines.into_iter().collect();
                // No wrap — pre-truncated to exact width
                Paragraph::new(text).render(inner, buf);
            }
            LayoutKind::Thinking {
                content,
                collapsed,
                key: _,
            } => {
                let filtered = filter_tool_json(content);
                let line_count = filtered.lines().count();
                let mut lines: Vec<Line<'static>> = Vec::new();

                // Header with toggle hint
                let header_style = self.styles.accent;
                let count_str = if line_count > 0 {
                    format!(
                        " ({} line{})",
                        line_count,
                        if line_count == 1 { "" } else { "s" }
                    )
                } else {
                    String::new()
                };

                if *collapsed {
                    lines.push(Line::from(vec![
                        Span::styled("\u{25B8} ", header_style),
                        Span::styled("thinking".to_string(), header_style),
                        Span::styled(count_str, self.styles.muted),
                        Span::styled(" \u{00B7} click to expand".to_string(), self.styles.muted),
                    ]));
                    if let Some(first) = filtered.lines().next() {
                        let preview: String = first.chars().take(80).collect();
                        lines.push(Line::from(Span::styled(
                            format!("  {}", preview),
                            self.styles.muted.add_modifier(Modifier::ITALIC),
                        )));
                    }
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("\u{25BE} ", header_style),
                        Span::styled("thinking".to_string(), header_style),
                        Span::styled(count_str, self.styles.muted),
                    ]));
                    let thinking_style = self.styles.muted.add_modifier(Modifier::ITALIC);
                    let md_rendered = md_lines(&filtered, rect.width);
                    for md_line in md_rendered {
                        let spans: Vec<Span<'static>> = md_line
                            .spans
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
            LayoutKind::Image {
                mime_type,
                size_str,
            } => {
                let lines = vec![
                    Line::from(Span::styled(
                        format!("[image: {}, {}]", mime_type, size_str),
                        self.styles.normal,
                    )),
                    Line::from(Span::styled(
                        "  Ctrl+I -> open in viewer",
                        self.styles.muted,
                    )),
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
                    format!("  {} Working...", ch),
                    self.styles.accent,
                )))
                .render(rect, buf);
            }
            LayoutKind::Dashboard { info } => {
                let lines = dashboard_lines(info, rect.width, self.styles);
                let text: ratatui::text::Text = lines.into_iter().collect();
                Paragraph::new(text).render(rect, buf);
            }
        }
    }
}

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
                let mut tmp = Buffer::empty(tmp_rect);
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
        let long = (0..20)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = clamp_str(long.clone(), 10000, 5);
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
        use ratatui::style::Modifier;
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

/// Filter JSON tool call arrays from thinking text.
/// GLM-5.1 writes tool call plans as `[{\"function\":...}]` inside
/// reasoning_content. We detect standalone JSON array lines (starting
/// with `[{\"` and ending with `]`) and remove them. Line-by-line
/// filtering avoids false positives from inline `[{\"` patterns in
/// normal reasoning text.
fn filter_tool_json(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim();
            // Only remove lines that are standalone JSON tool call arrays.
            // GLM pattern: [{"function":...}] on its own line.
            // Inline occurrences like "see [{"key": "val"}] here" are preserved.
            !(trimmed.starts_with("[{\"") && trimmed.ends_with(']'))
        })
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
