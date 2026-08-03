//! models.dev live enrichment (Layer 2.5 of the catalog).
//!
//! Fetches the community-maintained model catalog from
//! <https://models.dev/api.json> (MIT, also used by opencode) and enriches
//! the built-in Layer 1 TOML entries with up-to-date pricing, context
//! windows, max output tokens, and reasoning flags.
//!
//! # Layering
//!
//! ```text
//! Layer 1   built-in TOML (compiled in)           fallback
//! Layer 2   user overrides (~/.oxicode/catalog/...)    wins
//! Layer 2.5 models.dev enrichment (this module)   fills gaps / refreshes
//! Layer 3   /v1/models runtime discovery          local servers
//! ```
//!
//! Enrichment runs inside the consumer's model database (oxicode-ai's `model_db`) after
//! Layer 2 overrides are applied. Only fields that are missing or
//! unverifiable in Layer 1 are overwritten — see the precedence rules below.
//!
//! # Precedence (highest wins)
//!
//! 1. Layer 2 user override
//! 2. models.dev enrichment (this module) — only positive prices / known
//!    limits; never overwrites a verified Layer 1 value with a worse one
//! 3. Layer 1 built-in TOML
//!
//! # Offline behavior
//!
//! If the cache is fresh, enrichment is near-instant (file read). If the
//! cache is stale or absent, a live fetch is attempted (10s timeout, 2
//! retries). On total failure, [`get`] returns `None` and Layer 1 is used
//! unchanged — the application still works, only cost accuracy degrades.
//!
//! # Attribution
//!
//! Model data © [models.dev](https://models.dev) (MIT). See
//! <https://github.com/sst/models.dev>.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::Api;
use crate::catalog::provider::AuthMethod;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Local-only freshness window: if the cache file's mtime is within this
/// window, no HTTP request is made at all (zero-cost). Default 1 hour.
const DEFAULT_MTIME_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Per-request timeout for the live fetch.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Number of retries on transient fetch failures.
const FETCH_RETRIES: u32 = 2;

/// Backoff between retries (first retry waits this long).
const RETRY_BACKOFF: Duration = Duration::from_millis(200);

/// Default models.dev endpoint.
const DEFAULT_URL: &str = "https://models.dev";

/// User-Agent sent to models.dev.
const USER_AGENT: &str = concat!("oxicode/", env!("CARGO_PKG_VERSION"));

// ---------------------------------------------------------------------------
// Schema (mirrors models.dev `api.json`, see opencode `packages/core/src/models-dev.ts`)
// ---------------------------------------------------------------------------

/// Top-level catalog: provider id → provider.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MdCatalog(pub BTreeMap<String, MdProvider>);

/// A single provider entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct MdProvider {
    /// Display name.
    #[allow(dead_code)]
    pub name: String,
    /// Environment variables that hold the API key.
    #[allow(dead_code)]
    pub env: Vec<String>,
    /// AI SDK npm package identifying the API protocol.
    #[serde(default)]
    #[allow(dead_code)]
    pub npm: Option<String>,
    /// Native API base URL for OpenAI-compatible providers.
    #[serde(default)]
    #[allow(dead_code)]
    pub api: Option<String>,
    /// Link to provider documentation.
    #[serde(default)]
    #[allow(dead_code)]
    pub doc: Option<String>,
    /// Models served by this provider.
    pub models: BTreeMap<String, MdModel>,
}

