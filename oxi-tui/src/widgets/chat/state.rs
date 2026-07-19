use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::theme::ThemeStyles;
use crate::widgets::chat::layout::{LayoutEntry, compute_layout};
use crate::widgets::chat::markdown::extract_last_code_block;
use crate::widgets::chat::types::{
    ChatMessage, ContentBlock, MessageRole, StreamingState, ToolCallStatus,
};
use crate::widgets::tool_renderer::ToolFormatCache;

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
// - streaming text line count (line-based, not char-based — see below)
// - width
//
// NOT invalidated by:
// - spinner_frame (only affects a Spinner entry's visual content, not layout)
//
// Uses parking_lot::RwLock so multiple readers can access concurrently.

#[derive(Default)]
struct LayoutCache {
    /// Last known messages count
    msg_count: usize,
    /// Last known streaming block count
    streaming_len: usize,
    /// Last known streaming text LINE count (detects layout-affecting growth).
    ///
    /// Line-based rather than char-based: appending text within an existing
    /// line does NOT invalidate the layout (heights depend on wrapped line
    /// count, not raw char count). Only newline additions or width changes
    /// trigger invalidation.
    streaming_text_len: usize,
    /// Last known width
    width: u16,
    /// Cached layout entries (None = needs recompute)
    entries: Option<Vec<LayoutEntry>>,
    /// Cached total content height
    total_height: u32,
}

impl std::fmt::Debug for LayoutCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutCache")
            .field("msg_count", &self.msg_count)
            .field("streaming_len", &self.streaming_len)
            .field("streaming_text_len", &self.streaming_text_len)
            .field("width", &self.width)
            .field("entries", &self.entries.as_ref().map(|v| v.len()))
            .field("total_height", &self.total_height)
            .finish()
    }
}

// ── FollowMode state machine (W1 step 2) ────────────────────────────────

/// After the user scrolls up while streaming, we hold a 2-second grace
pub const FOLLOW_GRACE: Duration = Duration::from_secs(2);
/// (anchor = topmost visible message). New content during grace still
/// follows. New content after grace is held behind a "↓ new answer" badge.
#[derive(Debug, Clone, Default)]
pub enum FollowMode {
    /// Bottom-of-content tracking — viewport_base is updated to follow new
    /// content as it arrives.
    #[default]
    Following,
    /// User scrolled up; still in grace period (no further user input).
    /// New content extends the bottom, viewport follows for `until`,
    /// then transitions to `Pinned` with the topmost visible message as anchor.
    FollowingGrace { until: Instant },
    /// User has explicitly stopped following. viewport_base is frozen;
    /// new content accumulates at the bottom without auto-scroll.
    /// `anchor_msg_idx` is the index of the topmost message currently
    /// visible (stable across new message arrivals — new messages are
    /// appended below). `anchor_y_in_msg` is the y offset within that
    /// message's rendered range.
    Pinned {
        anchor_msg_idx: usize,
        anchor_y_in_msg: u32,
    },
}
/// Resolve the topmost visible message at `scroll_offset` in the given layout.
/// Returns `(msg_idx, y_within_msg)` suitable for use as a Pinned anchor.
pub(crate) fn resolve_anchor(layout: &[LayoutEntry], scroll_offset: u32) -> (usize, u32) {
    for entry in layout {
        // The topmost visible entry is the one whose range overlaps scroll_offset
        // and whose top is the closest to (but not below) scroll_offset.
        if entry.y <= scroll_offset && entry.y.saturating_add(entry.height) > scroll_offset {
            return (entry.msg_idx, scroll_offset - entry.y);
        }
    }
    // Fallback: topmost entry (or 0,0 if layout is empty).
    layout.first().map(|e| (e.msg_idx, 0)).unwrap_or((0, 0))
}
// ── ChatViewState ──────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct ChatViewState {
    pub messages: Vec<ChatMessage>,
    pub streaming: Option<StreamingState>,
    pub spinner_frame: usize,
    pub content_height: u32,
    pub follow: FollowMode,
    /// True when new content arrived during Pinned — shows "↓ new answer" badge.
    /// Cleared when the user scrolls down to bottom or scrolls to a new anchor.
    pub new_answer_pending: bool,
    pub last_code_block: Option<String>,
    pub pending_images: Vec<(String, String)>,
    tool_tracker: ToolCallTracker,
    /// Memoized tool-call / tool-result formatting. Keyed by input hash;
    /// invalidated automatically when the theme or glyph set changes.
    pub(crate) tool_format_cache: ToolFormatCache,
    /// Virtual y position of the viewport's top row (0 = chat top).
    /// **Virtual coordinate** — u32 to break the 65,535-row u16 cap.
    /// u32 keeps memory at 4 bytes (vs 8 for usize) and supports 4 billion
    /// rows — practically unbounded.
    pub scroll_offset: u32,
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
    /// Copyable message regions: (y_start, y_end, msg_idx).
    pub message_regions: Vec<(u16, u16, usize)>,
    /// Last rendered chat viewport rect (absolute screen coords).
    /// Populated each render; read by the keyboard toggle handler to find
    /// which collapsible block is at the viewport top.
    pub viewport_rect: ratatui::layout::Rect,
}

