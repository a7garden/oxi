//! Auth guidance messages for providers
//!
//! Formats helpful messages when API keys are missing or authentication fails.
//! Always guides users toward file-based auth (`oxi setup` / `/login`) first,
//! with environment variables mentioned only as a CI/CD fallback.

/// Format a message when no API key is found for a provider
pub fn format_no_api_key_found_message(provider: &str) -> String {
    let env_key_hint = oxi_ai::register_builtins::get_provider_env_key(provider)
        .map(|k| format!("\n  For CI/CD: export {}=<key>", k))
        .unwrap_or_default();

    match provider {
        "anthropic" => format!(
            "No API key found for Anthropic.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login anthropic <your-api-key>\n\
             3. Or get a key from https://console.anthropic.com/{}",
            env_key_hint
        ),
        "openai" => format!(
            "No API key found for OpenAI.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login openai <your-api-key>\n\
             3. Or get a key from https://platform.openai.com/api-keys{}",
            env_key_hint
        ),
        "google" => format!(
            "No API key found for Google.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login google <your-api-key>\n\
             3. Or get a key from https://aistudio.google.com/apikey{}",
            env_key_hint
        ),
        "deepseek" => format!(
            "No API key found for DeepSeek.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login deepseek <your-api-key>{}",
            env_key_hint
        ),
        "mistral" => format!(
            "No API key found for Mistral.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login mistral <your-api-key>{}",
            env_key_hint
        ),
        "groq" => format!(
            "No API key found for Groq.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login groq <your-api-key>{}",
            env_key_hint
        ),
        "xai" => format!(
            "No API key found for xAI.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login xai <your-api-key>{}",
            env_key_hint
        ),
        "openrouter" => format!(
            "No API key found for OpenRouter.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login openrouter <your-api-key>{}",
            env_key_hint
        ),
        _ => format!(
            "No API key found for {}.\n\
             Quick fix:\n\
             1. Run: oxi setup\n\
             2. Or in-session: /login {} <your-api-key>{}",
            provider, provider, env_key_hint
        ),
    }
}

/// Format a message when no model is selected
pub fn format_no_model_selected_message() -> String {
    "No model selected. Use /model to select a model.".to_string()
}

/// Get provider-specific login help
///
/// Guides users toward file-based auth (`oxi setup`) first.
pub fn get_provider_login_help(provider: &str) -> String {
    let env_key_hint = oxi_ai::register_builtins::get_provider_env_key(provider)
        .map(|k| format!("\n  For CI/CD: export {}=<key>", k))
        .unwrap_or_default();

    match provider {
        "anthropic" => format!(
            "To authenticate with Anthropic:\n  \
             1. Get an API key from https://console.anthropic.com/\n  \
             2. Run: oxi setup\n  \
             Or in-session: /login anthropic <your-api-key>{}",
            env_key_hint
        ),
        "openai" => format!(
            "To authenticate with OpenAI:\n  \
             1. Get an API key from https://platform.openai.com/api-keys\n  \
             2. Run: oxi setup\n  \
             Or in-session: /login openai <your-api-key>{}",
            env_key_hint
        ),
        "google" => format!(
            "To authenticate with Google:\n  \
             1. Get an API key from https://aistudio.google.com/apikey\n  \
             2. Run: oxi setup\n  \
             Or in-session: /login google <your-api-key>{}",
            env_key_hint
        ),
        _ => format!(
            "Run: oxi setup\n  \
             Or in-session: /login {} <your-api-key>{}",
            provider, env_key_hint
        ),
    }
}
