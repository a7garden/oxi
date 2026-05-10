//! ChatView widget — scrollable message list with streaming support.
//!
//! All content is unified into a single `Vec<Line>`:
//! - Markdown text → `tui_markdown::from_str()` returns pre-styled `Line`s
//! - Structural elements (separators, role labels, tool boxes) → manually styled `Line`s
//! - Both are pushed in order → correct interleaving → correct scroll

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap},
};
use tui_markdown;
use unicode_width::UnicodeWidthChar;

use crate::Theme;
use crate::theme::ThemeStyles;
use super::markdown;

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
    /// Cached markdown parse: (text_len, simple_hash) → rendered lines.
    cached_md_len: usize,
    cached_md_hash: u64,
    cached_md_lines: Vec<Line<'static>>,
    /// Flag: set when streaming text changed, requiring markdown re-cache.
    md_cache_dirty: bool,
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
        self.cached_md_len = 0;
        self.cached_md_hash = 0;
        self.cached_md_lines.clear();
        self.md_cache_dirty = false;
    }

    /// Alias for streaming text update.
    pub fn stream_text_delta(&mut self, delta: &str) {
        self.append_text(delta);
        self.update_last_code_block(delta);
        self.last_code_block = None;
        self.md_cache_dirty = true;
    }

    /// Alias used by app.rs for the same operation.
    pub fn stream_text(&mut self, text: &str) {
        self.append_text(text);
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

    /// Invalidate the markdown cache — call when text content changes.
    fn invalidate_md_cache(&mut self) {
        self.cached_md_len = 0;
        self.cached_md_hash = 0;
        self.cached_md_lines.clear();
        self.md_cache_dirty = false;
    }

    /// Simple non-cryptographic hash for cache invalidation.
    fn simple_hash(s: &str) -> u64 {
        let mut h: u64 = 0;
        for chunk in s.as_bytes().chunks(64) {
            h = h.wrapping_mul(31).wrapping_add(chunk.iter().fold(0u64, |acc, &b| acc.wrapping_add(b as u64)));
        }
        h
    }

    /// Parse and cache markdown, or return cached result if text hasn't changed.
    /// Only re-parses when `md_cache_dirty` is set AND text has actually changed.
    fn get_or_parse_markdown(&mut self, content: &str) -> &[Line<'static>] {
        let len = content.len();
        let hash = Self::simple_hash(content);

        if !self.md_cache_dirty && len == self.cached_md_len && hash == self.cached_md_hash {
            return &self.cached_md_lines;
        }

        let parsed = markdown_lines_internal(content);
        self.cached_md_len = len;
        self.cached_md_hash = hash;
        self.cached_md_lines = parsed;
        self.md_cache_dirty = false;
        &self.cached_md_lines
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
        tracing::info!("[TUI] stream_tool_call: id={:?}, name={:?}, status={:?}, streaming={}",
            id, name, status, self.streaming.is_some());
        if let Some(ref mut s) = self.streaming {
            // Guard against duplicate IDs (e.g. double ToolCall events from agent)
            if self.active_tool_calls.contains_key(&id) {
                tracing::warn!("[TUI] Duplicate ToolCall ID {:?} — ignoring", id);
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
            tracing::info!("[TUI] ToolCall pushed at idx={}, blocks count={}",
                idx, s.message.content_blocks.len());
        }
    }

    pub fn stream_tool_result(&mut self, tool_call_id: Option<String>, tool_name: String, content: String, is_error: bool) {
        tracing::info!("[TUI] stream_tool_result: tool_call_id={:?}, tool_name={:?}, streaming={}",
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
                            tracing::info!("[TUI] ID-matched result merged into ToolCall at idx={}", idx);
                            return;
                        }
                    }
                } else {
                    tracing::warn!("[TUI] ToolResult for unknown ID {:?} — falling back", id);
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
                        tracing::info!("[TUI] Fallback merge result into last ToolCall");
                        return;
                    }
                }
            }
            // ── Final fallback: push as standalone result ──
            tracing::warn!("[TUI] PUSHING standalone ToolResult");
            s.message.content_blocks.push(ContentBlock::ToolResult { tool_name, content, is_error });
        } else {
            tracing::warn!("[TUI] FALLBACK: streaming is None, ToolResult discarded");
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
        self.cached_md_len = 0;
        self.cached_md_hash = 0;
        self.cached_md_lines.clear();
        self.md_cache_dirty = false;
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
        // Look for ```
        if bytes[i] == b'`' && i + 2 < len && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
            let after = &bytes[i + 3..];
            // "Bare" = next non-whitespace char is \n, \r, or nothing (EOF)
            let is_bare = after.first().map_or(true, |&c| c == b'\n' || c == b'\r' || c == b'\t' || c == b' ');

            if is_bare {
                result.push_str("```text");
                i += 3;
                // Skip any trailing whitespace before the newline
                while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                // Don't consume the newline — fix_bare_code_fences is just a
                // pre-pass; the actual ``` line will be emitted by the parser.
                continue;
            }

            // Has a language token — check if it's a known unsupported pattern
            // that we want to remap (e.g. ```
            let lang_end = after.iter().position(|&c| c == b'\n' || c == b'\r').unwrap_or(after.len());
            let lang = &after[..lang_end];

            // Map unknown / empty-ish tokens to "text"
            let lang_str = String::from_utf8_lossy(lang).trim().to_lowercase();
            let needs_remap = lang_str.is_empty()
                || lang_str == "text"
                || lang_str == "plaintext"
                || lang_str == "plain"
                || lang_str == "none";

            if needs_remap && !lang_str.is_empty() {
                // Replace with ```text but keep the newline if present
                result.push_str("```text");
            }
        }
        result.push(bytes[i] as char);
        i += 1;
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

