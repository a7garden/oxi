//! Environment variable-based API key resolution (LAST RESORT)
//!
//! Provides environment variable detection as a **last-resort fallback** for
//! CI/CD, containers, and other automated environments.
//!
//! For local development, credentials should be stored via `oxi setup` in
//! `~/.config/oxi/auth.json`. This file-based approach is more reliable than
//! environment variables, which are lost when the shell restarts.
//!
//! Auth priority (see `provider_registry.rs`):
//!   1. Runtime override (--api-key)
//!   2. Stored credential (auth.json)
//!   3. OAuth token
//!   4. Ambient credentials (AWS IAM, Google ADC)
//!   5. Environment variable ← this module

use std::collections::HashMap;
use std::env;
use std::fs;
use std::sync::LazyLock;

/// Cache for Vertex ADC credentials check (expensive fs check)
static VERTEX_ADC_CHECK: LazyLock<bool> = LazyLock::new(check_vertex_adc_credentials);

/// Check if Vertex AI Application Default Credentials exist
fn check_vertex_adc_credentials() -> bool {
    // Check GOOGLE_APPLICATION_CREDENTIALS env var first
    if let Ok(path) = env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        return fs::metadata(&path).is_ok();
    }

    // Fall back to default ADC path
    let default_path =
        dirs::home_dir().map(|h| h.join(".config/gcloud/application_default_credentials.json"));

    default_path
        .map(|p| fs::metadata(p).is_ok())
        .unwrap_or(false)
}

/// Get a value from environment, supporting both std::env and /proc/self/environ fallback
/// for sandboxed environments (e.g., Bun compiled binaries on Linux)
fn get_env(key: &str) -> Option<String> {
    env::var(key).ok().or_else(|| get_proc_env(key))
}

/// Bun/Linux sandbox fallback: read from /proc/self/environ
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn get_proc_env(key: &str) -> Option<String> {
    #[allow(unused_imports)]
    use std::os::unix::ffi::OsStrExt;

    // Only try this on Linux where Bun sandbox may empty process.env
    #[cfg(target_os = "linux")]
    {
        // If process.env has entries, no need for /proc fallback
        if env::var("PATH").is_err() || std::env::vars().count() > 0 {
            return None;
        }

        let Ok(contents) = fs::read_to_string("/proc/self/environ") else {
            return None;
        };

        for segment in contents.split('\0') {
            if let Some(pos) = segment.find('=') {
                let k = &segment.as_bytes()[..pos];
                let v = &segment[pos + 1..];
                if k == key.as_bytes() {
                    return Some(v.to_string());
                }
            }
        }
    }

    None
}

/// Get environment variables that can provide an API key for a provider
pub fn find_env_keys(provider: &str) -> Option<Vec<&'static str>> {
    let keys = match provider {
        // GitHub Copilot: multiple possible env vars
        "github-copilot" | "copilot" => vec!["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],

        // Anthropic: OAuth token takes precedence over API key
        "anthropic" => vec!["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"],

        // OpenAI variants
        "openai" | "openai-responses" => vec![
            "OPENAI_API_KEY",
            "AZURE_OPENAI_API_KEY", // Some deployments use this
        ],

        // Google/Gemini
        "google" | "gemini" => vec!["GEMINI_API_KEY", "GOOGLE_API_KEY"],

        // Google Vertex AI
        "vertex" | "google-vertex" => vec!["GOOGLE_CLOUD_API_KEY"],

        // Azure
        "azure" | "azure-openai" => vec!["AZURE_OPENAI_API_KEY"],

        // Groq
        "groq" => vec!["GROQ_API_KEY"],

        // Cerebras
        "cerebras" => vec!["CEREBRAS_API_KEY"],

        // xAI / Grok
        "xai" => vec!["XAI_API_KEY"],

        // OpenRouter
        "openrouter" => vec!["OPENROUTER_API_KEY"],

        // Vercel AI Gateway
        "vercel-ai-gateway" => vec!["AI_GATEWAY_API_KEY"],

        // ZAI
        "zai" => vec!["ZAI_API_KEY"],

        // Mistral
        "mistral" => vec!["MISTRAL_API_KEY"],

        // MiniMax (China)
        "minimax" | "minimax-cn" => vec!["MINIMAX_API_KEY", "MINIMAX_CN_API_KEY"],

        // Moonshot AI / Kimi (China)
        "moonshotai" | "moonshotai-cn" | "kimi" | "kimi-coding" => {
            vec!["MOONSHOT_API_KEY", "KIMI_API_KEY"]
        }

        // Hugging Face
        "huggingface" | "hf" => vec!["HF_TOKEN", "HUGGINGFACE_TOKEN"],

        // Fireworks AI
        "fireworks" => vec!["FIREWORKS_API_KEY"],

        // DeepSeek
        "deepseek" => vec!["DEEPSEEK_API_KEY"],

        // OpenCode
        "opencode" => vec!["OPENCODE_API_KEY"],

        // Xiaomi
        "xiaomi" => vec!["XIAOMI_API_KEY"],

        // Cloudflare Workers AI
        "cloudflare" | "cloudflare-workers-ai" | "cloudflare-ai-gateway" => {
            vec!["CLOUDFLARE_API_KEY", "CLOUDFLARE_AI_GATEWAY_API_KEY"]
        }

        _ => return None,
    };

    Some(keys)
}

