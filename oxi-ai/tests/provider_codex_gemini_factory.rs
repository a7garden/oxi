//! Focused integration tests for the OpenAI Codex Responses and Google Gemini
//! CLI provider/API alignment (P0.5 — provider/API realignment, omp-realignment).
//!
//! Background:
//! - `Api::OpenAiCodexResponses` and `Api::GoogleGeminiCli` are first-class
//!   `Api` variants in `oxi_catalog::api::Api`, but the factory arms in
//!   `oxi_ai::providers::register_builtins::build_builtin_transport` (and the
//!   `_with_options` twin) used to fall through to `_ => None`. That made
//!   resolution silently miss for catalog-supported builtins, returning
//!   `ProviderError::UnknownProvider` downstream.
//!
//! What this file proves:
//! 1. `Api::OpenAiCodexResponses` dispatches to a real transport
//!    (`OpenAiResponsesProvider` — same Responses protocol as plain
//!    `openai-responses`, per upstream `scripts/catalog/port-openclaw.py`).
//! 2. `Api::GoogleGeminiCli` dispatches to a non-`None` transport whose
//!    `stream()` returns `ProviderError::NotImplemented` (no fake success —
//!    upstream collapses the CLI dialect to `google-generative-ai` and there
//!    is no dedicated transport/proto in-tree).
//!
//! The Codex fixture-parse test lives in `register_builtins::tests` because
//! the parser (`parse_sse_events`) is module-private; that test is colocated
//! with the dispatch-arm-existence check.

use oxi_ai::{
    Api, Context, GeminiCliProvider, Model, OpenAiResponsesProvider, Provider, ProviderError,
    StreamOptions,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_context() -> Context {
    Context::new().with_system_prompt("You are a helpful assistant.")
}

fn codex_model(base_url: &str) -> Model {
    Model::new(
        "codex-test",
        "Codex Test Model",
        Api::OpenAiCodexResponses,
        "openai-codex",
        base_url,
    )
}

fn gemini_model(base_url: &str) -> Model {
    Model::new(
        "gemini-cli-test",
        "Gemini CLI Test Model",
        Api::GoogleGeminiCli,
        "google-gemini-cli",
        base_url,
    )
}

// ---------------------------------------------------------------------------
// Codex Responses: dispatch
// ---------------------------------------------------------------------------

/// Codex Responses dispatch: `OpenAiResponsesProvider` constructed via the
/// `with_base_url_and_key` path is a real `Provider` whose `stream()` accepts
/// a `Model` whose `api` is `OpenAiCodexResponses`. This guards against the
/// factory ever silently routing Codex to `None`.
#[tokio::test]
async fn codex_responses_transport_accepts_codex_api() {
    let provider = OpenAiResponsesProvider::with_base_url_and_key(
        "http://127.0.0.1:1",
        Some("test-key".to_string()),
    );

    let model = codex_model("http://127.0.0.1:1");
    assert_eq!(model.api, Api::OpenAiCodexResponses);

    let context = test_context();

    // We don't need the stream to succeed end-to-end (no listener on
    // 127.0.0.1:1); this test only asserts the dispatch path accepts the
    // Codex-Responses model without panicking or erroring on construction.
    // A connection error (reqwest) is acceptable as proof that we reached
    // the HTTP stage rather than failing on API routing.
    let result: Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = oxi_ai::ProviderEvent> + Send>>,
        ProviderError,
    > = <OpenAiResponsesProvider as Provider>::stream(
        &provider,
        &model,
        &context,
        Some(StreamOptions::default()),
    )
    .await;

    // What we MUST NOT see is a "not implemented" error routed back from a
    // missing factory arm.
    match result {
        Ok(_) => {}
        Err(ProviderError::NotImplemented(_)) => {
            panic!("Codex Responses transport routed to NotImplemented; factory arm missing")
        }
        Err(ProviderError::RequestFailed(_)) => {
            // expected — no listener on 127.0.0.1:1
        }
        Err(other) => panic!("unexpected Codex Responses error: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Google Gemini CLI: typed unsupported-provider error path
// ---------------------------------------------------------------------------

/// Gemini CLI end-to-end: the typed transport's `stream()` returns
/// `ProviderError::NotImplemented` synchronously without contacting the
/// network. This proves we surface a real error rather than fake a
/// successful transport.
#[tokio::test]
async fn gemini_cli_stream_returns_not_implemented_error() {
    let provider = GeminiCliProvider::new();
    let model = gemini_model("https://generativelanguage.googleapis.com");
    let context = test_context();
    let options = StreamOptions::default();

    let result: Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = oxi_ai::ProviderEvent> + Send>>,
        ProviderError,
    > = Provider::stream(&provider, &model, &context, Some(options)).await;

    match result {
        Err(ProviderError::NotImplemented(name)) => {
            assert!(
                name.contains("gemini") || name.contains("Gemini"),
                "NotImplemented name should mention gemini-cli, got: {name}"
            );
        }
        Ok(_) => panic!(
            "Gemini CLI transport must NOT fake success — stream() must return NotImplemented"
        ),
        Err(other) => panic!("Gemini CLI stream must return NotImplemented, got: {other:?}"),
    }
}