// ── Block-style tool/error boxes ──────────────────────────────────────

/// Truncate text to fit within `max_width` display columns, adding "…".
fn truncate_to_width(text: &str, max_width: usize) -> (String, String) {
    let mut width = 0;
    let mut result = String::new();
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
        // Reserve 1 for the ellipsis character
        if width + cw > max_width.saturating_sub(1) {
            if !result.is_empty() {
                result.push('…');
                width += 1;
            }
            break;
        }
        result.push(ch);
        width += cw;
    }
    let fill = " ".repeat(max_width.saturating_sub(width));
    (result, fill)
}

/// Compute terminal display width of a string.
fn unicode_display_width(s: &str) -> usize {
    s.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(1)).sum()
}

// ── Block-style tool/error boxes (right-bordereed) ────────────────────

/// Build a header line: ┌─ label ──────┐
fn block_header_line(label: &str, box_width: u16, border_style: Style, label_style: Style) -> Line<'static> {
    let label_display_w = unicode_display_width(label);
    let total_inner = (box_width as usize).saturating_sub(2); // ┌ + ┐
    // layout: ──(2) + space(1) + label + space(1) + ──(remaining)
    let right_dashes = total_inner.saturating_sub(label_display_w + 4);
    vec![
        Span::styled("\u{250c}", border_style),                          // ┌
        Span::styled("\u{2500}".repeat(2), border_style),               // ──
        Span::styled(format!(" {} ", label), label_style),               //  label
        Span::styled("\u{2500}".repeat(right_dashes.max(1)), border_style), // ──
        Span::styled("\u{2510}", border_style),                          // ┐
    ].into()
}

/// Build a body line: │ content... │
fn block_body_line(text: &str, box_width: u16, border_style: Style, body_style: Style) -> Line<'static> {
    let total_inner = (box_width as usize).saturating_sub(2); // │(left) + │(right)
    let content_max = total_inner.saturating_sub(1); // space prefix
    let text_w = unicode_display_width(text);
    if text_w + 1 <= content_max {
        let fill = " ".repeat(content_max.saturating_sub(text_w + 1));
        vec![
            Span::styled("\u{2502}", border_style),                        // │
            Span::styled(format!(" {}{}", text, fill), body_style),         // text + pad
            Span::styled("\u{2502}", border_style),                        // │
        ].into()
    } else {
        // Truncate
        let (truncated, fill) = truncate_to_width(text, content_max.saturating_sub(1));
        vec![
            Span::styled("\u{2502}", border_style),
            Span::styled(format!(" {}{}", truncated, fill), body_style),
            Span::styled("\u{2502}", border_style),
        ].into()
    }
}

/// Build a divider line inside a box: │ ─────── │
fn block_divider_line(box_width: u16, border_style: Style, symbol: &str) -> Line<'static> {
    let total_inner = (box_width as usize).saturating_sub(2); // │ + │
    let dashes = total_inner.saturating_sub(1); // space prefix + dashes
    vec![
        Span::styled("\u{2502}", border_style),                              // │
        Span::styled(format!(" {}", symbol.repeat(dashes.max(1))), border_style), // ──────
        Span::styled("\u{2502}", border_style),                              // │
    ].into()
}

/// Build a truncated body line: │ ... ── │
fn block_truncate_line(box_width: u16, border_style: Style, body_style: Style) -> Line<'static> {
    let total_inner = (box_width as usize).saturating_sub(2); // │ + │
    let content_max = total_inner.saturating_sub(1); // space prefix
    // " ..." + remaining filled with ─
    let dots_w = 4; // " ..."
    let remaining = content_max.saturating_sub(dots_w);
    let dash = " \u{2500}".repeat(remaining.max(0));
    let fill = if remaining > 0 { "" } else { "" };
    let _ = (fill, body_style); // suppress unused
    vec![
        Span::styled("\u{2502}", border_style),
        Span::styled(format!(" ...{}", dash), border_style),
        Span::styled("\u{2502}", border_style),
    ].into()
}

/// Build a footer line: └──────────────┘
fn block_footer_line(box_width: u16, border_style: Style) -> Line<'static> {
    let inner = (box_width as usize).saturating_sub(2);
    vec![
        Span::styled("\u{2514}", border_style),                              // └
        Span::styled("\u{2500}".repeat(inner.max(1)), border_style),         // ──
        Span::styled("\u{2518}", border_style),                              // ┘
    ].into()
}