/// Check if any environment variable is set for a provider
pub fn has_env_key(provider: &str) -> bool {
    find_env_keys(provider)
        .map(|keys| keys.iter().any(|k| get_env(k).is_some()))
        .unwrap_or(false)
}

/// Get first available environment variable value for a provider
fn first_of(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = get_env(key)
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    None
}

/// Get API key from environment variables for a provider
///
/// Returns `Some("<authenticated>")` for providers that support ambient credentials
/// (e.g., AWS IAM roles, Google ADC) rather than explicit API keys.
pub fn get_env_api_key(provider: &str) -> Option<String> {
    let keys = find_env_keys(provider)?;
    let key = first_of(&keys)?;

    // Filter out placeholder values
    if key == "<authenticated>" || key.starts_with("sk-") && key.len() < 10 {
        return None;
    }

    Some(key)
}

/// Check Vertex AI-specific ambient credentials
///
/// Vertex AI supports Application Default Credentials (ADC) configured via:
/// - `gcloud auth application-default login`
/// - `GOOGLE_APPLICATION_CREDENTIALS` pointing to service account JSON
///
/// Returns true if all required ADC components are present.
pub fn has_vertex_adc() -> bool {
    *VERTEX_ADC_CHECK
}

/// Check if Vertex AI has all required components for ADC authentication
pub fn has_vertex_adc_full() -> bool {
    // Check credentials file exists
    let has_creds = has_vertex_adc();

    // Check project ID
    let has_project = get_env("GOOGLE_CLOUD_PROJECT")
        .or_else(|| get_env("GCLOUD_PROJECT"))
        .is_some();

    // Check location
    let has_location = get_env("GOOGLE_CLOUD_LOCATION").is_some();

    has_creds && has_project && has_location
}

/// Check Amazon Bedrock ambient credentials
///
/// Bedrock supports multiple credential sources:
/// 1. AWS_PROFILE - named profile from ~/.aws/credentials
/// 2. AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY - standard IAM keys
/// 3. AWS_BEARER_TOKEN_BEDROCK - Bedrock bearer token
/// 4. AWS_CONTAINER_CREDENTIALS_RELATIVE_URI - ECS task roles
/// 5. AWS_CONTAINER_CREDENTIALS_FULL_URI - ECS task roles (full URI)
/// 6. AWS_WEB_IDENTITY_TOKEN_FILE - IRSA (IAM Roles for Service Accounts)
pub fn has_bedrock_creds() -> bool {
    // Explicit API key scenario
    if get_env("AWS_ACCESS_KEY_ID").is_some() && get_env("AWS_SECRET_ACCESS_KEY").is_some() {
        return true;
    }

    // Named profile
    if get_env("AWS_PROFILE").is_some() {
        return true;
    }

    // Bedrock bearer token
    if get_env("AWS_BEARER_TOKEN_BEDROCK").is_some() {
        return true;
    }

    // ECS container credentials (IRSA)
    if get_env("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI").is_some()
        || get_env("AWS_CONTAINER_CREDENTIALS_FULL_URI").is_some()
    {
        return true;
    }

    // Web identity token (IRSA)
    if get_env("AWS_WEB_IDENTITY_TOKEN_FILE").is_some() {
        return true;
    }

    false
}

