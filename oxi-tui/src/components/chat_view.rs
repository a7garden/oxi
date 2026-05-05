//! Chat view component – displays a scrollable list of chat messages with
//! streaming support, collapsible thinking blocks, and formatted tool calls.
//!
//! # Overview
//!
//! The [`ChatView`] component renders a conversation between the user and an
//! AI assistant.  It supports:
//!
//! - **Streaming partial messages** – append text/thinking deltas in real time.
//! - **Collapsible thinking blocks** – toggle visibility with a click or key.
//! - **Tool call formatting** – rendered as compact cards with name, args, and
//!   result.
//! - **Scroll** – mouse wheel, arrow keys, PageUp/PageDown.
//!
//! The component is self-contained and only depends on oxi-tui primitives
//! (`Cell`, `Surface`, `Rect`, `Theme`, `Component`).

use crate::cell::{Attributes, Cell, Color};
use crate::component::Component;
use crate::event::{Event, KeyCode, MouseEventKind};
use crate::surface::{Rect, Surface};
use crate::terminal::Size;
use crate::Theme;

// ---------------------------------------------------------------------------
// Display types (owned copies, decoupled from oxi-ai message types)
// ---------------------------------------------------------------------------

/// Role of a chat message participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    /// Message sent by the user.
    User,
    /// Message from the AI assistant.
    Assistant,
    /// Result of a tool execution.
    ToolResult,
}

/// A single content block within a chat message.
#[derive(Debug, Clone)]
pub enum ContentBlockDisplay {
    /// Ordinary text / markdown content.
    Text {
        /// The text content.
        content: String,
    },
    /// Thinking / reasoning content (collapsible).
    Thinking {
        /// The thinking text.
        content: String,
        /// Whether the block is collapsed.
        collapsed: bool,
    },
    /// A tool call made by the assistant.
    ToolCall {
        /// Unique call identifier.
        id: String,
        /// Tool name.
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
    },
    /// The result of a tool call.
    ToolResult {
        /// Name of the tool that produced this result.
        tool_name: String,
        /// The result content.
        content: String,
        /// Whether the result is an error.
        is_error: bool,
    },
    /// An error message displayed prominently to the user.
    Error {
        /// Short error title.
        title: String,
        /// Detailed error message.
        message: String,
        /// Whether the user can retry.
        retryable: bool,
    },
}

/// Display representation of a single chat message.
#[derive(Debug, Clone)]
pub struct ChatMessageDisplay {
    /// Role of the message sender.
    pub role: MessageRole,
    /// Ordered list of content blocks.
    pub content_blocks: Vec<ContentBlockDisplay>,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

/// Streaming state for the currently-in-progress assistant message.
#[derive(Debug, Clone)]
pub struct StreamingState {
    /// The partial message being streamed.
    pub message: ChatMessageDisplay,
    /// Content index of the active block being streamed.
    pub active_content_index: usize,
}

// ---------------------------------------------------------------------------
// Chat view component
// ---------------------------------------------------------------------------

/// A scrollable chat log with streaming support.
pub struct ChatView {
    messages: Vec<ChatMessageDisplay>,
    /// Currently streaming partial message (if any).
    streaming: Option<StreamingState>,
    theme: Theme,
    dirty: bool,
    /// Vertical scroll offset (rows from the top of the virtual content).
    scroll_offset: u16,
    /// Cached total content height in rows.
    content_height: u16,
    /// Last rendered area width (to detect resize → reflow).
    last_area_width: u16,
    /// Reflowed lines cache: each message maps to a vec of rendered lines.
    /// We store the styled rows so we only need to recompute on change.
    rendered_lines: Vec<RenderedMessage>,
    /// Index of the focused thinking block (message_index, block_index).
    #[allow(dead_code)]
    focused_thinking: Option<(usize, usize)>,
}

/// Pre-rendered representation of a single message.
#[derive(Debug, Clone)]
struct RenderedMessage {
    #[allow(dead_code)]
    role: MessageRole,
    lines: Vec<RenderedLine>,
}

/// A single line of rendered content (row of styled cells).
#[derive(Debug, Clone)]
struct RenderedLine {
    cells: Vec<StyledCell>,
}

#[derive(Debug, Clone)]
struct StyledCell {
    ch: char,
    fg: Color,
    bg: Color,
    attrs: Attributes,
}

impl StyledCell {
    fn new(ch: char, fg: Color, bg: Color, attrs: Attributes) -> Self {
        Self { ch, fg, bg, attrs }
    }

