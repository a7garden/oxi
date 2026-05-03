//! Google Vertex AI provider
//!
//! This provider uses Google Cloud authentication (service account or gcloud CLI)
//! to access Vertex AI models via the Gemini API.

use async_trait::async_trait;
use futures::stream::StreamExt;
use futures::Stream;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::pin::Pin;

use super::{Provider, ProviderError, ProviderEvent, StreamOptions};
use crate::{Api, AssistantMessage, ContentBlock, Context, Model, StopReason, Usage};

/// Google Vertex AI provider
///
/// Uses Bearer token authentication via:
/// - Service account JSON file (GOOGLE_APPLICATION_CREDENTIALS)
/// - gcloud CLI access token (from `gcloud auth print-access-token`)
///
/// Endpoint: https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:streamGenerateContent
#[derive(Clone)]
pub struct VertexProvider {
    client: Client,
}

impl VertexProvider {
    /// Create a new Vertex AI provider
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Get the access token for Google Cloud authentication
    ///
    /// Priority:
    /// 1. GOOGLE_ACCESS_TOKEN environment variable
    /// 2. gcloud CLI access token (via `gcloud auth print-access-token`)
    /// 3. Service account credentials from GOOGLE_APPLICATION_CREDENTIALS
    async fn get_access_token(&self) -> Result<String, ProviderError> {
        // Check for explicit token first
        if let Ok(token) = std::env::var("GOOGLE_ACCESS_TOKEN") {
            if !token.is_empty() {
                return Ok(token);
            }
        }

        // Try gcloud CLI
        if let Ok(token) = Self::get_gcloud_token().await {
            return Ok(token);
        }

        // Try service account credentials
        if let Ok(creds) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
            if !creds.is_empty() {
                return Self::get_token_from_service_account(&creds).await;
            }
        }

        Err(ProviderError::MissingApiKey)
    }

    /// Get access token from gcloud CLI
    async fn get_gcloud_token() -> Result<String, ProviderError> {
        use std::io;
        use tokio::process::Command;

        let output = Command::new("gcloud")
            .args(["auth", "print-access-token"])
            .output()
            .await
            .map_err(ProviderError::IoError)?;

        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }

        Err(ProviderError::IoError(io::Error::new(
            io::ErrorKind::NotFound,
            "gcloud token not available",
        )))
    }

    /// Get access token from service account credentials JSON
    async fn get_token_from_service_account(
        credentials_path: &str,
    ) -> Result<String, ProviderError> {
        use std::fs;
        use tokio::time::{sleep, Duration};

        // Read credentials file
        let creds_json = fs::read_to_string(credentials_path).map_err(ProviderError::IoError)?;

        let creds: ServiceAccountCreds =
            serde_json::from_str(&creds_json).map_err(|_| ProviderError::InvalidApiKey)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Create JWT assertion
        let header = base64_url_encode(&serde_json::json!({
            "alg": "RS256",
            "typ": "JWT"
        }));

        let claims = serde_json::json!({
            "iss": creds.client_email,
            "sub": creds.client_email,
            "aud": "https://oauth2.googleapis.com/token",
            "iat": now,
            "exp": now + 3600,
            "scope": "https://www.googleapis.com/auth/cloud-platform"
        });

        let claims_b64 = base64_url_encode(&claims);

        // Sign with RSA-SHA256
        let signature = sign_rs256(&header, &claims_b64, &creds.private_key)?;
        let jwt = signature;

        // Exchange JWT for access token
        let client = Client::new();
        let response = client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(ProviderError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(ProviderError::HttpError(
                response.status().as_u16(),
                response.text().await.unwrap_or_default(),
            ));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::RequestFailed(e))?;

        // Cache the token briefly
        sleep(Duration::from_secs(60 * 55)).await;

        Ok(token_response.access_token)
    }

    /// Get Google Cloud project ID
    fn get_project_id() -> Result<String, ProviderError> {
        std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GOOGLE_PROJECT"))
            .map_err(|_| ProviderError::MissingApiKey)
    }

    /// Get Google Cloud region
    fn get_region() -> String {
        std::env::var("GOOGLE_CLOUD_REGION").unwrap_or_else(|_| "us-central1".to_string())
    }
}

