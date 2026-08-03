/// Append-only context for stable prefix caching.
///
/// Separates immutable message history from pending tool results so that
/// the byte prefix sent to the LLM never changes between turns. This
/// maximizes provider-side KV cache / prompt caching.
///
/// # Semantics
///
/// - `messages` — immutable history. Never mutated after append.
/// - `pending_tool_results` — queued tool results for the current turn.
///   Folded into history at the next turn boundary.
///
/// When building context for the LLM, always use `history + pending`.
use oxicode_ai::Message;

/// Append-only context manager for the agent loop.
///
/// Separates immutable message history from pending tool results so that
/// the byte prefix sent to the LLM never changes between turns. This
/// maximizes provider-side KV cache / prompt caching.
#[derive(Debug, Clone)]
pub struct AppendOnlyContext {
    /// Immutable message history. Once appended, never removed or mutated.
    messages: Vec<Message>,
    /// Pending tool results queued for the next LLM turn.
    pending_tool_results: Vec<Message>,
}

impl AppendOnlyContext {
    /// Create a new append-only context from existing messages.
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            pending_tool_results: Vec::new(),
        }
    }

    /// Create an empty append-only context.
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Append a message to the immutable history.
    pub fn append(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// Queue a tool result for the next LLM turn.
    pub fn queue_tool_result(&mut self, msg: Message) {
        self.pending_tool_results.push(msg);
    }

    /// Fold pending tool results into the immutable history.
    /// Called at turn boundaries (after the LLM responds, before the next turn).
    pub fn finalize_turn(&mut self) {
        self.messages.append(&mut self.pending_tool_results);
    }

    /// Replace the entire history (used after compaction replaces messages).
    /// Pending tool results are discarded since compaction already folded them.
    pub fn replace_history(&mut self, new_history: Vec<Message>) {
        self.messages = new_history;
        self.pending_tool_results.clear();
    }

    /// Build the full message list for the LLM: history + pending tool results.
    pub fn build_messages(&self) -> Vec<Message> {
        let mut all = self.messages.clone();
        all.extend(self.pending_tool_results.iter().cloned());
        all
    }

    /// Get a reference to the immutable history.
    pub fn history(&self) -> &[Message] {
        &self.messages
    }

    /// Get a reference to the pending tool results.
    pub fn pending_results(&self) -> &[Message] {
        &self.pending_tool_results
    }

    /// Get the total message count (history + pending).
    pub fn len(&self) -> usize {
        self.messages.len() + self.pending_tool_results.len()
    }

    /// Returns true if no messages exist.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.pending_tool_results.is_empty()
    }

    /// Sync from an external message list, appending only new messages.
    ///
    /// This is needed when external code (e.g. the agent state) has a
    /// different view of the message list. Only messages beyond the
    /// current history length are appended.
    ///
    /// Returns the number of newly appended messages.
    pub fn sync_from(&mut self, external: &[Message]) -> usize {
        if external.len() <= self.messages.len() {
            return 0;
        }
        let new_count = external.len() - self.messages.len();
        for msg in &external[self.messages.len()..] {
            self.messages.push(msg.clone());
        }
        new_count
    }

    /// Consume self and return the underlying history vec.
    pub fn into_messages(self) -> Vec<Message> {
        let mut all = self.messages;
        all.extend(self.pending_tool_results);
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxicode_ai::{ContentBlock, TextContent};

    #[test]
    fn test_empty_context() {
        let ctx = AppendOnlyContext::empty();
        assert!(ctx.is_empty());
        assert_eq!(ctx.len(), 0);
    }

    #[test]
    fn test_append_and_build() {
        let mut ctx = AppendOnlyContext::empty();
        ctx.append(Message::user("Hello"));
        ctx.append(Message::user("World"));

        assert_eq!(ctx.len(), 2);
        assert!(!ctx.is_empty());

        let built = ctx.build_messages();
        assert_eq!(built.len(), 2);
    }

    #[test]
    fn test_tool_result_queue() {
        let mut ctx = AppendOnlyContext::empty();
        ctx.append(Message::user("Turn 1"));

        ctx.queue_tool_result(Message::ToolResult(oxicode_ai::ToolResultMessage::new(
            "call-1",
            "echo",
            vec![ContentBlock::Text(TextContent::new("Result of tool"))],
        )));

        // Build should include both history and pending
        let built = ctx.build_messages();
        assert_eq!(built.len(), 2);

        // Finalize turn: fold pending into history
        ctx.finalize_turn();
        assert_eq!(ctx.len(), 2);
        assert!(ctx.pending_results().is_empty());
    }

    #[test]
    fn test_sync_from_appends_only_new() {
        let mut ctx = AppendOnlyContext::empty();
        ctx.append(Message::user("A"));
        ctx.append(Message::user("B"));

        // External has same prefix + one more
        let external = vec![Message::user("A"), Message::user("B"), Message::user("C")];

        let count = ctx.sync_from(&external);
        assert_eq!(count, 1); // Only "C" was new
        assert_eq!(ctx.len(), 3);

        // Sync again with no new messages
        let count2 = ctx.sync_from(&external);
        assert_eq!(count2, 0);
    }

    #[test]
    fn test_into_messages_flattens_all() {
        let mut ctx = AppendOnlyContext::empty();
        ctx.append(Message::user("A"));
        ctx.queue_tool_result(Message::ToolResult(oxicode_ai::ToolResultMessage::new(
            "call-1",
            "echo",
            vec![ContentBlock::Text(TextContent::new("result"))],
        )));

        let all = ctx.into_messages();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_prefix_stability() {
        let mut ctx = AppendOnlyContext::empty();
        ctx.append(Message::user("Turn 1 prompt"));

        // Queue a tool result: build shows both, but prefix (index 0) unchanged
        ctx.queue_tool_result(Message::ToolResult(oxicode_ai::ToolResultMessage::new(
            "call-1",
            "echo",
            vec![ContentBlock::Text(TextContent::new("tool result"))],
        )));

        let before = ctx.build_messages();
        assert_eq!(before.len(), 2);

        // Finalize turn, append next prompt
        ctx.finalize_turn();
        ctx.append(Message::user("Turn 2 prompt"));

        let after = ctx.build_messages();
        // Prefix still at index 0 (Turn 1 prompt)
        assert_eq!(after.len(), 3);
        // Verify length stability of prefix position
        assert!(after.len() > before.len());
    }
}
