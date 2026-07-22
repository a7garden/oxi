use std::time::Instant;

pub type MessageId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContentBlock {
    Text(String),
    Thinking(String),
    ToolCall {
        id: String,
        name: String,
        args: String,
        status: ToolCallStatus,
    },
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCallStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: MessageId,
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
    pub created_at: Instant,
}

impl ChatMessage {
    #[must_use]
    pub fn new(id: MessageId, role: MessageRole) -> Self {
        Self {
            id,
            role,
            blocks: Vec::new(),
            created_at: Instant::now(),
        }
    }

    #[must_use]
    pub fn text_content(&self) -> Option<&str> {
        self.blocks.iter().find_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
    }

    pub fn append_text(&mut self, text: &str) {
        if let Some(ContentBlock::Text(content)) = self.blocks.last_mut() {
            content.push_str(text);
        } else {
            self.blocks.push(ContentBlock::Text(text.to_owned()));
        }
    }

    #[must_use]
    pub fn content_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.role.hash(&mut hasher);
        self.blocks.hash(&mut hasher);

        crate::widget::hash_combine(self.id, hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatMessage, ContentBlock, MessageRole, ToolCallStatus};

    #[test]
    fn append_text_creates_a_text_block_when_empty() {
        let mut message = ChatMessage::new(1, MessageRole::Assistant);
        message.append_text("hello");
        assert_eq!(message.blocks, [ContentBlock::Text("hello".into())]);
        assert_eq!(message.text_content(), Some("hello"));
    }

    #[test]
    fn append_text_extends_the_last_text_block() {
        let mut message = ChatMessage::new(1, MessageRole::Assistant);
        message.blocks.push(ContentBlock::Text("hel".into()));
        message.append_text("lo");
        assert_eq!(message.blocks, [ContentBlock::Text("hello".into())]);
    }

    #[test]
    fn append_text_creates_a_block_after_non_text_content() {
        let mut message = ChatMessage::new(1, MessageRole::Assistant);
        message.blocks.push(ContentBlock::ToolCall {
            id: "call-1".into(),
            name: "search".into(),
            args: "{}".into(),
            status: ToolCallStatus::Completed,
        });
        message.append_text("done");
        assert_eq!(message.blocks.len(), 2);
        assert_eq!(message.blocks[1], ContentBlock::Text("done".into()));
    }
}