/// A single model entry — serialised from models.dev `api.json`.
#[derive(Debug, Serialize, Deserialize)]
pub struct MdModel {
    /// Display name.
    #[allow(dead_code)]
    pub name: String,
    /// Model family (e.g. "claude-sonnet", "gpt-4").
    #[serde(default)]
    #[allow(dead_code)]
    pub family: Option<String>,
    /// Whether the model supports reasoning / chain-of-thought.
    pub reasoning: bool,
    /// Whether the model supports tool calling.
    #[serde(default)]
    pub tool_call: bool,
    /// Whether the model supports file attachments (images, PDFs).
    #[serde(default)]
    pub attachment: bool,
    /// Whether the model supports temperature control.
    #[serde(default)]
    #[allow(dead_code)]
    pub temperature: Option<bool>,
    /// Whether the model supports structured output / JSON mode.
    #[serde(default)]
    #[allow(dead_code)]
    pub structured_output: Option<bool>,
    /// Knowledge cutoff date.
    #[serde(default)]
    #[allow(dead_code)]
    pub knowledge: Option<String>,
    /// Release date of the model.
    #[serde(default)]
    #[allow(dead_code)]
    pub release_date: Option<String>,
    /// Last update time of this entry.
    #[serde(default)]
    #[allow(dead_code)]
    pub last_updated: Option<String>,
    /// Whether the model uses open weights.
    #[serde(default)]
    #[allow(dead_code)]
    pub open_weights: Option<bool>,
    /// Whether the model supports interleaved thinking + tool calls.
    #[serde(default)]
    #[allow(dead_code)]
    pub interleaved: Option<serde_json::Value>,
    /// Reasoning options (effort levels, budget tokens).
    #[serde(default)]
    #[allow(dead_code)]
    pub reasoning_options: Option<Vec<MdReasoningOption>>,
    /// Token limits.
    pub limit: MdLimit,
    /// Pricing (USD per million tokens). Optional — some are free.
    #[serde(default)]
    pub cost: Option<MdCost>,
    /// Supported input/output modalities.
    #[serde(default)]
    #[allow(dead_code)]
    pub modalities: Option<MdModalities>,
    /// Model status (alpha, beta, deprecated).
    #[serde(default)]
    #[allow(dead_code)]
    pub status: Option<String>,
    /// Per-model provider override (npm + api).
    #[serde(default)]
    pub provider: Option<MdModelProvider>,
}

/// Per-model provider override — lets a specific model use a different
/// API protocol or endpoint than its parent provider.
#[derive(Debug, Serialize, Deserialize)]
pub struct MdModelProvider {
    /// Override npm package (API protocol).
    #[serde(default)]
    pub npm: Option<String>,
    /// Override API base URL (empty = inherit from parent).
    #[serde(default)]
    pub api: Option<String>,
}

/// Token limits.
#[derive(Debug, Serialize, Deserialize)]
pub struct MdLimit {
    /// Maximum context window (total tokens).
    pub context: f64,
    /// Max input tokens (optional, for reasoning models with input budget).
    #[serde(default)]
    pub input: Option<f64>,
    /// Maximum output tokens (maps to oxicode `max_tokens`).
    pub output: f64,
}

/// Pricing. All values are USD per million tokens.
#[derive(Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct MdCost {
    /// Cost per million input tokens.
    pub input: f64,
    /// Cost per million output tokens.
    pub output: f64,
    /// Cost per million cached read tokens, if billed separately.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Cost per million cached write tokens, if billed separately.
    #[serde(default)]
    pub cache_write: Option<f64>,
    /// Tiered pricing (e.g. context-length-based tiers).
    #[serde(default)]
    pub tiers: Option<Vec<MdCostTier>>,
    /// Context >200K pricing (Anthropic-specific extended pricing tier).
    #[serde(default)]
    pub context_over_200k: Option<MdCostTierData>,
    /// Separate pricing for reasoning/thinking tokens.
    #[serde(default)]
    pub reasoning: Option<f64>,
    /// Audio modality input pricing.
    #[serde(default)]
    pub input_audio: Option<f64>,
    /// Audio modality output pricing.
    #[serde(default)]
    pub output_audio: Option<f64>,
}

/// A single pricing tier (used within `tiers` array).
#[derive(Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct MdCostTier {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    pub tier: MdTierSpec,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct MdTierSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub size: f64,
}

/// Context-over-200K pricing tier data (Anthropic-specific).
#[derive(Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct MdCostTierData {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// Supported input/output modalities.
#[derive(Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct MdModalities {
    #[serde(default)]
    #[allow(dead_code)]
    pub input: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)]
    pub output: Option<Vec<String>>,
}

/// Reasoning options (effort levels, budget tokens).
#[derive(Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct MdReasoningOption {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub values: Option<Vec<Option<String>>>,
    #[serde(default)]
    #[allow(dead_code)]
    pub min: Option<f64>,
}