    fn to_cell(&self) -> Cell {
        Cell {
            char: self.ch,
            fg: self.fg,
            bg: self.bg,
            attrs: self.attrs,
        }
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Horizontal margin for assistant messages.
const ASSISTANT_MARGIN: u16 = 2;
/// Horizontal margin for tool result blocks.
#[allow(dead_code)]
const TOOL_MARGIN: u16 = 4;
/// Max height for collapsed thinking preview (in rows).
#[allow(dead_code)]
const THINKING_COLLAPSED_LINES: usize = 1;
/// Tool result preview max lines.
const TOOL_RESULT_PREVIEW_LINES: usize = 3;
/// Tool result max chars per line.
const TOOL_RESULT_MAX_CHARS: usize = 120;

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl ChatView {
    /// Create a new, empty chat view with the given theme.
    pub fn new(theme: Theme) -> Self {
        Self {
            messages: Vec::new(),
            streaming: None,
            theme,
            dirty: true,
            scroll_offset: 0,
            content_height: 0,
            last_area_width: 0,
            rendered_lines: Vec::new(),
            focused_thinking: None,
        }
    }

    /// Add a completed message to the chat.
    pub fn add_message(&mut self, msg: ChatMessageDisplay) {
        self.messages.push(msg);
        // If we were streaming this message, clear streaming state.
        self.streaming = None;
        self.dirty = true;
    }

    // ----- Streaming API -----

    /// Start streaming a new assistant message.
    pub fn start_streaming(&mut self) {
        self.streaming = Some(StreamingState {
            message: ChatMessageDisplay {
                role: MessageRole::Assistant,
                content_blocks: Vec::new(),
                timestamp: chrono_now_millis(),
            },
            active_content_index: 0,
        });
        self.dirty = true;
    }

    /// Append a text delta to the streaming message.
    pub fn stream_text_delta(&mut self, delta: &str) {
        if let Some(ref mut state) = self.streaming {
            // Find or create the last text block
            if let Some(ContentBlockDisplay::Text { ref mut content }) =
                state.message.content_blocks.last_mut()
            {
                content.push_str(delta);
            } else {
                state
                    .message
                    .content_blocks
                    .push(ContentBlockDisplay::Text {
                        content: delta.to_string(),
                    });
            }
            self.dirty = true;
        }
    }

    /// Start a new thinking block in the streaming message.
    pub fn stream_thinking_start(&mut self) {
        if let Some(ref mut state) = self.streaming {
            state
                .message
                .content_blocks
                .push(ContentBlockDisplay::Thinking {
                    content: String::new(),
                    collapsed: false,
                });
            state.active_content_index = state.message.content_blocks.len() - 1;
            self.dirty = true;
        }
    }

    /// Append a thinking delta to the streaming message.
    pub fn stream_thinking_delta(&mut self, delta: &str) {
        if let Some(ref mut state) = self.streaming {
            if let Some(ContentBlockDisplay::Thinking {
                ref mut content, ..
            }) = state.message.content_blocks.last_mut()
            {
                content.push_str(delta);
                self.dirty = true;
            }
        }
    }

    /// End the thinking block – auto-collapse it.
    pub fn stream_thinking_end(&mut self) {
        if let Some(ref mut state) = self.streaming {
            if let Some(ContentBlockDisplay::Thinking {
                content: _,
                ref mut collapsed,
            }) = state.message.content_blocks.last_mut()
            {
                *collapsed = true;
                self.dirty = true;
            }
        }
    }

    /// Add a tool call to the streaming message.
    pub fn stream_tool_call(&mut self, id: String, name: String, arguments: String) {
        if let Some(ref mut state) = self.streaming {
            state
                .message
                .content_blocks
                .push(ContentBlockDisplay::ToolCall {
                    id,
                    name,
                    arguments,
                });
            self.dirty = true;
        }
    }

    /// Add a tool result to the streaming message.
    pub fn stream_tool_result(&mut self, tool_name: String, content: String, is_error: bool) {
        if let Some(ref mut state) = self.streaming {
            state
                .message
                .content_blocks
                .push(ContentBlockDisplay::ToolResult {
                    tool_name,
                    content,
                    is_error,
                });
            self.dirty = true;
        }
    }

    /// Finish streaming – move the streaming message into the message list.
    pub fn finish_streaming(&mut self) {
        if let Some(state) = self.streaming.take() {
            self.messages.push(state.message);
            self.dirty = true;
        }
    }

    /// Finish streaming with an error.
    pub fn finish_streaming_error(&mut self, error: &str) {
        if let Some(ref mut state) = self.streaming {
            state
                .message
                .content_blocks
                .push(ContentBlockDisplay::Error {
                    title: "Error".into(),
                    message: error.into(),
                    retryable: false,
                });
        }
        self.finish_streaming();
    }

    /// Add a standalone error message to the chat.
    ///
    /// Use this for errors that happen outside of streaming (e.g. a failed
    /// retry, a model fallback failure).
    pub fn add_error(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) {
        self.add_message(ChatMessageDisplay {
            role: MessageRole::Assistant,
            content_blocks: vec![ContentBlockDisplay::Error {
                title: title.into(),
                message: message.into(),
                retryable,
            }],
            timestamp: chrono_now_millis(),
        });
    }

    /// Stream an error block into the current streaming message.
    pub fn stream_error(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) {
        if let Some(ref mut state) = self.streaming {
            state
                .message
                .content_blocks
                .push(ContentBlockDisplay::Error {
                    title: title.into(),
                    message: message.into(),
                    retryable,
                });
            self.dirty = true;
        }
    }

    // ----- Query -----

    /// Number of completed messages.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Is there an active stream?
    pub fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    /// Get the current scroll offset.
    pub fn scroll_offset(&self) -> u16 {
        self.scroll_offset
    }

    /// Scroll to the bottom of the conversation.
    pub fn scroll_to_bottom(&mut self) {
        self.reflow_if_needed(u16::MAX);
        let max_offset = self.content_height.saturating_sub(1);
        if self.scroll_offset != max_offset {
            self.scroll_offset = max_offset;
            self.dirty = true;
        }
    }

    /// Scroll up by `n` rows.
    pub fn scroll_up(&mut self, n: u16) {
        let new_offset = self.scroll_offset.saturating_sub(n);
        if new_offset != self.scroll_offset {
            self.scroll_offset = new_offset;
            self.dirty = true;
        }
    }

    /// Scroll down by `n` rows.
    pub fn scroll_down(&mut self, n: u16) {
        self.reflow_if_needed(u16::MAX);
        let max_offset = self.content_height.saturating_sub(1);
        let new_offset = (self.scroll_offset + n).min(max_offset);
        if new_offset != self.scroll_offset {
            self.scroll_offset = new_offset;
            self.dirty = true;
        }
    }

    /// Toggle the collapsed state of a thinking block.
    pub fn toggle_thinking(&mut self, message_idx: usize, block_idx: usize) {
        if let Some(msg) = self.messages.get_mut(message_idx) {
            if let Some(ContentBlockDisplay::Thinking {
                content: _,
                ref mut collapsed,
            }) = msg.content_blocks.get_mut(block_idx)
            {
                *collapsed = !*collapsed;
                self.dirty = true;
            }
        }
    }

    /// Set the theme.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.dirty = true;
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.streaming = None;
        self.rendered_lines.clear();
        self.content_height = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    // ----- Internal rendering -----

    /// Ensure the rendered_lines cache is up to date.
    ///
    /// Optimisation: when only the streaming message is dirty (the common
    /// case during typing), we re-render **only** the streaming portion
    /// instead of the full message list.  Non-streaming messages are
    /// cached until a structural change (add / remove / toggle) forces a
    /// full reflow.
    fn reflow_if_needed(&mut self, width: u16) {
        let width_changed = width != self.last_area_width;

        if !self.dirty && !width_changed && !self.rendered_lines_needs_update() {
            return;
        }

        if width_changed {
            self.last_area_width = width;
        }

        if width_changed || self.dirty {
            // Full reflow – clear and rebuild everything.
            self.rendered_lines.clear();
            let mut total_height: u16 = 0;

            for (mi, msg) in self.messages.iter().enumerate() {
                let rendered = self.render_message(msg, mi, width);
                total_height = total_height.saturating_add(rendered.lines.len() as u16);
                self.rendered_lines.push(rendered);
            }

            if let Some(ref state) = self.streaming {
                let rendered = self.render_message(&state.message, self.messages.len(), width);
                total_height = total_height.saturating_add(rendered.lines.len() as u16);
                self.rendered_lines.push(rendered);
            }

            self.content_height = total_height.max(1);
            self.dirty = false;
        } else if self.streaming.is_some() {
            // Incremental: only update the last entry (the streaming message).
            // Remove the old streaming entry if it exists.
            if !self.rendered_lines.is_empty() && self.streaming.is_some() {
                // The streaming message is always the last rendered entry.
                self.content_height = self.content_height.saturating_sub(
                    self.rendered_lines
                        .last()
                        .map(|r| r.lines.len() as u16)
                        .unwrap_or(0),
                );
                self.rendered_lines.pop();
            }

            if let Some(ref state) = self.streaming {
                let rendered = self.render_message(&state.message, self.messages.len(), width);
                self.content_height = self
                    .content_height
                    .saturating_add(rendered.lines.len() as u16);
                self.rendered_lines.push(rendered);
            }

            self.content_height = self.content_height.max(1);
        }
    }

    /// Check if rendered lines need update beyond dirty flag.
    fn rendered_lines_needs_update(&self) -> bool {
        // If streaming is active, we always need to reflow.
        self.streaming.is_some()
    }

    /// Render a single message into styled lines.
    fn render_message(
        &self,
        msg: &ChatMessageDisplay,
        message_index: usize,
        width: u16,
    ) -> RenderedMessage {
        let mut lines: Vec<RenderedLine> = Vec::new();
        let max_content_width = (width as usize).saturating_sub(ASSISTANT_MARGIN as usize * 2);
        if max_content_width == 0 {
            return RenderedMessage {
                role: msg.role,
                lines: vec![],
            };
        }

        match msg.role {
            MessageRole::User => {
                // Render user messages with a label
                lines.push(self.make_label_line(
                    " You",
                    self.theme.colors.primary,
                    Color::Default,
                    self.theme.fonts.bold,
                ));
                for block in &msg.content_blocks {
                    self.render_text_block(block, max_content_width, &mut lines, ASSISTANT_MARGIN);
                }
                // Blank separator
                lines.push(empty_line());
            }
            MessageRole::Assistant => {
                // Render assistant messages with a label
                lines.push(self.make_label_line(
                    " Assistant",
                    self.theme.colors.accent,
                    Color::Default,
                    self.theme.fonts.bold,
                ));
                for (bi, block) in msg.content_blocks.iter().enumerate() {
                    self.render_content_block(
                        block,
                        message_index,
                        bi,
                        max_content_width,
                        &mut lines,
                    );
                }
                // Blank separator
                lines.push(empty_line());
            }
            MessageRole::ToolResult => {
                for block in &msg.content_blocks {
                    if let ContentBlockDisplay::ToolResult {
                        tool_name,
                        content,
                        is_error,
                    } = block
                    {
                        self.render_tool_result_block(
                            tool_name,
                            content,
                            *is_error,
                            max_content_width,
                            &mut lines,
                        );
                    }
                }
            }
        }

        RenderedMessage {
            role: msg.role,
            lines,
        }
    }

    fn make_label_line(&self, text: &str, fg: Color, bg: Color, attrs: Attributes) -> RenderedLine {
        let mut line = RenderedLine { cells: Vec::new() };
        // Add left margin
        for _ in 0..ASSISTANT_MARGIN {
            line.cells.push(StyledCell::new(
                ' ',
                Color::Default,
                Color::Default,
                Attributes::new(),
            ));
        }
        for ch in text.chars() {
            line.cells.push(StyledCell::new(ch, fg, bg, attrs));
        }
        line
    }

    /// Render a generic text content block.
    fn render_text_block(
        &self,
        block: &ContentBlockDisplay,
        max_width: usize,
        lines: &mut Vec<RenderedLine>,
        margin: u16,
    ) {
        let text = match block {
            ContentBlockDisplay::Text { content } => content.as_str(),
            _ => return,
        };

        for raw_line in text.lines() {
            if raw_line.is_empty() {
                lines.push(margin_line(margin));
                continue;
            }
            wrap_text(
                raw_line,
                max_width,
                self.theme.colors.foreground,
                Color::Default,
                Attributes::new(),
                margin,
                lines,
            );
        }
        // If text ends without newline, still fine – we already wrapped all lines.
        if text.is_empty() {
            lines.push(margin_line(margin));
        }
    }

    /// Render a content block (any type).
    fn render_content_block(
        &self,
        block: &ContentBlockDisplay,
        message_index: usize,
        block_index: usize,
        max_width: usize,
        lines: &mut Vec<RenderedLine>,
    ) {
        match block {
            ContentBlockDisplay::Text { content: _ } => {
                self.render_text_block(block, max_width, lines, ASSISTANT_MARGIN);
            }
            ContentBlockDisplay::Thinking { content, collapsed } => {
                self.render_thinking_block(
                    content,
                    *collapsed,
                    message_index,
                    block_index,
                    max_width,
                    lines,
                );
            }
            ContentBlockDisplay::ToolCall {
                id: _,
                name,
                arguments,
            } => {
                self.render_tool_call_block(name, arguments, max_width, lines);
            }
            ContentBlockDisplay::ToolResult {
                tool_name,
                content,
                is_error,
            } => {
                self.render_tool_result_block(tool_name, content, *is_error, max_width, lines);
            }
            ContentBlockDisplay::Error {
                title,
                message,
                retryable,
            } => {
                self.render_error_block(title, message, *retryable, max_width, lines);
            }
        }
    }

    /// Render a thinking block with collapsible UI.
    fn render_thinking_block(
        &self,
        content: &str,
        collapsed: bool,
        _message_index: usize,
        _block_index: usize,
        max_width: usize,
        lines: &mut Vec<RenderedLine>,
    ) {
        let thinking_fg = self.theme.colors.muted;
        let thinking_bg = Color::Default;

        // Header line with toggle indicator
        let indicator = if collapsed { "▸" } else { "▾" };
        let header = format!("{} Thinking…", indicator);
        lines.push(self.make_styled_line(
            &header,
            thinking_fg,
            thinking_bg,
            Attributes::new().with_italic(),
            ASSISTANT_MARGIN,
        ));

        if !collapsed {
            // Show full thinking content
            for raw_line in content.lines() {
                if raw_line.is_empty() {
                    lines.push(margin_line(ASSISTANT_MARGIN + 2));
                    continue;
                }
                wrap_text(
                    raw_line,
                    max_width.saturating_sub(2),
                    thinking_fg,
                    thinking_bg,
                    Attributes::new().with_italic(),
                    ASSISTANT_MARGIN + 2,
                    lines,
                );
            }
            if content.is_empty() {
                lines.push(margin_line(ASSISTANT_MARGIN + 2));
            }
        } else {
            // Show a one-line preview
            let preview: String = content
                .lines()
                .next()
                .map(|l| {
                    let max_chars = max_width.saturating_sub(4);
                    if l.chars().count() > max_chars {
                        format!("  {}…", truncate_to_chars(l, max_chars.saturating_sub(1)))
                    } else {
                        format!("  {}", l)
                    }
                })
                .unwrap_or_default();
            lines.push(self.make_styled_line(
                &preview,
                thinking_fg,
                thinking_bg,
                Attributes::new().with_italic(),
                ASSISTANT_MARGIN,
            ));
        }

        // Separator line
        lines.push(margin_line(ASSISTANT_MARGIN));
    }

    /// Render a tool call block.
    fn render_tool_call_block(
        &self,
        name: &str,
        arguments: &str,
        max_width: usize,
        lines: &mut Vec<RenderedLine>,
    ) {
        let tool_fg = self.theme.colors.warning;
        let border_fg = self.theme.colors.border;
        let arg_fg = self.theme.colors.muted;

        // ┌─ tool: name ───────────
        let header = format!("┌─ tool: {} ", name);
        lines.push(self.make_styled_line(
            &header,
            tool_fg,
            Color::Default,
            self.theme.fonts.bold,
            ASSISTANT_MARGIN,
        ));

        // Arguments (possibly multi-line JSON)
        let arg_text = arguments.trim();
        if !arg_text.is_empty() {
            for raw_line in arg_text.lines().take(8) {
                let truncated = if raw_line.chars().count() > max_width.saturating_sub(6) {
                    format!("│ {}", truncate_to_chars(raw_line, max_width.saturating_sub(7)))
                } else {
                    format!("│ {}", raw_line)
                };
                lines.push(self.make_styled_line(
                    &truncated,
                    arg_fg,
                    Color::Default,
                    Attributes::new(),
                    ASSISTANT_MARGIN,
                ));
            }
            let total_lines = arg_text.lines().count();
            if total_lines > 8 {
                lines.push(self.make_styled_line(
                    &format!("│ … ({} more lines)", total_lines - 8),
                    arg_fg,
                    Color::Default,
                    Attributes::new().with_italic(),
                    ASSISTANT_MARGIN,
                ));
            }
        }

        // └────────────────────────
        lines.push(self.make_styled_line(
            "└─",
            border_fg,
            Color::Default,
            Attributes::new(),
            ASSISTANT_MARGIN,
        ));
    }

    /// Render a tool result block.
    fn render_tool_result_block(
        &self,
        tool_name: &str,
        content: &str,
        is_error: bool,
        _max_width: usize,
        lines: &mut Vec<RenderedLine>,
    ) {
        let result_fg = if is_error {
            self.theme.colors.error
        } else {
            self.theme.colors.success
        };
        let content_fg = if is_error {
            self.theme.colors.error
        } else {
            self.theme.colors.muted
        };
        let border_fg = self.theme.colors.border;

        // ┌─ result: tool_name ────
        let header = format!("┌─ result: {} ", tool_name);
        lines.push(self.make_styled_line(
            &header,
            result_fg,
            Color::Default,
            self.theme.fonts.bold,
            ASSISTANT_MARGIN,
        ));

        // Content preview
        let content_lines: Vec<&str> = content.lines().take(TOOL_RESULT_PREVIEW_LINES).collect();
        for raw_line in content_lines {
            let truncated = if raw_line.chars().count() > TOOL_RESULT_MAX_CHARS {
                format!(
                    "│ {}…",
                    truncate_to_chars(raw_line, TOOL_RESULT_MAX_CHARS.saturating_sub(1))
                )
            } else {
                format!("│ {}", raw_line)
            };
            lines.push(self.make_styled_line(
                &truncated,
                content_fg,
                Color::Default,
                Attributes::new(),
                ASSISTANT_MARGIN,
            ));
        }
        let total_lines = content.lines().count();
        if total_lines > TOOL_RESULT_PREVIEW_LINES {
            lines.push(self.make_styled_line(
                &format!(
                    "│ … ({} more lines)",
                    total_lines - TOOL_RESULT_PREVIEW_LINES
                ),
                content_fg,
                Color::Default,
                Attributes::new().with_italic(),
                ASSISTANT_MARGIN,
            ));
        }

        // └────────────────────────
        lines.push(self.make_styled_line(
            "└─",
            border_fg,
            Color::Default,
            Attributes::new(),
            ASSISTANT_MARGIN,
        ));
    }

    /// Render an error block with prominent styling.
    fn render_error_block(
        &self,
        title: &str,
        message: &str,
        retryable: bool,
        max_width: usize,
        lines: &mut Vec<RenderedLine>,
    ) {
        let error_fg = self.theme.colors.error;
        let border_fg = self.theme.colors.error;
        let muted_fg = self.theme.colors.muted;

        // ┌─ ⚠ Error: title ──────
        let header = format!("┌─ ⚠ {}", title);
        lines.push(self.make_styled_line(
            &header,
            error_fg,
            Color::Default,
            self.theme.fonts.bold,
            ASSISTANT_MARGIN,
        ));

        // Error message lines
        for raw_line in message.lines().take(6) {
            let truncated = if raw_line.chars().count() > max_width.saturating_sub(4) {
                format!("│ {}…", truncate_to_chars(raw_line, max_width.saturating_sub(5)))
            } else {
                format!("│ {}", raw_line)
            };
            lines.push(self.make_styled_line(
                &truncated,
                muted_fg,
                Color::Default,
                Attributes::new(),
                ASSISTANT_MARGIN,
            ));
        }
        let total_lines = message.lines().count();
        if total_lines > 6 {
            lines.push(self.make_styled_line(
                &format!("│ … ({} more lines)", total_lines - 6),
                muted_fg,
                Color::Default,
                Attributes::new().with_italic(),
                ASSISTANT_MARGIN,
            ));
        }

        // Retry hint if applicable
        if retryable {
            lines.push(self.make_styled_line(
                "│ ↻ This error may be temporary – you can retry.",
                muted_fg,
                Color::Default,
                Attributes::new().with_italic(),
                ASSISTANT_MARGIN,
            ));
        }

        // └────────────────────────
        lines.push(self.make_styled_line(
            "└─",
            border_fg,
            Color::Default,
            Attributes::new(),
            ASSISTANT_MARGIN,
        ));
    }
    fn make_styled_line(
        &self,
        text: &str,
        fg: Color,
        bg: Color,
        attrs: Attributes,
        margin: u16,
    ) -> RenderedLine {
        let mut line = RenderedLine { cells: Vec::new() };
        for _ in 0..margin {
            line.cells.push(StyledCell::new(
                ' ',
                Color::Default,
                Color::Default,
                Attributes::new(),
            ));
        }
        for ch in text.chars() {
            line.cells.push(StyledCell::new(ch, fg, bg, attrs));
        }
        line
    }

    /// Paint rendered lines onto the surface.
    fn paint(&mut self, surface: &mut Surface, area: Rect) {
        // Clear the area
        for row in area.y..area.bottom() {
            for col in area.x..area.right() {
                surface.set(row, col, Cell::default());
            }
        }

        // Collect all rendered lines into a flat list
        let mut all_lines: Vec<&RenderedLine> = Vec::new();
        for rm in &self.rendered_lines {
            for line in &rm.lines {
                all_lines.push(line);
            }
        }

        let visible_height = area.height as usize;
        let scroll = self.scroll_offset as usize;

        for (vi, rendered_line) in all_lines
            .iter()
            .skip(scroll)
            .take(visible_height)
            .enumerate()
        {
            let row = area.y + vi as u16;
            for (ci, sc) in rendered_line.cells.iter().enumerate() {
                let col = area.x + ci as u16;
                if col < area.x + area.width {
                    surface.set(row, col, sc.to_cell());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Word-wrap a text line into styled rendered lines with left margin.
fn wrap_text(
    text: &str,
    max_width: usize,
    fg: Color,
    bg: Color,
    attrs: Attributes,
    margin: u16,
    lines: &mut Vec<RenderedLine>,
) {
    if max_width == 0 {
        lines.push(margin_line(margin));
        return;
    }

    let mut current_line = margin_prefix(margin);
    let mut current_len: usize = 0;

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if current_len == 0 {
            // First word on line
            if word_len <= max_width {
                for ch in word.chars() {
                    current_line.push(StyledCell::new(ch, fg, bg, attrs));
                }
                current_len = word_len;
            } else {
                // Word is longer than max_width – hard-wrap
                for ch in word.chars() {
                    if current_len >= max_width {
                        lines.push(RenderedLine {
                            cells: std::mem::take(&mut current_line),
                        });
                        current_line = margin_prefix(margin);
                        current_len = 0;
                    }
                    current_line.push(StyledCell::new(ch, fg, bg, attrs));
                    current_len += 1;
                }
            }
        } else if current_len + 1 + word_len <= max_width {
            // Word fits on current line
            current_line.push(StyledCell::new(' ', fg, bg, attrs));
            for ch in word.chars() {
                current_line.push(StyledCell::new(ch, fg, bg, attrs));
            }
            current_len += 1 + word_len;
        } else {
            // Wrap to next line
            lines.push(RenderedLine {
                cells: std::mem::take(&mut current_line),
            });
            current_line = margin_prefix(margin);
            current_len = 0;
            if word_len <= max_width {
                for ch in word.chars() {
                    current_line.push(StyledCell::new(ch, fg, bg, attrs));
                }
                current_len = word_len;
            } else {
                for ch in word.chars() {
                    if current_len >= max_width {
                        lines.push(RenderedLine {
                            cells: std::mem::take(&mut current_line),
                        });
                        current_line = margin_prefix(margin);
                        current_len = 0;
                    }
                    current_line.push(StyledCell::new(ch, fg, bg, attrs));
                    current_len += 1;
                }
            }
        }
    }

    if current_len > 0 || text.is_empty() {
        lines.push(RenderedLine {
            cells: current_line,
        });
    }
}

fn margin_prefix(margin: u16) -> Vec<StyledCell> {
    let mut cells = Vec::with_capacity(margin as usize);
    for _ in 0..margin {
        cells.push(StyledCell::new(
            ' ',
            Color::Default,
            Color::Default,
            Attributes::new(),
        ));
    }
    cells
}

fn margin_line(margin: u16) -> RenderedLine {
    RenderedLine {
        cells: margin_prefix(margin),
    }
}

fn empty_line() -> RenderedLine {
    RenderedLine { cells: Vec::new() }
}

/// Safely truncate a string to at most `max_chars` Unicode characters,
/// returning a slice guaranteed to end on a char boundary.
fn truncate_to_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Simple timestamp millisecond helper (avoids pulling in chrono in tests).
fn chrono_now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ---------------------------------------------------------------------------
// Component trait impl
// ---------------------------------------------------------------------------

impl Component for ChatView {
    fn name(&self) -> &str {
        "ChatView"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.streaming.is_some()
    }

    fn clear_dirty(&mut self) {
        // Don't clear dirty if streaming – we always need to re-render.
        if self.streaming.is_none() {
            self.dirty = false;
        }
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => match key.code {
                KeyCode::Up => {
                    self.scroll_up(1);
                    true
                }
                KeyCode::Down => {
                    self.scroll_down(1);
                    true
                }
                KeyCode::PageUp => {
                    self.scroll_up(10);
                    true
                }
                KeyCode::PageDown => {
                    self.scroll_down(10);
                    true
                }
                KeyCode::Home => {
                    self.scroll_offset = 0;
                    self.dirty = true;
                    true
                }
                KeyCode::End => {
                    self.scroll_to_bottom();
                    true
                }
                _ => false,
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_up(3);
                    true
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_down(3);
                    true
                }
                _ => false,
            },
            Event::Resize(_) => {
                self.dirty = true;
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        if !area.is_valid() || area.width < 4 || area.height < 1 {
            return;
        }

        // Auto-scroll to bottom if we were already at the bottom before reflow.
        let was_at_bottom = self.scroll_offset >= self.content_height.saturating_sub(2);

        self.reflow_if_needed(area.width);

        // If content fits within viewport or we were at the bottom, pin to bottom.
        let max_offset = self
            .content_height
            .saturating_sub(area.height)
            .saturating_sub(1);
        if was_at_bottom || self.content_height <= area.height {
            self.scroll_offset = max_offset;
        }

        // Clamp scroll offset
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }

        self.paint(surface, area);
    }

    fn min_size(&self) -> Size {
        Size {
            width: 20,
            height: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> Theme {
        Theme::dark()
    }

    #[test]
    fn chat_view_starts_empty() {
        let cv = ChatView::new(test_theme());
        assert_eq!(cv.message_count(), 0);
        assert!(!cv.is_streaming());
    }

    #[test]
    fn add_and_count_messages() {
        let mut cv = ChatView::new(test_theme());
        cv.add_message(ChatMessageDisplay {
            role: MessageRole::User,
            content_blocks: vec![ContentBlockDisplay::Text {
                content: "Hello".into(),
            }],
            timestamp: 0,
        });
        cv.add_message(ChatMessageDisplay {
            role: MessageRole::Assistant,
            content_blocks: vec![ContentBlockDisplay::Text {
                content: "Hi there!".into(),
            }],
            timestamp: 1,
        });
        assert_eq!(cv.message_count(), 2);
    }

    #[test]
    fn streaming_lifecycle() {
        let mut cv = ChatView::new(test_theme());

        // Start streaming
        cv.start_streaming();
        assert!(cv.is_streaming());

        // Append text
        cv.stream_text_delta("Hello");
        cv.stream_text_delta(" world");

        // Finish
        cv.finish_streaming();
        assert!(!cv.is_streaming());
        assert_eq!(cv.message_count(), 1);
    }

    #[test]
    fn streaming_thinking_block() {
        let mut cv = ChatView::new(test_theme());

        cv.start_streaming();
        cv.stream_text_delta("Let me think...");

        cv.stream_thinking_start();
        cv.stream_thinking_delta("I should consider...");
        cv.stream_thinking_delta(" multiple options.");
        cv.stream_thinking_end(); // auto-collapses

        cv.stream_text_delta(" Here's my answer.");
        cv.finish_streaming();

        assert_eq!(cv.message_count(), 1);
        // Should have: Text, Thinking(collapsed), Text
        let msg = &cv.messages[0];
        assert_eq!(msg.content_blocks.len(), 3);

        // Thinking block should be collapsed
        if let ContentBlockDisplay::Thinking { collapsed, .. } = &msg.content_blocks[1] {
            assert!(collapsed);
        } else {
            panic!("Expected Thinking block at index 1");
        }
    }

    #[test]
    fn streaming_tool_call_and_result() {
        let mut cv = ChatView::new(test_theme());

        cv.start_streaming();
        cv.stream_tool_call(
            "call_123".into(),
            "read_file".into(),
            r#"{"path": "/tmp/test.txt"}"#.into(),
        );
        cv.stream_tool_result("read_file".into(), "File contents here".into(), false);
        cv.stream_text_delta("I read the file.");
        cv.finish_streaming();

        assert_eq!(cv.message_count(), 1);
        let msg = &cv.messages[0];
        assert_eq!(msg.content_blocks.len(), 3); // ToolCall, ToolResult, Text
    }

    #[test]
    fn toggle_thinking_block() {
        let mut cv = ChatView::new(test_theme());
        cv.add_message(ChatMessageDisplay {
            role: MessageRole::Assistant,
            content_blocks: vec![
                ContentBlockDisplay::Text {
                    content: "Before".into(),
                },
                ContentBlockDisplay::Thinking {
                    content: "Deep thoughts".into(),
                    collapsed: true,
                },
                ContentBlockDisplay::Text {
                    content: "After".into(),
                },
            ],
            timestamp: 0,
        });

        // Toggle thinking block
        cv.toggle_thinking(0, 1);
        if let ContentBlockDisplay::Thinking { collapsed, .. } = &cv.messages[0].content_blocks[1] {
            assert!(!collapsed);
        }

        // Toggle again
        cv.toggle_thinking(0, 1);
        if let ContentBlockDisplay::Thinking { collapsed, .. } = &cv.messages[0].content_blocks[1] {
            assert!(collapsed);
        }
    }

    #[test]
    fn scroll_clamps_to_content() {
        let mut cv = ChatView::new(test_theme());
        cv.add_message(ChatMessageDisplay {
            role: MessageRole::User,
            content_blocks: vec![ContentBlockDisplay::Text {
                content: "Hello".into(),
            }],
            timestamp: 0,
        });

        // Can't scroll beyond content
        cv.scroll_down(1000);
        assert!(cv.scroll_offset() < 1000);
    }

    #[test]
    fn clear_empties_everything() {
        let mut cv = ChatView::new(test_theme());
        cv.add_message(ChatMessageDisplay {
            role: MessageRole::User,
            content_blocks: vec![ContentBlockDisplay::Text {
                content: "Hi".into(),
            }],
            timestamp: 0,
        });
        cv.start_streaming();
        cv.clear();
        assert_eq!(cv.message_count(), 0);
        assert!(!cv.is_streaming());
    }

    #[test]
    fn render_produces_cells() {
        let mut cv = ChatView::new(test_theme());
        cv.add_message(ChatMessageDisplay {
            role: MessageRole::User,
            content_blocks: vec![ContentBlockDisplay::Text {
                content: "Hello world".into(),
            }],
            timestamp: 0,
        });

        let mut surface = Surface::new(40, 10);
        cv.render(&mut surface, Rect::new(0, 0, 40, 10));

        // Check that something was rendered (not all default cells)
        let has_content = (0..10).any(|row| {
            (0..40).any(|col| {
                surface
                    .get(row, col)
                    .map(|c| c.char != ' ')
                    .unwrap_or(false)
            })
        });
        assert!(has_content, "Expected some non-space cells to be rendered");
    }

    #[test]
    fn streaming_error_finishes_message() {
        let mut cv = ChatView::new(test_theme());
        cv.start_streaming();
        cv.stream_text_delta("Partial...");
        cv.finish_streaming_error("Connection lost");

        assert!(!cv.is_streaming());
        assert_eq!(cv.message_count(), 1);

        // Last block should contain an error block
        let msg = &cv.messages[0];
        let last_block = msg.content_blocks.last().unwrap();
        if let ContentBlockDisplay::Error {
            title,
            message,
            retryable: _,
        } = last_block
        {
            assert_eq!(title, "Error");
            assert_eq!(message, "Connection lost");
        } else {
            panic!("Expected Error block as last block, got {:?}", last_block);
        }
    }

    #[test]
    fn word_wrap_long_line() {
        let mut lines: Vec<RenderedLine> = Vec::new();
        wrap_text(
            "This is a reasonably long line that should wrap at the width boundary",
            20,
            Color::Default,
            Color::Default,
            Attributes::new(),
            0,
            &mut lines,
        );
        assert!(lines.len() > 1, "Long line should wrap into multiple lines");
        // Each line should be at most 20 visible chars (excluding margin)
        for line in &lines {
            assert!(
                line.cells.len() <= 22, // 20 + some tolerance
                "Wrapped line should respect max width"
            );
        }
    }

    #[test]
    fn render_thinking_block_collapsed_and_expanded() {
        let cv = ChatView::new(test_theme());
        let mut lines_collapsed: Vec<RenderedLine> = Vec::new();
        cv.render_thinking_block(
            "Some deep thinking content here",
            true, // collapsed
            0,
            0,
            80,
            &mut lines_collapsed,
        );
        // Collapsed: header + 1 preview + separator = 3 lines
        assert_eq!(lines_collapsed.len(), 3);

        let mut lines_expanded: Vec<RenderedLine> = Vec::new();
        cv.render_thinking_block(
            "Some deep thinking content here",
            false, // expanded
            0,
            0,
            80,
            &mut lines_expanded,
        );
        // Expanded: header + content + separator = 3 lines (single line content)
        assert!(lines_expanded.len() >= 3);
    }

    #[test]
    fn add_error_message() {
        let mut cv = ChatView::new(test_theme());
        cv.add_error("Rate Limited", "Too many requests. Please wait.", true);
        assert_eq!(cv.message_count(), 1);
        let msg = &cv.messages[0];
        assert_eq!(msg.content_blocks.len(), 1);
        if let ContentBlockDisplay::Error {
            title,
            message,
            retryable,
        } = &msg.content_blocks[0]
        {
            assert_eq!(title, "Rate Limited");
            assert_eq!(message, "Too many requests. Please wait.");
            assert!(retryable);
        } else {
            panic!("Expected Error block");
        }
    }

    #[test]
    fn render_error_block_with_retry_hint() {
        let cv = ChatView::new(test_theme());
        let mut lines: Vec<RenderedLine> = Vec::new();
        cv.render_error_block(
            "Connection Error",
            "Failed to connect to provider",
            true, // retryable
            80,
            &mut lines,
        );
        // Header + message + retry hint + separator
        assert!(lines.len() >= 3);
        // Check that the retry hint is present
        let has_retry_hint = lines.iter().any(|l| l.cells.iter().any(|c| c.ch == '↻'));
        assert!(
            has_retry_hint,
            "Expected retry hint in rendered error block"
        );
    }

    #[test]
    fn render_error_block_without_retry() {
        let cv = ChatView::new(test_theme());
        let mut lines: Vec<RenderedLine> = Vec::new();
        cv.render_error_block(
            "Fatal",
            "Something went wrong",
            false, // not retryable
            80,
            &mut lines,
        );
        // Header + message + separator
        assert!(lines.len() >= 2);
        let has_retry_hint = lines.iter().any(|l| l.cells.iter().any(|c| c.ch == '↻'));
        assert!(
            !has_retry_hint,
            "Should not have retry hint for non-retryable error"
        );
    }

    #[test]
    fn stream_error_into_streaming_message() {
        let mut cv = ChatView::new(test_theme());
        cv.start_streaming();
        cv.stream_text_delta("Thinking...");
        cv.stream_error("Timeout", "Request timed out", true);

        // Should still be streaming
        assert!(cv.is_streaming());

        cv.finish_streaming();
        assert_eq!(cv.message_count(), 1);
        // Should have Text + Error
        assert_eq!(cv.messages[0].content_blocks.len(), 2);
    }
}