/// User message left-border stripe: solid bar + tinted space.
fn user_stripe(styles: &ThemeStyles) -> Vec<Span<'static>> {
    vec![
        Span::styled("\u{258c}", styles.user_border),
        Span::styled(" ", styles.user_bg),
    ]
}

/// Render markdown content via tui-markdown, converting to owned Lines.
/// Pre-processes empty/bare code fences to suppress spurious
/// "Could not find syntax for code block: """ warnings from tui_markdown.
///
/// This is the internal parser — callers should use `get_or_parse_markdown`
/// on `ChatViewState` which adds caching and change-detection.
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

/// Style for structural lines based on kind.
fn structural_style(kind: LineKind, styles: &ThemeStyles) -> Style {
    match kind {
        LineKind::Normal => styles.normal,
        LineKind::CodeBlock => markdown::code_block_style(styles.normal),
        LineKind::Heading(lvl) => markdown::heading_style(styles.normal, lvl),
        LineKind::ListItem => styles.normal,
        LineKind::HorizontalRule => styles.muted,
        LineKind::RoleLabel => styles.primary.bold(),
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
            push_blocks(&mut lines, msg.role, &msg.content_blocks, &styles, area.width);
        }

        // ── Streaming message ──
        if let Some(ref streaming) = state.streaming {
            push_blocks(&mut lines, MessageRole::Assistant, &streaming.message.content_blocks, &styles, area.width);
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
        {
            // Wrap at character boundaries so long lines don't bleed past area.width.
            let para = Paragraph::new(lines)
                .block(Block::default().style(styles.normal))
                .scroll((off, 0))
                .wrap(Wrap { trim: false });
            para.render(area, buf);
        }

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
    area_width: u16,
) {
    let box_width = area_width;
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
                // Streaming: use cached markdown lines. Completed messages:
                // still go through get_or_parse_markdown for safety.
                lines.extend(markdown_lines_internal(content));
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
            ContentBlock::ToolCall { id: _, name, arguments, result, status } => {
                let border = styles.muted;
                let body = styles.normal;
                // ── Status-aware header ──
                let (label_prefix, label_style) = match status {
                    ToolCallStatus::Requested => (
                        "⏳",
                        Style::default().fg(styles.muted.fg.unwrap_or(Color::White)),
                    ),
                    ToolCallStatus::Executing => (
                        "⚙",
                        Style::default().fg(styles.warning.fg.unwrap_or(Color::Yellow)),
                    ),
                    ToolCallStatus::Done => (
                        "✓",
                        Style::default().fg(styles.success.fg.unwrap_or(Color::Green)),
                    ),
                };
                let label = format!("{} tool: {}", label_prefix, name);
                let full_label_style = label_style.bold();
                lines.push(block_header_line(&label, box_width, border, full_label_style));
                // Arguments (the call input)
                let max_args = if arguments.lines().count() <= 3 { 5 } else { 3 };
                for l in arguments.lines().take(max_args) {
                    lines.push(block_body_line(l, box_width, border, body));
                }
                if arguments.lines().count() > max_args {
                    lines.push(block_truncate_line(box_width, border, body));
                }
                // Result (if available) — shown inside same box
                if let Some((result_content, is_error)) = result {
                    let (sep, res_border, res_body) = if *is_error {
                        ("─", styles.error, styles.error)
                    } else {
                        ("─", styles.success, styles.normal)
                    };
                    lines.push(block_divider_line(box_width, res_border, sep));
                    for l in result_content.lines().take(6) {
                        lines.push(block_body_line(l, box_width, res_border, res_body));
                    }
                    if result_content.lines().count() > 6 {
                        lines.push(block_truncate_line(box_width, res_border, res_body));
                    }
                }
                lines.push(block_footer_line(box_width, border));
            }
            // ToolResult shown only when NOT merged into ToolCall (standalone result)
            ContentBlock::ToolResult { tool_name, content, is_error } => {
                let (check, border, body) = if *is_error {
                    ("X", styles.error, styles.error)
                } else {
                    ("ok", styles.muted, styles.normal)
                };
                let label = if tool_name.is_empty() { check.to_string() } else { format!("{} {}", check, tool_name) };
                let label_style = if *is_error {
                    Style::default().fg(Color::White).bold()
                } else {
                    Style::default().fg(styles.success.fg.unwrap_or(Color::Green)).bold()
                };
                lines.push(block_header_line(&label, box_width, border, label_style));
                for l in content.lines().take(4) {
                    lines.push(block_body_line(l, box_width, border, body));
                }
                if content.lines().count() > 4 {
                    lines.push(block_truncate_line(box_width, border, body));
                }
                lines.push(block_footer_line(box_width, border));
            }
            ContentBlock::Error { title, message, retryable } => {
                let border = styles.error;
                let body = styles.normal;
                let label = Style::default().fg(Color::White).bold();
                lines.push(block_header_line(&format!("error: {}", title), box_width, border, label));
                for l in message.lines().take(4) {
                    lines.push(block_body_line(l, box_width, border, body));
                }
                if *retryable {
                    lines.push(block_body_line("retry: this error may be temporary", box_width, border, styles.muted));
                }
                lines.push(block_footer_line(box_width, border));
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