// ---------------------------------------------------------------------------
// Protocol resolver — npm → (Api + AuthMethod), 7줄 (본 설계 핵심)
// ---------------------------------------------------------------------------

/// Map a models.dev `npm` string to oxicode's API type and authentication method.
///
/// This is the **only** protocol knowledge oxicode has. For OpenAI-compatible
/// providers, the base URL from `MdProvider.api` is used at materialize time.
/// Fresh npm values not listed here default to OpenAI-compatible (`OpenAiCompletions`).
pub fn protocol_for(npm: &str) -> (Api, AuthMethod) {
    match npm {
        "@ai-sdk/anthropic" => (Api::AnthropicMessages, AuthMethod::XApiKey),
        "@ai-sdk/google" => (Api::GoogleGenerativeAi, AuthMethod::None),
        "@ai-sdk/google-vertex" | "@ai-sdk/google-vertex/anthropic" => {
            (Api::GoogleVertex, AuthMethod::None)
        }
        "@ai-sdk/azure" => (Api::AzureOpenAiResponses, AuthMethod::ApiKey),
        "@ai-sdk/amazon-bedrock" => (Api::BedrockConverseStream, AuthMethod::None),
        // @ai-sdk/openai, @ai-sdk/openai-compatible, groq, xai, togetherai,
        // vercel, perplexity, cerebras, deepinfra, cohere, gateway, etc.
        // And any unknown npm → OpenAI-compatible with Bearer auth.
        _ => (Api::OpenAiCompletions, AuthMethod::Bearer),
    }
}

// ---------------------------------------------------------------------------
// NOTE: provider_map, reasoning_preserve, and enrich() were removed.
// These were used by the legacy TOML enrichment path. With the materialize
// approach (materialize.rs), models.dev data flows directly into
// BuiltinProviderEntry/BuiltinModelEntry without per-entry enrichment.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Global enriched catalog, populated by [`init_models_dev`].
///
/// `Some(None)` after init means "init ran but no data was available"
/// (offline + no cache); the inner `Option` distinguishes that from
/// "init has not run yet" (`MODELS_DEV.get() == None`).
static MODELS_DEV: OnceLock<Option<Arc<MdCatalog>>> = OnceLock::new();

/// Initialize the models.dev catalog.
///
/// Fetches (or reads from cache) the catalog and stores it for later
/// enrichment. Safe to call multiple times — subsequent calls are no-ops.
/// Called once at bootstrap ([`crate`] consumers wire it in the CLI).
pub async fn init_models_dev() {
    if MODELS_DEV.get().is_some() {
        return;
    }
    let result = fetch_with_fallback().await;
    let arc_opt = result.map(Arc::new);
    // `set` is a race-safe no-op if another thread won the init race.
    let _ = MODELS_DEV.set(arc_opt);
}

/// Get the enriched catalog, if [`init_models_dev`] has run with data.
///
/// Returns `None` when init hasn't run, ran but found no data (offline), or
/// enrichment is disabled. Enrichment gracefully falls back to Layer 1 in
/// all these cases.
pub fn get() -> Option<&'static MdCatalog> {
    MODELS_DEV.get().and_then(|o| o.as_deref())
}

/// Force-refresh the models.dev cache.
///
/// Performs a conditional GET (ETag) regardless of the mtime window.
/// The result is written to the cache file. The in-memory catalog is
/// **not** updated (OnceLock is immutable) — the refreshed data takes
/// effect on the next process start.
///
/// Returns `true` if the cache was updated (200), `false` if unchanged
/// (304) or on error.
pub async fn refresh() -> bool {
    if !enabled() || fetch_disabled() {
        return false;
    }
    let etag = read_etag();
    match live_fetch_conditional(etag.as_deref()).await {
        Some(ConditionalResult::NotModified) => {
            tracing::info!("models.dev: already up to date (304)");
            touch_cache_mtime();
            false
        }
        Some(ConditionalResult::Updated(c, new_etag)) => {
            write_cache_atomic(&c);
            if let Some(e) = new_etag {
                write_etag(&e);
            }
            tracing::info!("models.dev: cache refreshed");
            true
        }
        None => {
            tracing::warn!("models.dev: refresh failed");
            false
        }
    }
}

