use super::ChatLog;
use crate::widget::hash_combine;

/// Whether the chat viewport tracks the newest messages or remains scrolled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowMode {
    /// Keep the viewport anchored to the end of the chat log.
    #[default]
    Bottom,
    /// Preserve the user's current position while messages are appended.
    Pinned,
}

/// A text selection within a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Index of the message containing the selection's start.
    pub start_message: usize,
    /// Character offset of the selection's start within that message.
    pub start_offset: usize,
    /// Index of the message containing the selection's end.
    pub end_message: usize,
    /// Character offset of the selection's end within that message.
    pub end_offset: usize,
}

/// Scroll, follow, and text-selection state for a chat viewport.
#[derive(Debug, Default)]
pub struct ChatView {
    scroll_offset: u32,
    follow_mode: FollowMode,
    selection: Option<Selection>,
    viewport_height_cache: u16,
}

impl ChatView {
    /// Resumes following the newest messages.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.follow_mode = FollowMode::Bottom;
    }

    /// Scrolls toward older messages and pins the viewport.
    pub fn scroll_up(&mut self, lines: u32) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
        self.follow_mode = FollowMode::Pinned;
    }

    /// Scrolls toward newer messages, resuming follow mode at the bottom.
    pub fn scroll_down(&mut self, lines: u32) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        if self.scroll_offset == 0 {
            self.follow_mode = FollowMode::Bottom;
        }
    }

    /// Returns the current follow behavior.
    #[must_use]
    pub const fn follow_mode(&self) -> FollowMode {
        self.follow_mode
    }

    /// Returns the active text selection, if any.
    #[must_use]
    pub const fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// Records the viewport height used by [`Self::viewport_hash`].
    pub const fn set_viewport_height(&mut self, height: u16) {
        self.viewport_height_cache = height;
    }

    /// Returns the half-open message-index range intersecting the viewport.
    ///
    /// Until message layout is available, each message occupies one virtual
    /// row. `scroll_offset` is measured upward from the newest message.
    #[must_use]
    pub fn visible_msg_range(&self, log: &ChatLog, viewport_h: u16) -> (usize, usize) {
        let len = log.messages().len();
        let offset = usize::try_from(self.scroll_offset).unwrap_or(usize::MAX);
        let end = len.saturating_sub(offset.min(len));
        let start = end.saturating_sub(usize::from(viewport_h));
        (start, end)
    }

    /// Returns a hash of the current viewport coordinates and visible content.
    #[must_use]
    pub fn viewport_hash(&self, log: &ChatLog, width: u16) -> u64 {
        let (start, end) = self.visible_msg_range(log, self.viewport_height_cache);
        let mut hash = hash_combine(u64::from(self.scroll_offset), u64::from(width));
        hash = hash_combine(hash, u64::try_from(start).unwrap_or(u64::MAX));
        hash = hash_combine(hash, u64::try_from(end).unwrap_or(u64::MAX));

        for message in &log.messages()[start..end] {
            hash = hash_combine(hash, message.content_hash());
        }

        hash
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatView, FollowMode};
    use crate::content::{ChatLog, MessageRole};

    fn log_with_messages(count: usize) -> ChatLog {
        let mut log = ChatLog::new();
        for index in 0..count {
            let role = if index % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::System
            };
            let _ = log.append_message(role);
        }
        log
    }

    #[test]
    fn follow_starts_bottom() {
        let view = ChatView::default();

        assert_eq!(view.follow_mode(), FollowMode::Bottom);
    }

    #[test]
    fn scroll_up_pinned() {
        let mut view = ChatView::default();

        view.scroll_up(1);

        assert_eq!(view.follow_mode(), FollowMode::Pinned);
    }

    #[test]
    fn scroll_to_bottom_switches_to_follow() {
        let mut view = ChatView::default();
        view.scroll_up(2);

        view.scroll_to_bottom();

        assert_eq!(view.follow_mode(), FollowMode::Bottom);
    }

    #[test]
    fn scroll_down_to_bottom_resumes_follow() {
        let mut view = ChatView::default();
        view.scroll_up(3);

        view.scroll_down(3);

        assert_eq!(view.follow_mode(), FollowMode::Bottom);
    }

    #[test]
    fn scroll_down_preserves_pinned_until_bottom() {
        let mut view = ChatView::default();
        view.scroll_up(3);

        view.scroll_down(2);

        assert_eq!(view.follow_mode(), FollowMode::Pinned);
    }

    #[test]
    fn scroll_up_saturates() {
        let mut view = ChatView::default();

        view.scroll_up(u32::MAX);
        view.scroll_up(1);

        assert_eq!(view.visible_msg_range(&log_with_messages(2), 1), (0, 0));
    }

    #[test]
    fn visible_range_bottom() {
        let log = log_with_messages(5);
        let view = ChatView::default();

        assert_eq!(view.visible_msg_range(&log, 2), (3, 5));
    }

    #[test]
    fn viewport_hash_changes_on_scroll() {
        let log = log_with_messages(5);
        let mut view = ChatView::default();
        view.set_viewport_height(2);
        assert_eq!(view.visible_msg_range(&log, 2), (3, 5));
        let bottom_hash = view.viewport_hash(&log, 80);

        view.scroll_up(1);
        assert_eq!(view.visible_msg_range(&log, 2), (2, 4));
        let scrolled_hash = view.viewport_hash(&log, 80);

        assert_ne!(bottom_hash, scrolled_hash);
    }

    #[test]
    fn viewport_hash_changes_with_visible_content() {
        let mut log = log_with_messages(1);
        let mut view = ChatView::default();
        view.set_viewport_height(1);
        let empty_message_hash = view.viewport_hash(&log, 80);

        log.append_token("updated");

        assert_ne!(empty_message_hash, view.viewport_hash(&log, 80));
    }

    #[test]
    fn viewport_hash_changes_with_width() {
        let log = log_with_messages(2);
        let view = ChatView::default();

        assert_ne!(view.viewport_hash(&log, 40), view.viewport_hash(&log, 80));
    }
}
