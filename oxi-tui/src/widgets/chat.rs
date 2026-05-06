//! ChatView widget — scrollable message list with streaming support.

use ratatui::{
    widgets::StatefulWidget,
    buffer::Buffer,
    layout::Rect,
};
use crate::Theme;

/// ChatView message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    /// Message from the user.
    User,
    /// Message from the assistant.
    Assistant,
    /// System message.
    System,
}

/// A single content block within a message.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    /// Ordinary text / markdown.
    Text {
        /// The text content.
        content: String,
    },
    /// Collapsible thinking / reasoning block.
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
        /// Name of the tool.
        tool_name: String,
        /// Result content.
        content: String,
        /// Whether the result is an error.
        is_error: bool,
    },
    /// An error message.
    Error {
        /// Error title.
        title: String,
        /// Detailed message.
        message: String,
        /// Whether the user can retry.
        retryable: bool,
    },
}

/// Display representation of a chat message.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Role of the sender.
    pub role: MessageRole,
    /// Ordered list of content blocks.
    pub content_blocks: Vec<ContentBlock>,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

/// Streaming state for the in-progress assistant message.
#[derive(Debug, Clone)]
pub struct StreamingState {
    /// The partial message being streamed.
    pub message: ChatMessage,
    /// Content index of the active block.
    pub active_content_index: usize,
}

/// State for the ChatView widget.
#[derive(Debug, Default)]
pub struct ChatViewState {
    /// Completed messages.
    pub messages: Vec<ChatMessage>,
    /// Currently streaming partial message.
    pub streaming: Option<StreamingState>,
    /// Vertical scroll offset.
    pub scroll_offset: u16,
    /// Content height in rows.
    content_height: u16,
}

impl ChatViewState {
    /// Add a completed message.
    pub fn add_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        self.streaming = None;
    }

    /// Start streaming a new assistant message.
    pub fn start_streaming(&mut self) {
        self.streaming = Some(StreamingState {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content_blocks: Vec::new(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            },
            active_content_index: 0,
        });
    }

    /// Append text delta to streaming message.
    pub fn stream_text_delta(&mut self, delta: &str) {
        if let Some(ref mut state) = self.streaming {
            if let Some(ContentBlock::Text { ref mut content }) = state.message.content_blocks.last_mut() {
                content.push_str(delta);
            } else {
                state.message.content_blocks.push(ContentBlock::Text {
                    content: delta.to_string(),
                });
            }
        }
    }

    /// Append a tool call content block to the streaming message.
    pub fn stream_tool_call(&mut self, id: String, name: String, arguments: String) {
        if let Some(ref mut state) = self.streaming {
            state.message.content_blocks.push(ContentBlock::ToolCall {
                id, name, arguments,
            });
        }
    }

    /// Append a tool result content block to the streaming message.
    pub fn stream_tool_result(&mut self, tool_name: String, content: String, is_error: bool) {
        if let Some(ref mut state) = self.streaming {
            state.message.content_blocks.push(ContentBlock::ToolResult {
                tool_name, content, is_error,
            });
        }
    }

    /// Append an error content block to the streaming message.
    pub fn stream_error(&mut self, title: String, message: String, retryable: bool) {
        if let Some(ref mut state) = self.streaming {
            state.message.content_blocks.push(ContentBlock::Error {
                title, message, retryable,
            });
        }
    }

    /// Finish streaming — move to messages.
    pub fn finish_streaming(&mut self) {
        if let Some(state) = self.streaming.take() {
            self.messages.push(state.message);
        }
    }

    /// Scroll to bottom (set offset to max).
    pub fn scroll_to_bottom(&mut self, visible_height: u16) {
        let max_scroll = self.content_height.saturating_sub(visible_height);
        self.scroll_offset = max_scroll;
    }

    /// Scroll up.
    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down.
    pub fn scroll_down(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.streaming = None;
        self.scroll_offset = 0;
        self.content_height = 0;
    }

    /// Get message count.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Check if streaming.
    pub fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }
}

