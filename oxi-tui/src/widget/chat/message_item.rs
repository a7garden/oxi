//! `MessageItem` — renders a single chat message.
//!
//! The message is laid out as:
//!
//! 1. A role label line ("You", "Assistant", "System", "Tool") styled
//!    from the theme.
//! 2. Zero or more text blocks, parsed and wrapped by `StreamingMarkdown`.
//! 3. Zero or more tool-call cards rendered via `ToolCall`.
//! 4. A trailing blank line separating this message from the next.
//!
//! The widget retains the `StreamingMarkdown` across frames so that
//! checkpoint optimization amortizes parsing cost during streaming.
//! `sync_from` is the mutation entry point used by `ChatView` to keep
//! the widget in lockstep with the source `ChatMessage`.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::content::{ChatMessage, ContentBlock, MessageRole};
use crate::text::StreamingMarkdown;
use crate::widget::{RenderCtx, Renderable, hash_combine};

use super::ToolCall;

/// Render a single chat message: role label + body blocks + trailing blank.
#[derive(Debug)]
pub struct MessageItem {
    message: ChatMessage,
    md: StreamingMarkdown,
    tool_calls: Vec<ToolCall>,
    cached_hash: u64,
}

impl MessageItem {
    /// Build a widget for the given message. The first text block is
    /// fed to the streaming markdown renderer; existing tool-call blocks
    /// become `ToolCall` cards.
    #[must_use]
    pub fn new(message: ChatMessage) -> Self {
        let mut md = StreamingMarkdown::new();
        if let Some(text) = message.text_content() {
            md.push_token(text);
        }
        let tool_calls = collect_tool_calls(&message);
        let cached_hash = compute_hash(&message, &tool_calls);
        Self {
            message,
            md,
            tool_calls,
            cached_hash,
        }
    }

    /// Pull the latest content from `message`. If the role or block list
    /// changed in ways that need re-render, update internal state and
    /// invalidate the cached hash. Returns `true` if anything changed.
    pub fn sync_from(&mut self, message: &ChatMessage) -> bool {
        if self.message.content_hash() == message.content_hash() {
            return false;
        }
        self.message = message.clone();
        // Replay text tokens into the markdown renderer. This is O(text);
        // the renderer's checkpoint optimization keeps streaming cheap.
        self.md.invalidate();
        if let Some(text) = message.text_content() {
            self.md.push_token(text);
        }
        self.tool_calls = collect_tool_calls(message);
        self.cached_hash = compute_hash(message, &self.tool_calls);
        true
    }

    /// Read-only access to the underlying message.
    #[must_use]
    pub fn message(&self) -> &ChatMessage {
        &self.message
    }
}

fn collect_tool_calls(message: &ChatMessage) -> Vec<ToolCall> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall {
                id,
                name,
                args,
                status,
            } => Some(ToolCall::new(id.clone(), name.clone(), *status, args)),
            _ => None,
        })
        .collect()
}

fn compute_hash(message: &ChatMessage, tool_calls: &[ToolCall]) -> u64 {
    let mut h = hash_combine(message.content_hash(), 0);
    for tc in tool_calls {
        h = hash_combine(h, tc.content_hash());
    }
    h
}

impl Renderable for MessageItem {
    fn content_hash(&self) -> u64 {
        self.cached_hash
    }

    fn height_for(&self, width: u16, ctx: &RenderCtx) -> u16 {
        let label = 1_u16;
        let mut text_height = 0_u16;
        if message_has_text(&self.message) {
            let lines = self.md.lines(width.saturating_sub(2).max(1), ctx.theme());
            text_height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        }
        let tool_height: u16 = self.tool_calls.iter().map(|_| 3_u16).sum();
        let separator = 1_u16;
        label
            .saturating_add(text_height)
            .saturating_add(tool_height)
            .saturating_add(separator)
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let theme = ctx.theme().clone();
        let mut y = area.y;

        // ── 1. Role label line ─────────────────────────────────────────
        let label = role_label(self.message.role);
        let label_style = role_style(self.message.role, &theme);
        let label_line = Line::from(vec![Span::styled(label, label_style)]);
        ctx.buffer_mut()
            .set_line(area.x, y, &label_line, area.width);
        y = y.saturating_add(1);
        if y >= area.y.saturating_add(area.height) {
            return;
        }

        // ── 2. Text content via StreamingMarkdown ─────────────────────
        if message_has_text(&self.message) {
            let content_width = area.width.saturating_sub(2);
            let lines = self.md.lines(content_width.max(1), &theme);
            let available = area.y.saturating_add(area.height).saturating_sub(y);
            let take = lines.len().min(available as usize);
            for line in lines.into_iter().take(take) {
                ctx.buffer_mut()
                    .set_line(area.x.saturating_add(1), y, &line, content_width);
                y = y.saturating_add(1);
                if y >= area.y.saturating_add(area.height) {
                    return;
                }
            }
        }

        // ── 3. Tool-call cards ─────────────────────────────────────────
        for tc in &mut self.tool_calls {
            let card_height = tc.height_for(area.width, ctx);
            let remaining = area.y.saturating_add(area.height).saturating_sub(y);
            if remaining < card_height {
                break;
            }
            let card_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: card_height,
            };
            tc.render(card_area, ctx);
            y = y.saturating_add(card_height);
            if y >= area.y.saturating_add(area.height) {
                return;
            }
        }

