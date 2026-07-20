// MarkdownStreaming — line-by-line markdown cache.
// Thin wrapper around the existing oxi-tui markdown renderer.

/// A line-cached markdown streaming renderer.
#[derive(Default)]
pub struct MarkdownStreaming {
    buffer: String,
}

impl MarkdownStreaming {
    /// Push new tokens and return freshly-completed lines.
    pub fn push(&mut self, _token: &str) -> Vec<String> {
        // PR-7+ uses pulldown-cmark to detect completed lines.
        // For now: just accumulate and return nothing.
        let _ = &self.buffer;
        Vec::new()
    }

    /// Reset the buffer (for new assistant message).
    pub fn reset(&mut self) {
        self.buffer.clear();
    }
}
