//! GitLab Duo provider — REST API proxy for GitLab AI Gateway.
//!
//! Port of omp `packages/ai/src/providers/gitlab-duo.ts` (399 lines).
//!
//! GitLab Duo is a **REST proxy** that:
//! 1. Authenticates with the GitLab access token (personal/group token)
//! 2. Exchanges it for a "direct access" token via GitLab's REST API
//! 3. Delegates to the Anthropic or OpenAI provider with the proxy base URL
//!
//! This is NOT the WebSocket-based `gitlab-duo-agent` workflow provider.
//! That's a separate, more complex provider (`gitlab-duo-workflow.ts`) that
//! requires WebSocket + protobuf infra and is tracked separately.
//!
//! ## Protocol
//! - REST: POST `/api/v4/ai/third_party_agents/direct_access` → direct access token
//! - LLM: Anthropic Messages API or OpenAI Chat/Responses API via AI Gateway
//! - Auth: GitLab personal access token → direct access JWT
//!
//! ## Model mappings
//! The `MODEL_MAPPINGS` table maps Duo model IDs (e.g. `duo-chat-sonnet-4-5`)
//! to upstream provider + model ID + proxy base URL.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::anthropic::AnthropicProvider;
use super::openai::OpenAiProvider;
use super::openai_responses::OpenAiResponsesProvider;
use crate::{
    Context, HttpErrorDetail, Model, Provider, StreamOptions, StreamResult, error::ProviderError,
};

// ── Constants ───────────────────────────────────────────────────────

const GITLAB_COM_URL: &str = "https://gitlab.com";
const ANTHROPIC_PROXY_URL: &str = "https://cloud.gitlab.com/ai/v1/proxy/anthropic/";
const OPENAI_PROXY_URL: &str = "https://cloud.gitlab.com/ai/v1/proxy/openai/v1";
const DIRECT_ACCESS_TTL_MS: u64 = 25 * 60 * 1000; // 25 minutes
const DIRECT_ACCESS_PATH: &str = "/api/v4/ai/third_party_agents/direct_access";

// ── Model mappings ──────────────────────────────────────────────────

/// Which upstream provider a GitLab Duo model delegates to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GitLabProviderType {
    Anthropic,
    OpenAi,
}

/// OpenAI API subtype for GitLab Duo models.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GitLabOpenAIApiType {
    Chat,
    Responses,
}

/// Mapping from a GitLab Duo model ID to its upstream configuration.
#[derive(Clone, Debug)]
pub struct GitLabModelMapping {
    pub provider: GitLabProviderType,
    pub model: &'static str,
    pub openai_api_type: Option<GitLabOpenAIApiType>,
    #[allow(dead_code)]
    pub name: &'static str,
    #[allow(dead_code)]
    pub reasoning: bool,
}

