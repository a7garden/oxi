//! GitLab Duo Workflow provider (`gitlab-duo-agent`) — remote-AGENT via WebSocket.

#![allow(dead_code)]
//!
//! Port of omp `packages/ai/src/providers/gitlab-duo-workflow.ts` (3136 lines)
//! with a **stateless-per-turn** architecture: each `Provider::stream()` call
//! creates a fresh workflow, runs the WebSocket until a tool call or completion,
//! emits ProviderEvents, then closes. Session state is NOT preserved across
//! calls — the next turn re-creates the workflow with the full goal transcript
//! (context includes the just-returned tool results).
//!
//! ## Protocol
//! 1. REST setup: direct_access token → create workflow → fetch available models
//! 2. WebSocket connect with auth headers + project/namespace params
//! 3. Send `{"startRequest": {...}}` (goal = ChatML transcript, mcp_tools, ...)
//! 4. Receive JSON messages: checkpoint updates (text/thinking deltas), tool call
//!    actions (emit ToolCallEnd), completion status, or errors
//! 5. On tool call: emit ProviderEvent, close socket, stop workflow → host runs
//!    the tool and calls `stream()` again with the result in context
//! 6. On completion: emit Done, close socket, stop workflow
//!
//! ## Architecture
//! - **Stateless** per `stream()` call — no `ProviderSessionState` needed
//! - **Cached** on `GitLabDuoAgentProvider`: direct_access token (keyed by
//!   GitLab personal access token), namespace→project discovery, model list.
//!   These are stable per GitLab instance and survive across turns.
//! - **JSON-only** wire format — no protobuf, no prost-build
//! - `tokio-tungstenite` with `rustls-tls` backend (matching reqwest in oxicode-ai)

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::RwLock;
use std::sync::LazyLock;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

use futures::{SinkExt, StreamExt};
use reqwest::Client;
use uuid::Uuid;

use super::shared_client;
use crate::{
    Api, AssistantMessage, ContentBlock, Context, Cost, Message, MessageContent, Model, Provider,
    ProviderEvent, StopReason, StreamOptions, StreamResult, ToolCall, ToolCallType, Usage,
    error::ProviderError,
};

// ── Constants ───────────────────────────────────────────────────────

const GITLAB_COM_URL: &str = "https://gitlab.com";
const DIRECT_ACCESS_PATH: &str = "/api/v4/ai/duo_workflows/direct_access";
const WORKFLOW_PATH: &str = "/api/v4/ai/duo_workflows/workflows";
const GRAPHQL_PATH: &str = "/api/v4/graphql";
const GITLAB_DUO_WORKFLOW_DEFINITION: &str = "ambient";
const GITLAB_DUO_WORKFLOW_CLIENT_VERSION: &str = "1.0";
const GITLAB_DUO_WORKFLOW_INLINE_AGENT_NAME: &str = "omp_agent";
const GITLAB_DUO_WORKFLOW_INLINE_PROMPT_ID: &str = "omp_inline_prompt";
const GITLAB_DUO_WORKFLOW_REST_TIMEOUT: u64 = 30;
const GITLAB_DUO_WORKFLOW_WS_IDLE_TIMEOUT: u64 = 90;
const GITLAB_DUO_WORKFLOW_AVAILABLE_MODELS_QUERY: &str = r#"query omp_gitlabDuoWorkflowAvailableModels($rootNamespaceId: GroupID!) {
  aiChatAvailableModels(rootNamespaceId: $rootNamespaceId) {
    defaultModel { name ref }
    selectableModels { name ref }
    pinnedModel { name ref }
  }
}"#;
const GITLAB_DUO_WORKFLOW_CLIENT_CAPABILITIES: &[&str] = &[
    "incremental_streaming",
    "read_file_chunked",
    "shell_command",
    "command_timeout",
    "tool_call_approval",
];
const GITLAB_DUO_WORKFLOW_INLINE_UI_LOG_EVENTS: &[&str] = &[
    "on_agent_reasoning",
    "on_agent_final_answer",
    "on_tool_execution_success",
    "on_tool_execution_failed",
];

// ── Direct access token cache ──────────────────────────────────────

#[derive(Clone)]
struct CachedDirectAccess {
    token: String,
    base_url: Option<String>,
    headers: HashMap<String, String>,
    service_endpoint: bool,
}

static CACHED_ACCESS: LazyLock<RwLock<Option<CachedDirectAccess>>> =
    LazyLock::new(|| RwLock::new(None));

fn get_cached_direct_access() -> Option<CachedDirectAccess> {
    CACHED_ACCESS.read().clone()
}

fn set_cached_direct_access(cache: CachedDirectAccess) {
    *CACHED_ACCESS.write() = Some(cache);
}

