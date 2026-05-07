//! ChatView widget — scrollable message list with streaming support.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, StatefulWidget, Widget},
};
use crate::Theme;
use super::markdown;

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
    /// An image content block (base64-encoded).
    Image {
        /// MIME type (e.g. "image/png").
        mime_type: String,
        /// Base64-encoded image data.
        base64_data: String,
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

// ---------------------------------------------------------------------------
// Per-line markdown metadata
// ---------------------------------------------------------------------------

/// Kind of a collected line, used to pick the right render style.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LineKind {
    /// Normal text — inline markdown will be parsed at render time.
    Normal,
    /// Inside a fenced code block.
    CodeBlock,
    /// ATX heading (stores level 1–6).
    Heading(u8),
    /// List item (extra indent already applied).
    ListItem,
    /// Horizontal rule.
    HorizontalRule,
    /// Role label (e.g. "You") rendered bold with primary color.
    RoleLabel,
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
    /// 마지막 코드 블록 내용 (Ctrl+Y 복사용).
    pub last_code_block: Option<String>,
    /// Internal: currently inside a ``` fence (for streaming tracking).
    code_block_active: bool,
    /// Internal: buffer accumulating code block content during streaming.
    code_block_buf: String,
    /// Pending images collected from messages, newest last.
    /// Each tuple is (base64_data, mime_type).
    pub pending_images: Vec<(String, String)>,
    /// Spinner animation frame index.
    pub spinner_frame: usize,
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
    /// Also tracks the last code block for Ctrl+Y copy.
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
        // Track code blocks from the streamed text
        self.update_last_code_block(delta);
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

    /// Append an image content block to the streaming message.
    pub fn stream_image(&mut self, mime_type: String, base64_data: String) {
        self.pending_images.push((base64_data.clone(), mime_type.clone()));
        if let Some(ref mut state) = self.streaming {
            state.message.content_blocks.push(ContentBlock::Image {
                mime_type,
                base64_data,
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
        self.pending_images.clear();
        self.last_code_block = None;
        self.code_block_active = false;
        self.code_block_buf.clear();
    }

    /// Get message count.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Check if streaming.
    pub fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    /// Update `last_code_block` by scanning the delta for ``` fences.
    /// Tracks partial fences across multiple deltas.
    fn update_last_code_block(&mut self, delta: &str) {
        let mut pos = 0;
        while let Some(idx) = delta[pos..].find("```") {
            let abs_idx = pos + idx;
            if self.code_block_active {
                // Closing fence — everything before is code
                let before = &delta[pos..abs_idx];
                self.code_block_buf.push_str(before);
                let content = self.code_block_buf.trim().to_string();
                if !content.is_empty() {
                    self.last_code_block = Some(content);
                }
                self.code_block_buf.clear();
                self.code_block_active = false;
            } else {
                // Opening fence — skip the ``` and optional language tag
                let after_fence = &delta[abs_idx + 3..];
                let skip_to = after_fence.find('\n').map(|i| i + 1).unwrap_or(after_fence.len());
                self.code_block_buf.clear();
                // Content after the opening ``` line
                if skip_to < after_fence.len() {
                    self.code_block_buf.push_str(&after_fence[skip_to..]);
                }
                self.code_block_active = true;
                // Advance pos past the fence + language tag line
                pos = abs_idx + 3 + skip_to;
                continue;
            }
            pos = abs_idx + 3;
        }
        // If in a code block, append remaining text after the last fence
        if self.code_block_active && pos < delta.len() {
            self.code_block_buf.push_str(&delta[pos..]);
        }
    }

    /// Extract the last code block from all completed messages (used after
    /// streaming finishes to ensure accuracy).
    pub fn refresh_last_code_block(&mut self) {
        for msg in self.messages.iter().rev() {
            for block in msg.content_blocks.iter().rev() {
                if let ContentBlock::Text { content } = block {
                    if let Some(code) = extract_last_code_block(content) {
                        self.last_code_block = Some(code);
                        return;
                    }
                }
            }
        }
    }
}

/// Extract the last fenced code block from text.
/// Returns the code content (without fence markers and language tag).
fn extract_last_code_block(text: &str) -> Option<String> {
    let mut result: Option<String> = None;
    let mut in_block = false;
    let mut block_content = String::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_block {
                let content = block_content.trim().to_string();
                if !content.is_empty() {
                    result = Some(content);
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

        // ------------------------------------------------------------------
        // Collect all lines with markdown metadata
        // ------------------------------------------------------------------
        // Each entry: (role, display_text, kind)
        let mut all_lines: Vec<(MessageRole, String, LineKind)> = Vec::new();

        // Helper: process Text content block with markdown line-type detection.
        let process_text = |role: MessageRole, content: &str, lines: &mut Vec<(MessageRole, String, LineKind)>| {
            let mut in_code_block = false;
            for line in content.lines() {
                let lt = markdown::detect_line_type(line);
                match lt {
                    markdown::LineType::Heading(level) => {
                        if in_code_block {
                            // treat as code block content if inside fence
                            lines.push((role, line.to_string(), LineKind::CodeBlock));
                        } else {
                            let text = markdown::heading_text(line, level);
                            lines.push((role, text, LineKind::Heading(level)));
                        }
                    }
                    markdown::LineType::CodeFence { .. } => {
                        in_code_block = !in_code_block;
                        // Don't render the fence markers themselves.
                    }
                    markdown::LineType::ListItem => {
                        if in_code_block {
                            lines.push((role, line.to_string(), LineKind::CodeBlock));
                        } else {
                            lines.push((role, format!("  {}", line), LineKind::ListItem));
                        }
                    }
                    markdown::LineType::HorizontalRule => {
                        if in_code_block {
                            lines.push((role, line.to_string(), LineKind::CodeBlock));
                        } else {
                            lines.push((role, "──────────────────────".to_string(), LineKind::HorizontalRule));
                        }
                    }
                    markdown::LineType::Normal => {
                        let kind = if in_code_block {
                            LineKind::CodeBlock
                        } else {
                            LineKind::Normal
                        };
                        lines.push((role, line.to_string(), kind));
                    }
                }
            }
        };

        for msg in &state.messages {
            // Add role label for User messages
            if msg.role == MessageRole::User {
                all_lines.push((msg.role, "You".to_string(), LineKind::RoleLabel));
            }

            for block in &msg.content_blocks {
                match block {
                    ContentBlock::Text { content } => {
                        process_text(msg.role, content, &mut all_lines);
                    }
                    ContentBlock::Thinking { content, collapsed } => {
                        let indicator = if *collapsed { "▸" } else { "▾" };
                        all_lines.push((msg.role, format!("{} Thinking…", indicator), LineKind::Normal));
                        if !*collapsed {
                            for line in content.lines() {
                                all_lines.push((msg.role, format!("  {}", line), LineKind::Normal));
                            }
                        } else if let Some(first) = content.lines().next() {
                            all_lines.push((msg.role, format!("  {}", first), LineKind::Normal));
                        }
                    }
                    ContentBlock::ToolCall { name, arguments, .. } => {
                        all_lines.push((msg.role, format!("┌─ tool: {} ───", name), LineKind::Normal));
                        for line in arguments.lines().take(8) {
                            all_lines.push((msg.role, format!("│ {}", line), LineKind::Normal));
                        }
                        all_lines.push((msg.role, "└─".to_string(), LineKind::Normal));
                    }
                    ContentBlock::ToolResult { tool_name, content, is_error } => {
                        let prefix = if *is_error { "✗" } else { "✓" };
                        all_lines.push((msg.role, format!("┌─ {}: {} ───", prefix, tool_name), LineKind::Normal));
                        for line in content.lines().take(3) {
                            all_lines.push((msg.role, format!("│ {}", line), LineKind::Normal));
                        }
                        all_lines.push((msg.role, "└─".to_string(), LineKind::Normal));
                    }
                    ContentBlock::Error { title, message, retryable } => {
                        all_lines.push((msg.role, format!("[!] {}", title), LineKind::Normal));
                        for line in message.lines().take(6) {
                            all_lines.push((msg.role, format!("│ {}", line), LineKind::Normal));
                        }
                        if *retryable {
                            all_lines.push((msg.role, "│ ↻ This error may be temporary".to_string(), LineKind::Normal));
                        }
                        all_lines.push((msg.role, "└─".to_string(), LineKind::Normal));
                    }
                    ContentBlock::Image { mime_type, base64_data } => {
                        let size_bytes = base64_data.len() * 3 / 4;
                        let size_str = if size_bytes >= 1_048_576 {
                            format!("{:.1} MB", size_bytes as f64 / 1_048_576.0)
                        } else if size_bytes >= 1024 {
                            format!("{:.1} KB", size_bytes as f64 / 1024.0)
                        } else {
                            format!("{} B", size_bytes)
                        };
                        all_lines.push((msg.role, format!("[image: {}, {}]", mime_type, size_str), LineKind::Normal));
                        all_lines.push((msg.role, "  Ctrl+I → open in viewer".to_string(), LineKind::Normal));
                    }
                }
            }
            // Blank separator after each message
            all_lines.push((msg.role, String::new(), LineKind::Normal));
        }

        // Add streaming message
        if let Some(ref streaming) = state.streaming {
            for block in &streaming.message.content_blocks {
                match block {
                    ContentBlock::Text { content } => {
                        process_text(MessageRole::Assistant, content, &mut all_lines);
                    }
                    ContentBlock::Thinking { content, collapsed } => {
                        let indicator = if *collapsed { "▸" } else { "▾" };
                        all_lines.push((MessageRole::Assistant, format!("  {} Thinking…", indicator), LineKind::Normal));
                        if !*collapsed {
                            for line in content.lines() {
                                all_lines.push((MessageRole::Assistant, format!("    {}", line), LineKind::Normal));
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Streaming indicator — uses spinner_frame for animation
            let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let ch = spinner_chars[state.spinner_frame % spinner_chars.len()];
            all_lines.push((MessageRole::Assistant, format!("  {} thinking…", ch), LineKind::Normal));
        }

        // ------------------------------------------------------------------
        // Build ratatui Lines from collected metadata
        // ------------------------------------------------------------------
        let h_pad: usize = 2;
        // Width available for text content after prefix char and left padding.
        let content_fill = area.width as usize - 1 - h_pad;

        let mut ratatui_lines: Vec<Line<'static>> = Vec::new();

        for (role, text, kind) in &all_lines {
            let prefix_style = match role {
                MessageRole::User => styles.primary,
                MessageRole::Assistant => styles.accent,
                MessageRole::System => styles.muted,
            };

            let line_base_style: Style = match kind {
                LineKind::Normal => styles.normal,
                LineKind::CodeBlock => markdown::code_block_style(styles.normal),
                LineKind::Heading(level) => markdown::heading_style(styles.normal, *level),
                LineKind::ListItem => styles.normal,
                LineKind::HorizontalRule => styles.muted,
                LineKind::RoleLabel => styles.primary.add_modifier(Modifier::BOLD),
            };

            let mut spans: Vec<Span<'static>> = Vec::new();

            // Role prefix (1 char with role-specific color)
            spans.push(Span::styled(String::from(" "), prefix_style));

            // Horizontal padding (left)
            spans.push(Span::styled(" ".repeat(h_pad), line_base_style));

            // Content spans depend on line kind
            match kind {
                LineKind::CodeBlock => {
                    // Pad to fill area width so the code-block background covers the full row
                    let padded = format!("{:<width$}", text, width = content_fill);
                    spans.push(Span::styled(padded, line_base_style));
                }
                LineKind::HorizontalRule | LineKind::RoleLabel => {
                    // Uniform style, no inline parsing
                    spans.push(Span::styled(text.clone(), line_base_style));
                }
                _ => {
                    // Normal / Heading / ListItem — parse inline markdown into styled Spans
                    let segments = markdown::parse_inline(text);
                    for seg in &segments {
                        let seg_style = match seg {
                            markdown::Segment::Normal(_) => line_base_style,
                            markdown::Segment::Bold(_) => markdown::bold_style(line_base_style),
                            markdown::Segment::Italic(_) => line_base_style,
                            markdown::Segment::Code(_) => markdown::code_style(line_base_style),
                            markdown::Segment::Link { .. } => markdown::link_style(line_base_style),
                        };
                        let s: &str = match seg {
                            markdown::Segment::Normal(s) => s,
                            markdown::Segment::Bold(s) => s,
                            markdown::Segment::Italic(s) => s,
                            markdown::Segment::Code(s) => s,
                            markdown::Segment::Link { text, .. } => text,
                        };
                        spans.push(Span::styled(s.to_string(), seg_style));
                    }
                }
            }

            ratatui_lines.push(Line::from(spans));
        }

        // ------------------------------------------------------------------
        // Compute content height & visible range
        // ------------------------------------------------------------------
        state.content_height = ratatui_lines.len() as u16;

        let visible_height = area.height as usize;
        let max_scroll = state.content_height.saturating_sub(visible_height as u16);
        let clamped_offset = state.scroll_offset.min(max_scroll);

        // ------------------------------------------------------------------
        // Render via Paragraph (handles scrolling + background fill)
        // ------------------------------------------------------------------
        let paragraph = Paragraph::new(ratatui_lines)
            .block(Block::default().style(styles.normal))
            .scroll((clamped_offset, 0));
        paragraph.render(area, buf);

        // ------------------------------------------------------------------
        // Scrollbar (manual buffer access — justified for █ overlay)
        // ------------------------------------------------------------------
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

    #[test]
    fn extract_code_block_simple() {
        let text = "Some text\n```rust\nfn main() {}\n```\nMore text";
        assert_eq!(extract_last_code_block(text), Some("fn main() {}".to_string()));
    }

    #[test]
    fn extract_code_block_multiple() {
        let text = "```\nfirst\n```\n```python\nsecond\n```";
        assert_eq!(extract_last_code_block(text), Some("second".to_string()));
    }

    #[test]
    fn extract_code_block_none() {
        let text = "No code blocks here";
        assert_eq!(extract_last_code_block(text), None);
    }

    #[test]
    fn extract_code_block_empty() {
        let text = "```\n```";
        assert_eq!(extract_last_code_block(text), None);
    }

    #[test]
    fn streaming_code_block_tracking() {
        let mut state = ChatViewState::default();
        state.start_streaming();
        state.stream_text_delta("Here is code:\n");
        state.stream_text_delta("```rust\n");
        state.stream_text_delta("let x = 42;\n");
        state.stream_text_delta("```");
        assert_eq!(state.last_code_block, Some("let x = 42;".to_string()));
    }

    #[test]
    fn refresh_code_block_from_messages() {
        let mut state = ChatViewState::default();
        state.add_message(ChatMessage {
            role: MessageRole::Assistant,
            content_blocks: vec![ContentBlock::Text {
                content: "```js\nconsole.log('hi');\n```".to_string(),
            }],
            timestamp: 0,
        });
        state.refresh_last_code_block();
        assert_eq!(state.last_code_block, Some("console.log('hi');".to_string()));
    }
}