/// GitLab Duo model ID to upstream model/proxy mapping.
static MODEL_MAPPINGS: &[(&str, GitLabModelMapping)] = &[
    (
        "duo-chat-opus-4-6",
        GitLabModelMapping {
            provider: GitLabProviderType::Anthropic,
            model: "claude-opus-4-6",
            openai_api_type: None,
            name: "Duo Chat Opus 4.6",
            reasoning: true,
        },
    ),
    (
        "duo-chat-sonnet-4-6",
        GitLabModelMapping {
            provider: GitLabProviderType::Anthropic,
            model: "claude-sonnet-4-6",
            openai_api_type: None,
            name: "Duo Chat Sonnet 4.6",
            reasoning: true,
        },
    ),
    (
        "duo-chat-opus-4-5",
        GitLabModelMapping {
            provider: GitLabProviderType::Anthropic,
            model: "claude-opus-4-5-20251101",
            openai_api_type: None,
            name: "Duo Chat Opus 4.5",
            reasoning: true,
        },
    ),
    (
        "duo-chat-sonnet-4-5",
        GitLabModelMapping {
            provider: GitLabProviderType::Anthropic,
            model: "claude-sonnet-4-5-20250929",
            openai_api_type: None,
            name: "Duo Chat Sonnet 4.5",
            reasoning: true,
        },
    ),
    (
        "duo-chat-haiku-4-5",
        GitLabModelMapping {
            provider: GitLabProviderType::Anthropic,
            model: "claude-haiku-4-5-20251001",
            openai_api_type: None,
            name: "Duo Chat Haiku 4.5",
            reasoning: true,
        },
    ),
    (
        "duo-chat-gpt-5-1",
        GitLabModelMapping {
            provider: GitLabProviderType::OpenAi,
            model: "gpt-5.1-2025-11-13",
            openai_api_type: Some(GitLabOpenAIApiType::Chat),
            name: "Duo Chat GPT-5.1",
            reasoning: true,
        },
    ),
    (
        "duo-chat-gpt-5-2",
        GitLabModelMapping {
            provider: GitLabProviderType::OpenAi,
            model: "gpt-5.2-2025-12-11",
            openai_api_type: Some(GitLabOpenAIApiType::Chat),
            name: "Duo Chat GPT-5.2",
            reasoning: true,
        },
    ),
    (
        "duo-chat-gpt-5-mini",
        GitLabModelMapping {
            provider: GitLabProviderType::OpenAi,
            model: "gpt-5-mini-2025-08-07",
            openai_api_type: Some(GitLabOpenAIApiType::Chat),
            name: "Duo Chat GPT-5 Mini",
            reasoning: true,
        },
    ),
    (
        "duo-chat-gpt-5-codex",
        GitLabModelMapping {
            provider: GitLabProviderType::OpenAi,
            model: "gpt-5-codex",
            openai_api_type: Some(GitLabOpenAIApiType::Responses),
            name: "Duo Chat GPT-5 Codex",
            reasoning: true,
        },
    ),
    (
        "duo-chat-gpt-5-2-codex",
        GitLabModelMapping {
            provider: GitLabProviderType::OpenAi,
            model: "gpt-5.2-codex",
            openai_api_type: Some(GitLabOpenAIApiType::Responses),
            name: "Duo Chat GPT-5.2 Codex",
            reasoning: true,
        },
    ),
];

/// Look up a GitLab Duo model mapping by model ID.
pub fn get_model_mapping(model_id: &str) -> Option<&'static GitLabModelMapping> {
    if let Some((_, mapping)) = MODEL_MAPPINGS.iter().find(|(id, _)| *id == model_id) {
        return Some(mapping);
    }
    MODEL_MAPPINGS
        .iter()
        .find(|(_, m)| m.model == model_id)
        .map(|(_, m)| m)
}

// ── Direct access token cache ───────────────────────────────────────

struct CachedDirectAccess {
    token: String,
    headers: Vec<(String, String)>,
    expires_at: Instant,
}

static DIRECT_ACCESS_CACHE: std::sync::LazyLock<Mutex<Option<CachedDirectAccess>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

fn get_cached_direct_access() -> Option<CachedDirectAccess> {
    let cache = DIRECT_ACCESS_CACHE.lock().ok()?;
    let cached = cache.as_ref()?;
    if cached.expires_at > Instant::now() {
        Some(CachedDirectAccess {
            token: cached.token.clone(),
            headers: cached.headers.clone(),
            expires_at: cached.expires_at,
        })
    } else {
        None
    }
}

fn set_cached_direct_access(token: String, headers: Vec<(String, String)>) {
    if let Ok(mut cache) = DIRECT_ACCESS_CACHE.lock() {
        *cache = Some(CachedDirectAccess {
            token,
            headers,
            expires_at: Instant::now() + Duration::from_millis(DIRECT_ACCESS_TTL_MS),
        });
    }
}

#[allow(dead_code)]
fn clear_direct_access_cache() {
    if let Ok(mut cache) = DIRECT_ACCESS_CACHE.lock() {
        *cache = None;
    }
}

