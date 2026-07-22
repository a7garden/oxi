pub type StreamId = u64;

#[derive(Debug, Clone, Default)]
pub struct StreamingState {
    pub active_stream: Option<StreamId>,
    pub partial_text: String,
}

impl StreamingState {
    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        self.active_stream.is_some()
    }

    pub fn start(&mut self, id: StreamId) {
        self.active_stream = Some(id);
        self.partial_text.clear();
    }

    pub fn push_token(&mut self, token: &str) {
        self.partial_text.push_str(token);
    }

    pub fn finalize(&mut self) -> String {
        self.active_stream = None;
        std::mem::take(&mut self.partial_text)
    }
}

#[cfg(test)]
mod tests {
    use super::StreamingState;

    #[test]
    fn streaming_lifecycle_resets_collects_and_finalizes_text() {
        let mut state = StreamingState::default();
        assert!(!state.is_streaming());

        state.partial_text.push_str("stale");
        state.start(42);
        assert!(state.is_streaming());
        assert_eq!(state.active_stream, Some(42));
        assert!(state.partial_text.is_empty());

        state.push_token("hel");
        state.push_token("lo");
        assert_eq!(state.finalize(), "hello");
        assert!(!state.is_streaming());
        assert!(state.partial_text.is_empty());
    }
}
