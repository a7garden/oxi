//! `ToolCall` — bordered card showing a tool invocation's name, status
//! indicator, and a (truncated) args preview.
//!
//! The widget is meant to be embedded inside `MessageItem` whenever the
//! owning `ChatMessage` contains `ContentBlock::ToolCall`. It is a
//! `Renderable` so it can be hosted as a `RetainedChild` from a parent.

use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};

use crate::content::ToolCallStatus;
use crate::widget::{RenderCtx, Renderable, hash_combine, hash_str};

/// Maximum number of characters from `args` shown in the preview line.
const ARGS_PREVIEW_LIMIT: usize = 60;

/// Card displaying one tool invocation.
#[derive(Debug, Clone)]
pub struct ToolCall {
    id: String,
    name: String,
    status: ToolCallStatus,
    args_preview: String,
    cached_hash: u64,
}

impl ToolCall {
    /// Build a card from a tool-call block's fields. `args` is truncated
    /// to `ARGS_PREVIEW_LIMIT` characters; a trailing `…` marks truncation.
    #[must_use]
    pub fn new(id: String, name: String, status: ToolCallStatus, args: &str) -> Self {
        let args_preview = truncate(args, ARGS_PREVIEW_LIMIT);
        let cached_hash = compute_hash(&id, &name, status, &args_preview);
        Self {
            id,
            name,
            status,
            args_preview,
            cached_hash,
        }
    }

    /// Return the tool call id (matches the `id` in `ContentBlock::ToolCall`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Return the tool name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the current status.
    #[must_use]
    pub fn status(&self) -> ToolCallStatus {
        self.status
    }

    /// Update the status (used when the call transitions to Running /
    /// Completed / Failed).
    pub fn set_status(&mut self, status: ToolCallStatus) {
        if self.status == status {
            return;
        }
        self.status = status;
        self.cached_hash = compute_hash(&self.id, &self.name, self.status, &self.args_preview);
    }
}

fn compute_hash(id: &str, name: &str, status: ToolCallStatus, args_preview: &str) -> u64 {
    hash_combine(
        hash_str(id),
        hash_combine(
            hash_str(name),
            hash_combine(status as u64, hash_str(args_preview)),
        ),
    )
}

fn truncate(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max) + 1);
    for (count, ch) in s.chars().enumerate() {
        if count == max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

impl Renderable for ToolCall {
    fn content_hash(&self) -> u64 {
        self.cached_hash
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        // Three lines: name + status row, args preview row, blank trailing.
        3
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let theme = ctx.theme();
        let status_glyph = status_glyph(self.status);
        let status_color = status_color(self.status, theme);

        // Line 1: "🔧 <name>  <status_glyph>"
        // Line 2: args preview (indented under the border).
        let header = format!("🔧 {}  {}", self.name, status_glyph);
        let header_style = Style::default()
            .fg(theme.colors.tool)
            .add_modifier(Modifier::BOLD);
        let status_style = Style::default().fg(status_color);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        // Encode the header on the top border title and the status color
        // by patching the block style; the preview line goes inside.
        block = block.title(header.clone()).title_style(header_style);
        block.render(area, ctx.buffer_mut());

        // The inner area sits between the borders. Lay the args preview
        // on the second visible line.
        let inner_y = area.y.saturating_add(1);
        if inner_y >= area.y.saturating_add(area.height) {
            return;
        }
        let line = ratatui::text::Line::from(ratatui::text::Span::styled(
            format!(" {}", self.args_preview),
            status_style,
        ));
        ctx.buffer_mut().set_line(
            area.x.saturating_add(1),
            inner_y,
            &line,
            area.width.saturating_sub(2),
        );
    }
}

fn status_glyph(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Pending => "…",
        ToolCallStatus::Running => "⏳",
        ToolCallStatus::Completed => "✔",
        ToolCallStatus::Failed => "✘",
    }
}

fn status_color(status: ToolCallStatus, theme: &crate::theme::Theme) -> Color {
    match status {
        ToolCallStatus::Pending => theme.colors.muted,
        ToolCallStatus::Running => theme.colors.info,
        ToolCallStatus::Completed => theme.colors.success,
        ToolCallStatus::Failed => theme.colors.error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn render_tool_call(tc: &mut ToolCall, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let theme = Theme::dark();
            let caps = TerminalCaps::default();
            let mut ctx = RenderCtx::new(frame, &theme, &caps);
            tc.render(
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
    fn tool_call_pending_renders_dots() {
        let mut tc = ToolCall::new(
            "call-1".into(),
            "search".into(),
            ToolCallStatus::Pending,
            "{}",
        );
        let buf = render_tool_call(&mut tc, 30, 3);
        // Title should contain the name and the pending glyph (…).
        let top: String = (0..30).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            top.contains("search"),
            "title should contain tool name, got {top:?}"
        );
        assert!(
            top.contains('…'),
            "pending should render … glyph, got {top:?}"
        );
        // The block is drawn: corners are box-drawing chars.
        assert_eq!(buf[(0, 0)].symbol(), "┌");
        assert_eq!(buf[(29, 0)].symbol(), "┐");
    }

    #[test]
    fn tool_call_completed_renders_status() {
        let mut tc = ToolCall::new(
            "call-2".into(),
            "search".into(),
            ToolCallStatus::Completed,
            "{}",
        );
        let buf = render_tool_call(&mut tc, 30, 3);
        let top: String = (0..30).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            top.contains('✔'),
            "completed should render ✔ glyph, got {top:?}"
        );
    }

    #[test]
    fn tool_call_running_renders_hourglass() {
        let mut tc = ToolCall::new(
            "call-3".into(),
            "search".into(),
            ToolCallStatus::Running,
            "{}",
        );
        let buf = render_tool_call(&mut tc, 30, 3);
        let top: String = (0..30).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            top.contains('⏳'),
            "running should render ⏳ glyph, got {top:?}"
        );
    }

    #[test]
    fn tool_call_hash_changes_on_status() {
        let pending = ToolCall::new("a".into(), "n".into(), ToolCallStatus::Pending, "{}");
        let completed = ToolCall::new("a".into(), "n".into(), ToolCallStatus::Completed, "{}");
        assert_ne!(pending.content_hash(), completed.content_hash());
    }

    #[test]
    fn tool_call_hash_changes_on_args() {
        let a = ToolCall::new("a".into(), "n".into(), ToolCallStatus::Completed, "{foo:1}");
        let b = ToolCall::new("a".into(), "n".into(), ToolCallStatus::Completed, "{foo:2}");
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn tool_call_truncates_long_args() {
        let long = "x".repeat(ARGS_PREVIEW_LIMIT + 20);
        let tc = ToolCall::new("a".into(), "n".into(), ToolCallStatus::Completed, &long);
        assert!(tc.args_preview.ends_with('…'));
        assert!(tc.args_preview.chars().count() <= ARGS_PREVIEW_LIMIT + 1);
    }

    #[test]
    fn tool_call_set_status_updates_hash() {
        let mut tc = ToolCall::new("a".into(), "n".into(), ToolCallStatus::Pending, "{}");
        let before = tc.content_hash();
        tc.set_status(ToolCallStatus::Completed);
        assert_ne!(before, tc.content_hash());
    }
}