// ── GitLab Duo Provider ─────────────────────────────────────────────

/// GitLab Duo provider — delegates to Anthropic/OpenAI through GitLab's AI Gateway.
///
/// This is a **REST proxy** provider. It handles GitLab Duo auth (direct access
/// token exchange) and delegates the actual LLM call to the appropriate upstream
/// provider (Anthropic Messages API or OpenAI Chat/Responses API) with the proxy
/// base URL set to GitLab's AI Gateway.
///
/// **Not** to be confused with `GitLabDuoAgent` (WebSocket workflow provider),
/// which is a separate provider tracked separately.
#[derive(Clone)]
pub struct GitLabDuoProvider {
    gitlab_token: Option<String>,
}

impl GitLabDuoProvider {
    pub fn new() -> Self {
        Self { gitlab_token: None }
    }

    #[allow(dead_code)]
    pub fn with_gitlab_token(token: impl Into<String>) -> Self {
        Self {
            gitlab_token: Some(token.into()),
        }
    }
}

impl Default for GitLabDuoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for GitLabDuoProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        // Clone the Arc'd fields we need outside the async block.
        let gitlab_token = self.gitlab_token.clone();
        let model_id = model.id.clone();

        Box::pin(async move {
            // 1. Resolve model mapping
            let mapping = get_model_mapping(&model_id).ok_or_else(|| {
                ProviderError::UnknownProvider(format!(
                    "Unknown GitLab Duo model: {}. Supported models: duo-chat-*",
                    model_id
                ))
            })?;

            // 2. Resolve GitLab access token
            let token = gitlab_token
                .or_else(|| std::env::var("GITLAB_TOKEN").ok())
                .or_else(|| std::env::var("GITLAB_ACCESS_TOKEN").ok())
                .ok_or_else(|| {
                    ProviderError::InvalidResponse(
                        "GitLab access token required. Set GITLAB_TOKEN or provide via options."
                            .to_string(),
                    )
                })?;

            // 3. Exchange for direct access token (with cache)
            let creds = get_or_fetch_direct_access(&token).await?;

            // 4. Determine proxy URL and delegate
            match mapping.provider {
                GitLabProviderType::Anthropic => {
                    let inner = AnthropicProvider::with_config(
                        ANTHROPIC_PROXY_URL,
                        Some(creds.token),
                        creds.headers,
                    );
                    let inner_model = Model {
                        id: mapping.model.to_string(),
                        api: crate::Api::AnthropicMessages,
                        provider: "anthropic".to_string(),
                        base_url: ANTHROPIC_PROXY_URL.to_string(),
                        ..model.clone()
                    };
                    inner.stream(&inner_model, context, options).await
                }
                GitLabProviderType::OpenAi => match mapping.openai_api_type {
                    Some(GitLabOpenAIApiType::Responses) => {
                        let inner = OpenAiResponsesProvider::with_base_url_and_key(
                            OPENAI_PROXY_URL,
                            Some(creds.token),
                        );
                        let inner_model = Model {
                            id: mapping.model.to_string(),
                            api: crate::Api::OpenAiResponses,
                            provider: "openai".to_string(),
                            base_url: OPENAI_PROXY_URL.to_string(),
                            ..model.clone()
                        };
                        inner.stream(&inner_model, context, options).await
                    }
                    _ => {
                        let inner = OpenAiProvider::with_base_url_and_key(
                            OPENAI_PROXY_URL,
                            Some(creds.token),
                        );
                        let inner_model = Model {
                            id: mapping.model.to_string(),
                            api: crate::Api::OpenAiCompletions,
                            provider: "openai".to_string(),
                            base_url: OPENAI_PROXY_URL.to_string(),
                            ..model.clone()
                        };
                        inner.stream(&inner_model, context, options).await
                    }
                },
            }
        })
    }
}

