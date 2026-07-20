// Slash dispatch — routes /-prefixed input to oxi-cli's slash registry.

#[derive(Debug)]
pub enum SlashDecision {
    /// Forward the entire raw text to oxi-cli's slash dispatcher.
    Dispatch(String),
    /// The text does not start with `/` — treat as plain message.
    Unknown(String),
}

/// Route slash-prefixed input.
pub fn route_slash(text: &str) -> SlashDecision {
    if text.starts_with('/') {
        SlashDecision::Dispatch(text.to_string())
    } else {
        SlashDecision::Unknown(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_command_is_dispatched() {
        let result = route_slash("/model");
        assert!(matches!(result, SlashDecision::Dispatch(s) if s == "/model"));
    }

    #[test]
    fn plain_text_is_unknown() {
        let result = route_slash("hello");
        assert!(matches!(result, SlashDecision::Unknown(s) if s == "hello"));
    }
}