/// ChatView widget.
pub struct ChatView<'a> {
    theme: &'a Theme,
    scrollbar: bool,
}

impl<'a> ChatView<'a> {
    /// Create a new ChatView with the given theme.
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme, scrollbar: true }
    }

    /// Toggle scrollbar visibility.
    pub fn with_scrollbar(mut self, show: bool) -> Self {
        self.scrollbar = show;
        self
    }
}

impl StatefulWidget for ChatView<'_> {
    type State = ChatViewState;

    fn render(
        self,
        area: Rect,
        buf: &mut Buffer,
        state: &mut Self::State,
    ) {
        if area.width < 4 || area.height < 1 {
            return;
        }

        // Base styles from theme
        let styles = self.theme.to_styles();

        // Fill background
        for row in area.y..area.y + area.height {
            for col in area.x..area.x + area.width {
                buf[(col, row)].set_char(' ')
                    .set_style(styles.normal);
            }
        }

        // Collect all lines
        let mut all_lines: Vec<(MessageRole, String, bool)> = Vec::new();
        // (role, text, is_timestamp_row)

        for msg in &state.messages {
            for block in &msg.content_blocks {
                match block {
                    ContentBlock::Text { content } => {
                        for line in content.lines() {
                            all_lines.push((msg.role, line.to_string(), false));
                        }
                    }
                    ContentBlock::Thinking { content, collapsed } => {
                        let indicator = if *collapsed { "▸" } else { "▾" };
                        all_lines.push((msg.role, format!("{} Thinking…", indicator), false));
                        if !*collapsed {
                            for line in content.lines() {
                                all_lines.push((msg.role, format!("  {}", line), false));
                            }
                        } else if let Some(first) = content.lines().next() {
                            all_lines.push((msg.role, format!("  {}", first), false));
                        }
                    }
                    ContentBlock::ToolCall { name, arguments, .. } => {
                        all_lines.push((msg.role, format!("┌─ tool: {} ───", name), false));
                        for line in arguments.lines().take(8) {
                            all_lines.push((msg.role, format!("│ {}", line), false));
                        }
                        all_lines.push((msg.role, "└─".to_string(), false));
                    }
                    ContentBlock::ToolResult { tool_name, content, is_error } => {
                        let prefix = if *is_error { "✗" } else { "✓" };
                        all_lines.push((msg.role, format!("┌─ {}: {} ───", prefix, tool_name), false));
                        for line in content.lines().take(3) {
                            all_lines.push((msg.role, format!("│ {}", line), false));
                        }
                        all_lines.push((msg.role, "└─".to_string(), false));
                    }
                    ContentBlock::Error { title, message, retryable } => {
                        all_lines.push((msg.role, format!("┌─ ⚠ {} ───", title), false));
                        for line in message.lines().take(6) {
                            all_lines.push((msg.role, format!("│ {}", line), false));
                        }
                        if *retryable {
                            all_lines.push((msg.role, "│ ↻ This error may be temporary".to_string(), false));
                        }
                        all_lines.push((msg.role, "└─".to_string(), false));
                    }
                }
            }
            // Blank separator after each message
            all_lines.push((msg.role, String::new(), false));
        }

        // Add streaming message
        if let Some(ref streaming) = state.streaming {
            for block in &streaming.message.content_blocks {
                match block {
                    ContentBlock::Text { content } => {
                        for line in content.lines() {
                            all_lines.push((MessageRole::Assistant, line.to_string(), false));
                        }
                    }
                    _ => {}
                }
            }
            // Streaming indicator
            all_lines.push((MessageRole::Assistant, "  ⠋ thinking…".to_string(), false));
        }

        // Compute content height
        state.content_height = all_lines.len() as u16;

        // Calculate visible range
        let visible_height = area.height as usize;
        let max_scroll = state.content_height.saturating_sub(visible_height as u16);
        let clamped_offset = state.scroll_offset.min(max_scroll);

        // Render visible lines
        let start = clamped_offset as usize;
        let _end = (start + visible_height).min(all_lines.len());

        for (vi, line_tuple) in all_lines.iter().skip(start).take(visible_height).enumerate() {
            let (role, text, _timestamp) = line_tuple;
            let row = area.y + vi as u16;

            let prefix_char = match role {
                MessageRole::User => " ▌",
                MessageRole::Assistant => " ▐",
                MessageRole::System => " · ",
            };
            let prefix_style = match role {
                MessageRole::User => styles.primary,
                MessageRole::Assistant => styles.accent,
                MessageRole::System => styles.muted,
            };

            // Write prefix
            let mut col = area.x;
            for c in prefix_char.chars() {
                buf[(col, row)].set_char(c).set_style(prefix_style);
                col += 1;
            }

            // Write text
            let margin = prefix_char.chars().count() as u16;
            let max_text = (area.width.saturating_sub(margin)) as usize;
            for (i, c) in text.chars().take(max_text).enumerate() {
                let cell_col = area.x + margin + i as u16;
                if cell_col < area.x + area.width {
                    let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) as u16;
                    buf[(cell_col, row)].set_char(c).set_style(styles.normal);
                    // Mark continuation cells for wide chars
                    if char_width > 1 {
                        for w in 1..char_width {
                            let cont_col = cell_col + w;
                            if cont_col < area.x + area.width {
                                buf[(cont_col, row)].set_char('\u{0}').set_style(styles.normal);
                            }
                        }
                    }
                }
            }

            // Clear remainder of row
            let text_end = area.x + margin + text.chars().count() as u16;
            for cl in text_end..area.x + area.width {
                buf[(cl, row)].set_char(' ').set_style(styles.normal);
            }
        }

        // Scrollbar (simple right-edge indicator)
        if self.scrollbar && max_scroll > 0 {
            let thumb_pos = (clamped_offset as f32 / max_scroll as f32 * visible_height as f32) as u16;
            let thumb_size = ((visible_height as f32 * visible_height as f32)
                / (state.content_height as f32))
                .max(1.0) as u16;

            for i in 0..thumb_size.min(visible_height as u16) {
                let sb_row = area.y + thumb_pos.saturating_add(i).min(area.y + area.height - 1);
                buf[(area.x + area.width - 1, sb_row)].set_char('█')
                    .set_style(styles.muted);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_state_empty() {
        let state = ChatViewState::default();
        assert_eq!(state.message_count(), 0);
        assert!(!state.is_streaming());
    }

    #[test]
    fn chat_state_add_message() {
        let mut state = ChatViewState::default();
        state.add_message(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![ContentBlock::Text { content: "Hello".into() }],
            timestamp: 0,
        });
        assert_eq!(state.message_count(), 1);
    }

    #[test]
    fn chat_state_streaming() {
        let mut state = ChatViewState::default();
        state.start_streaming();
        assert!(state.is_streaming());
        state.stream_text_delta("Hello ");
        state.stream_text_delta("world");
        state.finish_streaming();
        assert!(!state.is_streaming());
        assert_eq!(state.message_count(), 1);
    }

    #[test]
    fn chat_state_scroll() {
        let mut state = ChatViewState::default();
        state.scroll_offset = 5;
        state.scroll_up(3);
        assert_eq!(state.scroll_offset, 2);
        state.scroll_down(10);
        // Should not exceed content_height
    }

    #[test]
    fn chat_state_clear() {
        let mut state = ChatViewState::default();
        state.add_message(ChatMessage {
            role: MessageRole::User,
            content_blocks: vec![],
            timestamp: 0,
        });
        state.clear();
        assert_eq!(state.message_count(), 0);
        assert!(!state.is_streaming());
    }
}