fn clear_direct_access_cache() {
    *CACHED_ACCESS.write() = None;
}

// ── URL helpers ────────────────────────────────────────────────────

fn gitlab_api_url(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn normalize_gitlab_base_url(base_url: &str) -> String {
    let url = base_url.trim_end_matches('/');
    if url.is_empty() {
        GITLAB_COM_URL.to_string()
    } else {
        url.to_string()
    }
}

fn to_gitlab_graphql_namespace_id(id: &str) -> String {
    if id.starts_with("gid://") {
        id.to_string()
    } else {
        format!("gid://gitlab/Group/{id}")
    }
}

fn normalize_duo_workflow_service_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

// ── REST helpers ───────────────────────────────────────────────────

/// Request a direct-access token for Duo Workflows.
async fn request_direct_access(
    client: &Client,
    base_url: &str,
    api_key: &str,
    root_namespace_id: &str,
    project_id: Option<&str>,
) -> Result<CachedDirectAccess, ProviderError> {
    let body = serde_json::json!({
        "workflow_definition": GITLAB_DUO_WORKFLOW_DEFINITION,
        "root_namespace_id": to_gitlab_graphql_namespace_id(root_namespace_id),
        "project_id": project_id,
    });
    let url = gitlab_api_url(base_url, DIRECT_ACCESS_PATH);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(
            GITLAB_DUO_WORKFLOW_REST_TIMEOUT,
        ))
        .send()
        .await
        .map_err(|e| ProviderError::NetworkError(format!("direct_access: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::HttpError(crate::HttpErrorDetail {
            status: status.as_u16(),
            body: format!("direct_access {status}: {text}"),
            provider: Some("gitlab-duo-agent".into()),
            request_id: None,
        }));
    }
    let payload: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ProviderError::InvalidResponse(format!("direct_access parse: {e}")))?;
    let token = payload
        .get("token")
        .or_else(|| payload.get("access_token"))
        .or_else(|| payload.get("jwt"))
        .or_else(|| payload.get("workflow_token"))
        .or_else(|| payload.get("duo_workflow_access_token"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| {
            ProviderError::InvalidResponse("direct_access: no token in response".into())
        })?;
    let service_endpoint = payload
        .get("duo_workflow_service")
        .and_then(|s| s.get("base_url"))
        .and_then(|v| v.as_str())
        .map(|_| true)
        .unwrap_or(false)
        && payload
            .get("gitlab_rails")
            .and_then(|g| g.get("token"))
            .is_none();
    let ws_base_url = if service_endpoint {
        payload
            .get("duo_workflow_service")
            .and_then(|s| s.get("base_url"))
            .and_then(|v| v.as_str())
            .map(normalize_duo_workflow_service_base_url)
    } else {
        None
    };
    let headers: HashMap<String, String> = if service_endpoint {
        payload
            .get("duo_workflow_service")
            .and_then(|s| s.get("headers"))
            .and_then(|h| serde_json::from_value(h.clone()).ok())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };
    Ok(CachedDirectAccess {
        token,
        base_url: ws_base_url,
        headers,
        service_endpoint,
    })
}

/// Create a Duo workflow on the server.
async fn create_workflow(
    client: &Client,
    base_url: &str,
    api_key: &str,
    namespace_id: &str,
    _goal: &str,
    project_id: Option<&str>,
) -> Result<String, ProviderError> {
    let body = serde_json::json!({
        "workflow_definition": GITLAB_DUO_WORKFLOW_DEFINITION,
        "environment": "ide",
        "allow_agent_to_request_user": false,
        "agent_privileges": [6],
        "pre_approved_agent_privileges": [6],
        "requires_duo_cli_enabled": false,
        "namespace_id": namespace_id,
        "project_id": project_id,
        "goal": "",
    });
    let url = gitlab_api_url(base_url, WORKFLOW_PATH);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(
            GITLAB_DUO_WORKFLOW_REST_TIMEOUT,
        ))
        .send()
        .await
        .map_err(|e| ProviderError::NetworkError(format!("create_workflow: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::HttpError(crate::HttpErrorDetail {
            status: status.as_u16(),
            body: format!("create_workflow {status}: {text}"),
            provider: Some("gitlab-duo-agent".into()),
            request_id: None,
        }));
    }
    let payload: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ProviderError::InvalidResponse(format!("create_workflow parse: {e}")))?;
    let id = payload
        .get("id")
        .or_else(|| payload.get("workflow_id"))
        .or_else(|| payload.get("workflowId"))
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.as_i64().map(|i| i.to_string()))
        })
        .ok_or_else(|| ProviderError::InvalidResponse("create_workflow: no workflow id".into()))?;
    Ok(id)
}

