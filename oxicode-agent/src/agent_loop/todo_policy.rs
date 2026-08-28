//! Eager todo-list creation policy for the first agent turn. Ports the
//! eager-prelude half of omp's `TodoTracker` (`todo-tracker.ts`); the
//! reminders/mid-run-nudge half lives in `agent_loop/mod.rs` next to
//! `build_stop_reminder`/`MidRunNudgeState` to avoid a second todo-state
//! owner.

use oxicode_ai::{Api, Message, ToolChoice, UserMessage};

/// Mirrors `oxicode_cli`'s `Settings::TodoEagerMode` without a crate
/// dependency in the other direction; `oxicode-cli` converts when building
/// `AgentLoopConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum TodoEagerMode {
    /// No automatic todo prelude (preserves today's behavior).
    #[default]
    Off,
    /// Inject a hidden "create a todo plan" message, but never force the
    /// `todo` tool call.
    Preferred,
    /// Inject the hidden message AND force the `todo` tool call when the
    /// provider supports native forced tool choice.
    Always,
}

const QUESTION_PROMPT_PREFIXES: &[&str] = &[
    "what", "which", "when", "where", "why", "how", "who", "whom", "whose", "do", "does", "did",
    "can", "could", "would", "will", "should", "is", "are", "am", "may", "shall",
];

/// Whether `text` reads as a question rather than a task request. Ports
/// omp's `QUESTION_PROMPT_RE` + non-ASCII fallback (`todo-tracker.ts:24-30`).
pub(crate) fn looks_like_a_question(text: &str) -> bool {
    let trimmed = text.trim_end();
    if !(trimmed.ends_with('?') || trimmed.ends_with('!')) {
        return false;
    }
    // Non-ASCII prose ending in "?"/"!" is treated as a genuine question
    // regardless of the English word list (CJK, Spanish "¿…?", etc.) — the
    // punctuation alone is the reliable signal there.
    if !trimmed.is_ascii() {
        return true;
    }
    let first_word = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    QUESTION_PROMPT_PREFIXES.contains(&first_word.as_str())
}

/// Whether a provider's wire format supports native forced-tool-choice
/// (`ToolChoice::Named`). The built-in JSON providers do; Ollama and owned
/// (in-band XML) dialects don't, so a `Named` choice degrades to `Auto` there.
pub fn provider_supports_tool_choice(api: Api) -> bool {
    matches!(
        api,
        Api::OpenAiCompletions
            | Api::OpenAiResponses
            | Api::AnthropicMessages
            | Api::GoogleGenerativeAi
            | Api::GoogleVertex
            | Api::AzureOpenAiResponses
            | Api::BedrockConverseStream
    )
}

/// Builds the first-turn eager-todo message + optional forced tool choice.
/// Returns `None` when eager mode is off, a plan already exists, this is a
/// sub-agent, or the prompt looks like a question rather than a task.
pub fn build_eager_todo_prelude(
    prompt_text: Option<&str>,
    mode: TodoEagerMode,
    has_existing_phases: bool,
    is_subagent: bool,
    model_supports_forcing: bool,
) -> Option<(Message, Option<ToolChoice>)> {
    if mode == TodoEagerMode::Off || has_existing_phases || is_subagent {
        return None;
    }
    if let Some(text) = prompt_text
        && looks_like_a_question(text)
    {
        return None;
    }
    let text = "Before starting, create a todo list with the `todo` tool covering the \
                full scope of this request, then begin working through it."
        .to_string();
    let message = Message::User(UserMessage::hidden(text));
    let choice = if mode == TodoEagerMode::Always && model_supports_forcing {
        Some(ToolChoice::Named("todo".to_string()))
    } else {
        None
    };
    Some((message, choice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eager_prelude_none_when_mode_off() {
        assert!(
            build_eager_todo_prelude(
                Some("build a login page"),
                TodoEagerMode::Off,
                false,
                false,
                true
            )
            .is_none()
        );
    }

    #[test]
    fn eager_prelude_none_when_phases_already_exist() {
        assert!(
            build_eager_todo_prelude(
                Some("build a login page"),
                TodoEagerMode::Always,
                true,
                false,
                true
            )
            .is_none()
        );
    }

    #[test]
    fn eager_prelude_none_for_subagent() {
        assert!(
            build_eager_todo_prelude(
                Some("build a login page"),
                TodoEagerMode::Always,
                false,
                true,
                true
            )
            .is_none()
        );
    }

    #[test]
    fn eager_prelude_none_when_prompt_looks_like_a_question() {
        assert!(
            build_eager_todo_prelude(
                Some("what does this function do?"),
                TodoEagerMode::Always,
                false,
                false,
                true
            )
            .is_none()
        );
    }

    #[test]
    fn eager_prelude_preferred_never_forces_tool_choice() {
        let (_, choice) = build_eager_todo_prelude(
            Some("build a login page"),
            TodoEagerMode::Preferred,
            false,
            false,
            true,
        )
        .unwrap();
        assert!(choice.is_none());
    }

    #[test]
    fn eager_prelude_always_forces_tool_choice_when_model_supports_it() {
        let (_, choice) = build_eager_todo_prelude(
            Some("build a login page"),
            TodoEagerMode::Always,
            false,
            false,
            true,
        )
        .unwrap();
        assert_eq!(choice, Some(oxicode_ai::ToolChoice::Named("todo".into())));
    }

    #[test]
    fn eager_prelude_always_falls_back_to_reminder_only_when_model_cannot_force() {
        let (_, choice) = build_eager_todo_prelude(
            Some("build a login page"),
            TodoEagerMode::Always,
            false,
            false,
            false,
        )
        .unwrap();
        assert!(choice.is_none());
    }

    #[test]
    fn eager_prelude_message_is_hidden() {
        let (msg, _) = build_eager_todo_prelude(
            Some("build a login page"),
            TodoEagerMode::Always,
            false,
            false,
            true,
        )
        .unwrap();
        match msg {
            Message::User(u) => assert!(!u.visible),
            _ => panic!("expected a hidden user message"),
        }
    }

    #[test]
    fn looks_like_a_question_detects_wh_words_and_non_ascii() {
        assert!(looks_like_a_question("what does this do?"));
        assert!(looks_like_a_question("Why is this failing?"));
        assert!(looks_like_a_question("이 기능이 뭐야?"));
        assert!(!looks_like_a_question("build a login page"));
        assert!(!looks_like_a_question("add tests for the parser"));
    }

    #[test]
    fn provider_supports_tool_choice_covers_builtin_json_providers() {
        assert!(provider_supports_tool_choice(Api::OpenAiCompletions));
        assert!(provider_supports_tool_choice(Api::AnthropicMessages));
        assert!(provider_supports_tool_choice(Api::BedrockConverseStream));
        assert!(!provider_supports_tool_choice(Api::OllamaChat));
    }
}
