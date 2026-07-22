//! Chat widgets: per-message item, tool-call card, animated spinner, and the
//! scrollable [`ChatView`].

pub mod message_item;
pub mod spinner;
pub mod tool_call;

use ratatui::layout::Rect;

use crate::content::{ChatLog, ChatViewState};
use crate::widget::{RenderCtx, Renderable, RetainedChild, hash_combine};

pub use message_item::MessageItem;
pub use spinner::Spinner;
pub use tool_call::ToolCall;

/// A virtualized, retained chat transcript.
///
/// Messages are kept in stable [`RetainedChild`] slots. During streaming the
/// active message's hash changes, while unchanged siblings retain their last
/// rendered hash and are skipped.
#[derive(Debug, Default)]
pub struct ChatView {
    log: ChatLog,
    view: ChatViewState,
    items: Vec<RetainedChild<message_item::MessageItem>>,
}

impl ChatView {
    /// Creates an empty chat view.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the conversation log.
    #[must_use]
    pub const fn log(&self) -> &ChatLog {
        &self.log
    }

    /// Returns the conversation log for mutation.
    pub const fn log_mut(&mut self) -> &mut ChatLog {
        &mut self.log
    }

    /// Returns the viewport state.
    #[must_use]
    pub const fn view(&self) -> &ChatViewState {
        &self.view
    }

    /// Returns the viewport state for mutation.
    pub const fn view_mut(&mut self) -> &mut ChatViewState {
        &mut self.view
    }

    /// Synchronizes retained message widgets with newly appended messages.
    fn sync_items(&mut self) {
        let messages = self.log.messages();
        while self.items.len() < messages.len() {
            let index = self.items.len();
            self.items
                .push(RetainedChild::new(message_item::MessageItem::new(
                    messages[index].clone(),
                )));
        }
    }
}

impl Renderable for ChatView {
    fn content_hash(&self) -> u64 {
        // The width is a default because the retained-tree pass has no area.
        // The render pass uses the actual width when laying out each child.
        hash_combine(
            self.log.content_hash(),
            self.view.viewport_hash(&self.log, 80),
        )
    }

    fn height_for(&self, _width: u16, _ctx: &RenderCtx) -> u16 {
        let (start, end) = self.view.visible_msg_range(&self.log, 24);
        u16::try_from(end.saturating_sub(start)).unwrap_or(u16::MAX)
    }

    fn render(&mut self, area: Rect, ctx: &mut RenderCtx) {
        self.view.set_viewport_height(area.height);
        self.sync_items();
        let (start, end) = self.view.visible_msg_range(&self.log, area.height);
        let mut y = area.y;
        for index in start..end {
            if index >= self.items.len() {
                break;
            }
            self.items[index]
                .inner_mut()
                .sync_from(&self.log.messages()[index]);
            let height = self.items[index].inner().height_for(area.width, ctx);
            let remaining = area.y.saturating_add(area.height).saturating_sub(y);
            let item_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: height.min(remaining),
            };
            self.items[index].render_if_changed(item_area, ctx);
            y = y.saturating_add(height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::MessageRole;
    use crate::theme::{TerminalCaps, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(view: &mut ChatView, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let theme = Theme::dark();
                let caps = TerminalCaps::default();
                let mut ctx = RenderCtx::new(frame, &theme, &caps);
                view.render(ctx.area(), &mut ctx);
            })
            .unwrap();
    }

    #[test]
    fn streaming_token_re_renders_only_active_message() {
        let mut view = ChatView::new();
        view.log_mut().append_message(MessageRole::User);
        view.log_mut().append_message(MessageRole::User);
        view.log_mut().append_message(MessageRole::Assistant);
        view.log_mut().append_token("token");
        render(&mut view, 80, 24);
        let before: Vec<usize> = view
            .items
            .iter()
            .map(|item| item.inner().render_count())
            .collect();
        view.log_mut().append_token(" more");
        render(&mut view, 80, 24);
        let after: Vec<usize> = view
            .items
            .iter()
            .map(|item| item.inner().render_count())
            .collect();
        assert_eq!(after[0], before[0]);
        assert_eq!(after[1], before[1]);
        assert_eq!(after[2], before[2] + 1);
    }

    #[test]
    fn renders_visible_messages_only() {
        let mut view = ChatView::new();
        for _ in 0..5 {
            view.log_mut().append_message(MessageRole::User);
        }
        render(&mut view, 80, 2);
        assert_eq!(
            view.items
                .iter()
                .filter(|item| item.inner().render_count() > 0)
                .count(),
            2
        );
    }

    #[test]
    fn scroll_skips_offscreen_messages() {
        let mut view = ChatView::new();
        for _ in 0..5 {
            view.log_mut().append_message(MessageRole::User);
        }
        view.view_mut().scroll_up(2);
        render(&mut view, 80, 2);
        let rendered = view
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| (item.inner().render_count() > 0).then_some(i))
            .collect::<Vec<_>>();
        assert_eq!(rendered, vec![1, 2]);
    }

    #[test]
    fn new_message_appended_correctly() {
        let mut view = ChatView::new();
        view.log_mut().append_message(MessageRole::User);
        render(&mut view, 80, 2);
        view.log_mut().append_message(MessageRole::Assistant);
        assert_eq!(view.items.len(), 1);
        render(&mut view, 80, 2);
        assert_eq!(view.items.len(), 2);
    }
}