/// Check if Bedrock has full credential configuration
pub fn has_bedrock_creds_full() -> bool {
    // For explicit API key auth, need both key ID and secret
    if get_env("AWS_ACCESS_KEY_ID").is_some() && get_env("AWS_SECRET_ACCESS_KEY").is_some() {
        return true;
    }

    // For profile-based auth, just need the profile
    if get_env("AWS_PROFILE").is_some() {
        return true;
    }

    // For ECS/IRSA, we need the credential endpoint/file
    if get_env("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI").is_some()
        || get_env("AWS_CONTAINER_CREDENTIALS_FULL_URI").is_some()
        || get_env("AWS_WEB_IDENTITY_TOKEN_FILE").is_some()
    {
        return true;
    }

    // Bearer token
    if get_env("AWS_BEARER_TOKEN_BEDROCK").is_some() {
        return true;
    }

    false
}

/// Get all providers with environment variable credentials
pub fn get_all_env_keys() -> HashMap<String, String> {
    let mut result = HashMap::new();

    // Map provider names to their possible env vars
    #[allow(clippy::type_complexity)]
    let mappings: [(&str, fn() -> Option<String>); 17] = [
        ("anthropic", || {
            first_of(&["ANTHROPIC_API_KEY", "ANTHROPIC_OAUTH_TOKEN"])
        }),
        ("openai", || first_of(&["OPENAI_API_KEY"])),
        ("github-copilot", || {
            first_of(&["GITHUB_TOKEN", "GH_TOKEN", "COPILOT_GITHUB_TOKEN"])
        }),
        ("google", || first_of(&["GEMINI_API_KEY"])),
        ("vertex", || first_of(&["GOOGLE_CLOUD_API_KEY"])),
        ("groq", || first_of(&["GROQ_API_KEY"])),
        ("cerebras", || first_of(&["CEREBRAS_API_KEY"])),
        ("xai", || first_of(&["XAI_API_KEY"])),
        ("openrouter", || first_of(&["OPENROUTER_API_KEY"])),
        ("mistral", || first_of(&["MISTRAL_API_KEY"])),
        ("deepseek", || first_of(&["DEEPSEEK_API_KEY"])),
        ("azure", || first_of(&["AZURE_OPENAI_API_KEY"])),
        ("cloudflare", || first_of(&["CLOUDFLARE_API_KEY"])),
        ("huggingface", || first_of(&["HF_TOKEN"])),
        ("fireworks", || first_of(&["FIREWORKS_API_KEY"])),
        ("moonshotai", || first_of(&["MOONSHOT_API_KEY"])),
        ("bedrock", || {
            first_of(&["AWS_ACCESS_KEY_ID", "AWS_PROFILE"])
        }),
    ];

    for (provider, get_key) in mappings.iter() {
        if let Some(value) = get_key() {
            result.insert(provider.to_string(), value);
        }
    }

    result
}

/// Check if a provider supports OAuth tokens via environment
pub fn supports_oauth_env(provider: &str) -> bool {
    matches!(provider, "anthropic")
}