/// Force-clear the cached catalog. Test-only.
#[cfg(test)]
pub fn reset_for_tests() {
    // OnceLock cannot be reset; tests instead construct MdCatalog directly
    // and call `enrich`. This stub documents that intent.
}

// ---------------------------------------------------------------------------
// Fetch / cache
// ---------------------------------------------------------------------------

/// Resolve the cache path.
///
/// - `OXICODE_MODELS_DEV_CACHE_PATH` overrides the location (test/enterprise use)
/// - otherwise `<product-home>/cache/models-dev.json` (`$OXICODE_HOME` or `~/.oxicode`)
fn cache_path() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("OXICODE_MODELS_DEV_CACHE_PATH")
        && !custom.is_empty()
    {
        return Some(PathBuf::from(custom));
    }
    crate::product_env::cache_dir().map(|d| d.join("models-dev.json"))
}

/// Whether enrichment is enabled at all.
///
/// - `OXICODE_MODELS_DEV=off` → disabled
/// - `OXICODE_MODELS_DEV=on` or `auto` (or unset) → enabled
fn enabled() -> bool {
    !matches!(
        std::env::var("OXICODE_MODELS_DEV").as_deref(),
        Ok("off") | Ok("OFF") | Ok("0") | Ok("false") | Ok("FALSE")
    )
}

/// Whether live network fetch is forbidden (air-gapped mode).
fn fetch_disabled() -> bool {
    matches!(
        std::env::var("OXICODE_MODELS_DEV_DISABLE_FETCH").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

/// Configured models.dev endpoint.
fn models_url() -> String {
    std::env::var("OXICODE_MODELS_DEV_URL").unwrap_or_else(|_| DEFAULT_URL.to_string())
}

/// Configured mtime window (local-only freshness check).
///
/// `OXICODE_MODELS_DEV_MTIME_WINDOW` (seconds) overrides the default (1 hour).
/// Within this window, no HTTP request is made — zero-cost cache hit.
fn mtime_window() -> Duration {
    std::env::var("OXICODE_MODELS_DEV_MTIME_WINDOW")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_MTIME_WINDOW)
}

/// Whether to force a conditional GET regardless of mtime window.
/// Set by `oxicode models refresh` or `OXICODE_MODELS_DEV_FORCE_REFRESH=1`.
fn force_refresh() -> bool {
    matches!(
        std::env::var("OXICODE_MODELS_DEV_FORCE_REFRESH").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

/// Cache-or-live fallback chain with conditional GET (ETag).
///
/// Sync resolution order:
/// 1. If cache mtime is within `mtime_window()` (default 1h) and not forced →
///    use cache, no HTTP (zero-cost).
/// 2. Otherwise, conditional GET with `If-None-Match` (stored ETag).
///    - `304 Not Modified` → cache is still valid, touch mtime, use cache.
///    - `200 OK` → write new cache + ETag, use new data.
/// 3. On fetch failure, use stale cache (any age) if available.
async fn fetch_with_fallback() -> Option<MdCatalog> {
    if !enabled() {
        return None;
    }

    // 1) Fresh disk cache within mtime window (unless force_refresh).
    if !force_refresh()
        && let Some(c) = read_cache_if_fresh()
    {
        tracing::debug!("models.dev: using cache within mtime window");
        return Some(c);
    }

    // 2) Conditional GET (unless air-gapped).
    if !fetch_disabled() {
        let etag = read_etag();
        match live_fetch_conditional(etag.as_deref()).await {
            Some(ConditionalResult::NotModified) => {
                // 304 means our cached data is still valid. But if the cache
                // file is missing/corrupt, we have the ETag but no data —
                // fall through to a non-conditional fetch to recover.
                if let Some(c) = read_cache_any() {
                    tracing::debug!("models.dev: 304 Not Modified, touching cache mtime");
                    touch_cache_mtime();
                    return Some(c);
                }
                tracing::warn!("models.dev: 304 received but cache missing — refetching");
                // Remove stale ETag and retry without conditional.
                clear_etag();
                if let Some(ConditionalResult::Updated(c, new_etag)) =
                    live_fetch_conditional(None).await
                {
                    write_cache_atomic(&c);
                    if let Some(e) = new_etag {
                        write_etag(&e);
                    }
                    return Some(c);
                }
            }
            Some(ConditionalResult::Updated(c, new_etag)) => {
                write_cache_atomic(&c);
                if let Some(e) = new_etag {
                    write_etag(&e);
                }
                return Some(c);
            }
            None => { /* fetch failed, fall through to stale */ }
        }
    }

    // 3) Stale cache is better than nothing.
    if let Some(c) = read_cache_any() {
        tracing::debug!("models.dev: using stale cache (live fetch unavailable)");
        return Some(c);
    }

    None
}

/// Result of a conditional GET.
enum ConditionalResult {
    /// Server returned 304 — data unchanged.
    NotModified,
    /// Server returned 200 — new data + optional new ETag.
    Updated(MdCatalog, Option<String>),
}

/// Read the cache only if its mtime is within the mtime window.
fn read_cache_if_fresh() -> Option<MdCatalog> {
    let path = cache_path()?;
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age > mtime_window() {
        return None;
    }
    read_cache(&path)
}

/// Read the cache regardless of freshness.
fn read_cache_any() -> Option<MdCatalog> {
    let path = cache_path()?;
    read_cache(&path)
}

fn read_cache(path: &std::path::Path) -> Option<MdCatalog> {
    let body = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<MdCatalog>(&body) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(error = %e, "models.dev: cache corrupt, ignoring");
            // Corrupt cache: remove so next run refetches cleanly.
            let _ = std::fs::remove_file(path);
            None
        }
    }
}

/// Touch the cache file's mtime to reset the mtime window (after 304).
fn touch_cache_mtime() {
    let Some(path) = cache_path() else { return };
    // Set mtime to now. `set_modified` is stable in Rust 1.75+.
    let now = std::time::SystemTime::now();
    let _ = filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(now));
}

