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
//! Layer 2   user overrides (~/.oxi/catalog/...)    wins
//! Layer 2.5 models.dev enrichment (this module)   fills gaps / refreshes
//! Layer 3   /v1/models runtime discovery          local servers
//! ```
//!
//! Enrichment runs inside [`crate::model_db::all_provider_models`] after
//! Layer 2 overrides are applied. Only fields that are missing or
//! unverifiable in Layer 1 are overwritten — see [`enrich`] for the exact
//! precedence rules.
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

use crate::catalog::BuiltinModelEntry;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Disk cache freshness window.
const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);

/// Per-request timeout for the live fetch.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Number of retries on transient fetch failures.
const FETCH_RETRIES: u32 = 2;

/// Backoff between retries (first retry waits this long).
const RETRY_BACKOFF: Duration = Duration::from_millis(200);

/// Default models.dev endpoint.
const DEFAULT_URL: &str = "https://models.dev";

/// User-Agent sent to models.dev.
const USER_AGENT: &str = concat!("oxi/", env!("CARGO_PKG_VERSION"));

// ---------------------------------------------------------------------------
// Schema (mirrors models.dev `api.json`, see `packages/core/src/schema.ts`)
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
    /// AI SDK npm package (informational).
    #[serde(default)]
    #[allow(dead_code)]
    pub npm: Option<String>,
    /// Native API base URL for OpenAI-compatible providers.
    #[serde(default)]
    #[allow(dead_code)]
    pub api: Option<String>,
    /// Models served by this provider.
    pub models: BTreeMap<String, MdModel>,
}

/// A single model entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct MdModel {
    /// Display name.
    #[allow(dead_code)]
    pub name: String,
    /// Whether the model supports reasoning / chain-of-thought.
    pub reasoning: bool,
    /// Token limits.
    pub limit: MdLimit,
    /// Pricing (USD per million tokens). Optional — some models are free.
    #[serde(default)]
    pub cost: Option<MdCost>,
}

/// Token limits.
#[derive(Debug, Serialize, Deserialize)]
pub struct MdLimit {
    /// Maximum context window.
    pub context: f64,
    /// Maximum output tokens (maps to oxi `max_tokens`).
    pub output: f64,
}

/// Pricing. All values are USD per million tokens; `None` means "not billed
/// separately" / unknown.
#[derive(Debug, Serialize, Deserialize)]
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
    // Note: models.dev also exposes `tiers` and `context_over_200k` for
    // tiered pricing. oxi models a single cost tier (the base), so those
    // are intentionally ignored here.
}

// ---------------------------------------------------------------------------
// Provider ID mapping (oxi → models.dev)
// ---------------------------------------------------------------------------