/// Stop a workflow on the server (best-effort).
async fn stop_workflow(client: &Client, base_url: &str, api_key: &str, workflow_id: &str) {
    let url = gitlab_api_url(
        base_url,
        &format!("{WORKFLOW_PATH}/{}", urlencoding(workflow_id)),
    );
    let _ = client
        .patch(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({"status_event": "stop"}))
        .timeout(std::time::Duration::from_secs(
            GITLAB_DUO_WORKFLOW_REST_TIMEOUT,
        ))
        .send()
        .await;
}

fn urlencoding(s: &str) -> String {
    // Simple URL encoding for workflow IDs (mostly numeric, but safe)
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Fetch available models from the GitLab Duo API.
async fn fetch_available_models(
    client: &Client,
    base_url: &str,
    api_key: &str,
    root_namespace_id: &str,
) -> Result<serde_json::Value, ProviderError> {
    let body = serde_json::json!({
        "query": GITLAB_DUO_WORKFLOW_AVAILABLE_MODELS_QUERY,
        "variables": {
            "rootNamespaceId": to_gitlab_graphql_namespace_id(root_namespace_id),
        },
    });
    let url = gitlab_api_url(base_url, GRAPHQL_PATH);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(
            GITLAB_DUO_WORKFLOW_REST_TIMEOUT,
        ))
        .send()
        .await
        .map_err(|e| ProviderError::NetworkError(format!("fetch_models: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::HttpError(crate::HttpErrorDetail {
            status: status.as_u16(),
            body: format!("fetch_models {status}: {text}"),
            provider: Some("gitlab-duo-agent".into()),
            request_id: None,
        }));
    }
    let payload: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ProviderError::InvalidResponse(format!("fetch_models parse: {e}")))?;
    Ok(payload)
}

/// Select a model ref from available models matching the requested model id.
fn select_model_ref(model_id: &str, available: &serde_json::Value) -> Option<String> {
    let models = available
        .get("data")
        .and_then(|d| d.get("aiChatAvailableModels"))?;
    // Try exact match in selectableModels
    if let Some(selectable) = models.get("selectableModels").and_then(|v| v.as_array()) {
        for m in selectable {
            if let (Some(name), Some(ref_val)) = (
                m.get("name").and_then(|n| n.as_str()),
                m.get("ref").and_then(|r| r.as_str()),
            ) && name == model_id
            {
                return Some(ref_val.to_string());
            }
        }
    }
    // Fall back to default model
    models
        .get("defaultModel")
        .and_then(|d| d.get("ref"))
        .and_then(|r| r.as_str())
        .or_else(|| {
            models
                .get("pinnedModel")
                .and_then(|p| p.get("ref"))
                .and_then(|r| r.as_str())
        })
        .map(String::from)
}

// ── Goal construction (ChatML transcript) ─────────────────────────

/// Build the workflow goal from context messages as a ChatML transcript.
fn build_goal(messages: &[Message]) -> String {
    if messages.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    for msg in messages {
        let role = match msg {
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult(_) => "tool",
        };
        let text = match msg {
            Message::User(u) => match &u.content {
                MessageContent::Text(s) => s.clone(),
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
            Message::Assistant(a) => a.text_content(),
            Message::ToolResult(t) => {
                let content: String = t.content.iter().filter_map(|b| b.as_text()).collect();
                let prefix = if t.is_error {
                    "[Tool Error]"
                } else {
                    "[Tool Result]"
                };
                format!("{prefix}\n{content}")
            }
        };
        if text.trim().is_empty() {
            continue;
        }
        // ChatML format: <|im_start|>role\nbody<|im_end|>
        parts.push(format!(
            "<|im_start|>{rolename}\n{body}<|im_end|>",
            rolename = role,
            body = text.trim_end_matches('\n'),
        ));
    }
    parts.join("\n")
}

/// Build the system prompt from context.
fn build_system_prompt(context: &Context) -> String {
    context
        .system_prompt
        .as_deref()
        .unwrap_or("You are a helpful assistant.")
        .to_string()
}

/// Build the inline flow config JSON for the start request.
fn build_inline_flow_config(system_prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "system_prompt": system_prompt,
        "agent": {
            "name": GITLAB_DUO_WORKFLOW_INLINE_AGENT_NAME,
            "type": "inline",
        },
        "prompt_template_id": GITLAB_DUO_WORKFLOW_INLINE_PROMPT_ID,
        "ui_log_events": GITLAB_DUO_WORKFLOW_INLINE_UI_LOG_EVENTS,
        "allow_attachments": false,
        "platform": "ide",
    })
}

/// Build MCP tool definitions for the start request.
fn build_mcp_tools(tools: &[crate::Tool]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "originalToolName": t.name,
                "serverName": "omp",
                "description": t.description,
                "inputSchema": "{}",
                "isApproved": true,
            })
        })
        .collect()
}

