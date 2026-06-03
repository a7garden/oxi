//! ChatViewState, StreamingState, ToolCallTracker, LayoutCache, and limits.

use std::collections::{HashMap, HashSet};

use parking_lot::RwLock;

use crate::theme::ThemeStyles;
use crate::widgets::chat::layout::{compute_layout, LayoutEntry};
use crate::widgets::chat::markdown::extract_last_code_block;
use crate::widgets::chat::types::{
    ChatMessage, ContentBlock, MessageRole, StreamingState, ToolCallStatus,
};

// ── Limits (truncation at ingest) ──────────────────────────────────────

const MAX_TOOL_ARG_CHARS: usize = 50_000;
const MAX_TOOL_ARG_LINES: usize = 200;
const MAX_TOOL_RESULT_CHARS: usize = 50_000;
const MAX_TOOL_RESULT_LINES: usize = 100;
pub(crate) const MAX_TEXT_CHARS: usize = 500_000;

pub(crate) fn clamp_str(s: String, max_chars: usize, max_lines: usize) -> String {
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
    pub(crate) fn clamp_scroll(&mut self, visible_height: u16) {
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

    pub fn start_streaming(&mut self) -> bool {
        // Auto-commit any existing streaming before starting new.
        // This prevents tool execution results from being lost when
        // a new MessageStart arrives while tool results are streaming.
        let auto_committed = if self.streaming.is_some() {
            self.finish_streaming();
            true
        } else {
            false
        };
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
        auto_committed
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
                // Push to the END so the rendering order matches arrival order.
                // Previously `insert(0, ...)` put new text at the TOP, which
                // caused the response to appear ABOVE thinking blocks.
                s.message
                    .content_blocks
                    .push(ContentBlock::Text { content: truncated });
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
                content: existing,
                collapsed: existing_collapsed,
            }) = s.message.content_blocks.last_mut()
            {
                existing.push_str(&content);
                *existing = clamp_str(existing.clone(), 50_000, 200);
                // When streaming new content, mark as expanded so the user
                // can see the thinking process unfold in real-time.
                *existing_collapsed = false;
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
        // Preserve partial message (same as finish_streaming) so the user
        // doesn't lose content that was already generated.
        if let Some(mut s) = self.streaming.take() {
            s.message.content_blocks.retain(|b| match b {
                ContentBlock::Text { content } => !content.trim().is_empty(),
                ContentBlock::Thinking { content, .. } => !content.trim().is_empty(),
                _ => true,
            });
            if !s.message.content_blocks.is_empty() {
                // Mark the last text block as cancelled
                if let Some(ContentBlock::Text { content }) = s.message.content_blocks.last_mut() {
                    if !content.ends_with('\n') {
                        content.push('\n');
                    }
                    content.push_str("\u{2026} [cancelled]");
                }
                self.messages.push(s.message);
            }
        }
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
    pub(crate) fn get_layout(&self, width: u16, styles: &ThemeStyles) -> Vec<LayoutEntry> {
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
        let entries = compute_layout(self, width, styles);
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
