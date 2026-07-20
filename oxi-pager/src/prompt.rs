// PromptState — tracks user input, cursor, completion.

#[derive(Debug, Clone, Default)]
pub struct PromptState {
    pub text: String,
    pub cursor: usize,
    pub history_cursor: Option<usize>,
}

impl PromptState {
    pub fn cursor_left(&mut self) -> bool {
        if self.cursor > 0 { self.cursor -= 1; true } else { false }
    }

    pub fn cursor_right(&mut self) -> bool {
        if self.cursor < self.text.len() { self.cursor += 1; true } else { false }
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn delete_before(&mut self) -> bool {
        if self.cursor == 0 { return false; }
        let prev = self.text[..self.cursor].chars().next_back().map(|c| c.len_utf8()).unwrap_or(1);
        self.text.drain(self.cursor - prev..self.cursor);
        self.cursor -= prev;
        true
    }

    pub fn delete_after(&mut self) -> bool {
        if self.cursor >= self.text.len() { return false; }
        let next = self.text[self.cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        self.text.drain(self.cursor..self.cursor + next);
        true
    }

    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_adds_char_at_cursor() {
        let mut p = PromptState::default();
        p.insert('a');
        assert_eq!(p.text, "a");
        assert_eq!(p.cursor, 1);
    }

    #[test]
    fn submit_clears_text() {
        let mut p = PromptState::default();
        p.insert('x');
        let text = p.submit();
        assert_eq!(text, "x");
        assert!(p.text.is_empty());
    }
}