impl Default for VertexProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for VertexProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Get authentication
        let access_token = self.get_access_token().await?;
        let project_id = Self::get_project_id()?;
        let region = Self::get_region();

        // Build the request URL
        let model_id = &model.id;
        let url = format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/google/models/{}:streamGenerateContent",
            region, project_id, region, model_id
        );

        // Build contents in Google Gemini format
        let contents = build_vertex_contents(context)?;

        // Build request body
        let mut body = serde_json::json!({
            "contents": contents,
            "stream": true,
        });

        // Add generation config
        let mut generation_config = serde_json::json!({});

        if let Some(temp) = options.temperature {
            generation_config["temperature"] = serde_json::json!(temp);
        }

        if let Some(max) = options.max_tokens {
            generation_config["maxOutputTokens"] = serde_json::json!(max);
        }

        if let serde_json::Value::Object(obj) = &generation_config {
            if !obj.is_empty() {
                body["generationConfig"] = generation_config;
            }
        }

        // Add system instruction
        if let Some(ref prompt) = context.system_prompt {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{ "text": prompt }]
            });
        }

        // Add tools if present
        if !context.tools.is_empty() {
            body["tools"] = build_vertex_tools(&context.tools)?;
        }

        // Make request with Bearer token
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::RequestFailed)?;

        if !response.status().is_success() {
            let status = response.status();
            let body: String = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpError(status.as_u16(), body));
        }

        // Create event stream
        let model_name = model.id.clone();

        let stream = response.bytes_stream().flat_map(move |chunk| match chunk {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                futures::stream::iter(parse_vertex_events(&text, &model_name))
            }
            Err(e) => futures::stream::iter(vec![ProviderEvent::Error {
                reason: StopReason::Error,
                error: create_error_message(&e.to_string()),
            }]),
        });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "vertex"
    }
}

// Helper functions (shared logic with Google provider)

/// Build contents in Vertex AI format (same as Google Gemini)
fn build_vertex_contents(context: &Context) -> Result<Vec<JsonValue>, ProviderError> {
    let mut contents = Vec::new();

    for msg in &context.messages {
        match msg {
            crate::Message::User(u) => {
                let parts = match &u.content {
                    crate::MessageContent::Text(s) => vec![serde_json::json!({ "text": s })],
                    crate::MessageContent::Blocks(blocks) => blocks_to_vertex_parts(blocks)?,
                };
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
            crate::Message::Assistant(a) => {
                let parts = blocks_to_vertex_parts(&a.content)?;
                contents.push(serde_json::json!({
                    "role": "model",
                    "parts": parts,
                }));
            }
            crate::Message::ToolResult(t) => {
                let parts = blocks_to_vertex_parts(&t.content)?;
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
        }
    }

    Ok(contents)
}

/// Convert content blocks to Vertex parts format
fn blocks_to_vertex_parts(blocks: &[ContentBlock]) -> Result<Vec<JsonValue>, ProviderError> {
    let mut parts = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text(t) => {
                parts.push(serde_json::json!({
                    "text": t.text,
                }));
            }
            ContentBlock::ToolCall(tc) => {
                parts.push(serde_json::json!({
                    "functionCall": {
                        "name": tc.name,
                        "args": tc.arguments,
                    },
                }));
            }
            ContentBlock::Image(img) => {
                parts.push(serde_json::json!({
                    "inlineData": {
                        "mimeType": img.mime_type,
                        "data": img.data,
                    },
                }));
            }
            ContentBlock::Thinking(th) => {
                // Google/Vertex doesn't have native thinking blocks, send as text
                parts.push(serde_json::json!({
                    "text": format!("[Thinking: {}]", th.thinking),
                }));
            }
            ContentBlock::Unknown(_) => {
                // Skip unknown blocks
            }
        }
    }

    Ok(parts)
}

/// Build tools in Vertex AI format (Google functionDeclarations)
fn build_vertex_tools(tools: &[crate::Tool]) -> Result<JsonValue, ProviderError> {
    let declarations: Vec<_> = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "functionDeclarations": [{
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }]
            })
        })
        .collect();

    Ok(serde_json::json!(declarations))
}