/// Path to the ETag sidecar file.
fn etag_path() -> Option<PathBuf> {
    let base = cache_path()?;
    Some(base.with_extension("json.etag"))
}

/// Read the stored ETag (if any) for conditional GET.
fn read_etag() -> Option<String> {
    let path = etag_path()?;
    let body = std::fs::read_to_string(&path).ok()?;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Write the ETag sidecar atomically.
fn write_etag(etag: &str) {
    let Some(path) = etag_path() else { return };
    let tmp = path.with_extension("json.etag.tmp");
    if std::fs::write(&tmp, etag).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Remove the ETag sidecar (used when recovering from a stale-ETag state).
fn clear_etag() {
    let Some(path) = etag_path() else { return };
    let _ = std::fs::remove_file(&path);
}

/// Write the catalog atomically (temp + rename), per AGENTS.md I/O rules.
fn write_cache_atomic(catalog: &MdCatalog) {
    let Some(path) = cache_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(body) = serde_json::to_string(catalog) else {
        return;
    };
    // PID-suffixed temp name avoids concurrent-writer collisions.
    let tmp = path.with_file_name(format!("models-dev.json.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, &body).is_err() {
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        tracing::debug!(error = %e, "models.dev: cache rename failed");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Live fetch with bounded retries and conditional GET (ETag) support.
///
/// - If `etag` is `Some`, sends `If-None-Match` header.
/// - Returns `NotModified` on 304, `Updated` on 200, `None` on failure.
async fn live_fetch_conditional(etag: Option<&str>) -> Option<ConditionalResult> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .ok()?;
    let url = format!("{}/api.json", models_url().trim_end_matches('/'));

    for attempt in 0..FETCH_RETRIES {
        let mut req = client.get(&url).header("User-Agent", USER_AGENT);
        if let Some(e) = etag {
            req = req.header("If-None-Match", e);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.as_u16() == 304 {
                    tracing::debug!("models.dev: 304 Not Modified");
                    return Some(ConditionalResult::NotModified);
                }
                if status.is_success() {
                    // Capture the new ETag (if any) before consuming the body.
                    let new_etag = resp
                        .headers()
                        .get(reqwest::header::ETAG)
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    match resp.text().await {
                        Ok(body) => match serde_json::from_str::<MdCatalog>(&body) {
                            Ok(c) => {
                                tracing::debug!(
                                    models = c.0.values().map(|p| p.models.len()).sum::<usize>(),
                                    "models.dev: fetched"
                                );
                                return Some(ConditionalResult::Updated(c, new_etag));
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "models.dev: parse failed");
                                return None;
                            }
                        },
                        Err(e) => {
                            tracing::warn!(error = %e, "models.dev: body read failed");
                        }
                    }
                } else {
                    tracing::warn!(status = %status, "models.dev: non-success status");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, attempt, "models.dev: fetch failed");
            }
        }
        if attempt + 1 < FETCH_RETRIES {
            tokio::time::sleep(RETRY_BACKOFF).await;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn md(
        provider: &str,
        model_id: &str,
        cost: Option<(f64, f64)>,
        ctx: f64,
        output: f64,
        reasoning: bool,
    ) -> MdCatalog {
        let mut cat = MdCatalog::default();
        let m = MdModel {
            name: model_id.to_string(),
            family: None,
            reasoning,
            tool_call: false,
            attachment: false,
            temperature: None,
            structured_output: None,
            knowledge: None,
            release_date: None,
            last_updated: None,
            open_weights: None,
            interleaved: None,
            reasoning_options: None,
            limit: MdLimit {
                context: ctx,
                input: None,
                output,
            },
            cost: cost.map(|(i, o)| MdCost {
                input: i,
                output: o,
                cache_read: None,
                cache_write: None,
                tiers: None,
                context_over_200k: None,
                reasoning: None,
                input_audio: None,
                output_audio: None,
            }),
            modalities: None,
            status: None,
            provider: None,
        };
        let mut models = BTreeMap::new();
        models.insert(model_id.to_string(), m);
        cat.0.insert(
            provider.to_string(),
            MdProvider {
                name: provider.to_string(),
                env: vec![],
                npm: None,
                api: None,
                doc: None,
                models,
            },
        );
        cat
    }

    #[test]
    fn schema_parses_snapshot() {
        // Minimal valid api.json shape.
        let json = r#"{
            "deepseek": {
                "id": "deepseek",
                "name": "DeepSeek",
                "env": ["DEEPSEEK_API_KEY"],
                "npm": "@ai-sdk/openai-compatible",
                "api": "https://api.deepseek.com",
                "models": {
                    "deepseek-chat": {
                        "id": "deepseek-chat",
                        "name": "DeepSeek Chat",
                        "release_date": "2025-12-01",
                        "attachment": true,
                        "reasoning": false,
                        "tool_call": true,
                        "temperature": true,
                        "limit": { "context": 1000000, "output": 384000 },
                        "cost": { "input": 0.14, "output": 0.28, "cache_read": 0.0028 }
                    }
                }
            }
        }"#;
        let cat: MdCatalog = serde_json::from_str(json).unwrap();
        let m = &cat.0["deepseek"].models["deepseek-chat"];
        assert!((m.cost.as_ref().unwrap().input - 0.14).abs() < 1e-9);
        assert_eq!(m.limit.context, 1000000.0);
        assert_eq!(m.limit.output, 384000.0);
    }

    #[test]
    fn write_cache_roundtrips() {
        let cat = md(
            "deepseek",
            "deepseek-chat",
            Some((0.14, 0.28)),
            1000000.0,
            384000.0,
            false,
        );
        let tmp = std::env::temp_dir().join(format!("oxicode-md-test-{}.json", std::process::id()));
        let body = serde_json::to_string(&cat).unwrap();
        std::fs::write(&tmp, &body).unwrap();
        let back: MdCatalog =
            serde_json::from_str(&std::fs::read_to_string(&tmp).unwrap()).unwrap();
        let _ = std::fs::remove_file(&tmp);
        assert!(back.0.contains_key("deepseek"));
    }
}
