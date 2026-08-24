//! End-to-end integration test for the models.dev materialize pipeline.
//!
//! Verifies that a cache fixture flows through `init_models_dev` →
//! `model_db::get_model_entry` and that the catalog reflects models.dev
//! data (pricing, context window, reasoning). Runs fully offline by
//! pointing the cache at a temp fixture and disabling live fetch.
//!
//! This is a single test in its own binary because the global `MODELS_DEV`
//! `OnceLock` is process-scoped — sharing a binary with other tests would
//! race on initialization.

use std::io::Write;

use oxicode_ai::model_db::get_model_entry;

/// Minimal models.dev `api.json` fixture covering the cases we assert.
/// Includes deepseek, openai, and anthropic so enrichment is exercised
/// across multiple providers and the provider_id mapping collapse
/// (oxicode's `openai-responses`/`openai-completions` all map to md's `openai`).
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
async fn models_dev_cache_loads_and_model_db_serves_snap() {
    // Write the fixture to a temp file and point the cache at it. Disable
    // live fetch so the test is fully offline and deterministic.
    let mut tmp = tempfile::NamedTempFile::new().expect("temp file");
    tmp.write_all(FIXTURE.as_bytes()).expect("write fixture");
    let (_permit, path) = tmp.keep().expect("keep temp file");

    // SAFETY on env vars: this binary owns its process; no other test runs here.
    // The cache file's mtime is fresh, so the TTL check passes.
    safety::set_env("OXICODE_MODELS_DEV", "on");
    safety::set_env("OXICODE_MODELS_DEV_DISABLE_FETCH", "1");
    safety::set_env(
        "OXICODE_MODELS_DEV_CACHE_PATH",
        path.to_str().expect("utf8 path"),
    );

    // LIVE layer: init_models_dev reads the cache fixture and exposes it
    // via models_dev::get().
    oxicode_ai::catalog::models_dev::init_models_dev().await;
    let md = oxicode_ai::catalog::models_dev::get().expect("cache fixture loaded");
    let ds = md.0.get("deepseek").expect("fixture deepseek provider");
    let chat = ds.models.get("deepseek-chat").expect("fixture model");
    assert_eq!(chat.limit.context as u32, 1_000_000);
    assert!((chat.cost.as_ref().expect("cost").input - 0.14).abs() < 1e-9);
    let sonnet35 =
        md.0.get("anthropic")
            .expect("fixture anthropic provider")
            .models
            .get("claude-3-5-sonnet-20241022")
            .expect("fixture legacy model present in cache layer");
    assert_eq!(sonnet35.limit.context as u32, 200_000);

    // model_db serves the embedded SNAP (the legacy per-entry enrichment
    // was removed with the TOML catalog — see the NOTE in models_dev.rs).
    // models.dev retired the claude-3.5 ids upstream, so the fixture-only
    // id must NOT appear in model_db, while current ids do.
    assert!(
        get_model_entry("anthropic", "claude-3-5-sonnet-20241022").is_none(),
        "retired ids stay out of the embedded SNAP table"
    );
    let sonnet = get_model_entry("anthropic", "claude-sonnet-4-6")
        .expect("claude-sonnet-4-6 present in SNAP");
    assert_eq!(sonnet.provider, "anthropic");
    assert!(sonnet.context_window >= 200_000);

    // deepseek values flow from the embedded snapshot.
    let chat_snap =
        get_model_entry("deepseek", "deepseek-chat").expect("deepseek-chat present in SNAP");
    assert_eq!(chat_snap.context_window, 1_000_000);
    assert_eq!(chat_snap.max_tokens, 384_000);

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
