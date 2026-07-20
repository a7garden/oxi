// ScrollbackState — message history with block tracking.

pub struct RenderedBlock {
    pub id: u64,
    pub kind: BlockKind,
    pub text: String,
    pub lines: Vec<ratatui::text::Line<'static>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    User,
    Assistant,
    ToolCall { name: String, call_id: String },
    ToolResult { call_id: String },
    System,
    Thinking,
    Error(String),
}

pub struct ScrollbackState {
    pub blocks: Vec<RenderedBlock>,
    pub next_id: u64,
    pub follow_tail: bool,
    pub scroll_offset: usize,
}

impl Default for ScrollbackState {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            next_id: 1,
            follow_tail: true,
            scroll_offset: 0,
        }
    }
}

impl ScrollbackState {
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn begin_assistant(&mut self) -> u64 {
        let id = self.alloc_id();
        self.blocks.push(RenderedBlock {
            id,
            kind: BlockKind::Assistant,
            text: String::new(),
            lines: Vec::new(),
        });
        id
    }

    pub fn append_token(&mut self, chunk: &str) {
        if let Some(block) = self
            .blocks
            .last_mut()
            .filter(|b| b.kind == BlockKind::Assistant)
        {
            block.text.push_str(chunk);
        }
    }

    pub fn end_assistant(&mut self) {}

    pub fn begin_tool_call(&mut self, name: &str, call_id: &str) -> u64 {
        let id = self.alloc_id();
        self.blocks.push(RenderedBlock {
            id,
            kind: BlockKind::ToolCall {
                name: name.to_string(),
                call_id: call_id.to_string(),
            },
            text: String::new(),
            lines: Vec::new(),
        });
        id
    }

    pub fn end_tool_call(&mut self, _call_id: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_assistant_creates_block() {
        let mut s = ScrollbackState::default();
        let id = s.begin_assistant();
        assert_eq!(id, 1);
        assert_eq!(s.blocks.len(), 1);
        assert_eq!(s.blocks[0].kind, BlockKind::Assistant);
    }

    #[test]
    fn append_token_adds_to_last_block() {
        let mut s = ScrollbackState::default();
        s.begin_assistant();
        s.append_token("Hello ");
        s.append_token("world");
        assert_eq!(s.blocks[0].text, "Hello world");
    }
}