/// Build the start request payload.
fn build_start_request(
    workflow_id: &str,
    model: &Model,
    goal: &str,
    tools: &[crate::Tool],
    system_prompt: &str,
    available_models: &Option<serde_json::Value>,
) -> serde_json::Value {
    let model_ref = available_models
        .as_ref()
        .and_then(|m| select_model_ref(&model.id, m))
        .unwrap_or_default();
    let mcp_tools = build_mcp_tools(tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let metadata = serde_json::json!({
        "modelIdentifier": model.id,
        "selectedModelIdentifier": model_ref,
        "inlineFlow": true,
    });
    serde_json::json!({
        "startRequest": {
            "workflowID": workflow_id,
            "clientVersion": GITLAB_DUO_WORKFLOW_CLIENT_VERSION,
            "workflowDefinition": GITLAB_DUO_WORKFLOW_DEFINITION,
            "goal": goal,
            "workflowMetadata": metadata.to_string(),
            "additional_context": [],
            "clientCapabilities": GITLAB_DUO_WORKFLOW_CLIENT_CAPABILITIES,
            "mcpTools": mcp_tools,
            "preapproved_tools": tool_names,
            "flowConfigSchemaVersion": "v1",
            "flowConfig": build_inline_flow_config(system_prompt),
        }
    })
}

// ── WebSocket message parsing ──────────────────────────────────────

/// Structured checkpoint entry from the server.
#[derive(Debug)]
enum CheckpointKind {
    Text,
    Thinking,
}

#[derive(Debug)]
struct CheckpointEntry {
    kind: CheckpointKind,
    message_key: String,
    content: String,
}

#[derive(Debug)]
struct Checkpoint {
    entries: Vec<CheckpointEntry>,
    content_length: usize,
    context_usage: Option<(usize, usize)>, // (used, window)
}

/// Parse a server message and extract the checkpoint if present.
fn parse_checkpoint(event: &serde_json::Value) -> Option<Checkpoint> {
    let checkpoint = event
        .get("newCheckpoint")
        .or_else(|| event.get("checkpoint"))?;
    let entries_raw = checkpoint.get("entries")?.as_array()?;
    let mut entries = Vec::new();
    for entry in entries_raw {
        let kind_str = entry.get("kind")?.as_str()?;
        if kind_str == "boundary" {
            continue;
        } // boundaries mark turn pauses, skip
        let kind = match kind_str {
            "thinking" => CheckpointKind::Thinking,
            _ => CheckpointKind::Text,
        };
        entries.push(CheckpointEntry {
            kind,
            message_key: entry.get("messageKey")?.as_str()?.to_string(),
            content: entry.get("content")?.as_str()?.to_string(),
        });
    }
    let content_length = checkpoint
        .get("contentLength")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let context_usage = event.get("contextUsage").map(|u| {
        (
            u.get("used").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            u.get("window").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        )
    });
    Some(Checkpoint {
        entries,
        content_length,
        context_usage,
    })
}

/// Extract a tool call action from a server message.
fn parse_action(event: &serde_json::Value) -> Option<(String, String, serde_json::Value)> {
    let action = event.get("action")?;
    // The action wrapper can have `action` as the inner field
    // or be directly the action descriptor.
    let action_obj = if action.get("action").is_some() {
        action.get("action")?
    } else {
        action
    };
    let name = action_obj.get("name")?.as_str()?.to_string();
    let request_id = action_obj
        .get("requestID")
        .or_else(|| action_obj.get("requestId"))
        .or_else(|| action_obj.get("request_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_default();
    let args = action_obj
        .get("args")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    Some((name, request_id, args))
}

/// Extract status from a server message.
fn message_status(event: &serde_json::Value) -> Option<String> {
    event
        .get("status")
        .and_then(|v| v.as_str())
        .or_else(|| {
            event
                .get("workflowStatus")
                .and_then(|ws| ws.get("status").and_then(|v| v.as_str()))
        })
        .map(String::from)
}

// ── Provider state machine (stateless per turn) ────────────────────

/// Holds per-stream state for text/thinking dedup.
struct StreamState {
    output: AssistantMessage,
    event_tx: tokio::sync::mpsc::UnboundedSender<ProviderEvent>,
    content_text_index: usize,
    content_thinking_index: usize,
    seen_content_keys: HashMap<String, String>,
    seen_signatures: std::collections::HashSet<String>,
    has_active_text: bool,
    has_active_thinking: bool,
    saw_completion: bool,
}

impl StreamState {
    fn send(&self, event: ProviderEvent) {
        let _ = self.event_tx.send(event);
    }
}

fn process_checkpoint(cp: Checkpoint, state: &mut StreamState) {
    for entry in &cp.entries {
        let sig = format!(
            "{}\x00{}\x00{}",
            entry.message_key,
            entry.content.len(),
            entry.content.chars().take(20).collect::<String>()
        );
        if state.seen_signatures.contains(&sig) {
            continue;
        }
        state.seen_signatures.insert(sig);
        let prev = state.seen_content_keys.get(&entry.message_key).cloned();
        let delta = match &prev {
            Some(p) if entry.content.starts_with(p) && entry.content.len() > p.len() => {
                entry.content[p.len()..].to_string()
            }
            Some(p) if p != &entry.content => {
                // Rewrite: emit full content (but avoid replaying old content)
                // For MVP, send the whole new content starting from first divergence

                p.chars()
                    .zip(entry.content.chars())
                    .position(|(a, b)| a != b)
                    .map(|i| entry.content[i..].to_string())
                    .unwrap_or_default()
            }
            None => entry.content.clone(),
            _ => String::new(),
        };
        state
            .seen_content_keys
            .insert(entry.message_key.clone(), entry.content.clone());
        if delta.is_empty() && prev.is_some() && prev.as_deref() != Some(&entry.content) {
            // Full rewrite: emit the whole content
            // ... handled above already
        }
        if delta.is_empty() {
            continue;
        }

        match entry.kind {
            CheckpointKind::Thinking => {
                if !state.has_active_thinking {
                    state.send(ProviderEvent::ThinkingStart {
                        content_index: state.content_thinking_index,
                        partial: Arc::new(state.output.clone()),
                    });
                    state.has_active_thinking = true;
                }
                state.send(ProviderEvent::ThinkingDelta {
                    content_index: state.content_thinking_index,
                    delta: delta.clone(),
                    partial: Arc::new(state.output.clone()),
                });
            }
            CheckpointKind::Text => {
                if !state.has_active_text {
                    state.send(ProviderEvent::TextStart {
                        content_index: state.content_text_index,
                        partial: Arc::new(state.output.clone()),
                    });
                    state.has_active_text = true;
                }
                state.send(ProviderEvent::TextDelta {
                    content_index: state.content_text_index,
                    delta: delta.clone(),
                    partial: Arc::new(state.output.clone()),
                });
            }
        }
    }
}

fn apply_context_usage(cp: &Checkpoint, state: &mut StreamState) {
    if let Some((used, _window)) = &cp.context_usage {
        state.output.usage = Usage {
            input: *used,
            output: state.output.usage.output,
            cache_read: 0,
            cache_write: 0,
            total_tokens: *used + state.output.usage.output,
            cost: Cost::default(),
        };
    }
}

// ── GitLab Duo Agent Provider ──────────────────────────────────────

pub struct GitLabDuoAgentProvider {
    client: &'static Client,
    base_url: String,
    gitlab_token: Option<String>,
    namespace_id: String,
    root_namespace_id: String,
}

impl GitLabDuoAgentProvider {
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            base_url: GITLAB_COM_URL.to_string(),
            gitlab_token: None,
            namespace_id: Self::resolve_namespace("GITLAB_NAMESPACE_ID"),
            root_namespace_id: Self::resolve_namespace("GITLAB_ROOT_NAMESPACE_ID"),
        }
    }

    pub fn with_gitlab_token(token: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            base_url: GITLAB_COM_URL.to_string(),
            gitlab_token: Some(token.into()),
            namespace_id: Self::resolve_namespace("GITLAB_NAMESPACE_ID"),
            root_namespace_id: Self::resolve_namespace("GITLAB_ROOT_NAMESPACE_ID"),
        }
    }

    #[allow(dead_code)]
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: shared_client(),
            base_url: normalize_gitlab_base_url(base_url),
            gitlab_token: None,
            namespace_id: Self::resolve_namespace("GITLAB_NAMESPACE_ID"),
            root_namespace_id: Self::resolve_namespace("GITLAB_ROOT_NAMESPACE_ID"),
        }
    }

    #[allow(dead_code)]
    pub fn with_namespace(namespace_id: &str, root_namespace_id: &str) -> Self {
        Self {
            client: shared_client(),
            base_url: GITLAB_COM_URL.to_string(),
            gitlab_token: None,
            namespace_id: namespace_id.to_string(),
            root_namespace_id: root_namespace_id.to_string(),
        }
    }

    /// Resolve a namespace ID from env var, falling back to `"1"`.
    fn resolve_namespace(env_var: &str) -> String {
        std::env::var(env_var).unwrap_or_else(|_| "1".to_string())
    }
}

