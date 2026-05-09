//! ChatView widget — scrollable message list with streaming support.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, StatefulWidget},
};
use ratatui::widgets::Widget;
use crate::{Theme, ThemeStyles};
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
    Text { content: String },
    /// Collapsible thinking / reasoning block.
    Thinking { content: String, collapsed: bool },
    /// A tool call made by the assistant.
    ToolCall { id: String, name: String, arguments: String },
    /// The result of a tool call.
    ToolResult { tool_name: String, content: String, is_error: bool },
    /// An error message.
    Error { title: String, message: String, retryable: bool },
    /// An image content block (base64-encoded).
    Image { mime_type: String, base64_data: String },
}

/// Display representation of a chat message.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content_blocks: Vec<ContentBlock>,
    pub timestamp: i64,
}

/// Streaming state for the in-progress assistant message.
#[derive(Debug, Clone)]
pub struct StreamingState {
    pub message: ChatMessage,
    pub active_content_index: usize,
}

/// Kind of a collected line, used to pick the right render style.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LineKind {
    Normal,
    CodeBlock,
    Heading(u8),
    ListItem,
    HorizontalRule,
    RoleLabel,
    /// Tool call header: ┌─ ─ ─ tool: name ─ ─ ─┐
    ToolCallHeader,
    /// Tool call body lines (arguments)
    ToolCallBody,
    /// Tool call footer (blank separator)
    ToolCallFooter,
    /// Tool result header: ┌─ ✓/✗ tool_name ─ ─┐
    ToolResultHeader,
    /// Tool result body lines (content preview)
    ToolResultBody,
    /// Tool result footer (blank separator)
    ToolResultFooter,
    /// Error header line
    ErrorHeader,
    /// Error body lines
    ErrorBody,
    /// Error footer (blank separator)
    ErrorFooter,
    /// Table border/separator line
    TableBorder,
    /// Table header row
    TableHeader,
    /// Table data row
    TableRow,
}

/// State for the ChatView widget.
#[derive(Debug, Default)]
pub struct ChatViewState {
    pub messages: Vec<ChatMessage>,
    pub streaming: Option<StreamingState>,
    pub scroll_offset: u16,
    content_height: u16,
    pub last_code_block: Option<String>,
    code_block_active: bool,
    code_block_buf: String,
    pub pending_images: Vec<(String, String)>,
    pub spinner_frame: usize,
}

impl ChatViewState {
    pub fn add_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        self.streaming = None;
    }

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

    pub fn stream_text_delta(&mut self, delta: &str) {
        if let Some(ref mut state) = self.streaming {
            if let Some(ContentBlock::Text { ref mut content }) =
                state.message.content_blocks.last_mut()
            {
                content.push_str(delta);
            } else {
                state.message.content_blocks.push(ContentBlock::Text {
                    content: delta.to_string(),
                });
            }
        }
        self.update_last_code_block(delta);
    }

    pub fn stream_tool_call(&mut self, id: String, name: String, arguments: String) {
        if let Some(ref mut state) = self.streaming {
            state.message.content_blocks.push(ContentBlock::ToolCall { id, name, arguments });
        }
    }

    pub fn stream_tool_result(&mut self, tool_name: String, content: String, is_error: bool) {
        if let Some(ref mut state) = self.streaming {
            state.message.content_blocks.push(ContentBlock::ToolResult { tool_name, content, is_error });
        }
    }

    pub fn stream_error(&mut self, title: String, message: String, retryable: bool) {
        if let Some(ref mut state) = self.streaming {
            state.message.content_blocks.push(ContentBlock::Error { title, message, retryable });
        }
    }

    pub fn stream_image(&mut self, mime_type: String, base64_data: String) {
        self.pending_images.push((base64_data.clone(), mime_type.clone()));
        if let Some(ref mut state) = self.streaming {
            state.message.content_blocks.push(ContentBlock::Image { mime_type, base64_data });
        }
    }

    pub fn finish_streaming(&mut self) {
        if let Some(state) = self.streaming.take() {
            self.messages.push(state.message);
        }
    }

    pub fn scroll_to_bottom(&mut self, visible_height: u16) {
        let max_scroll = self.content_height.saturating_sub(visible_height);
        self.scroll_offset = max_scroll;
    }

    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
    }

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

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    fn update_last_code_block(&mut self, delta: &str) {
        let mut pos = 0;
        while let Some(idx) = delta[pos..].find("```") {
            let abs_idx = pos + idx;
            if self.code_block_active {
                let before = &delta[pos..abs_idx];
                self.code_block_buf.push_str(before);
                let content = self.code_block_buf.trim().to_string();
                if !content.is_empty() {
                    self.last_code_block = Some(content);
                }
                self.code_block_buf.clear();
                self.code_block_active = false;
            } else {
                let after_fence = &delta[abs_idx + 3..];
                let skip_to = after_fence.find('\n').map(|i| i + 1).unwrap_or(after_fence.len());
                self.code_block_buf.clear();
                if skip_to < after_fence.len() {
                    self.code_block_buf.push_str(&after_fence[skip_to..]);
                }
                self.code_block_active = true;
                pos = abs_idx + 3 + skip_to;
                continue;
            }
            pos = abs_idx + 3;
        }
        if self.code_block_active && pos < delta.len() {
            self.code_block_buf.push_str(&delta[pos..]);
        }
    }

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
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme, scrollbar: true }
    }

    pub fn with_scrollbar(mut self, show: bool) -> Self {
        self.scrollbar = show;
        self
    }
}