        // ── 4. Trailing blank line ─────────────────────────────────────
        // Already blank in the buffer; nothing to do.
    }
}

fn message_has_text(message: &ChatMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text(_) | ContentBlock::Thinking(_)))
}

fn role_label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "You",
        MessageRole::Assistant => "Assistant",
        MessageRole::System => "System",
        MessageRole::Tool => "Tool",
    }
}

fn role_style(role: MessageRole, theme: &crate::theme::Theme) -> Style {
    match role {
        MessageRole::User => Style::default()
            .fg(theme.colors.user)
            .add_modifier(Modifier::BOLD),
        MessageRole::Assistant => Style::default()
            .fg(theme.colors.response)
            .add_modifier(Modifier::BOLD),
        MessageRole::System => Style::default()
            .fg(theme.colors.muted)
            .add_modifier(Modifier::BOLD),
        MessageRole::Tool => Style::default()
            .fg(theme.colors.tool)
            .add_modifier(Modifier::BOLD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{ChatMessage, MessageRole, ToolCallStatus};
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn render_message(item: &mut MessageItem, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            item.render(
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                &mut ctx,
            );
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    #[test]
    fn message_item_renders_user_label_and_text() {
        let mut msg = ChatMessage::new(1, MessageRole::User);
        msg.append_text("hello");
        let mut item = MessageItem::new(msg);
        let buf = render_message(&mut item, 40, 5);
        let top: String = (0..40).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            top.contains("You"),
            "top line should contain the role label, got {top:?}"
        );
        // Text should appear on line 1.
        let body: String = (0..40).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(
            body.contains("hello"),
            "second line should contain the text, got {body:?}"
        );
    }

    #[test]
    fn message_item_hash_changes_on_sync() {
        let mut msg = ChatMessage::new(1, MessageRole::User);
        msg.append_text("hello");
        let mut item = MessageItem::new(msg.clone());
        let h_before = item.content_hash();
        msg.append_text(" world");
        assert!(item.sync_from(&msg));
        let h_after = item.content_hash();
        assert_ne!(h_before, h_after);
    }

    #[test]
    fn message_item_sync_returns_false_when_unchanged() {
        let mut msg = ChatMessage::new(1, MessageRole::User);
        msg.append_text("hello");
        let mut item = MessageItem::new(msg.clone());
        assert!(!item.sync_from(&msg));
    }

    #[test]
    fn message_item_renders_tool_call_blocks() {
        let mut msg = ChatMessage::new(2, MessageRole::Assistant);
        msg.append_text("Let me look that up.");
        msg.blocks.push(ContentBlock::ToolCall {
            id: "c1".into(),
            name: "search".into(),
            args: "{}".into(),
            status: ToolCallStatus::Completed,
        });
        let mut item = MessageItem::new(msg);
        let buf = render_message(&mut item, 40, 10);
        // The tool call title should appear somewhere in the buffer.
        let rows: Vec<Vec<String>> = (0..10)
            .map(|y| (0..40).map(|x| buf[(x, y)].symbol().to_owned()).collect())
            .collect();
        let full: String = rows.into_iter().flatten().collect();
        assert!(
            full.contains("search"),
            "tool call name should appear in buffer, got {full:?}"
        );
    }

    #[test]
    fn message_item_height_includes_role_label_text_and_tools() {
        let mut msg = ChatMessage::new(3, MessageRole::Assistant);
        msg.append_text("hi");
        msg.blocks.push(ContentBlock::ToolCall {
            id: "c1".into(),
            name: "search".into(),
            args: "{}".into(),
            status: ToolCallStatus::Completed,
        });
        let item = MessageItem::new(msg);
        let backend = TestBackend::new(40, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let ctx = RenderCtx::new(frame, &theme, &caps);
            let height = item.height_for(40, &ctx);
            // 1 label + 1 text line + 3 tool card + 1 separator = 6.
            assert!(height >= 6, "expected >=6 height, got {height}");
        })
        .unwrap();
    }
}