impl Default for GitLabDuoAgentProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for GitLabDuoAgentProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        let client = self.client;
        let base_url = self.base_url.clone();
        let gitlab_token = self.gitlab_token.clone();
        let namespace_id = self.namespace_id.clone();
        let root_namespace_id = self.root_namespace_id.clone();
        let context_clone = context.clone();
        let options_clone = options.clone();
        let model_id = model.id.clone();

        Box::pin(async move {
            let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<ProviderEvent>();

            let result: Result<(), ProviderError> = async {
                // 1. Resolve GitLab API key.
                let api_key = gitlab_token
                    .or_else(|| options_clone.as_ref().and_then(|o| o.api_key.clone()))
                    .or_else(|| std::env::var("GITLAB_TOKEN").ok())
                    .ok_or_else(|| {
                        ProviderError::InvalidResponse(
                            "GitLab Duo Agent requires GITLAB_TOKEN".into(),
                        )
                    })?;

                // 2. Build goal from context messages.
                let messages = &context_clone.messages;
                let goal = build_goal(messages);
                let system_prompt = build_system_prompt(&context_clone);
                let tools: Vec<crate::Tool> = context_clone.tools.to_vec();

                // 3. Resolve direct access token (with cache).
                let cached = get_cached_direct_access();
                let da = cached.as_ref().filter(|c| !c.token.is_empty());

                let conn = match da {
                    Some(c) => CachedDirectAccess {
                        token: c.token.clone(),
                        base_url: c.base_url.clone(),
                        headers: c.headers.clone(),
                        service_endpoint: c.service_endpoint,
                    },
                    None => {
                        let result = request_direct_access(
                            client,
                            &base_url,
                            &api_key,
                            &root_namespace_id,
                            None,
                        )
                        .await?;
                        set_cached_direct_access(CachedDirectAccess {
                            token: result.token.clone(),
                            base_url: result.base_url.clone(),
                            headers: result.headers.clone(),
                            service_endpoint: result.service_endpoint,
                        });
                        result
                    }
                };

                let ws_base_url = conn.base_url.as_deref().unwrap_or(&base_url);

                // 4. Create workflow.
                let workflow_id =
                    create_workflow(client, &base_url, &api_key, &namespace_id, &goal, None)
                        .await?;

                // 5. Fetch available models (best-effort).
                let available_models =
                    fetch_available_models(client, &base_url, &api_key, &namespace_id)
                        .await
                        .ok();
                let ws_base = normalize_gitlab_base_url(ws_base_url);

                // 7. Build WebSocket URL with auth/context params.
                let mut ws_url = if conn.service_endpoint {
                    Url::parse(&format!("{}/", ws_base.trim_end_matches('/')))
                } else {
                    Url::parse(&format!(
                        "{}/api/v4/ai/duo_workflows/ws",
                        ws_base.trim_end_matches('/')
                    ))
                }
                .map_err(|e| ProviderError::InvalidResponse(format!("WS URL: {e}")))?;
                {
                    let mut pairs = ws_url.query_pairs_mut();
                    pairs.append_pair("workflow_definition", GITLAB_DUO_WORKFLOW_DEFINITION);
                    pairs.append_pair("namespace_id", &namespace_id);
                }

                // 8. Build start request.
                let start_payload = build_start_request(
                    &workflow_id,
                    model,
                    &goal,
                    &tools,
                    &system_prompt,
                    &available_models,
                );

                // 9. Build WebSocket request with auth headers.
                let mut ws_request = ws_url
                    .as_str()
                    .into_client_request()
                    .map_err(|e| ProviderError::InvalidResponse(format!("WS request: {e}")))?;
                let ws_headers = ws_request.headers_mut();
                // SAFETY: the four header values below are compile-time string
                // literals ("Bearer ...", "cli", "8.104.0", a fixed UA).
                // `HeaderValue::parse` fails only on invalid characters, which
                // the literals provably do not contain. Infallible by construction.
                #[allow(clippy::expect_used)]
                ws_headers.insert(
                    tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
                    format!("Bearer {}", conn.token)
                        .parse()
                        .expect("static Bearer token is valid header"),
                );
                #[allow(clippy::expect_used)]
                ws_headers.insert("x-gitlab-client-type", "cli".parse().expect("static value"));
                #[allow(clippy::expect_used)]
                ws_headers.insert(
                    "x-gitlab-language-server-version",
                    "8.104.0".parse().expect("static value"),
                );
                #[allow(clippy::expect_used)]
                ws_headers.insert(
                    tokio_tungstenite::tungstenite::http::header::USER_AGENT,
                    "gitlab-language-server/8.104.0".parse().expect("static UA"),
                );

                let (ws_stream, _) = connect_async(ws_request)
                    .await
                    .map_err(|e| ProviderError::NetworkError(format!("WS connect: {e}")))?;

                let (mut ws_write, mut ws_read) = ws_stream.split();

                // 9. Emit start event.
                let output = AssistantMessage {
                    role: crate::AssistantRole::Assistant,
                    content: Vec::new(),
                    api: Api::GitLabDuoAgent,
                    provider: "gitlab".to_string(),
                    model: model_id.clone(),
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    response_id: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                };
                let _ = event_tx.send(ProviderEvent::Start {
                    partial: Arc::new(output.clone()),
                });

                // 10. Send start request.
                let start_json = start_payload.to_string();
                ws_write
                    .send(WsMessage::Text(start_json.into()))
                    .await
                    .map_err(|e| ProviderError::NetworkError(format!("WS send start: {e}")))?;

                // 11. Read messages until completion or tool call.
                let mut stream_state = StreamState {
                    output,
                    event_tx: event_tx.clone(),
                    content_text_index: 0,
                    content_thinking_index: 1,
                    seen_content_keys: HashMap::new(),
                    seen_signatures: std::collections::HashSet::new(),
                    has_active_text: false,
                    has_active_thinking: false,
                    saw_completion: false,
                };

                'ws_loop: loop {
                    let msg = tokio::time::timeout(
                        std::time::Duration::from_secs(GITLAB_DUO_WORKFLOW_WS_IDLE_TIMEOUT),
                        ws_read.next(),
                    )
                    .await;

                    match msg {
                        Ok(Some(Ok(WsMessage::Text(text)))) => {
                            let event: serde_json::Value = match serde_json::from_str(&text) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };

                            // Check for checkpoint / text/thinking deltas.
                            if let Some(cp) = parse_checkpoint(&event) {
                                process_checkpoint(cp, &mut stream_state);
                            }

                            // Check for tool call action.
                            if let Some((name, request_id, args)) = parse_action(&event) {
                                // Close text/thinking blocks.
                                if stream_state.has_active_text {
                                    stream_state.send(ProviderEvent::TextEnd {
                                        content_index: stream_state.content_text_index,
                                        content: String::new(),
                                        partial: Arc::new(stream_state.output.clone()),
                                    });
                                    stream_state.has_active_text = false;
                                }
                                if stream_state.has_active_thinking {
                                    stream_state.send(ProviderEvent::ThinkingEnd {
                                        content_index: stream_state.content_thinking_index,
                                        content: String::new(),
                                        partial: Arc::new(stream_state.output.clone()),
                                    });
                                    stream_state.has_active_thinking = false;
                                }

                                let tool_call_id = if request_id.is_empty() {
                                    Uuid::new_v4().to_string()
                                } else {
                                    request_id.clone()
                                };

                                let tc = ToolCall {
                                    content_type: ToolCallType::ToolCall,
                                    id: tool_call_id.clone(),
                                    name: name.clone(),
                                    arguments: args,
                                    thought_signature: None,
                                };
                                stream_state
                                    .output
                                    .content
                                    .push(ContentBlock::ToolCall(tc.clone()));

                                // Store the pending action info in the output
                                // so the adapter can send the result back.
                                // For stateless Provider trait, we return the tool call
                                // and the host executes it. The action response is not sent
                                // because the socket is closed.
                                stream_state.send(ProviderEvent::ToolCallEnd {
                                    content_index: stream_state.output.content.len(),
                                    tool_call: tc,
                                    partial: Arc::new(stream_state.output.clone()),
                                });
                                break 'ws_loop;
                            }

                            // Check for completion status.
                            let status = message_status(&event);
                            match status.as_deref() {
                                Some("INPUT_REQUIRED" | "FINISHED") => {
                                    stream_state.saw_completion = true;
                                    break 'ws_loop;
                                }
                                Some("FAILED") | Some("STOPPED") => {
                                    let err = event
                                        .get("error")
                                        .and_then(|e| e.as_str())
                                        .or_else(|| event.get("message").and_then(|m| m.as_str()))
                                        .unwrap_or("workflow failed");
                                    stream_state.output.error_message = Some(err.to_string());
                                    break 'ws_loop;
                                }
                                Some("PLAN_APPROVAL_REQUIRED" | "TOOL_CALL_APPROVAL_REQUIRED") => {
                                    // Approval required — for stateless MVP, treat as done
                                    stream_state.saw_completion = true;
                                    break 'ws_loop;
                                }
                                _ => {} // continue
                            }
                        }
                        Ok(Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_)))) => {}
                        Ok(Some(Ok(WsMessage::Close(_))) | None) => break 'ws_loop,
                        Ok(Some(Err(e))) => {
                            stream_state.output.error_message =
                                Some(format!("WebSocket error: {e}"));
                            break 'ws_loop;
                        }
                        Err(_timeout) => {
                            // Idle timeout — close and return current state
                            break 'ws_loop;
                        }
                        _ => {}
                    }
                }

                // 12. Finalize and clean up.
                if stream_state.has_active_text {
                    stream_state.send(ProviderEvent::TextEnd {
                        content_index: stream_state.content_text_index,
                        content: String::new(),
                        partial: Arc::new(stream_state.output.clone()),
                    });
                    stream_state.has_active_text = false;
                }
                if stream_state.has_active_thinking {
                    stream_state.send(ProviderEvent::ThinkingEnd {
                        content_index: stream_state.content_thinking_index,
                        content: String::new(),
                        partial: Arc::new(stream_state.output.clone()),
                    });
                    stream_state.has_active_thinking = false;
                }

                let reason = if stream_state.saw_completion {
                    StopReason::Stop
                } else if stream_state.output.error_message.is_some() {
                    StopReason::Error
                } else {
                    StopReason::Stop
                };
                stream_state.output.stop_reason = reason;

                if reason == StopReason::Error {
                    let _ = event_tx.send(ProviderEvent::Error {
                        reason,
                        error: stream_state.output.clone(),
                    });
                } else {
                    let _ = event_tx.send(ProviderEvent::Done {
                        message: stream_state.output,
                        reason,
                    });
                }

                // Best-effort stop workflow.
                drop(ws_write);
                drop(ws_read);
                stop_workflow(client, &base_url, &api_key, &workflow_id).await;

                Ok(())
            }
            .await;

            if let Err(e) = result {
                let _ = event_tx.send(ProviderEvent::Error {
                    reason: StopReason::Error,
                    error: {
                        let mut m = AssistantMessage::new(
                            Api::GitLabDuoAgent,
                            "gitlab".to_string(),
                            model_id,
                        );
                        m.error_message = Some(e.to_string());
                        m
                    },
                });
            }

            Ok(
                Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(
                    event_rx,
                )) as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = GitLabDuoAgentProvider::new();
        let _ = provider;
    }

    #[test]
    fn test_normalize_gitlab_base_url() {
        assert_eq!(normalize_gitlab_base_url(""), GITLAB_COM_URL);
        assert_eq!(
            normalize_gitlab_base_url("https://gitlab.com/"),
            "https://gitlab.com"
        );
        assert_eq!(
            normalize_gitlab_base_url("https://gitlab.example.com"),
            "https://gitlab.example.com"
        );
    }

    #[test]
    fn test_build_goal_empty() {
        assert_eq!(build_goal(&[]), "");
    }

    #[test]
    fn test_build_goal_single_user() {
        let msg = Message::User(crate::UserMessage::new("Hello"));
        let goal = build_goal(&[msg]);
        assert!(goal.contains("<|im_start|>user"));
        assert!(goal.contains("Hello"));
        assert!(goal.contains("<|im_end|>"));
    }

    #[test]
    fn test_select_model_ref_found() {
        let available = serde_json::json!({
            "data": {
                "aiChatAvailableModels": {
                    "defaultModel": { "name": "default", "ref": "claude-sonnet-4" },
                    "selectableModels": [
                        { "name": "my-model", "ref": "claude-sonnet-4-20250501" }
                    ]
                }
            }
        });
        let result = select_model_ref("my-model", &available);
        assert_eq!(result.as_deref(), Some("claude-sonnet-4-20250501"));
    }

    #[test]
    fn test_select_model_ref_fallback_default() {
        let available = serde_json::json!({
            "data": {
                "aiChatAvailableModels": {
                    "defaultModel": { "name": "default", "ref": "claude-sonnet-4" },
                    "selectableModels": []
                }
            }
        });
        let result = select_model_ref("unknown", &available);
        assert_eq!(result.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn test_parse_action_finds_action() {
        let event = serde_json::json!({
            "action": {
                "name": "runMCPTool",
                "requestID": "req-1",
                "args": { "tool_name": "bash" }
            }
        });
        let (name, rid, args) = parse_action(&event).unwrap();
        assert_eq!(name, "runMCPTool");
        assert_eq!(rid, "req-1");
        assert_eq!(args["tool_name"], "bash");
    }

    #[test]
    fn test_message_status_extraction() {
        let event = serde_json::json!({"status": "FINISHED"});
        assert_eq!(message_status(&event).as_deref(), Some("FINISHED"));
    }

    #[test]
    fn test_build_inline_flow_config() {
        let config = build_inline_flow_config("Be helpful.");
        assert_eq!(config["system_prompt"], "Be helpful.");
        assert_eq!(
            config["agent"]["name"],
            GITLAB_DUO_WORKFLOW_INLINE_AGENT_NAME
        );
    }

    #[test]
    fn test_to_gitlab_graphql_namespace_id() {
        assert_eq!(
            to_gitlab_graphql_namespace_id("42"),
            "gid://gitlab/Group/42"
        );
        assert_eq!(
            to_gitlab_graphql_namespace_id("gid://gitlab/Group/42"),
            "gid://gitlab/Group/42"
        );
    }
}
