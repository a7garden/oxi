//! Auth guidance messages for providers
//!
//! Formats helpful messages when API keys are missing or authentication fails.

/// Format a message when no API key is found for a provider
pub fn format_no_api_key_found_message(provider: &str) -> String {
    match provider {
        "anthropic" => format!(
            "No API key found for Anthropic. Set the ANTHROPIC_API_KEY environment variable:\n  \
             export ANTHROPIC_API_KEY=\"sk-ant-...\"\n\
             Or run: /login anthropic"
        ),
        "openai" => format!(
            "No API key found for OpenAI. Set the OPENAI_API_KEY environment variable:\n  \
             export OPENAI_API_KEY=\"sk-...\""
        ),
        "google" => format!(
            "No API key found for Google. Set the GEMINI_API_KEY environment variable:\n  \
             export GEMINI_API_KEY=\"...\""
        ),
        "deepseek" => format!(
            "No API key found for DeepSeek. Set the DEEPSEEK_API_KEY environment variable:\n  \
             export DEEPSEEK_API_KEY=\"...\""
        ),
        "mistral" => format!(
            "No API key found for Mistral. Set the MISTRAL_API_KEY environment variable:\n  \
             export MISTRAL_API_KEY=\"...\""
        ),
        "groq" => format!(
            "No API key found for Groq. Set the GROQ_API_KEY environment variable:\n  \
             export GROQ_API_KEY=\"...\""
        ),
        "xai" => format!(
            "No API key found for xAI. Set the XAI_API_KEY environment variable:\n  \
             export XAI_API_KEY=\"...\""
        ),
        "openrouter" => format!(
            "No API key found for OpenRouter. Set the OPENROUTER_API_KEY environment variable:\n  \
             export OPENROUTER_API_KEY=\"...\""
        ),
        "azure" => format!(
            "No API key found for Azure OpenAI. Set the AZURE_OPENAI_API_KEY environment variable:\n  \
             export AZURE_OPENAI_API_KEY=\"...\""
        ),
        "cloudflare" => format!(
            "No API key found for Cloudflare. Set the CLOUDFLARE_API_KEY environment variable:\n  \
             export CLOUDFLARE_API_KEY=\"...\""
        ),
        _ => format!(
            "No API key found for {}. Set the appropriate API key environment variable.",
            provider
        ),
    }
}

/// Format a message when no model is selected
pub fn format_no_model_selected_message() -> String {
    "No model selected. Use /model <provider/model> to select a model.".to_string()
}

/// Get provider-specific login help
pub fn get_provider_login_help(provider: &str) -> String {
    match provider {
        "anthropic" => "To authenticate with Anthropic:\n  \
                        1. Get an API key from https://console.anthropic.com/\n  \
                        2. Set ANTHROPIC_API_KEY environment variable\n  \
                        Or run: /login anthropic".to_string(),
        "openai" => "To authenticate with OpenAI:\n  \
                      1. Get an API key from https://platform.openai.com/api-keys\n  \
                      2. Set OPENAI_API_KEY environment variable".to_string(),
        "google" => "To authenticate with Google:\n  \
                      1. Get an API key from https://aistudio.google.com/apikey\n  \
                      2. Set GEMINI_API_KEY environment variable".to_string(),
        _ => format!("Set the appropriate API key environment variable for {}.", provider),
    }
}
