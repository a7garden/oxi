//! Env-gated smoke tests — exercise real provider paths.
//!
//! These tests run only when ANTHROPIC_API_KEY (or OPENAI_API_KEY) is set.
//! Without a key, they are silently skipped (not failed).

use oxi_sdk::SdkError;
use oxi_sdk::prelude::*;

/// Helper: skip if no Anthropic key.
fn anthropic_key() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Helper: skip if no OpenAI key.
fn openai_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
}

#[test]
fn model_resolution_smoke() {
    if std::env::var("SMOKE").ok().as_deref() != Some("1") {
        return;
    }

    let oxi = OxiBuilder::new().with_builtins().build();
    let model = oxi
        .resolve_model("anthropic/claude-sonnet-4-20250514")
        .expect("built-in Anthropic model should resolve");
    assert_eq!(model.provider, "anthropic");
    assert_eq!(model.id, "claude-sonnet-4-20250514");
    assert_eq!(model.api, oxi_ai::Api::AnthropicMessages);
}

#[tokio::test]
async fn provider_creation_smoke() {
    if anthropic_key().is_none() {
        return;
    }

    let oxi = OxiBuilder::new().with_builtins().build();
    oxi.create_provider("anthropic")
        .expect("Anthropic provider should be created with an API key");
}

#[tokio::test]
async fn agent_run_smoke() {
    if anthropic_key().is_none() {
        return;
    }

    let oxi = OxiBuilder::new().with_builtins().build();
    let agent = oxi
        .agent(AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            ..Default::default()
        })
        .system_prompt("Reply briefly and follow the user's requested format exactly.")
        .build()
        .expect("agent should build with Anthropic credentials");
    let (response, _) = agent
        .run("Reply with exactly: OK".into())
        .await
        .expect("Anthropic request should succeed");
    assert!(!response.content.trim().is_empty());
}

#[test]
fn error_path_smoke() {
    let oxi = OxiBuilder::new().with_builtins().build();
    let err = oxi
        .resolve_model("nonexistent/fake")
        .expect_err("unknown model should fail");
    assert!(matches!(err, SdkError::ModelNotFound { .. }));
}

#[test]
fn provider_not_found_structured() {
    let oxi = OxiBuilder::new().with_builtins().build();
    match oxi.create_provider("nonexistent-provider") {
        Err(err) => assert!(
            matches!(err, SdkError::ProviderNotFound { .. }),
            "expected ProviderNotFound, got: {err:?}"
        ),
        Ok(_) => panic!("unknown provider should fail"),
    }
}

#[allow(dead_code)]
fn _openai_key_helper_is_available() -> Option<String> {
    openai_key()
}