// ── Auth ────────────────────────────────────────────────────────────

/// Direct access token response from GitLab's REST API.
#[derive(serde::Deserialize)]
struct DirectAccessResponse {
    token: Option<String>,
    headers: Option<std::collections::HashMap<String, String>>,
}

/// Fetch a direct access token from GitLab's REST API, with caching.
async fn get_or_fetch_direct_access(
    gitlab_token: &str,
) -> Result<DirectAccessCredentials, ProviderError> {
    if let Some(cached) = get_cached_direct_access() {
        return Ok(DirectAccessCredentials {
            token: cached.token,
            headers: cached.headers,
        });
    }

    let client = super::shared_client();

    let response = client
        .post(format!("{}{}", GITLAB_COM_URL, DIRECT_ACCESS_PATH))
        .header("Authorization", format!("Bearer {}", gitlab_token))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "feature_flags": { "DuoAgentPlatformNext": true }
        }))
        .send()
        .await
        .map_err(|e| ProviderError::NetworkError(format!("Request failed: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return if status == 403 {
            Err(ProviderError::HttpError(HttpErrorDetail {
                status: 403,
                body: format!("GitLab Duo access denied. Ensure Duo is enabled. {}", body),
                provider: Some("gitlab-duo".to_string()),
                request_id: None,
            }))
        } else {
            Err(ProviderError::HttpError(HttpErrorDetail {
                status: status.as_u16(),
                body,
                provider: Some("gitlab-duo".to_string()),
                request_id: None,
            }))
        };
    }

    let payload: DirectAccessResponse = response.json().await.map_err(|e| {
        ProviderError::InvalidResponse(format!(
            "Failed to parse GitLab Duo direct access response: {}",
            e
        ))
    })?;

    let token = payload.token.ok_or_else(|| {
        ProviderError::InvalidResponse(
            "GitLab Duo direct access response missing token".to_string(),
        )
    })?;

    let headers: Vec<(String, String)> = payload.headers.unwrap_or_default().into_iter().collect();

    let creds = DirectAccessCredentials {
        token: token.clone(),
        headers: headers.clone(),
    };

    set_cached_direct_access(token, headers);
    Ok(creds)
}

/// Credentials returned from the direct access token exchange.
#[derive(Clone, Debug)]
struct DirectAccessCredentials {
    token: String,
    headers: Vec<(String, String)>,
}

/// Clear the direct access token cache (for testing).
#[allow(dead_code)]
pub fn clear_token_cache() {
    clear_direct_access_cache();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gitlab_duo_provider_creation() {
        let provider = GitLabDuoProvider::new();
        let _ = provider;
    }

    #[test]
    fn test_model_mapping_direct_lookup() {
        let mapping = get_model_mapping("duo-chat-sonnet-4-5").unwrap();
        assert_eq!(mapping.provider, GitLabProviderType::Anthropic);
        assert_eq!(mapping.model, "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn test_model_mapping_canonical_lookup() {
        let mapping = get_model_mapping("claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(mapping.provider, GitLabProviderType::Anthropic);
    }

    #[test]
    fn test_model_mapping_openai_chat() {
        let mapping = get_model_mapping("duo-chat-gpt-5-1").unwrap();
        assert_eq!(mapping.provider, GitLabProviderType::OpenAi);
        assert_eq!(mapping.openai_api_type, Some(GitLabOpenAIApiType::Chat));
    }

    #[test]
    fn test_model_mapping_openai_responses() {
        let mapping = get_model_mapping("duo-chat-gpt-5-codex").unwrap();
        assert_eq!(mapping.provider, GitLabProviderType::OpenAi);
        assert_eq!(
            mapping.openai_api_type,
            Some(GitLabOpenAIApiType::Responses)
        );
    }

    #[test]
    fn test_model_mapping_unknown() {
        assert!(get_model_mapping("unknown-model").is_none());
    }
}
