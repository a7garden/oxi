// TokenBar — 1-line footer showing model, tokens, cost.

use crate::status::StatusState;
use crate::widgets::spinner::spinner_frame;

/// Render a one-line token bar summary.
pub fn token_bar_line(state: &StatusState) -> String {
    let model = state.model.as_deref().unwrap_or("?");
    let spinner = spinner_frame(state.spinner_phase);
    format!(
        "{spinner} {model} | in:{} out:{} | ${:.6}",
        state.tokens_in, state.tokens_out, state.cost
    )
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn token_bar_line_includes_model() {
        let mut s = StatusState::default();
        s.model = Some("gpt-4".into());
        s.tokens_in = 100;
        s.tokens_out = 50;
        s.cost = 0.002;
        let line = token_bar_line(&s);
        assert!(line.contains("gpt-4"));
        assert!(line.contains("100"));
        assert!(line.contains("50"));
    }
}