/// Parse Vertex AI SSE event stream (same as Google provider)
fn parse_vertex_events(text: &str, model_id: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let mut partial_message = AssistantMessage::new(Api::GoogleVertex, "vertex", model_id);

    for line in text.lines() {
        if line.is_empty() || line == "data: [DONE]" {
            continue;
        }

        if let Some(data) = line.strip_prefix("data: ") {
            if let Ok(response) = serde_json::from_str::<VertexResponse>(data) {
                // Process candidates
                for candidate in &response.candidates {
                    // Process content
                    if let Some(content) = &candidate.content {
                        for (index, part) in content.parts.iter().enumerate() {
                            if let Some(text) = &part.text {
                                events.push(ProviderEvent::TextDelta {
                                    content_index: index,
                                    delta: text.clone(),
                                    partial: partial_message.clone(),
                                });
                            }

                            if let Some(function_call) = &part.function_call {
                                events.push(ProviderEvent::ToolCallDelta {
                                    content_index: index,
                                    delta: serde_json::to_string(&function_call.args)
                                        .unwrap_or_default(),
                                    partial: partial_message.clone(),
                                });
                            }
                        }
                    }
                }

                // Update usage if present
                if let Some(usage) = &response.usage_metadata {
                    partial_message.usage = Usage {
                        input: usage.prompt_token_count.unwrap_or(0),
                        output: usage.candidates_token_count.unwrap_or(0),
                        cache_read: 0,
                        cache_write: 0,
                        total_tokens: usage.total_token_count.unwrap_or(0),
                        cost: Default::default(),
                    };
                }

                // Check if done
                if let Some(ref finish_reason) = response
                    .candidates
                    .first()
                    .and_then(|c| c.finish_reason.clone())
                {
                    let reason = match finish_reason.as_str() {
                        "STOP" => StopReason::Stop,
                        "MAX_TOKENS" => StopReason::Length,
                        "SAFETY" | "OTHER" => StopReason::Error,
                        _ => StopReason::Stop,
                    };

                    if matches!(reason, StopReason::Stop | StopReason::Length) {
                        events.push(ProviderEvent::Done {
                            reason,
                            message: partial_message.clone(),
                        });
                    }
                }
            }
        }
    }

    events
}

/// Create error assistant message
fn create_error_message(msg: &str) -> AssistantMessage {
    let mut message = AssistantMessage::new(Api::GoogleVertex, "vertex", "unknown");
    message.stop_reason = StopReason::Error;
    message.error_message = Some(msg.to_string());
    message
}

// Helper functions for JWT signing

fn base64_url_encode(value: &serde_json::Value) -> String {
    use base64::Engine as _;
    let json = serde_json::to_string(value).unwrap();
    let bytes = json.as_bytes();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn sign_rs256(
    header_b64: &str,
    claims_b64: &str,
    private_key_pem: &str,
) -> Result<String, ProviderError> {
    use base64::Engine as _;
    use pkcs8::DecodePrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::RsaPrivateKey;
    use sha2::Sha256;
    use signature::{SignatureEncoding, Signer};

    // Construct the message to sign: header.claims
    let message = format!("{}.{}", header_b64, claims_b64);

    let key =
        RsaPrivateKey::from_pkcs8_pem(private_key_pem).map_err(|_| ProviderError::InvalidApiKey)?;
    let signing_key = SigningKey::<Sha256>::new_unprefixed(key);
    let signature = signing_key.sign(message.as_bytes());
    let sig_bytes = signature.to_bytes();

    // Encode signature using base64 with URL-safe encoding (no padding)
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&sig_bytes);

    // Return the complete JWT: header.claims.signature
    Ok(format!("{}.{}", message, sig_b64))
}

// Response structures for Vertex AI

