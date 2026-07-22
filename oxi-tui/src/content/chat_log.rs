use super::{ChatMessage, MessageId, MessageRole, StreamId, StreamingState};
use crate::widget::{hash_combine, hash_str};

/// Append-only conversation history with an optional assistant stream.
#[derive(Debug)]
pub struct ChatLog {
    messages: Vec<ChatMessage>,
    next_id: MessageId,
    active_stream: StreamingState,
    cached_hash: u64,
}

impl Default for ChatLog {
    fn default() -> Self {
        let mut log = Self {
            messages: Vec::new(),
            next_id: 0,
            active_stream: StreamingState::default(),
            cached_hash: 0,
        };
        log.refresh_hash();
        log
    }
}

impl ChatLog {
    /// Creates an empty chat log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a message and returns its monotonically assigned identifier.
    /// Assistant messages become the active streamed message.
    #[must_use]
    pub fn append_message(&mut self, role: MessageRole) -> MessageId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.messages.push(ChatMessage::new(id, role));

        if role == MessageRole::Assistant {
            self.active_stream.start(id);
        }
        self.refresh_hash();
        id
    }

    /// Appends a token to the active assistant message, creating one when
    /// there is no active assistant stream.
    pub fn append_token(&mut self, token: &str) {
        let message_index = self.active_stream.active_stream.and_then(|stream_id| {
            self.messages.iter().rposition(|message| {
                message.id == stream_id && message.role == MessageRole::Assistant
            })
        });

        let message_index = if let Some(index) = message_index {
            index
        } else {
            let _ = self.append_message(MessageRole::Assistant);
            self.messages.len().saturating_sub(1)
        };

        if let Some(message) = self.messages.get_mut(message_index) {
            message.append_text(token);
        }
        self.active_stream.push_token(token);
        self.refresh_hash();
    }

    /// Finishes the active stream, retaining its accumulated message text.
    pub fn finalize_stream(&mut self) {
        self.active_stream.finalize();
        self.refresh_hash();
    }

    /// Returns all messages in append order.
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Returns the identifier of the currently streaming assistant message.
    #[must_use]
    pub fn active_stream(&self) -> Option<StreamId> {
        self.active_stream.active_stream
    }

    /// Returns the cached content hash in O(1) time.
    #[must_use]
    pub const fn content_hash(&self) -> u64 {
        self.cached_hash
    }

    fn refresh_hash(&mut self) {
        let message_count = u64::try_from(self.messages.len()).unwrap_or(u64::MAX);
        let last_message_hash = self.messages.last().map_or(0, ChatMessage::content_hash);
        let stream_id_hash = match self.active_stream.active_stream {
            Some(stream_id) => hash_combine(1, stream_id),
            None => 0,
        };
        let partial_hash = hash_str(&self.active_stream.partial_text);
        let messages_hash = hash_combine(message_count, last_message_hash);
        let stream_hash = hash_combine(stream_id_hash, partial_hash);
        self.cached_hash = hash_combine(messages_hash, stream_hash);
    }
}

#[cfg(test)]
mod tests {
    use super::ChatLog;
    use crate::content::{ContentBlock, MessageRole};

    #[test]
    fn append_message_assigns_incrementing_ids() {
        let mut log = ChatLog::new();
        let first = log.append_message(MessageRole::User);
        let second = log.append_message(MessageRole::System);

        assert_eq!(first, 0);
        assert_eq!(second, 1);
        assert_eq!(log.messages()[0].id, first);
        assert_eq!(log.messages()[1].id, second);
    }

    #[test]
    fn append_token_creates_assistant_if_none() {
        let mut log = ChatLog::new();
        log.append_token("hello");

        assert_eq!(log.messages().len(), 1);
        assert_eq!(log.messages()[0].role, MessageRole::Assistant);
        assert_eq!(
            log.messages()[0].blocks,
            [ContentBlock::Text("hello".to_owned())]
        );
    }

    #[test]
    fn append_token_appends_to_last_text() {
        let mut log = ChatLog::new();
        let _ = log.append_message(MessageRole::Assistant);
        log.append_token("hel");
        log.append_token("lo");

        assert_eq!(
            log.messages()[0].blocks,
            [ContentBlock::Text("hello".to_owned())]
        );
    }

    #[test]
    fn finalize_stream_clears() {
        let mut log = ChatLog::new();
        let id = log.append_message(MessageRole::Assistant);
        assert_eq!(log.active_stream(), Some(id));

        log.finalize_stream();

        assert_eq!(log.active_stream(), None);
    }

    #[test]
    fn content_hash_changes_on_append() {
        let mut log = ChatLog::new();
        let before = log.content_hash();

        let _ = log.append_message(MessageRole::Assistant);
        log.append_token("hello");

        assert_ne!(before, log.content_hash());
    }
}
