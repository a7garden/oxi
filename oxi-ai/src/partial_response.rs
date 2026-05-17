//! Partial response accumulator for stream recovery.

/// Partial response accumulator.
///
/// Collects streamed text and thinking deltas so they can be recovered
/// if the stream terminates unexpectedly.
#[derive(Debug, Default)]
pub struct PartialResponse {
    /// Accumulated response text.
    text: String,
    /// Accumulated thinking / reasoning text.
    thinking: String,
    /// Whether any thinking content has been received.
    has_thinking: bool,
}

impl PartialResponse {
    /// Create a new empty partial response.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a text delta to the accumulated response.
    pub fn push_text(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    /// Append a thinking delta to the accumulated reasoning.
    pub fn push_thinking(&mut self, delta: &str) {
        self.has_thinking = true;
        self.thinking.push_str(delta);
    }

    /// Take ownership of the accumulated text, leaving an empty string behind.
    pub fn take_text(&mut self) -> String {
        std::mem::take(&mut self.text)
    }

    /// Access the accumulated response text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Access the accumulated thinking / reasoning text.
    pub fn thinking(&self) -> &str {
        &self.thinking
    }

    /// Returns `true` if any thinking content has been received.
    pub fn has_thinking(&self) -> bool {
        self.has_thinking
    }

    /// Returns `true` if no text or thinking has been accumulated.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.thinking.is_empty()
    }

    /// Clear all accumulated content.
    pub fn clear(&mut self) {
        self.text.clear();
        self.thinking.clear();
        self.has_thinking = false;
    }
}