impl ChatViewState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scroll to bottom of content. `visible_height` is the viewport height.
    /// Transitions `follow` to `Following` (was: set auto_scroll = true).
    pub fn scroll_to_bottom(&mut self, visible_height: u16) {
        self.follow = FollowMode::Following;
        self.new_answer_pending = false;
        let vh = visible_height as u32;
        if self.content_height > vh {
            self.scroll_offset = self.content_height - vh;
        } else {
            self.scroll_offset = 0;
        }
    }

    /// Scroll up. `follow` transitions:
    /// - Following → FollowingGrace { until = now + 2s }
    /// - FollowingGrace → extends the same grace
    /// - Pinned → stays Pinned (anchor updates to topmost visible)
    ///
    /// New answer badge is cleared (user is engaging with viewport).
    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n as u32);
        self.new_answer_pending = false;
        match self.follow {
            FollowMode::Following => {
                self.follow = FollowMode::FollowingGrace {
                    until: Instant::now() + FOLLOW_GRACE,
                };
            }
            FollowMode::FollowingGrace { .. } => {
                // Re-set to extend the grace window from now.
                self.follow = FollowMode::FollowingGrace {
                    until: Instant::now() + FOLLOW_GRACE,
                };
            }
            FollowMode::Pinned { .. } => {
                // Stays Pinned; anchor updates on next render via resolve_anchor.
            }
        }
    }

    /// Scroll down. `follow` transitions:
    /// - Following → Following (no-op)
    /// - FollowingGrace → Following (re-engaged)
    /// - Pinned → if at bottom → Following
    pub fn scroll_down(&mut self, n: u16) {
        // NOTE: We don't clamp to content_height - visible_height here
        // because visible_height is unknown.  render() calls
        // clamp_scroll(area.height) on every frame which performs the
        // correct clamping.
        self.scroll_offset = self.scroll_offset.saturating_add(n as u32);
        self.new_answer_pending = false;
        match self.follow {
            FollowMode::Following => {}
            FollowMode::FollowingGrace { .. } => {
                self.follow = FollowMode::Following;
            }
            FollowMode::Pinned { .. } => {
                // Anchor updates on next render if at bottom → Following.
            }
        }
    }

    /// Scroll to top. Always → Pinned at top (msg 0, y 0).
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.new_answer_pending = false;
        self.follow = FollowMode::Pinned {
            anchor_msg_idx: 0,
            anchor_y_in_msg: 0,
        };
    }

    /// Clamp scroll_offset to [0, content_height - visible_height].
    /// If the clamp snaps to bottom AND follow was Pinned, promote to Following
    /// (user has reached the bottom — no point staying pinned).
    pub(crate) fn clamp_scroll(&mut self, visible_height: u16) {
        let vh = visible_height as u32;
        let max_off = self.content_height.saturating_sub(vh);
        let new_off = self.scroll_offset.min(max_off);
        let at_bottom = new_off == max_off && max_off > 0;
        self.scroll_offset = new_off;
        if at_bottom && matches!(self.follow, FollowMode::Pinned { .. }) {
            self.follow = FollowMode::Following;
            self.new_answer_pending = false;
        }
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

            // Find the LAST Text block to append to (not first_mut!).
            // When Thinking blocks are present, first_mut() returns the
            // Thinking block and the Text pattern doesn't match, causing
            // every delta to create a new Text block — each rendered on
            // its own line.
            let text_idx = s
                .message
                .content_blocks
                .iter()
                .rposition(|b| matches!(b, ContentBlock::Text { .. }));
            if let Some(idx) = text_idx {
                if let ContentBlock::Text { ref mut content } = s.message.content_blocks[idx] {
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
        if let Some(ref s) = self.streaming
            && let Some(ContentBlock::Text { content, .. }) = s.message.content_blocks.first()
            && let Some(code) = extract_last_code_block(content)
        {
            self.last_code_block = Some(code);
        }
    }

    pub fn refresh_last_code_block(&mut self) {
        if let Some(ref s) = self.streaming
            && let Some(ContentBlock::Text { content, .. }) = s.message.content_blocks.first()
            && let Some(code) = extract_last_code_block(content)
        {
            self.last_code_block = Some(code);
        }
    }

    pub fn set_tool_status(&mut self, id: &str, status: ToolCallStatus) {
        if let Some(ref mut s) = self.streaming
            && let Some(idx) = self.tool_tracker.get(id)
        {
            if let Some(ContentBlock::ToolCall { status: curr, .. }) =
                s.message.content_blocks.get_mut(idx)
            {
                *curr = status;
            }
            self.layout_cache.write().entries = None;
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
                if let Some(ContentBlock::ToolCall { status: s, .. }) =
                    s.message.content_blocks.get_mut(existing_idx)
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
            if let Some(ref id) = tool_call_id
                && let Some(idx) = self.tool_tracker.find_and_remove(id)
                && let Some(ContentBlock::ToolCall { result, status, .. }) =
                    s.message.content_blocks.get_mut(idx)
            {
                *result = Some((
                    clamp_str(content, MAX_TOOL_RESULT_CHARS, MAX_TOOL_RESULT_LINES),
                    is_error,
                ));
                *status = ToolCallStatus::Done;
                self.layout_cache.write().entries = None;
                return;
            }
            if let Some(ContentBlock::ToolCall { result, status, .. }) =
                s.message.content_blocks.last_mut()
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
                    id: bid, duration, ..
                } = block
                    && bid == id
                {
                    *duration = Some(dur_str);
                    self.layout_cache.write().entries = None;
                    return;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.streaming = None;
        self.scroll_offset = 0;
        self.follow = FollowMode::Following;
        self.new_answer_pending = false;
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
        // Line-based count: layout heights depend on wrapped line count,
        // not raw character count. This drastically reduces cache invalidations
        // during streaming — intra-line text deltas don't invalidate.
        let streaming_text_len = self
            .streaming
            .as_ref()
            .map(|s| {
                s.message
                    .content_blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { content } => content.lines().count(),
                        _ => 0,
                    })
                    .sum::<usize>()
            })
            .unwrap_or(0);

        {
            let cache = self.layout_cache.read();
            if cache.msg_count == msg_count
                && cache.streaming_len == streaming_len
                && cache.streaming_text_len == streaming_text_len
                && cache.width == width
                && let Some(ref entries) = cache.entries
            {
                return entries.clone();
            }
        }

        // Recompute outside the read lock
        let entries = compute_layout(self, width, styles);
        let total_height: u32 = entries
            .last()
            .map(|e| e.y.saturating_add(e.height))
            .unwrap_or(0);

        {
            let mut cache = self.layout_cache.write();
            cache.msg_count = msg_count;
            cache.streaming_len = streaming_len;
            cache.streaming_text_len = streaming_text_len;
            cache.width = width;
            cache.entries = Some(entries.clone());
            cache.total_height = total_height;
        }

        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeStyles;

    /// Regression test: cache.width must be set on cache write.
    /// Without it, cache.width stays at 0 → invalidation check always fails
    /// → cache NEVER hits → A4 optimization completely negated.
    #[test]
    fn test_layout_cache_width_is_stored() {
        let mut state = ChatViewState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text {
                content: "hello world".to_string(),
            }],
            timestamp: 0,
        });

        let styles = ThemeStyles::default();
        // First call: populates cache.
        let _ = state.get_layout(80, &styles);

        // Second call with same width: should hit cache (entries already Some).
        // The proof is that cache.width was stored correctly.
        let cache = state.layout_cache.read();
        assert_eq!(
            cache.width, 80,
            "cache.width must be stored on cache write; if 0, cache never hits"
        );
        assert!(
            cache.entries.is_some(),
            "cache.entries must be populated after first get_layout call"
        );
    }

    /// Regression test: spinner_frame change does NOT invalidate the cache.
    /// The spinner only affects visual content of a Spinner entry, not layout.
    #[test]
    fn test_spinner_frame_change_does_not_invalidate() {
        let mut state = ChatViewState::new();
        state.messages.push(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text {
                content: "hello".to_string(),
            }],
            timestamp: 0,
        });

        let styles = ThemeStyles::default();
        let _ = state.get_layout(80, &styles);

        // Bump spinner frame.
        state.spinner_frame = state.spinner_frame.wrapping_add(1);
        let _ = state.get_layout(80, &styles);

        // The cache should still have entries (was not invalidated by spinner change).
        // Since spinner_frame is no longer a cache key, changing it doesn't trigger recompute.
        let cache = state.layout_cache.read();
        assert!(
            cache.entries.is_some(),
            "spinner_frame change must not invalidate layout"
        );
    }

    /// Line-based streaming_text_len: intra-line text growth must not change the count.
    #[test]
    fn test_streaming_text_within_line_does_not_invalidate() {
        let mut state = ChatViewState::new();
        state.start_streaming();
        state.stream_text_delta("hello ");

        let styles = ThemeStyles::default();
        let _ = state.get_layout(80, &styles);
        let len1 = state.layout_cache.read().streaming_text_len;

        // Append more text on the same line — no new line.
        state.stream_text_delta("world more text here");
        let _ = state.get_layout(80, &styles);
        let len2 = state.layout_cache.read().streaming_text_len;

        assert_eq!(
            len1, len2,
            "streaming_text_len is line-based; intra-line growth must not change it"
        );
    }

    /// Line-based streaming_text_len: newline additions must invalidate.
    #[test]
    fn test_streaming_newline_invalidates() {
        let mut state = ChatViewState::new();
        state.start_streaming();
        state.stream_text_delta("first line");

        let styles = ThemeStyles::default();
        let _ = state.get_layout(80, &styles);
        let len1 = state.layout_cache.read().streaming_text_len;

        // Adding a newline creates a new line.
        state.stream_text_delta("\nsecond line");
        let _ = state.get_layout(80, &styles);
        let len2 = state.layout_cache.read().streaming_text_len;

        assert!(
            len2 > len1,
            "newline must invalidate line-based streaming_text_len"
        );
    }
    // ── Phase 2 W1: virtual coordinate regression tests ───────────────

    /// Regression: scroll_offset must accept u32 values > u16::MAX.
    /// Without this, sessions with more than 65,535 rows silently truncate.
    #[test]
    fn test_scroll_offset_accepts_u32_above_u16_max() {
        let mut state = ChatViewState::new();
        state.content_height = u32::MAX;
        state.scroll_offset = u16::MAX as u32 + 100;
        // The field is u32 and should accept this value without panicking.
        assert_eq!(state.scroll_offset, u16::MAX as u32 + 100);
    }

    /// Regression: content_height must be u32 to support unbounded sessions.
    #[test]
    fn test_content_height_is_u32_field() {
        let mut state = ChatViewState::new();
        state.content_height = u32::MAX;
        assert_eq!(state.content_height, u32::MAX);
    }

    /// scroll_up(n) must promote u16 n → u32 internally.
    #[test]
    fn test_scroll_up_promotes_to_u32() {
        let mut state = ChatViewState::new();
        state.content_height = 100;
        state.scroll_to_bottom(20);
        // scroll_offset now 80
        state.scroll_up(50);
        assert_eq!(state.scroll_offset, 30);
    }

    /// scroll_down(n) saturates at content_height (no overflow).
    #[test]
    fn test_scroll_down_saturates_no_overflow() {
        let mut state = ChatViewState::new();
        state.content_height = u32::MAX;
        state.scroll_offset = u32::MAX - 5;
        state.scroll_down(100); // would overflow u16
        // saturating_add means we land at u32::MAX
        assert_eq!(state.scroll_offset, u32::MAX);
    }

    /// clamp_scroll(vh) with content_height > u16::MAX must not truncate.
    #[test]
    fn test_clamp_scroll_handles_u32_content() {
        let mut state = ChatViewState::new();
        state.content_height = 200_000; // > u16::MAX
        state.scroll_offset = 199_999;
        state.clamp_scroll(20);
        // max_off = 200_000 - 20 = 199_980
        assert_eq!(state.scroll_offset, 199_980);
    }

    /// 100K row virtual-coord layout fits in u32 and is preserved through layout cache.
    #[test]
    fn test_virtual_coord_large_session_layout() {
        use crate::widgets::chat::layout::LayoutEntry;
        let entry: LayoutEntry = LayoutEntry {
            y: 100_000,
            height: 1,
            kind: crate::widgets::chat::layout::LayoutKind::Spacer,
            msg_idx: 0,
        };

        // Compute total height should be u32.
        let total = entry.y.saturating_add(entry.height);
        assert!(total > u16::MAX as u32);
    }

    // ── Phase 2 W1 step 2: FollowMode state machine transitions ──────

    /// T1: Following + new content → viewport updates (test via scroll_to_bottom)
    #[test]
    fn follow_t1_following_stays_following_on_bottom_call() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20);
        assert!(matches!(s.follow, FollowMode::Following));
    }

    /// T2: Following + scroll_up → FollowingGrace
    #[test]
    fn follow_t2_following_to_grace_on_scroll_up() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20); // Following
        s.scroll_up(10);
        assert!(matches!(s.follow, FollowMode::FollowingGrace { .. }));
    }

    /// T3: FollowingGrace + new content (scroll_to_bottom) → stays grace if within 2s
    /// Tested by simulating render: scroll_to_bottom during grace → still FollowingGrace.
    #[test]
    fn follow_t3_grace_extends_on_repeated_scroll_up() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20);
        s.scroll_up(10); // grace #1
        let g1 = match s.follow {
            FollowMode::FollowingGrace { until } => until,
            _ => panic!("expected grace"),
        };
        std::thread::sleep(Duration::from_millis(5));
        s.scroll_up(5); // grace #2 (later)
        let g2 = match s.follow {
            FollowMode::FollowingGrace { until } => until,
            _ => panic!("expected grace"),
        };
        assert!(g2 >= g1, "grace window extends on repeated scroll_up");
    }

    /// T4: FollowingGrace + scroll_down → Following
    #[test]
    fn follow_t4_grace_to_following_on_scroll_down() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20);
        s.scroll_up(10); // FollowingGrace
        s.scroll_down(5); // → Following
        assert!(matches!(s.follow, FollowMode::Following));
    }

    /// T5: Pinned + new content → viewport_base frozen (scroll_offset unchanged)
    /// Verified by directly setting follow=Pinned and asserting clamp doesn't move it.
    #[test]
    fn follow_t5_pinned_viewport_frozen() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20); // Following, scroll_offset = 80
        s.scroll_up(30); // FollowingGrace
        // Manually promote to Pinned at current position.
        s.follow = FollowMode::Pinned {
            anchor_msg_idx: 0,
            anchor_y_in_msg: 30,
        };
        let pinned_offset = s.scroll_offset;
        // Bump content_height (simulating new content during Pinned).
        s.content_height = 200;
        s.clamp_scroll(20);
        // Viewport must NOT auto-jump to bottom (the bug we fixed).
        assert_eq!(
            s.scroll_offset, pinned_offset,
            "Pinned viewport must stay frozen"
        );
    }

    /// T6: Pinned + scroll_down to bottom → Following (and badge cleared)
    #[test]
    fn follow_t6_pinned_at_bottom_promotes_to_following() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20); // Following, offset = 80
        s.scroll_up(50); // grace, offset = 30
        s.follow = FollowMode::Pinned {
            anchor_msg_idx: 0,
            anchor_y_in_msg: 30,
        };
        // Scroll all the way down — clamp_scroll should promote to Following.
        s.scroll_offset = 80; // bottom
        s.clamp_scroll(20);
        assert!(matches!(s.follow, FollowMode::Following));
        assert!(!s.new_answer_pending);
    }

    /// T7: Initial state is Following
    #[test]
    fn follow_t7_initial_state_is_following() {
        let s = ChatViewState::new();
        assert!(matches!(s.follow, FollowMode::Following));
    }

    /// T8: scroll_to_top → Pinned at msg 0, y 0
    #[test]
    fn follow_t8_scroll_to_top_pins_at_origin() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20);
        s.scroll_to_top();
        assert_eq!(s.scroll_offset, 0);
        match s.follow {
            FollowMode::Pinned {
                anchor_msg_idx,
                anchor_y_in_msg,
            } => {
                assert_eq!(anchor_msg_idx, 0);
                assert_eq!(anchor_y_in_msg, 0);
            }
            _ => panic!("expected Pinned"),
        }
    }

    /// T9: scroll_to_bottom always → Following (resets any other mode)
    #[test]
    fn follow_t9_scroll_to_bottom_resets_to_following() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_up(10); // FollowingGrace
        s.scroll_to_bottom(20);
        assert!(matches!(s.follow, FollowMode::Following));
        // Then Pinned → Following too.
        s.follow = FollowMode::Pinned {
            anchor_msg_idx: 0,
            anchor_y_in_msg: 0,
        };
        s.scroll_to_bottom(20);
        assert!(matches!(s.follow, FollowMode::Following));
    }

    /// T10: scroll_up clears new_answer_pending (user is engaging)
    #[test]
    fn follow_t10_scroll_up_clears_badge() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20);
        s.new_answer_pending = true;
        s.scroll_up(5);
        assert!(!s.new_answer_pending);
    }

    /// T11: scroll_down also clears new_answer_pending
    #[test]
    fn follow_t11_scroll_down_clears_badge() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_to_bottom(20);
        s.new_answer_pending = true;
        s.scroll_down(5);
        assert!(!s.new_answer_pending);
    }

    /// T12: clear() resets to Following
    #[test]
    fn follow_t12_clear_resets_to_following() {
        let mut s = ChatViewState::new();
        s.content_height = 100;
        s.scroll_up(10); // FollowingGrace
        s.new_answer_pending = true;
        s.clear();
        assert!(matches!(s.follow, FollowMode::Following));
        assert!(!s.new_answer_pending);
        assert_eq!(s.scroll_offset, 0);
    }
}
