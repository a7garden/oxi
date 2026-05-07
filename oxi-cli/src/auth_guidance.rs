//! Auth guidance messages for providers
//!
//! Formats helpful messages when API keys are missing or authentication fails.

/// Format a message when no API key is found for a provider
pub fn format_no_api_key_found_message(provider: &str) -> String {
    match provider {
        "anthropic" => format!(
            "No API key found for Anthropic.\n\
             Run: /login anthropic <your-api-key>\n\
             Or restart oxi to use the setup wizard."
        ),
        "openai" => format!(
            "No API key found for OpenAI.\n\
             Run: /login openai <your-api-key>"
        ),
        "google" => format!(
            "No API key found for Google.\n\
             Run: /login google <your-api-key>"
        ),
        "deepseek" => format!(
            "No API key found for DeepSeek.\n\
             Run: /login deepseek <your-api-key>"
        ),
        "mistral" => format!(
            "No API key found for Mistral.\n\
             Run: /login mistral <your-api-key>"
        ),
        "groq" => format!(
            "No API key found for Groq.\n\
             Run: /login groq <your-api-key>"
        ),
        "xai" => format!(
            "No API key found for xAI.\n\
             Run: /login xai <your-api-key>"
        ),
        "openrouter" => format!(
            "No API key found for OpenRouter.\n\
             Run: /login openrouter <your-api-key>"
        ),
        _ => format!(
            "No API key found for {}.\n\
             Run: /login {} <your-api-key>",
            provider, provider
        ),
    }
}

/// Format a message when no model is selected
pub fn format_no_model_selected_message() -> String {
    "No model selected. Use /model to select a model.".to_string()
}

/// Get provider-specific login help
pub fn get_provider_login_help(provider: &str) -> String {
    match provider {
        "anthropic" => "To authenticate with Anthropic:\n  \
                        1. Get an API key from https://console.anthropic.com/\n  \
                        2. Run: /login anthropic <your-api-key>".to_string(),
        "openai" => "To authenticate with OpenAI:\n  \
                      1. Get an API key from https://platform.openai.com/api-keys\n  \
                      2. Run: /login openai <your-api-key>".to_string(),
        "google" => "To authenticate with Google:\n  \
                      1. Get an API key from https://aistudio.google.com/apikey\n  \
                      2. Run: /login google <your-api-key>".to_string(),
        _ => format!("Run: /login {} <your-api-key>", provider),
    }
}