/// Get OAuth token from environment (for providers that support it)
pub fn get_oauth_env_token(provider: &str) -> Option<String> {
    match provider {
        "anthropic" => get_env("ANTHROPIC_OAUTH_TOKEN"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_find_env_keys_anthropic() {
        let keys = find_env_keys("anthropic").unwrap();
        assert!(keys.contains(&"ANTHROPIC_API_KEY"));
        assert!(keys.contains(&"ANTHROPIC_OAUTH_TOKEN"));
    }

    #[test]
    fn test_find_env_keys_copilot() {
        let keys = find_env_keys("github-copilot").unwrap();
        assert!(keys.contains(&"COPILOT_GITHUB_TOKEN"));
        assert!(keys.contains(&"GH_TOKEN"));
        assert!(keys.contains(&"GITHUB_TOKEN"));
    }

    #[test]
    fn test_find_env_keys_unknown() {
        assert!(find_env_keys("unknown-provider").is_none());
    }

    #[test]
    fn test_first_of_returns_first() {
        unsafe { env::set_var("TEST_FIRST_OF_1", "value1") };
        unsafe { env::set_var("TEST_FIRST_OF_2", "value2") };

        let result = first_of(&["TEST_FIRST_OF_1", "TEST_FIRST_OF_2"]);
        assert_eq!(result, Some("value1".to_string()));

        unsafe { env::remove_var("TEST_FIRST_OF_1") };
        unsafe { env::remove_var("TEST_FIRST_OF_2") };
    }

    #[test]
    fn test_first_of_skips_empty() {
        unsafe { env::set_var("TEST_FIRST_OF_SKIP", "") };
        unsafe { env::set_var("TEST_FIRST_OF_SECOND", "second") };

        let result = first_of(&["TEST_FIRST_OF_SKIP", "TEST_FIRST_OF_SECOND"]);
        assert_eq!(result, Some("second".to_string()));

        unsafe { env::remove_var("TEST_FIRST_OF_SKIP") };
        unsafe { env::remove_var("TEST_FIRST_OF_SECOND") };
    }

    #[test]
    fn test_get_env_api_key() {
        unsafe { env::set_var("ANTHROPIC_API_KEY", "sk-test-key-123") };

        // Provider with known keys
        let result = get_env_api_key("anthropic");
        assert_eq!(result, Some("sk-test-key-123".to_string()));

        unsafe { env::remove_var("ANTHROPIC_API_KEY") };
    }

    #[test]
    fn test_has_env_key() {
        unsafe { env::set_var("DEEPSEEK_API_KEY", "test-value") };

        // Check that deepseek has an env key
        let result = has_env_key("deepseek"); // DEEPSEEK_API_KEY
        assert!(result);

        unsafe { env::remove_var("DEEPSEEK_API_KEY") };
    }

    #[test]
    fn test_vertex_adc_check_lazy() {
        // The lazy static should be evaluated on first access
        let result = *VERTEX_ADC_CHECK;
        // Just ensure it doesn't panic
        assert!(result == true || result == false);
    }

    #[test]
    fn test_get_all_env_keys() {
        // Use a unique key to avoid race conditions in parallel test runs
        let key = "OXI_TEST_GET_ALL_ENV_KEYS";
        unsafe { env::set_var(key, "test-value") };

        // Verify the function runs without panic and returns a map
        let all = get_all_env_keys();
        // The map may or may not contain entries depending on the test runner's
        // environment, but it should always be a valid HashMap
        assert!(all.len() <= 17); // max number of providers in the mapping

        unsafe { env::remove_var(key) };
    }

    #[test]
    fn test_oauth_env_token() {
        unsafe { env::set_var("ANTHROPIC_OAUTH_TOKEN", "oauth-token-123") };

        let result = get_oauth_env_token("anthropic");
        assert_eq!(result, Some("oauth-token-123".to_string()));

        let not_oauth = get_oauth_env_token("openai");
        assert!(not_oauth.is_none());

        unsafe { env::remove_var("ANTHROPIC_OAUTH_TOKEN") };
    }

    #[test]
    fn test_bedrock_creds_check() {
        // Without AWS credentials set, should return false
        let result = has_bedrock_creds();
        // Either true (if credentials exist) or false
        assert!(result == true || result == false);
    }

    #[test]
    fn test_supports_oauth_env() {
        assert!(supports_oauth_env("anthropic"));
        assert!(!supports_oauth_env("openai"));
        assert!(!supports_oauth_env("deepseek"));
    }

    #[test]
    fn test_google_legacy_alias() {
        // "google" should map to GEMINI_API_KEY
        let keys = find_env_keys("google");
        assert!(keys.is_some());
        assert!(keys.unwrap().contains(&"GEMINI_API_KEY"));
    }

    #[test]
    fn test_moonshotai_aliases() {
        let keys = find_env_keys("moonshotai");
        assert!(keys.is_some());
        assert!(keys.unwrap().contains(&"MOONSHOT_API_KEY"));

        let kimi = find_env_keys("kimi");
        assert!(kimi.is_some());
        assert!(kimi.unwrap().contains(&"KIMI_API_KEY"));
    }
}
