//! oxi-proxy — OpenAI-compatible proxy for grok TUI.
//!
//! Receives grok's LLM API requests (OpenAI Chat Completions format),
//! routes them through oxi-ai's Provider trait, and streams responses back.
//!
//! This lets grok's TUI use ANY oxi-ai provider (OpenAI, Anthropic, Google,
//! Vertex, Mistral, Azure, Bedrock) through a single localhost endpoint.
//!
//! ## Flow
//!
//! ```text
//! grok TUI → POST localhost:PORT/v1/chat/completions → oxi-proxy
//!                                                        ↓
//!                                                   oxi-ai Provider
//!                                                        ↓
//!                                                   LLM API (any)
//!                                                        ↓
//! grok TUI ← SSE stream ← oxi-proxy ← ProviderEvent stream ←
//! ```

mod translate;

use anyhow::Result;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use oxi_ai::{ProviderEvent, ProviderRegistry};
use std::sync::Arc;
use tokio::net::TcpListener;

#[derive(Clone)]
struct ProxyState {
    registry: Arc<ProviderRegistry>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("oxi_proxy=info")
        .init();

    // Build oxi-ai provider registry with all built-in providers
    let mut registry = ProviderRegistry::new();
    registry.register_builtins();
    let registry = Arc::new(registry);

    let state = ProxyState {
        registry: Arc::clone(&registry),
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", post(list_models).get(list_models))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tracing::info!("oxi-proxy listening on http://{addr}");

    // Print the port for the parent process to read
    println!("OXI_PROXY_PORT={}", addr.port());

    axum::serve(listener, app).await?;
    Ok(())
}

/// Handle POST /v1/chat/completions
///
/// grok sends OpenAI-format chat completion requests. We translate to
/// oxi-ai's Context, call the Provider, and stream back as SSE.
async fn chat_completions(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let req: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("Invalid JSON: {e}")).into_response();
        }
    };

    // Extract model name
    let model_name = req
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("gpt-4o")
        .to_string();

    // Resolve provider for this model
    let provider = match state.registry.resolve_provider_for_model(&model_name) {
        Some(p) => p,
        None => {
            tracing::warn!("no provider for model: {model_name}");
            return (
                StatusCode::BAD_REQUEST,
                format!("No provider registered for model: {model_name}"),
            )
                .into_response();
        }
    };

    // Translate OpenAI request → oxi-ai Context
    let model = match state.registry.resolve_model(&model_name) {
        Some(m) => m,
        None => {
            return (StatusCode::BAD_REQUEST, "Unknown model").into_response();
        }
    };

    let context = match translate::openai_request_to_context(&req) {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("Translation error: {e}"))
                .into_response();
        }
    };

    // Check if streaming requested
    let stream = req
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    if stream {
        // Stream response as SSE
        match provider.stream(&model, &context, None).await {
            Ok(event_stream) => {
                let sse = translate::provider_events_to_sse(event_stream, &model_name);
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .header("cache-control", "no-cache")
                    .header("connection", "keep-alive")
                    .body(axum::body::Body::from_stream(sse))
                    .unwrap()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Provider error: {e:?}"),
            )
                .into_response(),
        }
    } else {
        // Non-streaming: collect all events and return as JSON
        match provider.stream(&model, &context, None).await {
            Ok(event_stream) => {
                match translate::collect_provider_response(event_stream, &model_name).await {
                    Ok(json) => Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(json.to_string()))
                        .unwrap(),
                    Err(e) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Collection error: {e}"),
                    )
                        .into_response(),
                }
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Provider error: {e:?}"),
            )
                .into_response(),
        }
    }
}

/// Handle GET/POST /v1/models — return available models.
async fn list_models(State(state): State<ProxyState>) -> impl IntoResponse {
    let models = state.registry.list_model_ids();
    let response = serde_json::json!({
        "object": "list",
        "data": models.iter().map(|id| {
            serde_json::json!({
                "id": id,
                "object": "model",
                "owned_by": "oxi",
            })
        }).collect::<Vec<_>>(),
    });
    (StatusCode::OK, axum::Json(response))
}