#[derive(Debug, Deserialize)]
struct VertexResponse {
    candidates: Vec<VertexCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<VertexUsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct VertexCandidate {
    content: Option<VertexContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VertexContent {
    parts: Vec<VertexPart>,
}

#[derive(Debug, Deserialize)]
struct VertexPart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<VertexFunctionCall>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct VertexFunctionCall {
    name: String,
    args: JsonValue,
}

#[derive(Debug, Deserialize)]
struct VertexUsageMetadata {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<usize>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<usize>,
    #[serde(rename = "totalTokenCount")]
    total_token_count: Option<usize>,
}

// OAuth token exchange response

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TokenResponse {
    access_token: String,
    expires_in: usize,
    token_type: String,
}

// Service account credentials structure

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ServiceAccountCreds {
    #[serde(rename = "type")]
    _type: String,
    project_id: String,
    private_key_id: String,
    private_key: String,
    client_email: String,
    client_id: String,
    auth_uri: String,
    token_uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, ImageContent, Message, ToolCall};

    #[test]
    fn test_vertex_provider_name() {
        let provider = VertexProvider::new();
        assert_eq!(provider.name(), "vertex");
    }

    #[test]
    fn test_build_vertex_contents_with_text() {
        let mut ctx = Context::new();
        ctx.add_message(Message::user("Hello, world!"));

        let contents = build_vertex_contents(&ctx).unwrap();
        assert_eq!(contents.len(), 1);

        let content = &contents[0];
        assert_eq!(content["role"], "user");
        assert_eq!(content["parts"][0]["text"], "Hello, world!");
    }

    #[test]
    fn test_build_vertex_contents_with_assistant_response() {
        let mut ctx = Context::new();
        ctx.add_message(Message::user("Hi"));
        ctx.add_message(Message::Assistant(AssistantMessage::new(
            Api::GoogleVertex,
            "vertex",
            "gemini-1.5-pro",
        )));

        let contents = build_vertex_contents(&ctx).unwrap();
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[1]["role"], "model");
    }

    #[test]
    fn test_build_vertex_tools() {
        let tools = vec![crate::Tool {
            name: "get_weather".to_string(),
            description: "Get weather for a location".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "The city name"
                    }
                },
                "required": ["location"]
            }),
        }];

        let tools_json = build_vertex_tools(&tools).unwrap();

        // Check that the tool is properly formatted with functionDeclarations
        let declarations = tools_json[0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0]["name"], "get_weather");
        assert_eq!(declarations[0]["description"], "Get weather for a location");
    }

    #[test]
    fn test_parse_vertex_events_basic_text() {
        let sse_data = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}"#;

        let events = parse_vertex_events(sse_data, "gemini-1.5-pro");

        assert!(!events.is_empty());
        if let ProviderEvent::TextDelta { delta, .. } = &events[0] {
            assert_eq!(delta, "Hello");
        } else {
            panic!("Expected TextDelta event");
        }
    }

    #[test]
    fn test_parse_vertex_events_with_usage() {
        let sse_data = r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":20,"totalTokenCount":30}}"#;

        let events = parse_vertex_events(sse_data, "gemini-1.5-pro");

        // Should have a Done event
        let done_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::Done { .. }))
            .collect();
        assert!(!done_events.is_empty());
    }

    #[test]
    fn test_parse_vertex_events_with_function_call() {
        let sse_data = r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"location":"Boston"}}}]}}]}"#;

        let events = parse_vertex_events(sse_data, "gemini-1.5-pro");

        let tool_call_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::ToolCallDelta { .. }))
            .collect();
        assert!(!tool_call_events.is_empty());
    }

    #[test]
    fn test_get_region_default() {
        // Clear environment variables
        std::env::remove_var("GOOGLE_CLOUD_REGION");

        let region = VertexProvider::get_region();
        assert_eq!(region, "us-central1");
    }

    #[test]
    fn test_blocks_to_vertex_parts_with_tool_call() {
        let tool_call = ContentBlock::ToolCall(ToolCall::new(
            "call_123",
            "my_function",
            serde_json::json!({"arg1": "value1"}),
        ));

        let parts = blocks_to_vertex_parts(&[tool_call]).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["functionCall"]["name"], "my_function");
        assert_eq!(parts[0]["functionCall"]["args"]["arg1"], "value1");
    }

    #[test]
    fn test_blocks_to_vertex_parts_with_image() {
        let image = ContentBlock::Image(ImageContent::new("base64data", "image/png"));

        let parts = blocks_to_vertex_parts(&[image]).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
        assert_eq!(parts[0]["inlineData"]["data"], "base64data");
    }

    #[test]
    fn test_create_error_message() {
        let msg = create_error_message("Something went wrong");

        assert_eq!(msg.provider, "vertex");
        assert_eq!(msg.api, Api::GoogleVertex);
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(msg.error_message, Some("Something went wrong".to_string()));
    }
}