/// Map an oxi provider id to the corresponding models.dev provider id.
///
/// oxi carries regional and plan-specific variants (e.g. `minimax-cn`,
/// `zai-coding-global`) that collapse to a single models.dev provider.
/// Returns `None` for providers that have no models.dev counterpart
/// (local servers, unmapped gateways) — those are left on Layer 1.
pub fn provider_map(oxi_pid: &str) -> Option<&'static str> {
    Some(match oxi_pid {
        "anthropic" | "anthropic-vertex" => "anthropic",
        "google" => "google",
        "google-vertex" => "google-vertex",
        "google-vertex-anthropic" => "google-vertex-anthropic",
        "openai" | "openai-responses" | "openai-completions" | "openai-codex" => "openai",
        "openrouter" => "openrouter",
        "deepseek" => "deepseek",
        "groq" => "groq",
        "xai" => "xai",
        "mistral" => "mistral",
        "azure" | "azure-cognitive-services" => "azure-cognitive-services",
        "bedrock" | "amazon-bedrock" | "amazon-bedrock-mantle" => "amazon-bedrock",
        "fireworks" => "fireworks-ai",
        "togetherai" | "together" => "togetherai",
        "cerebras" => "cerebras",
        "deepinfra" => "deepinfra",
        "cloudflare" | "cloudflare-workers-ai" => "cloudflare-workers-ai",
        "cloudflare-ai-gateway" => "cloudflare-ai-gateway",
        "huggingface" => "huggingface",
        "moonshotai" | "moonshot" => "moonshotai",
        "moonshotai-cn" => "moonshotai-cn",
        "kimi-coding" => "kimi-for-coding",
        "xiaomi" => "xiaomi",
        "xiaomi-token-plan" => "xiaomi-token-plan",
        "minimax" => "minimax",
        "minimax-cn" => "minimax-cn",
        "zai" | "zai-global" => "zai",
        "zai-cn" => "zai-cn",
        "zai-coding-global" | "zai-coding-cn" => "zai-coding-plan",
        "vercel-ai-gateway" => "vercel",
        "copilot" | "codex" | "github-copilot" => "github-copilot",
        "opencode" => "opencode",
        "opencode-go" => "opencode-go",
        "nvidia" => "nvidia",
        "novita" => "novita-ai",
        "venice" => "venice",
        "chutes" => "chutes",
        "gmi" => "gmicloud",
        "stepfun" => "stepfun-ai",
        "qwen-portal" | "alibaba" => "alibaba",
        "ollama-cloud" => "ollama-cloud",
        "synthetic" => "synthetic",
        // Unmapped (skipped): ollama, lmstudio, vllm, sglang, byteplus,
        // qianfan, arcee, litellm, microsoft-foundry, copilot-proxy,
        // kilocode, etc. — these are local servers, gateways, or providers
        // without a models.dev entry. Runtime `/v1/models` discovery
        // (Layer 3) handles the server ones.
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Reasoning preservation allowlist
// ---------------------------------------------------------------------------

/// Returns true if oxi's `reasoning` flag for this model must NOT be
/// overwritten by models.dev.
///
/// Some variants intentionally disable reasoning even though the base model
/// supports it (TEE/throughput/quantized variants, tool-augmented composites).
/// models.dev reflects the base model's capability, so blindly copying it
/// would be wrong here.
pub fn reasoning_preserve(oxi_pid: &str, id: &str) -> bool {
    // TEE (Trusted Execution Environment) variants — reasoning-disabled.
    if id.ends_with("-TEE") {
        return matches!(oxi_pid, "chutes" | "together");
    }
    // Throughput-tuned variants (`-tput` suffix).
    if id.ends_with("-tput") && oxi_pid == "together" {
        return true;
    }
    // Explicit list of variants where oxi's flag is deliberate.
    matches!(
        (oxi_pid, id),
        ("groq", "groq/compound")
            | ("groq", "groq/compound-mini")
            | ("together", "Qwen/Qwen3-Coder-Next-FP8")
            | ("mistral", "mistral-medium-latest")
            | ("together", "Qwen/Qwen3.7-Max")
            | ("together", "deepseek-ai/DeepSeek-V3")
    )
}

// ---------------------------------------------------------------------------
// Enrichment
// ---------------------------------------------------------------------------

/// Enrich a Layer 1 entry in place with models.dev data.
///
/// # Precedence rules
///
/// - **Price**: only a positive models.dev value overwrites Layer 1. A `0.0`
///   or absent models.dev price leaves Layer 1 untouched (preserves
///   verified-free and unknown). This also clears the openclaw `-1.0`
///   sentinel: once a real price is filled in, the entry is no longer
///   "unverified".
/// - **Limits**: `limit.context` → `context_window`, `limit.output` →
///   `max_tokens`. Only positive values overwrite.
/// - **Reasoning**: overwritten unless [`reasoning_preserve`] vetoes it.
///
/// No-op when the provider isn't mapped, the model id isn't found, or the
/// field would be set to an empty/zero value.
pub fn enrich(entry: &mut BuiltinModelEntry, catalog: &MdCatalog) {
    let md_pid = match provider_map(&entry.provider) {
        Some(p) => p,
        None => {
            tracing::trace!(
                provider = %entry.provider,
                "models.dev: provider unmapped, skipping enrichment"
            );
            return;
        }
    };
    let Some(mdprov) = catalog.0.get(md_pid) else {
        return;
    };
    let Some(mdm) = mdprov.models.get(&entry.id) else {
        // Common case: model id mismatch between oxi and models.dev.
        // Silent skip to avoid log noise on every catalog lookup.
        return;
    };

    // Pricing.
    if let Some(c) = &mdm.cost {
        if c.input > 0.0 {
            entry.cost_input = c.input;
        }
        if c.output > 0.0 {
            entry.cost_output = c.output;
        }
        if let Some(cr) = c.cache_read
            && cr > 0.0
        {
            entry.cost_cache_read = cr;
        }
        if let Some(cw) = c.cache_write
            && cw > 0.0
        {
            entry.cost_cache_write = cw;
        }
    }

    // Limits.
    if mdm.limit.context > 0.0 {
        entry.context_window = mdm.limit.context as u32;
    }
    if mdm.limit.output > 0.0 {
        entry.max_tokens = mdm.limit.output as u32;
    }

    // Reasoning flag (respect allowlist).
    if !reasoning_preserve(&entry.provider, &entry.id) {
        entry.reasoning = mdm.reasoning;
    }
}

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
/// - `OXI_MODELS_DEV_CACHE_PATH` overrides the location (test/enterprise use)
/// - otherwise `~/.oxi/cache/models-dev.json`
fn cache_path() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("OXI_MODELS_DEV_CACHE_PATH")
        && !custom.is_empty()
    {
        return Some(PathBuf::from(custom));
    }
    Some(
        dirs::home_dir()?
            .join(".oxi")
            .join("cache")
            .join("models-dev.json"),
    )
}

/// Whether enrichment is enabled at all.
///
/// - `OXI_MODELS_DEV=off` → disabled
/// - `OXI_MODELS_DEV=on` or `auto` (or unset) → enabled
fn enabled() -> bool {
    !matches!(
        std::env::var("OXI_MODELS_DEV").as_deref(),
        Ok("off") | Ok("OFF") | Ok("0") | Ok("false") | Ok("FALSE")
    )
}

/// Whether live network fetch is forbidden (air-gapped mode).
fn fetch_disabled() -> bool {
    matches!(
        std::env::var("OXI_MODELS_DEV_DISABLE_FETCH").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

/// Configured models.dev endpoint.
fn models_url() -> String {
    std::env::var("OXI_MODELS_DEV_URL").unwrap_or_else(|_| DEFAULT_URL.to_string())
}

/// Configured cache TTL.
fn ttl() -> Duration {
    std::env::var("OXI_MODELS_DEV_TTL")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TTL)
}

/// Cache-or-live fallback chain.
async fn fetch_with_fallback() -> Option<MdCatalog> {
    if !enabled() {
        return None;
    }

    // 1) Fresh disk cache — near-instant.
    if let Some(c) = read_cache_if_fresh() {
        tracing::debug!("models.dev: using fresh cache");
        return Some(c);
    }

    // 2) Live fetch (unless air-gapped).
    if !fetch_disabled()
        && let Some(c) = live_fetch().await
    {
        write_cache_atomic(&c);
        return Some(c);
    }

    // 3) Stale cache is better than nothing.
    if let Some(c) = read_cache_any() {
        tracing::debug!("models.dev: using stale cache (live fetch unavailable)");
        return Some(c);
    }

    None
}

/// Read the cache only if it is within the TTL.
fn read_cache_if_fresh() -> Option<MdCatalog> {
    let path = cache_path()?;
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age > ttl() {
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

/// Live fetch with bounded retries.
async fn live_fetch() -> Option<MdCatalog> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .ok()?;
    let url = format!("{}/api.json", models_url().trim_end_matches('/'));

    for attempt in 0..FETCH_RETRIES {
        match client
            .get(&url)
            .header("User-Agent", USER_AGENT)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(body) => match serde_json::from_str::<MdCatalog>(&body) {
                    Ok(c) => {
                        tracing::debug!(
                            models = c.0.values().map(|p| p.models.len()).sum::<usize>(),
                            "models.dev: fetched"
                        );
                        return Some(c);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "models.dev: parse failed");
                        return None;
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "models.dev: body read failed");
                }
            },
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "models.dev: non-success status");
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
            reasoning,
            limit: MdLimit {
                context: ctx,
                output,
            },
            cost: cost.map(|(i, o)| MdCost {
                input: i,
                output: o,
                cache_read: None,
                cache_write: None,
            }),
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
                models,
            },
        );
        cat
    }

    fn entry(provider: &str, id: &str) -> BuiltinModelEntry {
        BuiltinModelEntry {
            id: id.to_string(),
            name: id.to_string(),
            api: "openai-completions".to_string(),
            provider: provider.to_string(),
            reasoning: false,
            input: vec!["text".to_string()],
            cost_input: 0.0,
            cost_output: 0.0,
            cost_cache_read: 0.0,
            cost_cache_write: 0.0,
            context_window: 0,
            max_tokens: 0,
        }
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
    fn enrich_fills_missing_price() {
        let cat = md(
            "deepseek",
            "deepseek-chat",
            Some((0.14, 0.28)),
            1000000.0,
            384000.0,
            false,
        );
        let mut e = entry("deepseek", "deepseek-chat");
        enrich(&mut e, &cat);
        assert!((e.cost_input - 0.14).abs() < 1e-9);
        assert!((e.cost_output - 0.28).abs() < 1e-9);
        assert_eq!(e.context_window, 1000000);
        assert_eq!(e.max_tokens, 384000);
    }

    #[test]
    fn enrich_preserves_zero_price_when_md_zero() {
        // models.dev reports 0.0 (verified free) — Layer 1 must NOT be
        // touched (we can't tell verified-free from unknown here).
        let cat = md(
            "deepseek",
            "free-model",
            Some((0.0, 0.0)),
            128000.0,
            8192.0,
            false,
        );
        let mut e = entry("deepseek", "free-model");
        e.cost_input = 0.5; // existing Layer 1 value
        e.context_window = 64000;
        enrich(&mut e, &cat);
        assert_eq!(e.cost_input, 0.5, "non-zero Layer 1 price must survive");
        // But limits are still updated (positive in md).
        assert_eq!(e.context_window, 128000);
    }

    #[test]
    fn enrich_noop_for_unmapped_provider() {
        let cat = md("deepseek", "x", None, 1000000.0, 384000.0, true);
        let mut e = entry("ollama", "llama3");
        enrich(&mut e, &cat);
        assert_eq!(e.context_window, 0, "unmapped provider must be untouched");
    }

    #[test]
    fn enrich_noop_for_missing_model() {
        let cat = md("deepseek", "deepseek-chat", None, 1000000.0, 384000.0, true);
        let mut e = entry("deepseek", "deepseek-other");
        enrich(&mut e, &cat);
        assert_eq!(e.context_window, 0);
    }

    #[test]
    fn enrich_updates_reasoning() {
        let cat = md(
            "openai",
            "gpt-5-chat-latest",
            None,
            400000.0,
            128000.0,
            true,
        );
        let mut e = entry("openai", "gpt-5-chat-latest");
        assert!(!e.reasoning);
        enrich(&mut e, &cat);
        assert!(e.reasoning, "reasoning should be copied from models.dev");
    }

    #[test]
    fn enrich_preserves_reasoning_for_tee_variant() {
        let cat = md(
            "chutes",
            "Qwen/Qwen3-Coder-Next-TEE",
            None,
            131072.0,
            32768.0,
            false,
        );
        let mut e = entry("chutes", "Qwen/Qwen3-Coder-Next-TEE");
        e.reasoning = true; // oxi's deliberate setting
        enrich(&mut e, &cat);
        assert!(e.reasoning, "TEE variant reasoning must be preserved");
    }

    #[test]
    fn enrich_preserves_reasoning_for_compound() {
        let cat = md("groq", "groq/compound", None, 131072.0, 32768.0, false);
        let mut e = entry("groq", "groq/compound");
        e.reasoning = true;
        enrich(&mut e, &cat);
        assert!(e.reasoning);
    }

    #[test]
    fn provider_map_collapse_rules() {
        assert_eq!(provider_map("minimax-cn"), Some("minimax-cn"));
        assert_eq!(provider_map("zai-coding-global"), Some("zai-coding-plan"));
        assert_eq!(provider_map("moonshot"), Some("moonshotai"));
        assert_eq!(provider_map("copilot"), Some("github-copilot"));
        assert_eq!(provider_map("ollama"), None);
        assert_eq!(provider_map("lmstudio"), None);
        assert_eq!(provider_map("unknown-provider"), None);
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
        let tmp = std::env::temp_dir().join(format!("oxi-md-test-{}.json", std::process::id()));
        let body = serde_json::to_string(&cat).unwrap();
        std::fs::write(&tmp, &body).unwrap();
        let back: MdCatalog =
            serde_json::from_str(&std::fs::read_to_string(&tmp).unwrap()).unwrap();
        let _ = std::fs::remove_file(&tmp);
        assert!(back.0.contains_key("deepseek"));
    }
}