impl StatefulWidget for ChatView<'_> {
    type State = ChatViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width < 4 || area.height < 1 {
            return;
        }

        let styles = self.theme.to_styles();

        // Helper: symmetric box-drawing top border filler (n × "─ ")
        fn box_top(n: usize) -> String {
            "─".repeat(n)
        }

        fn box_header(label: &str) -> String {
            format!("── {} {}", label, "─".repeat(20))
        }

        // Helper: left-border stripe spans for user messages (▌ + tinted space)
        fn user_stripe(styles: &ThemeStyles) -> Vec<Span> {
            vec![
                Span::styled("▌", styles.user_border),
                Span::styled(" ", styles.user_bg),
            ]
        }


        // ------------------------------------------------------------------
        // Collect all lines
        // ------------------------------------------------------------------
        let mut all_lines: Vec<(MessageRole, String, LineKind)> = Vec::new();

        let process_text = |role: MessageRole,
                            content: &str,
                            lines: &mut Vec<(MessageRole, String, LineKind)>| {
            let mut in_code_block = false;
            for line in content.lines() {
                let lt = markdown::detect_line_type(line);
                match lt {
                    markdown::LineType::Heading(level) => {
                        if in_code_block {
                            lines.push((role, line.to_string(), LineKind::CodeBlock));
                        } else {
                            let text = markdown::heading_text(line, level);
                            lines.push((role, text, LineKind::Heading(level)));
                        }
                    }
                    markdown::LineType::CodeFence { .. } => {
                        in_code_block = !in_code_block;
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
                            lines.push((
                                role,
                                "──────────────────────".to_string(),
                                LineKind::HorizontalRule,
                            ));
                        }
                    }
                    markdown::LineType::TableSeparator { widths } => {
                        if in_code_block {
                            lines.push((role, line.to_string(), LineKind::CodeBlock));
                        } else {
                            // Draw border: ───┼───┼───
                            let border: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                            lines.push((role, format!(" {} ", border.join("┼")), LineKind::TableBorder));
                        }
                    }
                    markdown::LineType::TableRow { cells } => {
                        if in_code_block {
                            lines.push((role, line.to_string(), LineKind::CodeBlock));
                        } else {
                            // Format: " cell1 | cell2 | cell3 "
                            let formatted = format!(" {} ", cells.join(" | "));
                            lines.push((role, formatted, LineKind::TableRow));
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
                        all_lines.push((
                            msg.role,
                            format!("{} Thinking...", indicator),
                            LineKind::Normal,
                        ));
                        if !*collapsed {
                            for line in content.lines() {
                                all_lines.push((msg.role, format!("  {}", line), LineKind::Normal));
                            }
                        } else if let Some(first) = content.lines().next() {
                            all_lines.push((msg.role, format!("  {}", first), LineKind::Normal));
                        }
                    }
                    ContentBlock::ToolCall { name, arguments, .. } => {
                        all_lines.push((
                            msg.role,
                            format!("  {} tool: {} {}", box_top(15), name, box_top(15)),
                            LineKind::ToolCallHeader,
                        ));
                        // Truncate long argument blocks (4 lines if >4 lines, else 6)
                        let max_args = if arguments.lines().count() <= 4 { 6 } else { 4 };
                        for line in arguments.lines().take(max_args) {
                            all_lines.push((msg.role, format!("  {}", line), LineKind::ToolCallBody));
                        }
                        if arguments.lines().count() > max_args {
                            all_lines.push((msg.role, "  ...".to_string(), LineKind::ToolCallBody));
                        }
                        all_lines.push((msg.role, String::new(), LineKind::ToolCallFooter));
                    }
                    ContentBlock::ToolResult { tool_name, content, is_error } => {
                        let prefix = if *is_error { "✗" } else { "✓" };
                        let header = if tool_name.is_empty() {
                            format!("  {} {}", prefix, box_top(30))
                        } else {
                            format!("  {} {} {}", prefix, tool_name, box_top(20))
                        };
                        all_lines.push((
                            msg.role,
                            header,
                            LineKind::ToolResultHeader,
                        ));
                        for line in content.lines().take(2) {
                            all_lines.push((msg.role, format!("  {}", line), LineKind::ToolResultBody));
                        }
                        if content.lines().count() > 2 {
                            all_lines.push((msg.role, "  ...".to_string(), LineKind::ToolResultBody));
                        }
                        all_lines.push((msg.role, String::new(), LineKind::ToolResultFooter));
                    }
                    ContentBlock::Error { title, message, retryable } => {
                        all_lines.push((msg.role, format!("  [!] {}", title), LineKind::ErrorHeader));
                        for line in message.lines().take(4) {
                            all_lines.push((msg.role, format!("  {}", line), LineKind::ErrorBody));
                        }
                        if *retryable {
                            all_lines.push((
                                msg.role,
                                "  retry: this error may be temporary".to_string(),
                                LineKind::ErrorBody,
                            ));
                        }
                        all_lines.push((msg.role, String::new(), LineKind::ErrorFooter));
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
                        all_lines.push((
                            msg.role,
                            format!("[image: {}, {}]", mime_type, size_str),
                            LineKind::Normal,
                        ));
                        all_lines.push((
                            msg.role,
                            "  Ctrl+I → open in viewer".to_string(),
                            LineKind::Normal,
                        ));
                    }
                }
            }
            all_lines.push((msg.role, String::new(), LineKind::Normal));
        }

        if let Some(ref streaming) = state.streaming {
            for block in &streaming.message.content_blocks {
                match block {
                    ContentBlock::Text { content } => {
                        process_text(MessageRole::Assistant, content, &mut all_lines);
                    }
                    ContentBlock::Thinking { content, collapsed } => {
                        let indicator = if *collapsed { "▸" } else { "▾" };
                        all_lines.push((
                            MessageRole::Assistant,
                            format!("  {} Thinking...", indicator),
                            LineKind::Normal,
                        ));
                        if !*collapsed {
                            for line in content.lines() {
                                all_lines.push((MessageRole::Assistant, format!("    {}", line), LineKind::Normal));
                            }
                        }
                    }
                    ContentBlock::ToolCall { name, arguments, .. } => {
                        all_lines.push((
                            MessageRole::Assistant,
                            format!("  {} tool: {} {}", box_top(15), name, box_top(15)),
                            LineKind::ToolCallHeader,
                        ));
                        let max_args = if arguments.lines().count() <= 4 { 6 } else { 4 };
                        for line in arguments.lines().take(max_args) {
                            all_lines.push((MessageRole::Assistant, format!("  {}", line), LineKind::ToolCallBody));
                        }
                        if arguments.lines().count() > max_args {
                            all_lines.push((MessageRole::Assistant, "  ...".to_string(), LineKind::ToolCallBody));
                        }
                        all_lines.push((MessageRole::Assistant, String::new(), LineKind::ToolCallFooter));
                    }
                    ContentBlock::ToolResult { tool_name, content, is_error } => {
                        let prefix = if *is_error { "✗" } else { "✓" };
                        let header = if tool_name.is_empty() {
                            format!("  {} {}", prefix, box_top(30))
                        } else {
                            format!("  {} {} {}", prefix, tool_name, box_top(20))
                        };
                        all_lines.push((
                            MessageRole::Assistant,
                            header,
                            LineKind::ToolResultHeader,
                        ));
                        for line in content.lines().take(2) {
                            all_lines.push((MessageRole::Assistant, format!("  {}", line), LineKind::ToolResultBody));
                        }
                        if content.lines().count() > 2 {
                            all_lines.push((MessageRole::Assistant, "  ...".to_string(), LineKind::ToolResultBody));
                        }
                        all_lines.push((MessageRole::Assistant, String::new(), LineKind::ToolResultFooter));
                    }
                    _ => {}
                }
            }
            let spinner_chars =
                ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let ch = spinner_chars[state.spinner_frame % spinner_chars.len()];
            all_lines.push((MessageRole::Assistant, format!("  {} thinking...", ch), LineKind::Normal));
        }

        // ------------------------------------------------------------------
        // Build Vec<Line> with inline markdown spans
        // ------------------------------------------------------------------
        let mut ratatui_lines: Vec<Line> = Vec::new();

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
                // Tool call / result lines — muted foreground keeps them visually subordinate
                LineKind::ToolCallHeader
                | LineKind::ToolCallBody
                | LineKind::ToolCallFooter
                | LineKind::ToolResultHeader
                | LineKind::ToolResultBody
                | LineKind::ToolResultFooter => styles.muted,
                // Error lines — bright error color
                LineKind::ErrorHeader | LineKind::ErrorBody | LineKind::ErrorFooter => styles.error,
                LineKind::TableBorder => markdown::table_border_style(styles.normal),
                LineKind::TableHeader => markdown::table_header_style(styles.normal),
                LineKind::TableRow => styles.normal,
            };
            let mut spans: Vec<Span> = Vec::new();
            // User messages: left-border stripe (no role prefix)
            if *role == MessageRole::User {
                spans.extend(user_stripe(&styles));
            } else {
                spans.push(Span::styled(" ", prefix_style));
                spans.push(Span::styled(" ", line_base_style));
            }
            spans.push(Span::styled(" ", line_base_style));

            match kind {
                LineKind::CodeBlock | LineKind::HorizontalRule | LineKind::RoleLabel
                | LineKind::TableBorder | LineKind::TableHeader => {
                    spans.push(Span::styled(text.clone(), line_base_style));
                }
                _ => {
                    let segments = markdown::parse_inline(text);
                    for seg in &segments {
                        let seg_style = match seg {
                            markdown::Segment::Normal(_) => line_base_style,
                            markdown::Segment::Bold(_) => markdown::bold_style(line_base_style),
                            markdown::Segment::Italic(_) => markdown::italic_style(line_base_style),
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
        // Compute scroll
        // ------------------------------------------------------------------
        state.content_height = ratatui_lines.len() as u16;
        let visible_height = area.height as usize;
        let max_scroll = state
            .content_height
            .saturating_sub(visible_height as u16);
        let clamped_offset = state.scroll_offset.min(max_scroll);

        // ------------------------------------------------------------------
        // Render via Paragraph (handles background + scroll)
        // ------------------------------------------------------------------
        let paragraph = Paragraph::new(ratatui_lines)
            .block(Block::default().style(styles.normal))
            .scroll((clamped_offset, 0));
        paragraph.render(area, buf);

        // ------------------------------------------------------------------
        // Scrollbar — manual buffer (justified for █ thumb)
        // ------------------------------------------------------------------
        if self.scrollbar && max_scroll > 0 {
            let thumb_pos =
                (clamped_offset as f32 / max_scroll as f32 * visible_height as f32) as u16;
            let thumb_size = ((visible_height as f32 * visible_height as f32)
                / (state.content_height as f32))
                .max(1.0) as u16;

            for i in 0..thumb_size.min(visible_height as u16) {
                let sb_row = area.y
                    + thumb_pos
                        .saturating_add(i)
                        .min(area.y + area.height - 1);
                buf[(area.x + area.width - 1, sb_row)]
                    .set_char('█')
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
        assert_eq!(extract_last_code_block("No code blocks here"), None);
    }

    #[test]
    fn extract_code_block_empty() {
        assert_eq!(extract_last_code_block("```\n```"), None);
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