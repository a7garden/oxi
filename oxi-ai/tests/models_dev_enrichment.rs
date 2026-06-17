//! End-to-end integration test for the models.dev enrichment pipeline.
//!
//! Verifies that a cache fixture flows through `init_models_dev` →
//! `model_db::get_model_entry` and that Layer 1 entries are enriched with
//! models.dev data (pricing, context window, reasoning). Runs fully
//! offline by pointing the cache at a temp fixture and disabling live fetch.
//!
//! This is a single test in its own binary because the global `MODELS_DEV`
//! `OnceLock` is process-scoped — sharing a binary with other tests would
//! race on initialization.

use std::io::Write;

use oxi_ai::model_db::get_model_entry;

/// Minimal models.dev `api.json` fixture covering the cases we assert.
/// Includes deepseek, openai, and anthropic so enrichment is exercised
/// across multiple providers and the provider_id mapping collapse
/// (oxi's `openai-responses`/`openai-completions` all map to md's `openai`).
const FIXTURE: &str = r#"{
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
            },
            "deepseek-reasoner": {
                "id": "deepseek-reasoner",
                "name": "DeepSeek Reasoner",
                "release_date": "2025-12-01",
                "attachment": true,
                "reasoning": true,
                "tool_call": true,
                "temperature": true,
                "limit": { "context": 1000000, "output": 384000 },
                "cost": { "input": 0.14, "output": 0.28, "cache_read": 0.0028 }
            }
        }
    },
    "openai": {
        "id": "openai",
        "name": "OpenAI",
        "env": ["OPENAI_API_KEY"],
        "npm": "@ai-sdk/openai",
        "api": "https://api.openai.com/v1",
        "models": {
            "gpt-4o": {
                "id": "gpt-4o",
                "name": "GPT-4o",
                "release_date": "2024-08-06",
                "attachment": true,
                "reasoning": false,
                "tool_call": true,
                "temperature": true,
                "limit": { "context": 128000, "output": 16384 },
                "cost": { "input": 2.5, "output": 10.0, "cache_read": 1.25 }
            }
        }
    },
    "anthropic": {
        "id": "anthropic",
        "name": "Anthropic",
        "env": ["ANTHROPIC_API_KEY"],
        "npm": "@ai-sdk/anthropic",
        "api": "https://api.anthropic.com",
        "models": {
            "claude-3-5-sonnet-20241022": {
                "id": "claude-3-5-sonnet-20241022",
                "name": "Claude Sonnet 3.5 v2",
                "release_date": "2024-10-22",
                "attachment": true,
                "reasoning": false,
                "tool_call": true,
                "temperature": true,
                "limit": { "context": 200000, "output": 8192 },
                "cost": { "input": 3.0, "output": 15.0, "cache_read": 0.3, "cache_write": 3.75 }
            }
        }
    }
}"#;

#[tokio::test]
async fn cache_fixture_enriches_model_db() {
    // Write the fixture to a temp file and point the cache at it. Disable
    // live fetch so the test is fully offline and deterministic.
    let mut tmp = tempfile::NamedTempFile::new().expect("temp file");
    tmp.write_all(FIXTURE.as_bytes()).expect("write fixture");
    let (_permit, path) = tmp.keep().expect("keep temp file");

    // SAFETY on env vars: this binary owns its process; no other test runs here.
    // The cache file's mtime is fresh, so the TTL check passes.
    safety::set_env("OXI_MODELS_DEV", "on");
    safety::set_env("OXI_MODELS_DEV_DISABLE_FETCH", "1");
    safety::set_env(
        "OXI_MODELS_DEV_CACHE_PATH",
        path.to_str().expect("utf8 path"),
    );

    oxi_ai::catalog::models_dev::init_models_dev().await;

    // deepseek-chat: Layer 1 ships context=131072, max=8192, cost=0.28/0.42.
    // Enrichment should overwrite all of these with models.dev values.
    let chat =
        get_model_entry("deepseek", "deepseek-chat").expect("deepseek-chat present in Layer 1");
    assert_eq!(
        chat.context_window, 1_000_000,
        "context_window enriched from 131072 → 1000000"
    );
    assert_eq!(
        chat.max_tokens, 384_000,
        "max_tokens enriched from 8192 → 384000"
    );
    assert!(
        (chat.cost_input - 0.14).abs() < 1e-9,
        "cost_input enriched 0.28 → 0.14"
    );
    assert!(
        (chat.cost_output - 0.28).abs() < 1e-9,
        "cost_output enriched 0.42 → 0.28"
    );

    // deepseek-reasoner: reasoning flag should already be true in Layer 1;
    // enrichment keeps it true. Limits also enriched.
    let reasoner = get_model_entry("deepseek", "deepseek-reasoner")
        .expect("deepseek-reasoner present in Layer 1");
    assert!(reasoner.reasoning, "reasoning preserved");
    assert_eq!(reasoner.context_window, 1_000_000);
    assert_eq!(reasoner.max_tokens, 384_000);

    // openai/gpt-4o: provider-id mapping collapse (oxi `openai` → md
    // `openai` directly; also covers openai-responses/openai-codex paths
    // since PROVIDER_MAP collapses all of them). Layer 1 ships cost=2.5/10
    // and ctx=128k — all match, but we still verify the wiring flows.
    let gpt4o = get_model_entry("openai", "gpt-4o").expect("gpt-4o present");
    assert!((gpt4o.cost_input - 2.5).abs() < 1e-9);
    assert!((gpt4o.cost_output - 10.0).abs() < 1e-9);
    assert!((gpt4o.cost_cache_read - 1.25).abs() < 1e-9);

    // anthropic/claude-3-5-sonnet-20241022: this is the canonical "Layer 1
    // ships 0.0" case (oxi-original anthropic.toml has cost_input=0.0).
    // After enrichment it must have a non-zero price.
    let sonnet = get_model_entry("anthropic", "claude-3-5-sonnet-20241022")
        .expect("claude-3-5-sonnet present");
    assert!(
        sonnet.cost_input > 0.0,
        "anthropic cost_input must be enriched past Layer 1's 0.0, got {}",
        sonnet.cost_input
    );
    assert!((sonnet.cost_input - 3.0).abs() < 1e-9);
    assert!((sonnet.cost_output - 15.0).abs() < 1e-9);
    assert!((sonnet.cost_cache_read - 0.3).abs() < 1e-9);
    assert!((sonnet.cost_cache_write - 3.75).abs() < 1e-9);
    assert_eq!(sonnet.context_window, 200_000);
    assert_eq!(sonnet.max_tokens, 8192);

    let _ = std::fs::remove_file(&path);
}

/// Env-var helpers isolated so the unsafe `set_env` calls are easy to audit.
/// `std::env::set_var` is unsafe on edition 2024; we acknowledge it here.
mod safety {
    #![allow(unsafe_code)]
    pub fn set_env(k: &str, v: &str) {
        // SAFETY: this test binary is single-threaded until the tokio runtime
        // starts, and no other test shares this process. The vars are read
        // only by `init_models_dev` below.
        unsafe {
            std::env::set_var(k, v);
        }
    }
}
